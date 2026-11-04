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
use download_manager_engine::integrity::{HashAlgorithm, VerificationControl, verify_file};

const TIMEOUT: Duration = Duration::from_secs(15);

#[test]
fn real_backend_health_crash_rejection_and_clean_shutdown() {
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

    let mut clean = spawn_backend();
    let mut second = wait_for_backend(&mut clean);
    assert_eq!(
        second.health().expect("replacement backend health").version,
        download_manager_engine::VERSION
    );

    let output_directory = std::env::temp_dir().join(format!(
        "download-manager-pu03-cli-{}",
        uuid::Uuid::new_v4()
    ));
    fs::create_dir_all(&output_directory).expect("create CLI output directory");
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
    let job_id = added["data"]["id"].as_str().expect("job ID");
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

    let status = run_cli(["status".into(), job_id.into(), "--json".into()]);
    assert!(status.status.success(), "status command");
    let status: serde_json::Value = serde_json::from_slice(&status.stdout).expect("status JSON");
    assert_eq!(status["data"]["state"], "completed");
    let list = run_cli(["list".into(), "--json".into()]);
    assert!(list.status.success(), "list command");
    let list: serde_json::Value = serde_json::from_slice(&list.stdout).expect("list JSON");
    assert_eq!(list["data"].as_array().expect("jobs").len(), 1);

    second.shutdown().expect("authenticated shutdown");
    wait_for_exit(&mut clean);

    assert!(
        !manifest_path().exists(),
        "normal shutdown must remove owned discovery state"
    );
    assert!(AuthenticatedIpcBackend::discover_current_user().is_err());
    fs::remove_dir_all(output_directory).expect("cleanup CLI output directory");
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
