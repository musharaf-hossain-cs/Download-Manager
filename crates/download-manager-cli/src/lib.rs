//! Versioned command contract for Download Manager automation.

use std::{ffi::OsString, fmt::Write as _, io::Write, path::PathBuf, str::FromStr};

use download_manager_engine::{
    domain::{HttpUrl, JobId},
    state::DownloadState,
};
use serde::Serialize;

pub const CLI_CONTRACT_VERSION: u16 = 1;

pub const HELP: &str = "Download Manager command-line interface\n\n\
Usage:\n  download-manager-cli add URL --output DIRECTORY [--name FILENAME] [--later] [--json]\n  \
download-manager-cli list [--json]\n  download-manager-cli status JOB_ID [--json]\n  \
download-manager-cli --version\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CliExit {
    Success,
    Usage,
    NotFound,
    Rejected,
    Transport,
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
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddRequest {
    pub source_url: HttpUrl,
    pub output_directory: PathBuf,
    pub filename: Option<String>,
    pub start_now: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct JobView {
    pub id: JobId,
    pub state: DownloadState,
    pub filename: String,
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
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
}

fn unavailable() -> BackendError {
    BackendError::Transport("authenticated engine IPC is unavailable".to_owned())
}

#[derive(Debug)]
enum Command {
    Help,
    Version,
    Add(AddRequest),
    List,
    Status(JobId),
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
        Command::Add(request) => match backend.add(request) {
            Ok(job) => emit_job(parsed.json, "add", &job, stdout),
            Err(error) => emit_backend_error(parsed.json, "add", &error, stderr),
        },
        Command::List => match backend.list() {
            Ok(jobs) => emit_jobs(parsed.json, &jobs, stdout),
            Err(error) => emit_backend_error(parsed.json, "list", &error, stderr),
        },
        Command::Status(job_id) => match backend.status(job_id) {
            Ok(job) => emit_job(parsed.json, "status", &job, stdout),
            Err(error) => emit_backend_error(parsed.json, "status", &error, stderr),
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
        "list" | "status" | "--help" | "-h" | "--version" | "-V" => {
            return Err(format!("invalid arguments for {command}\n\n{HELP}"));
        }
        value => return Err(format!("unknown command: {value}\n\n{HELP}")),
    };
    Ok(Parsed { command, json })
}

fn parse_add(args: &[OsString]) -> Result<AddRequest, String> {
    let Some(source) = args.first() else {
        return Err(format!(
            "add requires a URL and --output DIRECTORY\n\n{HELP}"
        ));
    };
    let source_url = HttpUrl::parse(unicode(source, "URL")?).map_err(|error| error.to_string())?;
    let mut output_directory = None;
    let mut filename = None;
    let mut start_now = true;
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
            value => return Err(format!("unknown add option: {value}")),
        }
        index += 1;
    }
    let output_directory =
        output_directory.ok_or_else(|| "add requires --output DIRECTORY".to_owned())?;
    Ok(AddRequest {
        source_url,
        output_directory,
        filename,
        start_now,
    })
}

fn unicode<'a>(value: &'a OsString, field: &str) -> Result<&'a str, String> {
    value
        .to_str()
        .ok_or_else(|| format!("{field} must be valid Unicode"))
}

fn emit_job<O: Write>(json: bool, command: &str, job: &JobView, stdout: &mut O) -> CliExit {
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
    use std::{path::PathBuf, str::FromStr};

    use download_manager_engine::{domain::JobId, state::DownloadState};

    use super::{AddRequest, BackendError, CliBackend, CliExit, JobView, run};

    const JOB_ID: &str = "123e4567-e89b-12d3-a456-426614174000";

    #[derive(Default)]
    struct FixtureBackend {
        added: Vec<AddRequest>,
        jobs: Vec<JobView>,
        status_error: Option<BackendError>,
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
                Ok(self.jobs[0].clone())
            }
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
}
