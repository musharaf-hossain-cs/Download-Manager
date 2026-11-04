use std::{
    fmt::Write as _,
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
use download_manager_engine::storage::Database;
use download_manager_engine::{domain::HttpUrl, ipc::JobAction, state::DownloadState};

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
fn queued_pause_is_not_overridden_when_fifo_worker_reaches_the_job() {
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
        DownloadState::Queued
    );
    assert_eq!(
        backend
            .control(second.id, JobAction::Pause)
            .expect("pause queued job")
            .state,
        DownloadState::Paused
    );
    wait_for_state(&mut backend, first.id, DownloadState::Completed);
    assert_eq!(
        fs::read(directories.output.join("first.bin")).expect("first file"),
        body
    );
    assert_eq!(
        backend.status(second.id).expect("paused second job").state,
        DownloadState::Paused
    );
    assert!(!directories.output.join("second.bin").exists());
    backend
        .control(second.id, JobAction::Cancel)
        .expect("cancel paused test job");
}

#[test]
fn shutdown_pauses_active_and_buffered_fifo_work_without_starting_the_second_get() {
    let active_body = patterned_body(2 * 1024 * 1024 + 13);
    let active_server = RangeFixtureServer::start(active_body, "\"shutdown-v1\"");
    let queued_body = patterned_body(64 * 1024 + 7);
    let queued_server = FixtureServer::start(FixtureKind::Known, queued_body);
    let directories = TestDirectories::new("shutdown-queue");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let first = add_named(
        &mut backend,
        &active_server.url("/active.bin"),
        &directories,
        "active.bin",
    );
    wait_for_progress(&mut backend, first.id);
    let second = add_named(
        &mut backend,
        &queued_server.url("/queued.bin"),
        &directories,
        "queued.bin",
    );
    assert_eq!(
        backend.status(second.id).expect("queued status").state,
        DownloadState::Queued
    );

    drop(backend);
    assert!(
        queued_server
            .requests()
            .iter()
            .all(|request| request.method != "GET"),
        "shutdown must not drain buffered work into a new transfer"
    );
    let mut restarted = DownloadBackend::new(&directories.partials).expect("reopen backend");
    assert_eq!(
        restarted.status(first.id).expect("active checkpoint").state,
        DownloadState::Paused
    );
    assert_eq!(
        restarted
            .status(second.id)
            .expect("queued checkpoint")
            .state,
        DownloadState::Paused
    );
    restarted
        .control(first.id, JobAction::Cancel)
        .expect("cancel active fixture job");
    restarted
        .control(second.id, JobAction::Cancel)
        .expect("cancel queued fixture job");
}

#[test]
fn paused_download_reopens_and_resumes_to_exact_bytes() {
    let body = patterned_body(768 * 1024 + 29);
    let server = RangeFixtureServer::start(body.clone(), "\"v1\"");
    let directories = TestDirectories::new("durable-resume");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/resumable.bin"), &directories);
    wait_for_progress(&mut backend, job.id);
    backend
        .control(job.id, JobAction::Pause)
        .expect("request pause");
    let paused = wait_for_state(&mut backend, job.id, DownloadState::Paused);
    assert!(paused.downloaded_bytes > 0);
    assert!(paused.downloaded_bytes < body.len() as u64);
    drop(backend);

    let durable =
        Database::open(directories.root.join("downloads.sqlite3")).expect("open durable database");
    let record = durable
        .get_job(job.id)
        .expect("load durable job")
        .expect("durable job record");
    assert_eq!(record.metadata.state, DownloadState::Paused);
    assert!(record.remote_identity.is_some());
    assert_eq!(
        durable.segments(job.id).expect("durable checkpoint")[0].durable_offset,
        paused.downloaded_bytes
    );
    assert!(
        !durable.events(job.id).expect("durable events").is_empty(),
        "state transitions must be journaled"
    );
    drop(durable);

    let mut restarted = DownloadBackend::new(&directories.partials).expect("reopen backend");
    assert_eq!(
        restarted.status(job.id).expect("durable paused job"),
        paused
    );
    restarted
        .control(job.id, JobAction::Resume)
        .expect("resume durable job");
    let completed = wait_for_state(&mut restarted, job.id, DownloadState::Completed);

    assert_eq!(completed.downloaded_bytes, body.len() as u64);
    assert_eq!(
        fs::read(directories.final_path()).expect("final file"),
        body
    );
    assert_directory_empty(&directories.partials);
    let expected_range = format!("bytes={}-", paused.downloaded_bytes);
    assert!(
        server
            .requests()
            .iter()
            .any(
                |request| request.range.as_deref() == Some(expected_range.as_str())
                    && request.if_range.as_deref() == Some("\"v1\"")
            )
    );
}

#[test]
fn changed_remote_identity_refuses_append_without_touching_partial() {
    let body = patterned_body(640 * 1024 + 11);
    let server = RangeFixtureServer::start(body, "\"v1\"");
    let directories = TestDirectories::new("changed-identity");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/identity.bin"), &directories);
    wait_for_progress(&mut backend, job.id);
    backend
        .control(job.id, JobAction::Pause)
        .expect("request pause");
    wait_for_state(&mut backend, job.id, DownloadState::Paused);
    let partial = files_in(&directories.partials)
        .into_iter()
        .next()
        .expect("retained partial");
    let before = fs::read(&partial).expect("partial before identity change");
    server.set_etag("\"v2\"");

    backend
        .control(job.id, JobAction::Resume)
        .expect("queue validated resume");
    let failed = wait_for_state(&mut backend, job.id, DownloadState::Failed);
    assert!(
        failed
            .failure_reason
            .as_deref()
            .is_some_and(|reason| reason.contains("identity changed")),
        "unexpected failure: {failed:?}"
    );
    assert_eq!(fs::read(partial).expect("unchanged partial"), before);
    assert!(!directories.final_path().exists());
}

#[test]
fn cancel_stops_active_work_without_publishing_a_completed_file() {
    let body = patterned_body(640 * 1024 + 19);
    let server = RangeFixtureServer::start(body, "\"v1\"");
    let directories = TestDirectories::new("cancel");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/cancel.bin"), &directories);
    wait_for_progress(&mut backend, job.id);
    let cancelled = backend
        .control(job.id, JobAction::Cancel)
        .expect("cancel active job");
    assert_eq!(cancelled.state, DownloadState::Cancelled);
    drop(backend);

    let mut restarted = DownloadBackend::new(&directories.partials).expect("reopen backend");
    assert_eq!(
        restarted
            .status(job.id)
            .expect("durable cancellation")
            .state,
        DownloadState::Cancelled
    );
    assert!(!directories.final_path().exists());
}

#[test]
fn stalled_peer_pause_and_clean_shutdown_finish_promptly() {
    let body = patterned_body(128 * 1024 + 3);
    let server = FixtureServer::start(FixtureKind::Stalled, body);
    let directories = TestDirectories::new("stalled-pause");
    let mut backend = DownloadBackend::new(&directories.partials).expect("create backend");

    let job = add(&mut backend, &server.url("/stalled.bin"), &directories);
    wait_for_state(&mut backend, job.id, DownloadState::Downloading);
    let started = Instant::now();
    backend
        .control(job.id, JobAction::Pause)
        .expect("pause stalled transfer");
    let paused = wait_for_state(&mut backend, job.id, DownloadState::Paused);
    assert_eq!(paused.downloaded_bytes, 0);
    assert!(
        started.elapsed() < Duration::from_secs(8),
        "pause must be bounded by the transfer control polling interval"
    );
    drop(backend);

    let mut restarted = DownloadBackend::new(&directories.partials).expect("reopen backend");
    assert_eq!(
        restarted.status(job.id).expect("durable paused job").state,
        DownloadState::Paused
    );
    restarted
        .control(job.id, JobAction::Cancel)
        .expect("cancel stalled fixture job");
    assert!(!directories.final_path().exists());
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

fn wait_for_progress(
    backend: &mut DownloadBackend,
    job_id: download_manager_engine::domain::JobId,
) -> JobView {
    let deadline = Instant::now() + TERMINAL_TIMEOUT;
    loop {
        let job = backend.status(job_id).expect("read job progress");
        if job.state == DownloadState::Downloading && job.downloaded_bytes > 0 {
            return job;
        }
        assert!(
            !matches!(
                job.state,
                DownloadState::Completed | DownloadState::Failed | DownloadState::Cancelled
            ),
            "job terminated before control could be exercised: {job:?}"
        );
        assert!(Instant::now() < deadline, "job progress timed out: {job:?}");
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
    Stalled,
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

#[derive(Clone, Debug)]
struct RangeRequest {
    method: String,
    range: Option<String>,
    if_range: Option<String>,
}

struct RangeFixtureServer {
    address: std::net::SocketAddr,
    etag: Arc<Mutex<String>>,
    requests: Arc<Mutex<Vec<RangeRequest>>>,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl RangeFixtureServer {
    fn start(body: Vec<u8>, etag: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind range fixture");
        listener
            .set_nonblocking(true)
            .expect("make range listener nonblocking");
        let address = listener.local_addr().expect("range fixture address");
        let etag = Arc::new(Mutex::new(etag.to_owned()));
        let response_etag = Arc::clone(&etag);
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
                            .expect("make range connection blocking");
                        let request = read_range_request(&mut stream);
                        captured
                            .lock()
                            .expect("range request lock")
                            .push(request.clone());
                        respond_range(
                            &mut stream,
                            &body,
                            &response_etag.lock().expect("ETag lock"),
                            &request,
                        );
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("range fixture accept failed: {error}"),
                }
            }
        });
        Self {
            address,
            etag,
            requests,
            stop,
            worker: Some(worker),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }

    fn set_etag(&self, value: &str) {
        value.clone_into(&mut self.etag.lock().expect("ETag lock"));
    }

    fn requests(&self) -> Vec<RangeRequest> {
        self.requests.lock().expect("range request lock").clone()
    }
}

impl Drop for RangeFixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().expect("range fixture worker");
        }
    }
}

fn read_range_request(stream: &mut TcpStream) -> RangeRequest {
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("range fixture read timeout");
    let mut bytes = Vec::new();
    let mut buffer = [0_u8; 1024];
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        let count = stream.read(&mut buffer).expect("range request bytes");
        assert!(count > 0, "client closed before range request headers");
        bytes.extend_from_slice(&buffer[..count]);
        assert!(bytes.len() <= 64 * 1024, "range headers are bounded");
    }
    let text = String::from_utf8(bytes).expect("ASCII range request");
    let mut lines = text.split("\r\n");
    let method = lines
        .next()
        .and_then(|line| line.split_whitespace().next())
        .expect("range request method")
        .to_owned();
    let header = |name: &str| {
        text.split("\r\n").find_map(|line| {
            let (found, value) = line.split_once(':')?;
            found
                .eq_ignore_ascii_case(name)
                .then(|| value.trim().to_owned())
        })
    };
    RangeRequest {
        method,
        range: header("Range"),
        if_range: header("If-Range"),
    }
}

fn respond_range(stream: &mut TcpStream, body: &[u8], etag: &str, request: &RangeRequest) {
    let start = request
        .range
        .as_deref()
        .and_then(|value| value.strip_prefix("bytes="))
        .and_then(|value| value.strip_suffix('-'))
        .and_then(|value| value.parse::<usize>().ok());
    let mut headers = if let Some(start) = start {
        format!(
            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\n",
            body.len() - start,
            start,
            body.len() - 1,
            body.len()
        )
    } else {
        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n", body.len())
    };
    write!(
        headers,
        "ETag: {etag}\r\nAccept-Ranges: bytes\r\nContent-Type: application/octet-stream\r\nConnection: close\r\n\r\n"
    )
    .expect("range response headers");
    if stream.write_all(headers.as_bytes()).is_err() || request.method == "HEAD" {
        return;
    }
    let start = start.unwrap_or(0);
    for chunk in body[start..].chunks(8 * 1024) {
        if stream.write_all(chunk).is_err() || stream.flush().is_err() {
            break;
        }
        thread::sleep(Duration::from_millis(2));
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
        write!(headers, "Content-Length: {}\r\n", body.len()).expect("content length");
    }
    headers.push_str("Content-Type: application/octet-stream\r\n\r\n");
    stream
        .write_all(headers.as_bytes())
        .expect("fixture response headers");
    if request.method == "HEAD" {
        return;
    }
    if matches!(kind, FixtureKind::Stalled) {
        thread::sleep(Duration::from_secs(7));
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
