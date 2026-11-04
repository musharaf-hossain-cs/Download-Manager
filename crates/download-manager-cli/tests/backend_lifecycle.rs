#![cfg(windows)]

use std::{
    fs,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    process::{Child, Command},
    thread,
    thread::JoinHandle,
    time::{Duration, Instant},
};

use download_manager_cli::AuthenticatedIpcBackend;
use download_manager_engine::{
    domain::JobId,
    integrity::{HashAlgorithm, VerificationControl, verify_file},
    storage::Database,
};

const TIMEOUT: Duration = Duration::from_secs(15);

#[test]
fn real_backend_health_crash_rejection_and_clean_shutdown() {
    verify_crash_stale_discovery_is_rejected();

    let mut clean = spawn_backend();
    let mut second = wait_for_backend(&mut clean);
    assert_eq!(
        second.health().expect("replacement backend health").version,
        download_manager_engine::VERSION
    );
    let output_directory = std::env::temp_dir().join(format!(
        "download-manager-pu04-cli-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&output_directory).expect("create CLI output directory");
    let completed_id = complete_known_download(&output_directory);
    let (resumable_id, paused_bytes, resume_body, resume_fixture) =
        pause_resumable_download(&output_directory);

    second.shutdown().expect("authenticated shutdown");
    wait_for_exit(&mut clean);
    resume_after_replacement_backend(
        &resumable_id,
        paused_bytes,
        &resume_body,
        &output_directory,
        resume_fixture,
    );

    assert!(
        !manifest_path().exists(),
        "normal shutdown must remove owned discovery state"
    );
    assert!(AuthenticatedIpcBackend::discover_current_user().is_err());
    cleanup_jobs(&[&completed_id, &resumable_id]);
    fs::remove_dir_all(output_directory).expect("cleanup CLI output directory");
}

fn verify_crash_stale_discovery_is_rejected() {
    let mut crashed = spawn_backend();
    let mut first = wait_for_backend(&mut crashed);
    assert_eq!(
        first.health().expect("first backend health").version,
        download_manager_engine::VERSION
    );

    crashed.kill().expect("simulate backend crash");
    assert!(
        crashed
            .wait()
            .expect("reap crashed backend")
            .code()
            .is_some()
    );
    assert!(
        first.health().is_err(),
        "a client discovered before the crash must reject the stale endpoint"
    );
    assert!(
        AuthenticatedIpcBackend::discover_current_user().is_err(),
        "stale discovery must not be accepted as a healthy backend"
    );
}

fn complete_known_download(output_directory: &std::path::Path) -> String {
    let body = b"PU03 known fixture\n".to_vec();
    let (url, fixture) = known_length_fixture(body.clone());
    let add = run_cli([
        "add".into(),
        url.into(),
        "--output".into(),
        output_directory.as_os_str().to_owned(),
        "--wait".into(),
        "--timeout".into(),
        "10".into(),
        "--json".into(),
    ]);
    assert!(
        add.status.success(),
        "packaged-style add failed: {}",
        String::from_utf8_lossy(&add.stderr)
    );
    fixture.join().expect("fixture thread");
    let added: serde_json::Value = serde_json::from_slice(&add.stdout).expect("add JSON");
    assert_eq!(added["data"]["state"], "completed");
    assert_eq!(added["data"]["downloaded_bytes"], body.len());
    assert_eq!(added["data"]["total_bytes"], body.len());
    let job_id = added["data"]["id"].as_str().expect("job ID").to_owned();
    let final_path = output_directory.join("fixture.bin");
    assert_eq!(fs::read(&final_path).expect("completed file"), body);
    let verified = verify_file(
        &final_path,
        HashAlgorithm::Sha256,
        Some("8b31a5eef787129703bb71e1dbeb28dd69b82ffd55810a729208d0306bf1c878"),
        &VerificationControl::default(),
        |_| {},
    )
    .expect("fixture SHA-256");
    assert_eq!(verified.bytes_verified, body.len() as u64);

    let status = run_cli(["status".into(), job_id.as_str().into(), "--json".into()]);
    assert!(status.status.success(), "status command");
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(status["data"]["state"], "completed");
    let list = run_cli(["list".into(), "--json".into()]);
    assert!(list.status.success(), "list command");
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).expect("list JSON");
    assert!(
        list["data"]
            .as_array()
            .expect("jobs")
            .iter()
            .any(|job| job["id"] == job_id)
    );
    job_id
}

fn pause_resumable_download(
    output_directory: &std::path::Path,
) -> (String, u64, Vec<u8>, JoinHandle<()>) {
    let resume_body = patterned_body(2 * 1024 * 1024 + 37);
    let (resume_url, resume_fixture) = ranged_fixture(resume_body.clone());
    let add_resume = run_cli([
        "add".into(),
        resume_url.into(),
        "--output".into(),
        output_directory.as_os_str().to_owned(),
        "--name".into(),
        "resumed.bin".into(),
        "--json".into(),
    ]);
    assert!(
        add_resume.status.success(),
        "resumable add failed: {}",
        String::from_utf8_lossy(&add_resume.stderr)
    );
    let resumable: serde_json::Value =
        serde_json::from_slice(&add_resume.stdout).expect("resumable add JSON");
    let resumable_id = resumable["data"]["id"]
        .as_str()
        .expect("resumable job ID")
        .to_owned();
    wait_for_cli_progress(&resumable_id);
    let pause = run_cli(["pause".into(), resumable_id.clone().into(), "--json".into()]);
    assert!(
        pause.status.success(),
        "pause failed: {}",
        String::from_utf8_lossy(&pause.stderr)
    );
    let paused = wait_for_cli_state(&resumable_id, "paused");
    let paused_bytes = paused["data"]["downloaded_bytes"]
        .as_u64()
        .expect("paused bytes");
    assert!(paused_bytes > 0 && paused_bytes < resume_body.len() as u64);
    (resumable_id, paused_bytes, resume_body, resume_fixture)
}

fn resume_after_replacement_backend(
    resumable_id: &str,
    paused_bytes: u64,
    resume_body: &[u8],
    output_directory: &std::path::Path,
    resume_fixture: JoinHandle<()>,
) {
    let mut replacement = spawn_backend();
    let mut third = wait_for_backend(&mut replacement);
    let durable = wait_for_cli_state(resumable_id, "paused");
    assert_eq!(durable["data"]["downloaded_bytes"], paused_bytes);
    let resume = run_cli(["resume".into(), resumable_id.into(), "--json".into()]);
    assert!(
        resume.status.success(),
        "resume failed: {}",
        String::from_utf8_lossy(&resume.stderr)
    );
    let resumed = wait_for_cli_state(resumable_id, "completed");
    assert_eq!(resumed["data"]["downloaded_bytes"], resume_body.len());
    assert_eq!(
        fs::read(output_directory.join("resumed.bin")).expect("resumed final file"),
        resume_body
    );
    resume_fixture.join().expect("range fixture thread");
    third.shutdown().expect("replacement shutdown");
    wait_for_exit(&mut replacement);
}

fn wait_for_cli_progress(job_id: &str) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let status = cli_status(job_id);
        if status["data"]["state"] == "downloading"
            && status["data"]["downloaded_bytes"].as_u64().unwrap_or(0) > 0
        {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "CLI progress timed out: {status}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn wait_for_cli_state(job_id: &str, expected: &str) -> serde_json::Value {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        let status = cli_status(job_id);
        if status["data"]["state"] == expected {
            return status;
        }
        assert!(
            Instant::now() < deadline,
            "CLI state {expected} timed out: {status}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn cli_status(job_id: &str) -> serde_json::Value {
    let status = run_cli(["status".into(), job_id.into(), "--json".into()]);
    assert!(
        status.status.success(),
        "status failed: {}",
        String::from_utf8_lossy(&status.stderr)
    );
    serde_json::from_slice(&status.stdout).expect("status JSON")
}

fn cleanup_jobs(job_ids: &[&str]) {
    let state = PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("DownloadManager")
        .join("State");
    let mut database = Database::open(state.join("downloads.sqlite3")).expect("open test state");
    for job_id in job_ids {
        let parsed: JobId = job_id.parse().expect("cleanup job ID");
        database.delete_job(parsed).expect("delete test job");
        let partial = state.join("partials").join(format!("{parsed}.part"));
        if partial.exists() {
            fs::remove_file(partial).expect("remove test partial");
        }
    }
}

fn known_length_fixture(body: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind HTTP fixture");
    let address = listener.local_addr().expect("fixture address");
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (mut stream, _) = listener.accept().expect("accept fixture request");
            let request = read_request(&mut stream);
            let is_head = request.starts_with("HEAD ");
            write!(
                stream,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Disposition: attachment; filename=\"fixture.bin\"\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .expect("write fixture headers");
            if !is_head {
                stream.write_all(&body).expect("write fixture body");
            }
        }
    });
    (format!("http://{address}/fixture.bin"), server)
}

fn ranged_fixture(body: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind ranged fixture");
    let address = listener.local_addr().expect("ranged fixture address");
    let server = thread::spawn(move || {
        for _ in 0..4 {
            let (mut stream, _) = listener.accept().expect("accept ranged request");
            let request = read_request(&mut stream);
            let method = request
                .split_whitespace()
                .next()
                .expect("ranged request method");
            let range = request.split("\r\n").find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("range")
                    .then(|| value.trim().to_owned())
            });
            let start = range
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
            headers.push_str(
                "ETag: \"process-v1\"\r\nAccept-Ranges: bytes\r\nContent-Disposition: attachment; filename=\"resumed.bin\"\r\nConnection: close\r\n\r\n",
            );
            if stream.write_all(headers.as_bytes()).is_err() || method == "HEAD" {
                continue;
            }
            let start = start.unwrap_or(0);
            for chunk in body[start..].chunks(8 * 1024) {
                if stream.write_all(chunk).is_err() || stream.flush().is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    });
    (format!("http://{address}/resumable.bin"), server)
}

fn patterned_body(length: usize) -> Vec<u8> {
    (0..length)
        .map(|index| u8::try_from(index % 251).expect("pattern byte"))
        .collect()
}

fn read_request(stream: &mut TcpStream) -> String {
    let mut request = Vec::new();
    let mut buffer = [0_u8; 512];
    while !request.ends_with(b"\r\n\r\n") {
        let count = stream.read(&mut buffer).expect("read fixture request");
        assert!(count > 0, "fixture request ended before headers");
        request.extend_from_slice(&buffer[..count]);
        assert!(request.len() <= 16 * 1024, "fixture request too large");
    }
    String::from_utf8(request).expect("fixture request UTF-8")
}

fn run_cli<I>(args: I) -> std::process::Output
where
    I: IntoIterator<Item = std::ffi::OsString>,
{
    Command::new(env!("CARGO_BIN_EXE_download-manager-cli"))
        .args(args)
        .output()
        .expect("run packaged-style CLI")
}

fn spawn_backend() -> Child {
    Command::new(env!("CARGO_BIN_EXE_download-manager-backend"))
        .spawn()
        .expect("start real backend executable")
}

fn wait_for_backend(child: &mut Child) -> AuthenticatedIpcBackend {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("query backend") {
            panic!("backend exited before health: {status}");
        }
        if let Ok(backend) = AuthenticatedIpcBackend::discover_current_user() {
            return backend;
        }
        assert!(Instant::now() < deadline, "backend health timed out");
        thread::sleep(Duration::from_millis(50));
    }
}

fn wait_for_exit(child: &mut Child) {
    let deadline = Instant::now() + TIMEOUT;
    loop {
        if let Some(status) = child.try_wait().expect("query backend") {
            assert!(status.success(), "backend shutdown failed: {status}");
            return;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("backend shutdown timed out");
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn manifest_path() -> PathBuf {
    PathBuf::from(std::env::var_os("LOCALAPPDATA").expect("LOCALAPPDATA"))
        .join("DownloadManager")
        .join("ipc")
        .join("cli-ipc-v1.json")
}
