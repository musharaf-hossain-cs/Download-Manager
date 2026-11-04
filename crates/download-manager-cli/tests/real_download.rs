use std::{
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use download_manager_cli::{AddRequest, CliBackend, DownloadBackend, JobView};
use download_manager_engine::{domain::HttpUrl, state::DownloadState};

const TERMINAL_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(10);

#[test]
fn known_length_download_publishes_exact_final_file() {
    let body = patterned_body(192 * 1024 + 17);
    let server = FixtureServer::start(FixtureKind::Known, body.clone());
    let directories = TestDirectories::new("known");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/fixture.bin"), &directories);
    let completed = wait_for_state(&mut backend, job.id, DownloadState::Completed);

    assert_eq!(completed.filename, "fixture.bin");
    assert_eq!(completed.downloaded_bytes, body.len() as u64);
    assert_eq!(completed.total_bytes, Some(body.len() as u64));
    assert_eq!(completed.failure_reason, None);
    assert_eq!(
        fs::read(directories.final_path()).expect("read final"),
        body
    );
    assert_directory_empty(&directories.partials);
    assert_list_and_status(&mut backend, &completed);
}

#[test]
fn unknown_length_download_never_invents_a_total() {
    let body = patterned_body(96 * 1024 + 3);
    let server = FixtureServer::start(FixtureKind::UnknownLength, body.clone());
    let directories = TestDirectories::new("unknown");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/stream"), &directories);
    assert_eq!(job.total_bytes, None);
    let completed = wait_for_state(&mut backend, job.id, DownloadState::Completed);

    assert_eq!(completed.downloaded_bytes, body.len() as u64);
    assert_eq!(completed.total_bytes, None);
    assert_eq!(
        fs::read(directories.final_path()).expect("read final"),
        body
    );
    assert_directory_empty(&directories.partials);
    assert_list_and_status(&mut backend, &completed);
}

#[test]
fn redirect_is_probed_and_downloaded_from_the_final_url() {
    let body = patterned_body(128 * 1024 + 31);
    let server = FixtureServer::start(FixtureKind::Redirect, body.clone());
    let directories = TestDirectories::new("redirect");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/redirect"), &directories);
    let completed = wait_for_state(&mut backend, job.id, DownloadState::Completed);

    assert_eq!(
        fs::read(directories.final_path()).expect("read final"),
        body
    );
    assert_directory_empty(&directories.partials);
    assert_list_and_status(&mut backend, &completed);
    let requests = server.requests();
    assert!(
        requests
            .iter()
            .any(|request| request.method == "HEAD" && request.target == "/redirect")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == "HEAD" && request.target == "/final")
    );
    assert!(
        requests
            .iter()
            .any(|request| request.method == "GET" && request.target == "/final")
    );
}

#[test]
fn truncated_known_body_fails_without_publishing_final_file() {
    let body = patterned_body(160 * 1024 + 7);
    let server = FixtureServer::start(FixtureKind::Truncated, body.clone());
    let directories = TestDirectories::new("truncated");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/fixture.bin"), &directories);
    let failed = wait_for_state(&mut backend, job.id, DownloadState::Failed);

    assert!(failed.downloaded_bytes < body.len() as u64);
    assert!(
        failed
            .failure_reason
            .as_deref()
            .is_some_and(|reason| !reason.trim().is_empty()),
        "failed job must expose a bounded reason: {failed:?}"
    );
    assert!(
        failed
            .failure_reason
            .as_deref()
            .expect("failure reason")
            .chars()
            .count()
            <= 4_096
    );
    assert!(
        !directories.final_path().exists(),
        "a failed transfer must not publish the final path"
    );
    let partials = files_in(&directories.partials);
    assert_eq!(
        partials.len(),
        1,
        "failed transfer should retain one partial"
    );
    assert_eq!(
        partials[0].extension().and_then(|value| value.to_str()),
        Some("part")
    );
    let partial = fs::read(&partials[0]).expect("read retained partial");
    assert!(!partial.is_empty(), "fixture should write a bounded prefix");
    assert_eq!(partial, body[..partial.len()]);
    assert_list_and_status(&mut backend, &failed);
}

#[test]
fn one_fifo_worker_keeps_the_second_job_waiting_during_the_first_transfer() {
    let body = patterned_body(64 * 1024 + 5);
    let server = FixtureServer::start(FixtureKind::Delayed, body.clone());
    let directories = TestDirectories::new("fifo");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let first = add_named(
        &mut backend,
        &server.url("/first"),
        &directories,
        "first.bin",
    );
    wait_for_state(&mut backend, first.id, DownloadState::Downloading);
    let second = add_named(
        &mut backend,
        &server.url("/second"),
        &directories,
        "second.bin",
    );

    assert_eq!(
        backend.status(second.id).expect("second status").state,
        DownloadState::Probing
    );
    wait_for_state(&mut backend, first.id, DownloadState::Completed);
    wait_for_state(&mut backend, second.id, DownloadState::Completed);
    assert_eq!(
        fs::read(directories.output.join("first.bin")).expect("first file"),
        body
    );
    assert_eq!(
        fs::read(directories.output.join("second.bin")).expect("second file"),
        body
    );
}

fn add(backend: &mut DownloadBackend, url: &str, directories: &TestDirectories) -> JobView {
    add_named(backend, url, directories, "fixture.bin")
}

fn add_named(
    backend: &mut DownloadBackend,
    url: &str,
    directories: &TestDirectories,
    filename: &str,
) -> JobView {
    backend
        .add(AddRequest {
            source_url: HttpUrl::parse(url).expect("fixture URL"),
            output_directory: directories.output.clone(),
            filename: Some(filename.to_owned()),
            start_now: true,
            queue: None,
        })
        .expect("add real download")
}

fn wait_for_state(
    backend: &mut DownloadBackend,
    job_id: download_manager_engine::domain::JobId,
    expected: DownloadState,
) -> JobView {
    let deadline = Instant::now() + TERMINAL_TIMEOUT;
    loop {
        let job = backend.status(job_id).expect("read job status");
        if job.state == expected {
            return job;
        }
        assert!(
            !matches!(
                job.state,
                DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
            ),
            "job reached unexpected terminal state: {job:?}"
        );
        assert!(Instant::now() < deadline, "job timed out in state {job:?}");
        thread::sleep(POLL_INTERVAL);
    }
}

fn assert_list_and_status(backend: &mut DownloadBackend, expected: &JobView) {
    assert_eq!(
        backend
            .status(expected.id)
            .expect("status after terminal state"),
        *expected
    );
    let listed = backend.list().expect("list jobs");
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0], *expected);
}

fn assert_directory_empty(directory: &Path) {
    assert!(
        files_in(directory).is_empty(),
        "successful publication must consume its private partial"
    );
}

fn files_in(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .expect("read partial directory")
        .map(|entry| entry.expect("partial entry").path())
        .collect()
}

fn patterned_body(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("pattern byte"))
        .collect()
}

static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);

struct TestDirectories {
    root: PathBuf,
    output: PathBuf,
    partials: PathBuf,
}

impl TestDirectories {
    fn new(label: &str) -> Self {
        let unique = NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!(
            "download-manager-pu03-{label}-{}-{unique}",
            std::process::id()
        ));
        let output = root.join("output");
        let partials = root.join("partials");
        fs::create_dir_all(&output).expect("create output directory");
        fs::create_dir_all(&partials).expect("create partial directory");
        Self {
            root,
            output,
            partials,
        }
    }

    fn final_path(&self) -> PathBuf {
        self.output.join("fixture.bin")
    }
}

impl Drop for TestDirectories {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

#[derive(Clone, Copy)]
enum FixtureKind {
    Known,
    UnknownLength,
    Redirect,
    Truncated,
    Delayed,
}

#[derive(Clone, Debug)]
struct CapturedRequest {
    method: String,
    target: String,
}

struct FixtureServer {
    address: std::net::SocketAddr,
    requests: Arc<Mutex<Vec<CapturedRequest>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl FixtureServer {
    fn start(kind: FixtureKind, body: Vec<u8>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP fixture");
        listener
            .set_nonblocking(true)
            .expect("make fixture listener nonblocking");
        let address = listener.local_addr().expect("fixture address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let stop = Arc::new(AtomicBool::new(false));
        let should_stop = Arc::clone(&stop);
        let worker = thread::spawn(move || {
            while !should_stop.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream
                            .set_nonblocking(false)
                            .expect("make fixture connection blocking");
                        let request = read_request(&mut stream);
                        captured
                            .lock()
                            .expect("captured request lock")
                            .push(request.clone());
                        respond(&mut stream, kind, &body, &request);
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

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    fn requests(&self) -> Vec<CapturedRequest> {
        self.requests.lock().expect("captured request lock").clone()
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
        .expect("fixture read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).expect("fixture request bytes");
        assert!(count > 0, "client closed before sending request headers");
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() <= 64 * 1024, "request headers are bounded");
    }
    let text = String::from_utf8(bytes).expect("ASCII fixture request");
    let mut parts = text
        .split("\r\n")
        .next()
        .expect("request line")
        .split_whitespace();
    CapturedRequest {
        method: parts.next().expect("request method").to_owned(),
        target: parts.next().expect("request target").to_owned(),
    }
}

fn respond(stream: &mut TcpStream, kind: FixtureKind, body: &[u8], request: &CapturedRequest) {
    if matches!(kind, FixtureKind::Redirect) && request.target == "/redirect" {
        stream
            .write_all(b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("redirect response");
        return;
    }

    let include_length = !matches!(kind, FixtureKind::UnknownLength);
    let mut headers = String::from("HTTP/1.1 200 OK\r\nConnection: close\r\n");
    if include_length {
        use std::fmt::Write as _;
        write!(headers, "Content-Length: {}\r\n", body.len()).expect("content length");
    }
    headers.push_str("Content-Type: application/octet-stream\r\n\r\n");
    stream
        .write_all(headers.as_bytes())
        .expect("fixture response headers");
    if request.method == "HEAD" {
        return;
    }
    if matches!(kind, FixtureKind::Delayed) {
        thread::sleep(Duration::from_millis(250));
    }
    if matches!(kind, FixtureKind::Truncated) {
        stream
            .write_all(&body[..body.len() / 3])
            .expect("truncated response prefix");
        let _ = stream.shutdown(Shutdown::Both);
    } else {
        stream.write_all(body).expect("fixture response body");
    }
}
