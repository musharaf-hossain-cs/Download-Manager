#![cfg(windows)]

use std::{
    path::PathBuf,
    process::{Child, Command},
    thread,
    time::{Duration, Instant},
};

use download_manager_cli::AuthenticatedIpcBackend;

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
    second.shutdown().expect("authenticated shutdown");
    wait_for_exit(&mut clean);

    assert!(
        !manifest_path().exists(),
        "normal shutdown must remove owned discovery state"
    );
    assert!(AuthenticatedIpcBackend::discover_current_user().is_err());
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
