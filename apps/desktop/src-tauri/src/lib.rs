use std::{
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::Mutex,
    thread,
    time::Duration,
};

use download_manager_cli::{AuthenticatedIpcBackend, BackendError};
use download_manager_engine::ipc::decode_ui_request;
use serde::{Deserialize, Serialize};
use tauri::Manager;

const STARTUP_ATTEMPTS: usize = 200;
const STARTUP_RETRY_DELAY: Duration = Duration::from_millis(50);
const PREFERENCES_FILE: &str = "preferences.json";
const BACKEND_LOG_FILE: &str = "backend.log";

#[derive(Clone, Debug)]
struct RuntimePaths {
    root: PathBuf,
    state: PathBuf,
    logs: PathBuf,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
struct DesktopPreferences {
    last_download_directory: Option<PathBuf>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
struct DesktopDiagnostics {
    product_version: String,
    backend_health: String,
    state_path: PathBuf,
    log_path: PathBuf,
}

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
    log_file: PathBuf,
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

        if let Some(parent) = self.log_file.parent() {
            fs::create_dir_all(parent).map_err(|error| {
                format!(
                    "could not create backend log directory {}: {error}",
                    parent.display()
                )
            })?;
        }
        let log = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.log_file)
            .map_err(|error| {
                format!(
                    "could not open backend log {}: {error}",
                    self.log_file.display()
                )
            })?;
        let stderr = log
            .try_clone()
            .map_err(|error| format!("could not prepare backend error log: {error}"))?;
        let mut command = Command::new(&self.executable);
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(stderr));
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
        let result = runtime_paths().and_then(|paths| {
            fs::create_dir_all(&paths.logs)
                .map_err(|error| format!("Could not create the local log directory: {error}"))?;
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(paths.logs.join(BACKEND_LOG_FILE))
                .map_err(|error| format!("Could not open the local backend log: {error}"))?;
            let executable = backend_executable()?;
            connect_or_start(
                &mut CurrentUserConnector,
                &mut ProcessLauncher {
                    executable,
                    log_file: paths.logs.join(BACKEND_LOG_FILE),
                },
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

fn runtime_paths() -> Result<RuntimePaths, String> {
    let local_app_data = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| "LOCALAPPDATA is unavailable for current-user desktop data".to_owned())?;
    let root = PathBuf::from(local_app_data).join("DownloadManager");
    Ok(RuntimePaths {
        state: root.join("State"),
        logs: root.join("Logs"),
        root,
    })
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
fn load_desktop_preferences() -> Result<DesktopPreferences, String> {
    let path = runtime_paths()?.root.join(PREFERENCES_FILE);
    load_preferences_at(&path)
}

#[tauri::command]
fn save_last_download_directory(directory: PathBuf) -> Result<(), String> {
    if !directory.is_absolute() || !directory.is_dir() {
        return Err("The last download directory must be an existing absolute folder.".to_owned());
    }
    let path = runtime_paths()?.root.join(PREFERENCES_FILE);
    save_preferences_at(
        &path,
        &DesktopPreferences {
            last_download_directory: Some(directory),
        },
    )
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands extract managed state by value.
fn desktop_diagnostics(
    state: tauri::State<'_, DesktopState>,
) -> Result<DesktopDiagnostics, String> {
    let paths = runtime_paths()?;
    let backend_health = {
        let mut session = state
            .session
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        match session.as_mut().and_then(LifecycleSession::connection_mut) {
            Some(backend) => match backend.health() {
                Ok(health) => format!("Healthy (backend {})", health.version),
                Err(error) => format!("Unavailable ({})", format_backend_error(error)),
            },
            None => format!("Unavailable ({})", state.status.message),
        }
    };
    Ok(DesktopDiagnostics {
        product_version: env!("CARGO_PKG_VERSION").to_owned(),
        backend_health,
        state_path: paths.state,
        log_path: paths.logs.join(BACKEND_LOG_FILE),
    })
}

#[tauri::command]
fn open_local_file(path: PathBuf) -> Result<(), String> {
    validate_local_target(&path, false)?;
    launch_local_target(
        "rundll32.exe",
        ["url.dll,FileProtocolHandler".into(), path.into()],
    )
}

#[tauri::command]
#[allow(clippy::needless_pass_by_value)] // Tauri commands deserialize owned path arguments.
fn reveal_local_file(path: PathBuf) -> Result<(), String> {
    validate_local_target(&path, false)?;
    launch_local_target(
        "explorer.exe",
        [format!("/select,{}", path.display()).into()],
    )
}

fn validate_local_target(path: &Path, directory: bool) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("The local target must be an absolute path.".to_owned());
    }
    let metadata =
        fs::metadata(path).map_err(|error| format!("The local target is unavailable: {error}"))?;
    if (directory && !metadata.is_dir()) || (!directory && !metadata.is_file()) {
        return Err("The local target has an unexpected type.".to_owned());
    }
    Ok(())
}

fn launch_local_target<const N: usize>(
    program: &str,
    arguments: [std::ffi::OsString; N],
) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    configure_background_process(&mut command);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Windows could not open the local target: {error}"))
}

fn load_preferences_at(path: &Path) -> Result<DesktopPreferences, String> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("Desktop preferences are invalid: {error}")),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(DesktopPreferences::default())
        }
        Err(error) => Err(format!("Desktop preferences could not be read: {error}")),
    }
}

fn save_preferences_at(path: &Path, preferences: &DesktopPreferences) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "Desktop preferences path has no parent directory.".to_owned())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Desktop preferences directory could not be created: {error}"))?;
    let bytes = serde_json::to_vec(preferences)
        .map_err(|error| format!("Desktop preferences could not be encoded: {error}"))?;
    fs::write(path, bytes)
        .map_err(|error| format!("Desktop preferences could not be saved: {error}"))
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
        .invoke_handler(tauri::generate_handler![
            backend_status,
            backend_request,
            load_desktop_preferences,
            save_last_download_directory,
            desktop_diagnostics,
            open_local_file,
            reveal_local_file
        ])
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
    use std::{
        cell::Cell,
        collections::VecDeque,
        fs,
        rc::Rc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use super::{
        BackendConnector, BackendLauncher, BackendOrigin, DesktopPreferences, ManagedChild,
        connect_or_start, load_preferences_at, save_preferences_at, validate_local_target,
    };

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

    #[test]
    fn current_user_preferences_survive_a_desktop_reopen() {
        let directory = test_directory("preferences");
        let path = directory.join("preferences.json");
        let expected = DesktopPreferences {
            last_download_directory: Some(directory.join("downloads")),
        };

        save_preferences_at(&path, &expected).expect("save preferences");
        assert_eq!(
            load_preferences_at(&path).expect("reload preferences"),
            expected
        );
        fs::remove_dir_all(directory).expect("cleanup preferences");
    }

    #[test]
    fn local_open_boundary_requires_an_existing_absolute_file() {
        let directory = test_directory("open-target");
        let file = directory.join("বাংলা archive.zip");
        fs::write(&file, b"fixture").expect("write target");

        assert!(validate_local_target(&file, false).is_ok());
        assert!(validate_local_target(&directory, false).is_err());
        assert!(validate_local_target(std::path::Path::new("relative.zip"), false).is_err());
        fs::remove_dir_all(directory).expect("cleanup target");
    }

    fn test_directory(label: &str) -> std::path::PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "download-manager-desktop-{label}-{}-{nonce}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory");
        path
    }
}
