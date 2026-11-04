use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};

use download_manager_cli::{AuthenticatedIpcBackend, BackendError};
use download_manager_engine::ipc::decode_ui_request;
use serde::Serialize;
use tauri::Manager;

const STARTUP_ATTEMPTS: usize = 200;
const STARTUP_RETRY_DELAY: Duration = Duration::from_millis(50);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum BackendOrigin {
    Existing,
    Started,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct BackendStatus {
    ready: bool,
    version: Option<String>,
    origin: Option<BackendOrigin>,
    message: String,
}

impl BackendStatus {
    fn ready(version: String, origin: BackendOrigin) -> Self {
        let message = match origin {
            BackendOrigin::Existing => "Connected to the current-user backend.",
            BackendOrigin::Started => "Started and connected to the current-user backend.",
        };
        Self {
            ready: true,
            version: Some(version),
            origin: Some(origin),
            message: message.to_owned(),
        }
    }

    fn failed(message: String) -> Self {
        Self {
            ready: false,
            version: None,
            origin: None,
            message,
        }
    }
}

trait BackendConnector {
    type Connection;

    fn connect(&mut self) -> Result<(Self::Connection, String), String>;
}

trait ManagedChild {
    fn try_exit(&mut self) -> Result<Option<String>, String>;
    fn terminate(&mut self) -> Result<(), String>;
}

trait BackendLauncher {
    type Child: ManagedChild;

    fn start(&mut self) -> Result<Self::Child, String>;
}

struct LifecycleSession<C, P> {
    connection: Option<C>,
    child: Option<P>,
}

impl<C, P> LifecycleSession<C, P> {
    fn existing(connection: C) -> Self {
        Self {
            connection: Some(connection),
            child: None,
        }
    }

    fn started(connection: C, child: P) -> Self {
        Self {
            connection: Some(connection),
            child: Some(child),
        }
    }

    fn close(mut self) {
        drop(self.connection.take());
        drop(self.child.take());
    }

    fn connection_mut(&mut self) -> Option<&mut C> {
        self.connection.as_mut()
    }
}

struct StartedBackend<C, P> {
    session: LifecycleSession<C, P>,
    version: String,
    origin: BackendOrigin,
}

fn connect_or_start<C, L>(
    connector: &mut C,
    launcher: &mut L,
    attempts: usize,
    retry_delay: Duration,
) -> Result<StartedBackend<C::Connection, L::Child>, String>
where
    C: BackendConnector,
    L: BackendLauncher,
{
    if let Ok((connection, version)) = connector.connect() {
        return Ok(StartedBackend {
            session: LifecycleSession::existing(connection),
            version,
            origin: BackendOrigin::Existing,
        });
    }

    let mut child = launcher
        .start()
        .map_err(|error| format!("Could not start the local backend: {error}"))?;
    let mut last_error = "the backend has not published its authenticated endpoint".to_owned();

    for attempt in 0..attempts.max(1) {
        match connector.connect() {
            Ok((connection, version)) => {
                return Ok(StartedBackend {
                    session: LifecycleSession::started(connection, child),
                    version,
                    origin: BackendOrigin::Started,
                });
            }
            Err(error) => last_error = error,
        }

        if let Some(status) = child
            .try_exit()
            .map_err(|error| format!("Could not inspect the local backend process: {error}"))?
        {
            return Err(format!(
                "The local backend exited before it became healthy ({status}). {last_error}"
            ));
        }

        if attempt + 1 < attempts.max(1) && !retry_delay.is_zero() {
            thread::sleep(retry_delay);
        }
    }

    let cleanup = child.terminate().err();
    let cleanup_note = cleanup
        .map(|error| format!(" Cleanup also failed: {error}"))
        .unwrap_or_default();
    Err(format!(
        "The local backend did not become healthy within the startup timeout. {last_error}{cleanup_note}"
    ))
}

struct CurrentUserConnector;

impl BackendConnector for CurrentUserConnector {
    type Connection = AuthenticatedIpcBackend;

    fn connect(&mut self) -> Result<(Self::Connection, String), String> {
        let mut connection =
            AuthenticatedIpcBackend::discover_current_user().map_err(|error| error.to_string())?;
        let version = connection
            .health()
            .map_err(|error| format!("backend health check failed: {error:?}"))?
            .version;
        Ok((connection, version))
    }
}

struct ProcessLauncher {
    executable: PathBuf,
}

impl BackendLauncher for ProcessLauncher {
    type Child = Child;

    fn start(&mut self) -> Result<Self::Child, String> {
        if !self.executable.is_file() {
            return Err(format!(
                "backend executable was not found at {}",
                self.executable.display()
            ));
        }

        let mut command = Command::new(&self.executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        configure_background_process(&mut command);
        command
            .spawn()
            .map_err(|error| format!("could not launch {}: {error}", self.executable.display()))
    }
}

impl ManagedChild for Child {
    fn try_exit(&mut self) -> Result<Option<String>, String> {
        self.try_wait()
            .map(|status| status.map(|value| value.to_string()))
            .map_err(|error| error.to_string())
    }

    fn terminate(&mut self) -> Result<(), String> {
        self.kill().map_err(|error| error.to_string())?;
        self.wait().map_err(|error| error.to_string())?;
        Ok(())
    }
}

#[cfg(windows)]
fn configure_background_process(command: &mut Command) {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    command.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(not(windows))]
fn configure_background_process(_command: &mut Command) {}

type ProductionSession = LifecycleSession<AuthenticatedIpcBackend, Child>;

struct DesktopState {
    status: BackendStatus,
    session: Mutex<Option<ProductionSession>>,
}

impl DesktopState {
    fn initialize() -> Self {
        let result = backend_executable().and_then(|executable| {
            connect_or_start(
                &mut CurrentUserConnector,
                &mut ProcessLauncher { executable },
                STARTUP_ATTEMPTS,
                STARTUP_RETRY_DELAY,
            )
        });
        match result {
            Ok(started) => Self {
                status: BackendStatus::ready(started.version, started.origin),
                session: Mutex::new(Some(started.session)),
            },
            Err(error) => Self {
                status: BackendStatus::failed(error),
                session: Mutex::new(None),
            },
        }
    }

    fn close(&self) {
        let session = self
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        if let Some(session) = session {
            session.close();
        }
    }
}

fn backend_executable() -> Result<PathBuf, String> {
    let current = std::env::current_exe()
        .map_err(|error| format!("Could not locate the desktop executable: {error}"))?;
    let directory = current
        .parent()
        .ok_or_else(|| "The desktop executable has no parent directory.".to_owned())?;
    Ok(directory.join(format!(
        "download-manager-backend{}",
        std::env::consts::EXE_SUFFIX
    )))
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands extract managed state by value.
fn backend_status(state: tauri::State<'_, DesktopState>) -> BackendStatus {
    state.status.clone()
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands extract managed state by value.
fn backend_request(
    state: tauri::State<'_, DesktopState>,
    request: String,
) -> Result<String, String> {
    let mut session = state
        .session
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let backend = session
        .as_mut()
        .and_then(LifecycleSession::connection_mut)
        .ok_or_else(|| "The local backend is not connected.".to_owned())?;
    let request = decode_ui_request(request.as_bytes())
        .map_err(|error| format!("The desktop request is invalid: {error}"))?;
    let response = backend
        .desktop_request(request)
        .map_err(format_backend_error)?;
    serde_json::to_string(&response)
        .map_err(|error| format!("The backend response could not be encoded: {error}"))
}

fn format_backend_error(error: BackendError) -> String {
    match error {
        BackendError::NotFound(message) | BackendError::Transport(message) => message,
        BackendError::Rejected { code, message } => format!("{code}: {message}"),
    }
}

/// Runs the one-window private-use desktop shell.
///
/// # Panics
/// Panics if Tauri cannot construct the desktop runtime from the checked-in configuration.
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .manage(DesktopState::initialize())
        .invoke_handler(tauri::generate_handler![backend_status, backend_request])
        .build(tauri::generate_context!())
        .expect("failed to build the Download Manager desktop shell");

    app.run(|handle, event| {
        if matches!(event, tauri::RunEvent::Exit) {
            handle.state::<DesktopState>().close();
        }
    });
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, collections::VecDeque, rc::Rc, time::Duration};

    use super::{BackendConnector, BackendLauncher, BackendOrigin, ManagedChild, connect_or_start};

    #[derive(Clone)]
    struct DropProbe(Rc<Cell<usize>>);

    impl Drop for DropProbe {
        fn drop(&mut self) {
            self.0.set(self.0.get() + 1);
        }
    }

    struct FakeConnector {
        results: VecDeque<Result<String, String>>,
        connection_drops: Rc<Cell<usize>>,
    }

    impl BackendConnector for FakeConnector {
        type Connection = DropProbe;

        fn connect(&mut self) -> Result<(Self::Connection, String), String> {
            self.results
                .pop_front()
                .unwrap_or_else(|| Err("still unavailable".to_owned()))
                .map(|version| (DropProbe(Rc::clone(&self.connection_drops)), version))
        }
    }

    struct FakeChild {
        exit: Option<String>,
        terminated: Rc<Cell<usize>>,
        drops: Rc<Cell<usize>>,
    }

    impl ManagedChild for FakeChild {
        fn try_exit(&mut self) -> Result<Option<String>, String> {
            Ok(self.exit.clone())
        }

        fn terminate(&mut self) -> Result<(), String> {
            self.terminated.set(self.terminated.get() + 1);
            Ok(())
        }
    }

    impl Drop for FakeChild {
        fn drop(&mut self) {
            self.drops.set(self.drops.get() + 1);
        }
    }

    struct FakeLauncher {
        starts: Rc<Cell<usize>>,
        start_error: Option<String>,
        child_exit: Option<String>,
        terminated: Rc<Cell<usize>>,
        child_drops: Rc<Cell<usize>>,
    }

    impl BackendLauncher for FakeLauncher {
        type Child = FakeChild;

        fn start(&mut self) -> Result<Self::Child, String> {
            self.starts.set(self.starts.get() + 1);
            if let Some(error) = &self.start_error {
                return Err(error.clone());
            }
            Ok(FakeChild {
                exit: self.child_exit.clone(),
                terminated: Rc::clone(&self.terminated),
                drops: Rc::clone(&self.child_drops),
            })
        }
    }

    struct Harness {
        connector: FakeConnector,
        launcher: FakeLauncher,
        starts: Rc<Cell<usize>>,
        connection_drops: Rc<Cell<usize>>,
        terminated: Rc<Cell<usize>>,
        child_drops: Rc<Cell<usize>>,
    }

    impl Harness {
        fn new(results: impl IntoIterator<Item = Result<&'static str, &'static str>>) -> Self {
            let starts = Rc::new(Cell::new(0));
            let connection_drops = Rc::new(Cell::new(0));
            let terminated = Rc::new(Cell::new(0));
            let child_drops = Rc::new(Cell::new(0));
            Self {
                connector: FakeConnector {
                    results: results
                        .into_iter()
                        .map(|result| result.map(str::to_owned).map_err(str::to_owned))
                        .collect(),
                    connection_drops: Rc::clone(&connection_drops),
                },
                launcher: FakeLauncher {
                    starts: Rc::clone(&starts),
                    start_error: None,
                    child_exit: None,
                    terminated: Rc::clone(&terminated),
                    child_drops: Rc::clone(&child_drops),
                },
                starts,
                connection_drops,
                terminated,
                child_drops,
            }
        }
    }

    #[test]
    fn starts_backend_and_connects_after_health_is_published() {
        let mut harness = Harness::new([Err("no manifest"), Err("not healthy yet"), Ok("0.1.0")]);
        let started = connect_or_start(
            &mut harness.connector,
            &mut harness.launcher,
            3,
            Duration::ZERO,
        )
        .expect("start backend");

        assert_eq!(started.origin, BackendOrigin::Started);
        assert_eq!(started.version, "0.1.0");
        assert_eq!(harness.starts.get(), 1);
        assert_eq!(harness.terminated.get(), 0);
        started.session.close();
        assert_eq!(harness.connection_drops.get(), 1);
        assert_eq!(harness.child_drops.get(), 1);
        assert_eq!(harness.terminated.get(), 0);
    }

    #[test]
    fn connects_to_an_already_running_backend_without_spawning() {
        let mut harness = Harness::new([Ok("0.1.0")]);
        let started = connect_or_start(
            &mut harness.connector,
            &mut harness.launcher,
            1,
            Duration::ZERO,
        )
        .expect("connect existing backend");

        assert_eq!(started.origin, BackendOrigin::Existing);
        assert_eq!(harness.starts.get(), 0);
        started.session.close();
        assert_eq!(harness.connection_drops.get(), 1);
        assert_eq!(harness.child_drops.get(), 0);
    }

    #[test]
    fn reports_a_failed_start_with_an_actionable_error() {
        let mut harness = Harness::new([Err("no manifest")]);
        harness.launcher.start_error = Some("access denied".to_owned());
        let error = connect_or_start(
            &mut harness.connector,
            &mut harness.launcher,
            1,
            Duration::ZERO,
        )
        .err()
        .expect("startup must fail");

        assert!(error.contains("Could not start the local backend"));
        assert!(error.contains("access denied"));
        assert_eq!(harness.starts.get(), 1);
    }

    #[test]
    fn stops_only_an_unhealthy_process_after_the_startup_timeout() {
        let mut harness = Harness::new([
            Err("no manifest"),
            Err("health unavailable"),
            Err("health unavailable"),
        ]);
        let error = connect_or_start(
            &mut harness.connector,
            &mut harness.launcher,
            2,
            Duration::ZERO,
        )
        .err()
        .expect("health timeout must fail");

        assert!(error.contains("did not become healthy"));
        assert!(error.contains("health unavailable"));
        assert_eq!(harness.terminated.get(), 1);
        assert_eq!(harness.child_drops.get(), 1);
    }
}
