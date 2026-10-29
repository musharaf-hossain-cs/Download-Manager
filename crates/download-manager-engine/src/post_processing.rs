//! Shell-free execution for user-approved external scanners and post-download hooks.

use std::{
    collections::BTreeSet,
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

use crate::antivirus::{AntivirusScanError, AntivirusScannerAdapter, ScanVerdict, ScannerResult};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_TIMEOUT: Duration = Duration::from_hours(1);
const MAX_ARGUMENTS: usize = 64;
const MAX_LITERAL_BYTES: usize = 4 * 1024;
const MAX_ALLOWLIST_ENTRIES: usize = 32;
const MAX_CLEAN_EXIT_CODES: usize = 16;
const MAX_SCANNER_NAME_CHARS: usize = 128;

/// One structurally typed process argument. The downloaded path is never interpolated into text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExternalArgument {
    Literal(String),
    DownloadedFile,
}

/// User-approved configuration for one external scanner or post-download hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolConfig {
    pub executable: PathBuf,
    pub allowed_executables: Vec<PathBuf>,
    pub arguments: Vec<ExternalArgument>,
    pub timeout: Duration,
}

/// Completed child-process status. Child output is intentionally neither captured nor exposed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExternalToolReport {
    pub success: bool,
    pub exit_code: Option<i32>,
}

#[derive(Debug, Error)]
pub enum ExternalToolError {
    #[error("invalid external-tool limit: {0}")]
    InvalidLimit(&'static str),
    #[error("external-tool arguments must contain exactly one downloaded-file value")]
    InvalidDownloadedFileArgument,
    #[error("external-tool literal argument {index} exceeds {maximum} bytes")]
    LiteralTooLong { index: usize, maximum: usize },
    #[error("{role} path must be absolute: {path}")]
    NonAbsolutePath { role: &'static str, path: PathBuf },
    #[error("failed to inspect external-tool {role} path: {path}")]
    Path {
        role: &'static str,
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("external-tool {role} path is not a regular file: {path}")]
    NotFile { role: &'static str, path: PathBuf },
    #[error("external-tool executable is not in the exact canonical allowlist: {0}")]
    ExecutableNotAllowed(PathBuf),
    #[error("external tool cannot execute the downloaded file itself: {0}")]
    ExecutableIsTarget(PathBuf),
    #[error("external scanner name is invalid")]
    InvalidScannerName,
    #[error("external scanner clean-exit policy is invalid")]
    InvalidCleanExitCodes,
    #[error("failed to start external tool")]
    Spawn(#[source] io::Error),
    #[error("failed while supervising external tool")]
    Supervision(#[source] io::Error),
    #[error("external tool exceeded its time limit")]
    TimedOut,
}

/// A validated, exactly allowlisted external scanner or post-download hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalToolRunner {
    executable: PathBuf,
    arguments: Vec<ExternalArgument>,
    timeout: Duration,
}

impl ExternalToolRunner {
    /// Validates and freezes an external command without invoking a shell.
    ///
    /// # Errors
    /// Rejects unsafe limits, malformed argument vectors, non-local paths, and executables absent
    /// from the exact canonical allowlist.
    pub fn new(config: ExternalToolConfig) -> Result<Self, ExternalToolError> {
        validate_config(&config)?;
        let executable = canonical_file("executable", &config.executable)?;
        let allowed = config
            .allowed_executables
            .iter()
            .map(|path| canonical_file("allowlist", path))
            .collect::<Result<BTreeSet<_>, _>>()?;
        if !allowed.contains(&executable) {
            return Err(ExternalToolError::ExecutableNotAllowed(executable));
        }
        Ok(Self {
            executable,
            arguments: config.arguments,
            timeout: config.timeout,
        })
    }

    /// Executes one hook against an absolute local regular file and captures only its exit status.
    ///
    /// The child inherits no general user environment. Minimal operating-system variables needed
    /// to start trusted binaries are copied explicitly, while stdout and stderr are discarded until
    /// the central redaction pipeline is available.
    ///
    /// # Errors
    /// Rejects invalid targets and reports spawn, supervision, or timeout failures.
    pub fn run(&self, target: &Path) -> Result<ExternalToolReport, ExternalToolError> {
        let target = canonical_file("target", target)?;
        if target == self.executable {
            return Err(ExternalToolError::ExecutableIsTarget(target));
        }
        let arguments = self
            .arguments
            .iter()
            .map(|argument| match argument {
                ExternalArgument::Literal(value) => OsString::from(value),
                ExternalArgument::DownloadedFile => target.as_os_str().to_owned(),
            })
            .collect::<Vec<_>>();

        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .env_clear();
        copy_runtime_environment(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }

        let mut child = command.spawn().map_err(ExternalToolError::Spawn)?;
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(status) = child.try_wait().map_err(ExternalToolError::Supervision)? {
                return Ok(ExternalToolReport {
                    success: status.success(),
                    exit_code: status.code(),
                });
            }
            if Instant::now() >= deadline {
                terminate(&mut child).map_err(ExternalToolError::Supervision)?;
                return Err(ExternalToolError::TimedOut);
            }
            thread::sleep(POLL_INTERVAL);
        }
    }
}

/// External scanner implementation that reuses the registered-scanner result contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalScannerAdapter {
    name: String,
    runner: ExternalToolRunner,
    clean_exit_codes: BTreeSet<i32>,
}

impl ExternalScannerAdapter {
    /// Creates a fail-closed scanner adapter with an explicit set of clean exit codes.
    ///
    /// # Errors
    /// Rejects unsafe scanner names and empty or excessively large exit-code policies.
    pub fn new(
        name: impl Into<String>,
        runner: ExternalToolRunner,
        clean_exit_codes: impl IntoIterator<Item = i32>,
    ) -> Result<Self, ExternalToolError> {
        let name = name.into();
        if name.is_empty()
            || name.trim() != name
            || name.chars().count() > MAX_SCANNER_NAME_CHARS
            || name.chars().any(char::is_control)
        {
            return Err(ExternalToolError::InvalidScannerName);
        }
        let clean_exit_codes = clean_exit_codes.into_iter().collect::<BTreeSet<_>>();
        if clean_exit_codes.is_empty() || clean_exit_codes.len() > MAX_CLEAN_EXIT_CODES {
            return Err(ExternalToolError::InvalidCleanExitCodes);
        }
        Ok(Self {
            name,
            runner,
            clean_exit_codes,
        })
    }
}

impl AntivirusScannerAdapter for ExternalScannerAdapter {
    fn scanner_name(&self) -> &str {
        &self.name
    }

    fn scan_file(&mut self, canonical_path: &Path) -> Result<ScannerResult, AntivirusScanError> {
        let report = self
            .runner
            .run(canonical_path)
            .map_err(|error| AntivirusScanError::Launch(io::Error::other(error)))?;
        Ok(ScannerResult {
            verdict: if report
                .exit_code
                .is_some_and(|code| self.clean_exit_codes.contains(&code))
            {
                ScanVerdict::Clean
            } else {
                ScanVerdict::AttentionRequired
            },
            exit_code: report.exit_code,
        })
    }
}

fn validate_config(config: &ExternalToolConfig) -> Result<(), ExternalToolError> {
    if config.timeout.is_zero() || config.timeout > MAX_TIMEOUT {
        return Err(ExternalToolError::InvalidLimit("timeout"));
    }
    if config.allowed_executables.is_empty()
        || config.allowed_executables.len() > MAX_ALLOWLIST_ENTRIES
    {
        return Err(ExternalToolError::InvalidLimit("allowlist"));
    }
    if config.arguments.is_empty() || config.arguments.len() > MAX_ARGUMENTS {
        return Err(ExternalToolError::InvalidLimit("arguments"));
    }
    if config
        .arguments
        .iter()
        .filter(|argument| matches!(argument, ExternalArgument::DownloadedFile))
        .count()
        != 1
    {
        return Err(ExternalToolError::InvalidDownloadedFileArgument);
    }
    for (index, argument) in config.arguments.iter().enumerate() {
        if let ExternalArgument::Literal(value) = argument
            && value.len() > MAX_LITERAL_BYTES
        {
            return Err(ExternalToolError::LiteralTooLong {
                index,
                maximum: MAX_LITERAL_BYTES,
            });
        }
    }
    Ok(())
}

fn canonical_file(role: &'static str, path: &Path) -> Result<PathBuf, ExternalToolError> {
    if !path.is_absolute() {
        return Err(ExternalToolError::NonAbsolutePath {
            role,
            path: path.to_owned(),
        });
    }
    let canonical = fs::canonicalize(path).map_err(|source| ExternalToolError::Path {
        role,
        path: path.to_owned(),
        source,
    })?;
    if !canonical.is_file() {
        return Err(ExternalToolError::NotFile {
            role,
            path: canonical,
        });
    }
    Ok(canonical)
}

fn copy_runtime_environment(command: &mut Command) {
    #[cfg(windows)]
    const VARIABLES: &[&str] = &["SystemRoot", "WINDIR", "TEMP", "TMP"];
    #[cfg(not(windows))]
    const VARIABLES: &[&str] = &["TMPDIR"];
    for variable in VARIABLES {
        if let Some(value) = std::env::var_os(variable) {
            command.env(variable, value);
        }
    }
}

fn terminate(child: &mut std::process::Child) -> io::Result<()> {
    if let Err(error) = child.kill()
        && error.kind() != io::ErrorKind::InvalidInput
    {
        return Err(error);
    }
    child.wait().map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{antivirus::scan_completed_download, domain::JobId};

    fn fixture_runner(
        directory: &Path,
        script: &str,
        arguments: Vec<ExternalArgument>,
        timeout: Duration,
    ) -> ExternalToolRunner {
        #[cfg(windows)]
        let (executable, prefix) = {
            let executable = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let script_path = directory.join("external-tool-fixture.ps1");
            fs::write(&script_path, script).expect("fixture script");
            (
                executable,
                vec![
                    ExternalArgument::Literal("-NoProfile".to_owned()),
                    ExternalArgument::Literal("-NonInteractive".to_owned()),
                    ExternalArgument::Literal("-File".to_owned()),
                    ExternalArgument::Literal(script_path.to_string_lossy().into_owned()),
                ],
            )
        };
        #[cfg(not(windows))]
        let (executable, prefix) = (
            PathBuf::from("/bin/sh"),
            vec![
                ExternalArgument::Literal("-c".to_owned()),
                ExternalArgument::Literal(script.to_owned()),
                ExternalArgument::Literal("fixture".to_owned()),
            ],
        );

        let mut all_arguments = prefix;
        all_arguments.extend(arguments);
        ExternalToolRunner::new(ExternalToolConfig {
            executable: executable.clone(),
            allowed_executables: vec![executable],
            arguments: all_arguments,
            timeout,
        })
        .expect("fixture runner")
    }

    #[test]
    fn metacharacters_and_spaces_remain_one_literal_file_argument() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory
            .path()
            .join("payload & echo-owned; $(owned) %PATH%.bin");
        let record = directory.path().join("argument.txt");
        let sentinel = directory.path().join("owned");
        fs::write(&target, b"unchanged").expect("target");
        #[cfg(windows)]
        let script = "param($record, $target) [IO.File]::WriteAllText($record, $target)";
        #[cfg(not(windows))]
        let script = "printf '%s' \"$2\" > \"$1\"";
        let runner = fixture_runner(
            directory.path(),
            script,
            vec![
                ExternalArgument::Literal(record.to_string_lossy().into_owned()),
                ExternalArgument::DownloadedFile,
            ],
            Duration::from_secs(15),
        );

        let report = runner.run(&target).expect("hook succeeds");

        assert!(report.success);
        assert_eq!(report.exit_code, Some(0));
        assert_eq!(
            fs::read_to_string(record).expect("recorded argument"),
            target.canonicalize().unwrap().to_string_lossy()
        );
        assert!(!sentinel.exists());
        assert_eq!(fs::read(target).expect("preserved target"), b"unchanged");
    }

    #[test]
    fn validation_rejects_nonallowlisted_executable_and_bad_targets_before_spawn() {
        let directory = tempfile::tempdir().expect("temp directory");
        let allowed_decoy = directory.path().join("allowed.exe");
        fs::write(&allowed_decoy, b"not the configured executable").expect("decoy");
        #[cfg(windows)]
        let executable = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
            .join("System32/WindowsPowerShell/v1.0/powershell.exe");
        #[cfg(not(windows))]
        let executable = PathBuf::from("/bin/sh");
        let result = ExternalToolRunner::new(ExternalToolConfig {
            executable,
            allowed_executables: vec![allowed_decoy],
            arguments: vec![ExternalArgument::DownloadedFile],
            timeout: Duration::from_secs(1),
        });
        assert!(matches!(
            result,
            Err(ExternalToolError::ExecutableNotAllowed(_))
        ));

        #[cfg(windows)]
        let script = "param($marker, $target) [IO.File]::WriteAllText($marker, 'spawned')";
        #[cfg(not(windows))]
        let script = "printf spawned > \"$1\"";
        let marker = directory.path().join("spawned.txt");
        let runner = fixture_runner(
            directory.path(),
            script,
            vec![
                ExternalArgument::Literal(marker.to_string_lossy().into_owned()),
                ExternalArgument::DownloadedFile,
            ],
            Duration::from_secs(1),
        );
        assert!(matches!(
            runner.run(Path::new("relative.bin")),
            Err(ExternalToolError::NonAbsolutePath { role: "target", .. })
        ));
        assert!(matches!(
            runner.run(directory.path()),
            Err(ExternalToolError::NotFile { role: "target", .. })
        ));
        assert!(matches!(
            runner.run(&runner.executable),
            Err(ExternalToolError::ExecutableIsTarget(_))
        ));
        assert!(!marker.exists());
    }

    #[test]
    fn child_environment_excludes_profile_proxy_and_path_values() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.bin");
        let record = directory.path().join("environment.txt");
        fs::write(&target, b"target").expect("target");
        #[cfg(windows)]
        let script = r"param($record, $target)
            $names = @('Path', 'USERPROFILE', 'APPDATA', 'LOCALAPPDATA', 'HTTP_PROXY', 'HTTPS_PROXY')
            $values = $names | ForEach-Object { [Environment]::GetEnvironmentVariable($_) }
            [IO.File]::WriteAllText($record, ($values -join '|'))";
        #[cfg(not(windows))]
        let script =
            r#"printf '%s' "${HOME-}|${USER-}|${LOGNAME-}|${HTTP_PROXY-}|${HTTPS_PROXY-}" > "$1""#;
        let runner = fixture_runner(
            directory.path(),
            script,
            vec![
                ExternalArgument::Literal(record.to_string_lossy().into_owned()),
                ExternalArgument::DownloadedFile,
            ],
            Duration::from_secs(15),
        );

        runner.run(&target).expect("environment fixture");

        assert!(
            fs::read_to_string(record)
                .expect("environment record")
                .split('|')
                .all(str::is_empty)
        );
    }

    #[test]
    fn nonzero_exit_is_captured_without_exposing_output() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.bin");
        fs::write(&target, b"target").expect("target");
        #[cfg(windows)]
        let script = "param($target) Write-Output 'untrusted output'; exit 17";
        #[cfg(not(windows))]
        let script = "printf 'untrusted output'; exit 17";
        let runner = fixture_runner(
            directory.path(),
            script,
            vec![ExternalArgument::DownloadedFile],
            Duration::from_secs(15),
        );

        let report = runner.run(&target).expect("completed process");

        assert_eq!(
            report,
            ExternalToolReport {
                success: false,
                exit_code: Some(17),
            }
        );
    }

    #[test]
    fn timeout_kills_and_reaps_the_child_without_changing_target() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.bin");
        fs::write(&target, b"preserved").expect("target");
        #[cfg(windows)]
        let script = "param($target) Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let script = "sleep 5";
        let runner = fixture_runner(
            directory.path(),
            script,
            vec![ExternalArgument::DownloadedFile],
            Duration::from_millis(100),
        );

        assert!(matches!(
            runner.run(&target),
            Err(ExternalToolError::TimedOut)
        ));
        assert_eq!(fs::read(target).expect("preserved target"), b"preserved");
    }

    #[test]
    fn external_scanner_maps_only_configured_exit_codes_to_clean() {
        let directory = tempfile::tempdir().expect("temp directory");
        let target = directory.path().join("target.bin");
        fs::write(&target, b"target").expect("target");
        #[cfg(windows)]
        let clean_script = "param($target) exit 0";
        #[cfg(not(windows))]
        let clean_script = "exit 0";
        let clean_runner = fixture_runner(
            directory.path(),
            clean_script,
            vec![ExternalArgument::DownloadedFile],
            Duration::from_secs(15),
        );
        let mut clean = ExternalScannerAdapter::new("External Test Scanner", clean_runner, [0])
            .expect("clean scanner");
        let clean_record =
            scan_completed_download(JobId::new(), &target, &mut clean).expect("clean scan record");
        assert_eq!(clean_record.verdict, ScanVerdict::Clean);
        assert_eq!(clean_record.exit_code, Some(0));

        #[cfg(windows)]
        let attention_script = "param($target) exit 9";
        #[cfg(not(windows))]
        let attention_script = "exit 9";
        let attention_runner = fixture_runner(
            directory.path(),
            attention_script,
            vec![ExternalArgument::DownloadedFile],
            Duration::from_secs(15),
        );
        let mut attention =
            ExternalScannerAdapter::new("External Test Scanner", attention_runner, [0])
                .expect("attention scanner");
        let attention_record = scan_completed_download(JobId::new(), &target, &mut attention)
            .expect("attention scan record");
        assert_eq!(attention_record.verdict, ScanVerdict::AttentionRequired);
        assert_eq!(attention_record.exit_code, Some(9));
    }
}
