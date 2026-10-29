//! Versioned command contract for Download Manager automation.

use std::{ffi::OsString, fmt::Write as _, io::Write, path::PathBuf, str::FromStr, time::Duration};

use download_manager_engine::{
    domain::{HttpUrl, JobId},
    ipc::JobAction,
    state::DownloadState,
};
use serde::Serialize;

pub const CLI_CONTRACT_VERSION: u16 = 1;

pub const HELP: &str = "Download Manager command-line interface\n\n\
Usage:\n  download-manager-cli add URL --output DIRECTORY [--name FILENAME] [--later] [--queue NAME] [--quiet] [--wait [--timeout SECONDS]] [--json]\n  \
download-manager-cli list [--json]\n  download-manager-cli status JOB_ID [--json]\n  \
download-manager-cli pause|resume|cancel JOB_ID [--json]\n  \
download-manager-cli queue start NAME [--quiet] [--json]\n  \
download-manager-cli --version\n";

const WAIT_POLL_INTERVAL: Duration = Duration::from_millis(200);
const DEFAULT_WAIT_TIMEOUT: Duration = Duration::from_secs(3_600);
const MAX_WAIT_TIMEOUT_SECS: u64 = 604_800;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliExit {
    Success,
    Usage,
    NotFound,
    Rejected,
    Transport,
    Timeout,
    Cancelled,
}

impl CliExit {
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Success => 0,
            Self::Usage => 2,
            Self::NotFound => 3,
            Self::Rejected => 4,
            Self::Transport => 5,
            Self::Timeout => 124,
            Self::Cancelled => 130,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub source_url: HttpUrl,
    pub output_directory: PathBuf,
    pub filename: Option<String>,
    pub start_now: bool,
    pub queue: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobView {
    pub id: JobId,
    pub state: DownloadState,
    pub filename: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct QueueStart {
    pub name: String,
    pub started_jobs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaitSignal {
    Elapsed(Duration),
    Cancelled,
}

/// Injectable clock/sleep/cancellation boundary for deterministic wait behavior.
pub trait WaitControl {
    fn wait(&mut self, interval: Duration) -> WaitSignal;
}

pub struct ThreadWaiter;

impl WaitControl for ThreadWaiter {
    fn wait(&mut self, interval: Duration) -> WaitSignal {
        std::thread::sleep(interval);
        WaitSignal::Elapsed(interval)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    NotFound(String),
    Rejected { code: String, message: String },
    Transport(String),
}

/// Engine-client boundary used by the CLI. Implementations must use authenticated local IPC.
pub trait CliBackend {
    /// Adds one validated URL.
    ///
    /// # Errors
    /// Returns a typed engine rejection or transport failure.
    fn add(&mut self, request: AddRequest) -> Result<JobView, BackendError>;

    /// Lists the engine's durable jobs.
    ///
    /// # Errors
    /// Returns a typed engine rejection or transport failure.
    fn list(&mut self) -> Result<Vec<JobView>, BackendError>;

    /// Reads one durable job by ID.
    ///
    /// # Errors
    /// Returns not-found, engine rejection, or transport failure.
    fn status(&mut self, job_id: JobId) -> Result<JobView, BackendError>;

    /// Applies one state-changing action through the engine authority.
    ///
    /// # Errors
    /// Returns not-found, invalid-state rejection, or transport failure.
    fn control(&mut self, job_id: JobId, action: JobAction) -> Result<JobView, BackendError>;

    /// Starts one named queue.
    ///
    /// # Errors
    /// Returns not-found, engine rejection, or transport failure.
    fn start_queue(&mut self, name: &str) -> Result<QueueStart, BackendError>;
}

/// Safe binary fallback until the authenticated local IPC transport is wired to this contract.
pub struct UnavailableBackend;

impl CliBackend for UnavailableBackend {
    fn add(&mut self, _request: AddRequest) -> Result<JobView, BackendError> {
        Err(unavailable())
    }

    fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
        Err(unavailable())
    }

    fn status(&mut self, _job_id: JobId) -> Result<JobView, BackendError> {
        Err(unavailable())
    }

    fn control(&mut self, _job_id: JobId, _action: JobAction) -> Result<JobView, BackendError> {
        Err(unavailable())
    }

    fn start_queue(&mut self, _name: &str) -> Result<QueueStart, BackendError> {
        Err(unavailable())
    }
}

fn unavailable() -> BackendError {
    BackendError::Transport("authenticated engine IPC is unavailable".to_owned())
}

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Add(AddCommand),
    List,
    Status(JobId),
    Control {
        name: &'static str,
        job_id: JobId,
        action: JobAction,
    },
    QueueStart {
        name: String,
        quiet: bool,
    },
}

#[derive(Debug)]
struct AddCommand {
    request: AddRequest,
    quiet: bool,
    wait_timeout: Option<Duration>,
}

#[derive(Debug)]
struct Parsed {
    command: Command,
    json: bool,
}

#[derive(Serialize)]
struct SuccessEnvelope<'a, T> {
    contract_version: u16,
    command: &'a str,
    ok: bool,
    data: T,
}

#[derive(Serialize)]
struct ErrorEnvelope<'a> {
    contract_version: u16,
    command: &'a str,
    ok: bool,
    error: ErrorBody<'a>,
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    code: &'a str,
    message: &'a str,
}

/// Parses and executes one CLI invocation against an authenticated engine-client boundary.
pub fn run<B, I, S, O, E>(args: I, backend: &mut B, stdout: &mut O, stderr: &mut E) -> CliExit
where
    B: CliBackend,
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    O: Write,
    E: Write,
{
    let mut waiter = ThreadWaiter;
    run_with_waiter(args, backend, &mut waiter, stdout, stderr)
}

/// Executes one invocation with injectable waiting and cancellation behavior.
pub fn run_with_waiter<B, I, S, O, E, W>(
    args: I,
    backend: &mut B,
    waiter: &mut W,
    stdout: &mut O,
    stderr: &mut E,
) -> CliExit
where
    B: CliBackend,
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
    O: Write,
    E: Write,
    W: WaitControl,
{
    let args: Vec<OsString> = args.into_iter().map(Into::into).collect();
    let json_requested = args.iter().any(|arg| arg == "--json");
    let command_hint = args
        .first()
        .and_then(|arg| arg.to_str())
        .unwrap_or("unknown")
        .to_owned();
    let parsed = match parse(args) {
        Ok(parsed) => parsed,
        Err(message) => {
            emit_error(json_requested, &command_hint, "usage", &message, stderr);
            return CliExit::Usage;
        }
    };

    match parsed.command {
        Command::Help => emit_text(stdout, HELP, CliExit::Success),
        Command::Version => emit_text(
            stdout,
            &format!(
                "download-manager-cli {}\n",
                download_manager_engine::VERSION
            ),
            CliExit::Success,
        ),
        Command::Add(command) => match backend.add(command.request) {
            Ok(job) => {
                let job = if let Some(timeout) = command.wait_timeout {
                    match wait_for_job(backend, waiter, job, timeout, parsed.json, stderr) {
                        Ok(job) => job,
                        Err(exit) => return exit,
                    }
                } else {
                    job
                };
                emit_job(parsed.json, command.quiet, "add", &job, stdout)
            }
            Err(error) => emit_backend_error(parsed.json, "add", &error, stderr),
        },
        Command::List => match backend.list() {
            Ok(jobs) => emit_jobs(parsed.json, &jobs, stdout),
            Err(error) => emit_backend_error(parsed.json, "list", &error, stderr),
        },
        Command::Status(job_id) => match backend.status(job_id) {
            Ok(job) => emit_job(parsed.json, false, "status", &job, stdout),
            Err(error) => emit_backend_error(parsed.json, "status", &error, stderr),
        },
        Command::Control {
            name,
            job_id,
            action,
        } => match backend.control(job_id, action) {
            Ok(job) => emit_job(parsed.json, false, name, &job, stdout),
            Err(error) => emit_backend_error(parsed.json, name, &error, stderr),
        },
        Command::QueueStart { name, quiet } => match backend.start_queue(&name) {
            Ok(queue) => emit_queue(parsed.json, quiet, &queue, stdout),
            Err(error) => emit_backend_error(parsed.json, "queue_start", &error, stderr),
        },
    }
}

fn parse(args: Vec<OsString>) -> Result<Parsed, String> {
    let mut json = false;
    let mut filtered = Vec::with_capacity(args.len());
    for argument in args {
        if argument == "--json" {
            json = true;
        } else {
            filtered.push(argument);
        }
    }

    let Some(command) = filtered.first().and_then(|value| value.to_str()) else {
        if filtered.is_empty() {
            return Ok(Parsed {
                command: Command::Help,
                json,
            });
        }
        return Err("command must be valid Unicode".to_owned());
    };
    let command = match command {
        "--help" | "-h" if filtered.len() == 1 => Command::Help,
        "--version" | "-V" if filtered.len() == 1 => Command::Version,
        "add" => Command::Add(parse_add(&filtered[1..])?),
        "list" if filtered.len() == 1 => Command::List,
        "status" if filtered.len() == 2 => {
            let value = unicode(&filtered[1], "job ID")?;
            Command::Status(JobId::from_str(value).map_err(|error| error.to_string())?)
        }
        name @ ("pause" | "resume" | "cancel") if filtered.len() == 2 => {
            let value = unicode(&filtered[1], "job ID")?;
            let job_id = JobId::from_str(value).map_err(|error| error.to_string())?;
            let (name, action) = match name {
                "pause" => ("pause", JobAction::Pause),
                "resume" => ("resume", JobAction::Resume),
                "cancel" => ("cancel", JobAction::Cancel),
                _ => unreachable!("matched control command"),
            };
            Command::Control {
                name,
                job_id,
                action,
            }
        }
        "queue" => parse_queue(&filtered[1..])?,
        "list" | "status" | "pause" | "resume" | "cancel" | "--help" | "-h" | "--version"
        | "-V" => {
            return Err(format!("invalid arguments for {command}\n\n{HELP}"));
        }
        value => return Err(format!("unknown command: {value}\n\n{HELP}")),
    };
    Ok(Parsed { command, json })
}

fn parse_add(args: &[OsString]) -> Result<AddCommand, String> {
    let Some(source) = args.first() else {
        return Err(format!(
            "add requires a URL and --output DIRECTORY\n\n{HELP}"
        ));
    };
    let source_url = HttpUrl::parse(unicode(source, "URL")?).map_err(|error| error.to_string())?;
    let mut output_directory = None;
    let mut filename = None;
    let mut queue = None;
    let mut start_now = true;
    let mut quiet = false;
    let mut wait = false;
    let mut timeout = None;
    let mut index = 1;
    while index < args.len() {
        match unicode(&args[index], "option")? {
            "--output" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--output requires a directory".to_owned())?;
                if output_directory.replace(PathBuf::from(value)).is_some() {
                    return Err("--output may be specified only once".to_owned());
                }
            }
            "--name" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--name requires a filename".to_owned())?;
                let value = unicode(value, "filename")?;
                if value.is_empty() {
                    return Err("filename must not be empty".to_owned());
                }
                if filename.replace(value.to_owned()).is_some() {
                    return Err("--name may be specified only once".to_owned());
                }
            }
            "--later" if start_now => start_now = false,
            "--later" => return Err("--later may be specified only once".to_owned()),
            "--queue" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--queue requires a name".to_owned())?;
                let value = bounded_name(unicode(value, "queue name")?)?;
                if queue.replace(value.to_owned()).is_some() {
                    return Err("--queue may be specified only once".to_owned());
                }
            }
            "--quiet" if !quiet => quiet = true,
            "--quiet" => return Err("--quiet may be specified only once".to_owned()),
            "--wait" if !wait => wait = true,
            "--wait" => return Err("--wait may be specified only once".to_owned()),
            "--timeout" => {
                index += 1;
                let value = args
                    .get(index)
                    .ok_or_else(|| "--timeout requires seconds".to_owned())?;
                let seconds = unicode(value, "timeout")?
                    .parse::<u64>()
                    .map_err(|_| "timeout must be a positive integer".to_owned())?;
                if seconds == 0 || seconds > MAX_WAIT_TIMEOUT_SECS {
                    return Err(format!(
                        "timeout must be between 1 and {MAX_WAIT_TIMEOUT_SECS} seconds"
                    ));
                }
                if timeout.replace(Duration::from_secs(seconds)).is_some() {
                    return Err("--timeout may be specified only once".to_owned());
                }
            }
            value => return Err(format!("unknown add option: {value}")),
        }
        index += 1;
    }
    let output_directory =
        output_directory.ok_or_else(|| "add requires --output DIRECTORY".to_owned())?;
    if timeout.is_some() && !wait {
        return Err("--timeout requires --wait".to_owned());
    }
    if wait && !start_now {
        return Err("--wait cannot be combined with --later".to_owned());
    }
    Ok(AddCommand {
        request: AddRequest {
            source_url,
            output_directory,
            filename,
            start_now,
            queue,
        },
        quiet,
        wait_timeout: wait.then_some(timeout.unwrap_or(DEFAULT_WAIT_TIMEOUT)),
    })
}

fn parse_queue(args: &[OsString]) -> Result<Command, String> {
    if args.len() < 2 || unicode(&args[0], "queue action")? != "start" {
        return Err(format!("queue requires: queue start NAME\n\n{HELP}"));
    }
    let name = bounded_name(unicode(&args[1], "queue name")?)?.to_owned();
    let mut quiet = false;
    for option in &args[2..] {
        match unicode(option, "queue option")? {
            "--quiet" if !quiet => quiet = true,
            "--quiet" => return Err("--quiet may be specified only once".to_owned()),
            value => return Err(format!("unknown queue option: {value}")),
        }
    }
    Ok(Command::QueueStart { name, quiet })
}

fn bounded_name(value: &str) -> Result<&str, String> {
    if value.is_empty() || value.len() > 256 || value.trim() != value {
        Err("queue name must contain 1 to 256 non-padding characters".to_owned())
    } else {
        Ok(value)
    }
}

fn unicode<'a>(value: &'a OsString, field: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{field} must be valid Unicode"))
}

fn emit_job<O: Write>(
    json: bool,
    quiet: bool,
    command: &str,
    job: &JobView,
    stdout: &mut O,
) -> CliExit {
    if json {
        emit_json(
            stdout,
            &SuccessEnvelope {
                contract_version: CLI_CONTRACT_VERSION,
                command,
                ok: true,
                data: job,
            },
        )
    } else if quiet {
        CliExit::Success
    } else {
        emit_text(
            stdout,
            &format!(
                "{}\t{}\t{}\t{}\t{}\n",
                job.id,
                job.state,
                job.downloaded_bytes,
                total(job.total_bytes),
                job.filename
            ),
            CliExit::Success,
        )
    }
}

fn emit_queue<O: Write>(json: bool, quiet: bool, queue: &QueueStart, stdout: &mut O) -> CliExit {
    if json {
        emit_json(
            stdout,
            &SuccessEnvelope {
                contract_version: CLI_CONTRACT_VERSION,
                command: "queue_start",
                ok: true,
                data: queue,
            },
        )
    } else if quiet {
        CliExit::Success
    } else {
        emit_text(
            stdout,
            &format!("{}\t{}\n", queue.name, queue.started_jobs),
            CliExit::Success,
        )
    }
}

fn wait_for_job<B: CliBackend, W: WaitControl, E: Write>(
    backend: &mut B,
    waiter: &mut W,
    mut job: JobView,
    timeout: Duration,
    json: bool,
    stderr: &mut E,
) -> Result<JobView, CliExit> {
    let mut elapsed = Duration::ZERO;
    loop {
        match job.state {
            DownloadState::Completed => return Ok(job),
            DownloadState::Cancelled => {
                emit_error(
                    json,
                    "add",
                    "wait_cancelled",
                    "download was cancelled",
                    stderr,
                );
                return Err(CliExit::Cancelled);
            }
            DownloadState::Failed | DownloadState::MissingLocalFile => {
                emit_error(
                    json,
                    "add",
                    "job_failed",
                    "download did not complete",
                    stderr,
                );
                return Err(CliExit::Rejected);
            }
            _ => {}
        }

        let remaining = timeout.saturating_sub(elapsed);
        let interval = WAIT_POLL_INTERVAL.min(remaining);
        match waiter.wait(interval) {
            WaitSignal::Cancelled => {
                emit_error(json, "add", "wait_cancelled", "wait was cancelled", stderr);
                return Err(CliExit::Cancelled);
            }
            WaitSignal::Elapsed(duration) => {
                elapsed = elapsed.saturating_add(duration);
                if elapsed >= timeout {
                    emit_error(json, "add", "wait_timeout", "wait timed out", stderr);
                    return Err(CliExit::Timeout);
                }
            }
        }
        job = match backend.status(job.id) {
            Ok(job) => job,
            Err(error) => return Err(emit_backend_error(json, "add", &error, stderr)),
        };
    }
}

fn emit_jobs<O: Write>(json: bool, jobs: &[JobView], stdout: &mut O) -> CliExit {
    if json {
        emit_json(
            stdout,
            &SuccessEnvelope {
                contract_version: CLI_CONTRACT_VERSION,
                command: "list",
                ok: true,
                data: jobs,
            },
        )
    } else {
        let mut output = String::from("ID\tSTATE\tDOWNLOADED\tTOTAL\tNAME\n");
        for job in jobs {
            writeln!(
                output,
                "{}\t{}\t{}\t{}\t{}",
                job.id,
                job.state,
                job.downloaded_bytes,
                total(job.total_bytes),
                job.filename
            )
            .expect("writing to a String cannot fail");
        }
        emit_text(stdout, &output, CliExit::Success)
    }
}

fn total(value: Option<u64>) -> String {
    value.map_or_else(|| "-".to_owned(), |value| value.to_string())
}

fn emit_backend_error<E: Write>(
    json: bool,
    command: &str,
    error: &BackendError,
    stderr: &mut E,
) -> CliExit {
    let (exit, code, message) = match &error {
        BackendError::NotFound(message) => (CliExit::NotFound, "not_found", message.as_str()),
        BackendError::Rejected { code, message } => {
            (CliExit::Rejected, code.as_str(), message.as_str())
        }
        BackendError::Transport(message) => (
            CliExit::Transport,
            "transport_unavailable",
            message.as_str(),
        ),
    };
    emit_error(json, command, code, message, stderr);
    exit
}

fn emit_error<E: Write>(json: bool, command: &str, code: &str, message: &str, stderr: &mut E) {
    if json {
        let envelope = ErrorEnvelope {
            contract_version: CLI_CONTRACT_VERSION,
            command,
            ok: false,
            error: ErrorBody { code, message },
        };
        if let Ok(mut output) = serde_json::to_string(&envelope) {
            output.push('\n');
            drop(stderr.write_all(output.as_bytes()));
        }
    } else {
        drop(writeln!(stderr, "{code}: {message}"));
    }
}

fn emit_json<O: Write, T: Serialize>(stdout: &mut O, value: &T) -> CliExit {
    let Ok(mut output) = serde_json::to_string(value) else {
        return CliExit::Transport;
    };
    output.push('\n');
    emit_text(stdout, &output, CliExit::Success)
}

fn emit_text<O: Write>(stdout: &mut O, output: &str, success: CliExit) -> CliExit {
    if stdout.write_all(output.as_bytes()).is_ok() {
        success
    } else {
        CliExit::Transport
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::VecDeque, path::PathBuf, str::FromStr, time::Duration};

    use download_manager_engine::{domain::JobId, ipc::JobAction, state::DownloadState};

    use super::{
        AddRequest, BackendError, CliBackend, CliExit, JobView, QueueStart, WaitControl,
        WaitSignal, run, run_with_waiter,
    };

    const JOB_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

    #[derive(Default)]
    struct FixtureBackend {
        added: Vec<AddRequest>,
        controls: Vec<(JobId, JobAction)>,
        jobs: Vec<JobView>,
        status_error: Option<BackendError>,
        control_error: Option<BackendError>,
        wait_states: VecDeque<DownloadState>,
        started_queues: Vec<String>,
    }

    impl CliBackend for FixtureBackend {
        fn add(&mut self, request: AddRequest) -> Result<JobView, BackendError> {
            self.added.push(request);
            Ok(self.jobs[0].clone())
        }

        fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
            Ok(self.jobs.clone())
        }

        fn status(&mut self, _job_id: JobId) -> Result<JobView, BackendError> {
            if let Some(error) = self.status_error.take() {
                Err(error)
            } else {
                let mut job = self.jobs[0].clone();
                if let Some(state) = self.wait_states.pop_front() {
                    job.state = state;
                }
                Ok(job)
            }
        }

        fn control(&mut self, job_id: JobId, action: JobAction) -> Result<JobView, BackendError> {
            self.controls.push((job_id, action));
            if let Some(error) = self.control_error.take() {
                return Err(error);
            }
            let mut job = self.jobs[0].clone();
            job.state = match action {
                JobAction::Pause => DownloadState::Paused,
                JobAction::Resume => DownloadState::Queued,
                JobAction::Cancel => DownloadState::Cancelled,
                JobAction::Start | JobAction::Stop => {
                    panic!("S143 fixture accepts only pause, resume, and cancel")
                }
            };
            Ok(job)
        }

        fn start_queue(&mut self, name: &str) -> Result<QueueStart, BackendError> {
            self.started_queues.push(name.to_owned());
            Ok(QueueStart {
                name: name.to_owned(),
                started_jobs: 3,
            })
        }
    }

    fn job() -> JobView {
        JobView {
            id: JobId::from_str(JOB_ID).expect("fixture ID"),
            state: DownloadState::Queued,
            filename: "release file.zip".to_owned(),
            downloaded_bytes: 12,
            total_bytes: Some(100),
        }
    }

    fn invoke(args: &[&str], backend: &mut FixtureBackend) -> (CliExit, String, String) {
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run(args.iter().copied(), backend, &mut stdout, &mut stderr);
        (
            exit,
            String::from_utf8(stdout).expect("stdout UTF-8"),
            String::from_utf8(stderr).expect("stderr UTF-8"),
        )
    }

    struct FixtureWaiter {
        signals: VecDeque<WaitSignal>,
    }

    impl WaitControl for FixtureWaiter {
        fn wait(&mut self, interval: Duration) -> WaitSignal {
            self.signals
                .pop_front()
                .unwrap_or(WaitSignal::Elapsed(interval))
        }
    }

    fn invoke_wait(
        args: &[&str],
        backend: &mut FixtureBackend,
        signals: impl IntoIterator<Item = WaitSignal>,
    ) -> (CliExit, String, String) {
        let mut waiter = FixtureWaiter {
            signals: signals.into_iter().collect(),
        };
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let exit = run_with_waiter(
            args.iter().copied(),
            backend,
            &mut waiter,
            &mut stdout,
            &mut stderr,
        );
        (
            exit,
            String::from_utf8(stdout).expect("stdout UTF-8"),
            String::from_utf8(stderr).expect("stderr UTF-8"),
        )
    }

    #[test]
    fn add_json_golden_preserves_quoted_values_and_start_choice() {
        let mut backend = FixtureBackend {
            jobs: vec![job()],
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke(
            &[
                "add",
                "https://example.test/a%20b.zip?token=value",
                "--output",
                r"C:\Downloads Folder",
                "--name",
                "release file.zip",
                "--later",
                "--queue",
                "Night Queue",
                "--json",
            ],
            &mut backend,
        );

        assert_eq!(exit.code(), 0);
        assert_eq!(
            stdout,
            concat!(
                r#"{"contract_version":1,"command":"add","ok":true,"data":{"id":"123e4567-e89b-12d3-a456-426614174000","state":"queued","filename":"release file.zip","downloaded_bytes":12,"total_bytes":100}}"#,
                "\n"
            )
        );
        assert_eq!(stderr, "");
        assert_eq!(backend.added.len(), 1);
        assert_eq!(
            backend.added[0].source_url.as_str(),
            "https://example.test/a%20b.zip?token=value"
        );
        assert_eq!(
            backend.added[0].output_directory,
            PathBuf::from(r"C:\Downloads Folder")
        );
        assert_eq!(
            backend.added[0].filename.as_deref(),
            Some("release file.zip")
        );
        assert!(!backend.added[0].start_now);
        assert_eq!(backend.added[0].queue.as_deref(), Some("Night Queue"));
    }

    #[test]
    fn list_human_output_is_a_stable_golden_table() {
        let mut backend = FixtureBackend {
            jobs: vec![job()],
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke(&["list"], &mut backend);

        assert_eq!(exit, CliExit::Success);
        assert_eq!(
            stdout,
            concat!(
                "ID\tSTATE\tDOWNLOADED\tTOTAL\tNAME\n",
                "123e4567-e89b-12d3-a456-426614174000\tqueued\t12\t100\trelease file.zip\n"
            )
        );
        assert_eq!(stderr, "");
    }

    #[test]
    fn status_not_found_has_stable_json_and_exit_code() {
        let mut backend = FixtureBackend {
            status_error: Some(BackendError::NotFound("job was not found".to_owned())),
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke(&["status", JOB_ID, "--json"], &mut backend);

        assert_eq!(exit.code(), 3);
        assert_eq!(stdout, "");
        assert_eq!(
            stderr,
            concat!(
                r#"{"contract_version":1,"command":"status","ok":false,"error":{"code":"not_found","message":"job was not found"}}"#,
                "\n"
            )
        );
    }

    #[test]
    fn invalid_url_and_transport_failure_use_distinct_exit_codes() {
        let mut backend = FixtureBackend::default();
        let (usage, _, usage_stderr) = invoke(
            &["add", "file:///unsafe", "--output", "downloads"],
            &mut backend,
        );
        assert_eq!(usage.code(), 2);
        assert!(usage_stderr.starts_with("usage: only HTTP and HTTPS"));

        let mut unavailable = super::UnavailableBackend;
        let mut stdout = Vec::new();
        let mut stderr = Vec::new();
        let transport = run(["list"], &mut unavailable, &mut stdout, &mut stderr);
        assert_eq!(transport.code(), 5);
        assert_eq!(stdout, b"");
        assert_eq!(
            stderr,
            b"transport_unavailable: authenticated engine IPC is unavailable\n"
        );
    }

    #[test]
    fn pause_resume_and_cancel_have_stable_state_contracts() {
        let cases = [
            ("pause", JobAction::Pause, "paused"),
            ("resume", JobAction::Resume, "queued"),
            ("cancel", JobAction::Cancel, "cancelled"),
        ];

        for (command, expected_action, expected_state) in cases {
            let mut backend = FixtureBackend {
                jobs: vec![job()],
                ..FixtureBackend::default()
            };
            let (exit, stdout, stderr) = invoke(&[command, JOB_ID, "--json"], &mut backend);

            assert_eq!(exit, CliExit::Success, "{command}");
            assert_eq!(stderr, "", "{command}");
            assert_eq!(
                stdout,
                format!(
                    "{{\"contract_version\":1,\"command\":\"{command}\",\"ok\":true,\"data\":{{\"id\":\"{JOB_ID}\",\"state\":\"{expected_state}\",\"filename\":\"release file.zip\",\"downloaded_bytes\":12,\"total_bytes\":100}}}}\n"
                )
            );
            assert_eq!(backend.controls, vec![(job().id, expected_action)]);
        }
    }

    #[test]
    fn control_rejections_preserve_engine_error_contract_and_exit_code() {
        let mut backend = FixtureBackend {
            control_error: Some(BackendError::Rejected {
                code: "invalid_state".to_owned(),
                message: "cannot pause a completed job".to_owned(),
            }),
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke(&["pause", JOB_ID, "--json"], &mut backend);

        assert_eq!(exit, CliExit::Rejected);
        assert_eq!(stdout, "");
        assert_eq!(
            stderr,
            concat!(
                r#"{"contract_version":1,"command":"pause","ok":false,"error":{"code":"invalid_state","message":"cannot pause a completed job"}}"#,
                "\n"
            )
        );
    }

    #[test]
    fn queue_start_and_quiet_modes_have_stable_contracts() {
        let mut backend = FixtureBackend::default();
        let (exit, stdout, stderr) = invoke(&["queue", "start", "Night", "--json"], &mut backend);
        assert_eq!(exit, CliExit::Success);
        assert_eq!(
            stdout,
            "{\"contract_version\":1,\"command\":\"queue_start\",\"ok\":true,\"data\":{\"name\":\"Night\",\"started_jobs\":3}}\n"
        );
        assert_eq!(stderr, "");
        assert_eq!(backend.started_queues, vec!["Night"]);

        let mut quiet_backend = FixtureBackend {
            jobs: vec![job()],
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke(
            &[
                "add",
                "https://example.test/file.zip",
                "--output",
                "downloads",
                "--quiet",
            ],
            &mut quiet_backend,
        );
        assert_eq!(exit, CliExit::Success);
        assert_eq!(stdout, "");
        assert_eq!(stderr, "");
    }

    #[test]
    fn wait_completes_after_injected_polls_and_emits_final_state() {
        let mut backend = FixtureBackend {
            jobs: vec![job()],
            wait_states: [DownloadState::Downloading, DownloadState::Completed]
                .into_iter()
                .collect(),
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke_wait(
            &[
                "add",
                "https://example.test/file.zip",
                "--output",
                "downloads",
                "--wait",
                "--timeout",
                "2",
                "--json",
            ],
            &mut backend,
            [
                WaitSignal::Elapsed(Duration::from_millis(200)),
                WaitSignal::Elapsed(Duration::from_millis(200)),
            ],
        );

        assert_eq!(exit, CliExit::Success);
        assert!(stdout.contains("\"state\":\"completed\""));
        assert_eq!(stderr, "");
    }

    #[test]
    fn wait_timeout_and_cancellation_have_stable_exits() {
        let args = [
            "add",
            "https://example.test/file.zip",
            "--output",
            "downloads",
            "--wait",
            "--timeout",
            "1",
            "--json",
        ];
        let mut timeout_backend = FixtureBackend {
            jobs: vec![job()],
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) = invoke_wait(
            &args,
            &mut timeout_backend,
            [WaitSignal::Elapsed(Duration::from_secs(1))],
        );
        assert_eq!(exit.code(), 124);
        assert_eq!(stdout, "");
        assert_eq!(
            stderr,
            "{\"contract_version\":1,\"command\":\"add\",\"ok\":false,\"error\":{\"code\":\"wait_timeout\",\"message\":\"wait timed out\"}}\n"
        );

        let mut cancel_backend = FixtureBackend {
            jobs: vec![job()],
            ..FixtureBackend::default()
        };
        let (exit, stdout, stderr) =
            invoke_wait(&args, &mut cancel_backend, [WaitSignal::Cancelled]);
        assert_eq!(exit.code(), 130);
        assert_eq!(stdout, "");
        assert!(stderr.contains("\"code\":\"wait_cancelled\""));
    }
}
