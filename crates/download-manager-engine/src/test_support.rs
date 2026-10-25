//! Deterministic HTTP/1.1 fixtures shared by engine unit tests.

use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Duration,
};

#[derive(Debug, Clone)]
pub(crate) struct Fixture {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    pub delay: Duration,
    pub include_content_length: bool,
}

impl Fixture {
    pub(crate) fn ok(body: impl Into<Vec<u8>>) -> Self {
        Self {
            status: 200,
            headers: Vec::new(),
            body: body.into(),
            delay: Duration::ZERO,
            include_content_length: true,
        }
    }

    pub(crate) fn status(mut self, status: u16) -> Self {
        self.status = status;
        self
    }

    pub(crate) fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_owned(), value.to_owned()));
        self
    }

    pub(crate) fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    #[allow(dead_code)]
    pub(crate) fn without_content_length(mut self) -> Self {
        self.include_content_length = false;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CapturedRequest {
    pub method: String,
    pub target: String,
    pub headers: Vec<(String, String)>,
}

impl CapturedRequest {
    pub(crate) fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(candidate, _)| candidate.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.as_str())
    }
}

pub(crate) struct FixtureServer {
    address: SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    pub(crate) fn start(fixtures: Vec<Fixture>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        listener
            .set_nonblocking(true)
            .expect("make fixture server nonblocking");
        let address = listener.local_addr().expect("fixture address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let should_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            let mut fixtures = fixtures.into_iter();
            loop {
                if should_stop.load(Ordering::Acquire) {
                    break;
                }
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("make fixture connection blocking");
                        let Some(fixture) = fixtures.next() else {
                            break;
                        };
                        let request = read_request(&mut stream);
                        let is_head = request.method == "HEAD";
                        captured.lock().expect("request lock").push(request);
                        write_response(&mut stream, &fixture, is_head);
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("fixture accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    pub(crate) fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("request lock").clone()
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("fixture worker");
        }
    }
}

fn read_request(stream: &mut TcpStream) -> CapturedRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("request timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 512];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).expect("read fixture request");
        if count == 0 {
            break;
        }
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() <= 64 * 1024, "fixture request is too large");
    }
    let text = String::from_utf8(bytes).expect("ASCII fixture request");
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next().expect("request line").split_whitespace();
    let method = request_line.next().expect("request method").to_owned();
    let target = request_line.next().expect("request target").to_owned();
    let headers = lines
        .take_while(|line| !line.is_empty())
        .map(|line| {
            let (name, value) = line.split_once(':').expect("valid request header");
            (name.to_owned(), value.trim().to_owned())
        })
        .collect();
    CapturedRequest {
        method,
        target,
        headers,
    }
}

fn write_response(stream: &mut TcpStream, fixture: &Fixture, is_head: bool) {
    thread::sleep(fixture.delay);
    let mut response = format!(
        "HTTP/1.1 {} {}\r\nConnection: close\r\n",
        fixture.status,
        reason(fixture.status)
    );
    for (name, value) in &fixture.headers {
        std::fmt::Write::write_fmt(&mut response, format_args!("{name}: {value}\r\n"))
            .expect("write fixture header");
    }
    if fixture.include_content_length
        && !fixture
            .headers
            .iter()
            .any(|(name, _)| name.eq_ignore_ascii_case("content-length"))
    {
        std::fmt::Write::write_fmt(
            &mut response,
            format_args!("Content-Length: {}\r\n", fixture.body.len()),
        )
        .expect("write fixture content length");
    }
    response.push_str("\r\n");
    stream
        .write_all(response.as_bytes())
        .expect("write fixture headers");
    if !is_head {
        stream.write_all(&fixture.body).expect("write fixture body");
    }
}

const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        206 => "Partial Content",
        302 => "Found",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        408 => "Request Timeout",
        425 => "Too Early",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Fixture",
    }
}

#[cfg(test)]
mod tests {
    use std::{io::Read, net::TcpStream, time::Instant};

    use super::{Fixture, FixtureServer};

    #[test]
    fn programmable_server_serves_status_headers_body_and_delay() {
        let server = FixtureServer::start(vec![
            Fixture::ok("fixture-body")
                .status(206)
                .header("X-Fixture", "yes")
                .delay(std::time::Duration::from_millis(20)),
        ]);
        let started = Instant::now();
        let mut stream = TcpStream::connect(server.address).expect("connect fixture");
        std::io::Write::write_all(
            &mut stream,
            b"GET /delayed HTTP/1.1\r\nHost: fixture\r\nX-Request: captured\r\n\r\n",
        )
        .expect("request");
        let mut response = String::new();
        stream.read_to_string(&mut response).expect("response");
        assert!(started.elapsed() >= std::time::Duration::from_millis(15));
        assert!(response.starts_with("HTTP/1.1 206 Partial Content"));
        assert!(response.contains("X-Fixture: yes"));
        assert!(response.ends_with("fixture-body"));
        let requests = server.requests();
        assert_eq!(requests[0].method, "GET");
        assert_eq!(requests[0].target, "/delayed");
        assert_eq!(requests[0].header("x-request"), Some("captured"));
    }
}
