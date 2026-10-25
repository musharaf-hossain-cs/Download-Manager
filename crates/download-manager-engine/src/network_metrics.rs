//! Transport-independent connection-pool and handshake counters.

use std::{
    collections::HashSet,
    sync::{Arc, Mutex},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionSnapshot {
    pub requests: u64,
    pub handshakes: u64,
    pub reused_requests: u64,
}

#[derive(Debug, Default)]
struct ConnectionState {
    seen_connections: HashSet<u64>,
    snapshot: ConnectionSnapshot,
}

/// Records request reuse by stable transport connection identity.
#[derive(Debug, Clone, Default)]
pub struct ConnectionMetricRecorder {
    state: Arc<Mutex<ConnectionState>>,
}

impl ConnectionMetricRecorder {
    /// Records one request assigned to a connection. A connection's first request counts as a
    /// handshake; subsequent requests count as pool reuse.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal metric lock.
    pub fn record_request(&self, connection_id: u64) {
        let mut state = self.state.lock().expect("connection metric lock");
        state.snapshot.requests += 1;
        if state.seen_connections.insert(connection_id) {
            state.snapshot.handshakes += 1;
        } else {
            state.snapshot.reused_requests += 1;
        }
    }

    /// Returns an atomic point-in-time metric snapshot.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal metric lock.
    #[must_use]
    pub fn snapshot(&self) -> ConnectionSnapshot {
        self.state.lock().expect("connection metric lock").snapshot
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        thread,
    };

    use reqwest::blocking::Client;

    use super::{ConnectionMetricRecorder, ConnectionSnapshot};

    #[test]
    fn keep_alive_fixture_reports_one_handshake_and_one_reuse() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let metrics = ConnectionMetricRecorder::default();
        let server_metrics = metrics.clone();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("accept one connection");
            for request_index in 0..2 {
                read_request(&mut stream);
                server_metrics.record_request(1);
                let connection = if request_index == 0 {
                    "keep-alive"
                } else {
                    "close"
                };
                write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 1\r\nConnection: {connection}\r\n\r\nx"
                )
                .expect("response");
                stream.flush().expect("flush");
            }
        });

        let client = Client::builder().build().expect("client");
        for path in ["first", "second"] {
            let body = client
                .get(format!("http://{address}/{path}"))
                .send()
                .expect("request")
                .text()
                .expect("body");
            assert_eq!(body, "x");
        }
        server.join().expect("server");
        assert_eq!(
            metrics.snapshot(),
            ConnectionSnapshot {
                requests: 2,
                handshakes: 1,
                reused_requests: 1,
            }
        );
    }

    fn read_request(stream: &mut TcpStream) {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request");
            assert!(count > 0, "connection closed before request headers");
            bytes.extend_from_slice(&buffer[..count]);
        }
    }
}
