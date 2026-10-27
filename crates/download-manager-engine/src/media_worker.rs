//! Restricted out-of-process `FFmpeg` execution for already-downloaded media tracks.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    thread,
    time::{Duration, Instant},
};

use thiserror::Error;

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_TIMEOUT: Duration = Duration::from_hours(24);
const MAX_STDERR_LIMIT: usize = 1024 * 1024;

/// Fixed `FFmpeg` muxers accepted by the media worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegContainer {
    Mp4,
    Matroska,
    WebM,
}

impl FfmpegContainer {
    const fn muxer(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Matroska => "matroska",
            Self::WebM => "webm",
        }
    }
}

/// A local-track mux request. Network inputs and arbitrary `FFmpeg` options are intentionally absent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegMuxRequest {
    pub inputs: Vec<PathBuf>,
    pub output: PathBuf,
    pub container: FfmpegContainer,
}

/// Per-invocation limits enforced by the worker supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MediaWorkerLimits {
    pub timeout: Duration,
    pub max_inputs: usize,
    pub max_output_bytes: u64,
    pub max_stderr_bytes: usize,
}

impl Default for MediaWorkerLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_mins(10),
            max_inputs: 8,
            max_output_bytes: 100 * 1024 * 1024 * 1024,
            max_stderr_bytes: 64 * 1024,
        }
    }
}

/// An opaque, shell-free argument vector suitable for `std::process::Command`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfmpegArguments(Vec<OsString>);

impl FfmpegArguments {
    #[must_use]
    pub fn as_slice(&self) -> &[OsString] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaWorkerOutput {
    pub output_bytes: u64,
    pub stderr: String,
    pub stderr_truncated: bool,
}

#[derive(Debug, Error)]
pub enum MediaWorkerError {
    #[error("invalid media-worker limit: {0}")]
    InvalidLimit(&'static str),
    #[error("media-worker input count {actual} is outside 1..={maximum}")]
    InvalidInputCount { actual: usize, maximum: usize },
    #[error("{role} path must be an absolute local path: {path}")]
    NonAbsolutePath { role: &'static str, path: PathBuf },
    #[error("media-worker input is not a regular file: {0}")]
    InvalidInput(PathBuf),
    #[error("media-worker output parent is not a directory: {0}")]
    InvalidOutputParent(PathBuf),
    #[error("media-worker refuses to overwrite output: {0}")]
    OutputExists(PathBuf),
    #[error("media-worker input and output resolve to the same path: {0}")]
    InputIsOutput(PathBuf),
    #[error("failed to inspect media-worker path: {0}")]
    Path(#[source] io::Error),
    #[error("failed to start FFmpeg worker: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed while supervising FFmpeg worker: {0}")]
    Supervision(#[source] io::Error),
    #[error("FFmpeg worker exceeded its {0} limit")]
    ResourceLimit(&'static str),
    #[error("FFmpeg worker exceeded its time limit")]
    TimedOut,
    #[error("FFmpeg worker exited unsuccessfully ({status}): {stderr}")]
    Exited { status: ExitStatus, stderr: String },
    #[error("FFmpeg worker reported success without creating its output")]
    MissingOutput,
}

/// Supervises a single restricted `FFmpeg` child process without invoking a shell.
#[derive(Debug, Clone)]
pub struct FfmpegWorker {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
    limits: MediaWorkerLimits,
}

impl FfmpegWorker {
    /// Creates a worker for an explicitly selected `FFmpeg` executable.
    ///
    /// # Errors
    /// Returns an error if the executable is not an absolute regular-file path or limits are unsafe.
    pub fn new(
        executable: impl AsRef<Path>,
        limits: MediaWorkerLimits,
    ) -> Result<Self, MediaWorkerError> {
        Self::with_prefix(executable.as_ref(), Vec::new(), limits)
    }

    fn with_prefix(
        executable: &Path,
        prefix_arguments: Vec<OsString>,
        limits: MediaWorkerLimits,
    ) -> Result<Self, MediaWorkerError> {
        validate_limits(limits)?;
        if !executable.is_absolute() {
            return Err(MediaWorkerError::NonAbsolutePath {
                role: "executable",
                path: executable.to_owned(),
            });
        }
        let executable = executable.canonicalize().map_err(MediaWorkerError::Path)?;
        if !executable.is_file() {
            return Err(MediaWorkerError::InvalidInput(executable));
        }
        Ok(Self {
            executable,
            prefix_arguments,
            limits,
        })
    }

    /// Runs `FFmpeg` with a fixed protocol/codec-copy policy and enforces timeout and output caps.
    ///
    /// Failed, crashed, timed-out, and over-limit runs remove only the incomplete output; input
    /// tracks are never opened for writing.
    ///
    /// # Errors
    /// Returns validation, process, timeout, resource, or unsuccessful-exit errors.
    pub fn run(&self, request: &FfmpegMuxRequest) -> Result<MediaWorkerOutput, MediaWorkerError> {
        let (arguments, output) = build_arguments(request, self.limits)?;
        let mut command = Command::new(&self.executable);
        command
            .args(&self.prefix_arguments)
            .args(arguments.as_slice())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .env_clear();
        copy_runtime_environment(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(MediaWorkerError::Spawn)?;
        let stderr = child.stderr.take().ok_or_else(|| {
            MediaWorkerError::Supervision(io::Error::other("worker stderr was not captured"))
        })?;
        let stderr_limit = self.limits.max_stderr_bytes;
        let stderr_reader = thread::spawn(move || read_bounded(stderr, stderr_limit));
        let deadline = Instant::now() + self.limits.timeout;
        let failure = loop {
            if let Some(status) = child.try_wait().map_err(MediaWorkerError::Supervision)? {
                break (!status.success()).then_some(status);
            }
            if output
                .metadata()
                .is_ok_and(|metadata| metadata.len() > self.limits.max_output_bytes)
            {
                terminate(&mut child);
                let _ = stderr_reader.join();
                remove_incomplete(&output);
                return Err(MediaWorkerError::ResourceLimit("output-size"));
            }
            if Instant::now() >= deadline {
                terminate(&mut child);
                let _ = stderr_reader.join();
                remove_incomplete(&output);
                return Err(MediaWorkerError::TimedOut);
            }
            thread::sleep(POLL_INTERVAL);
        };
        let (stderr, stderr_truncated) = stderr_reader
            .join()
            .map_err(|_| MediaWorkerError::Supervision(io::Error::other("stderr reader panicked")))?
            .map_err(MediaWorkerError::Supervision)?;
        if let Some(status) = failure {
            remove_incomplete(&output);
            return Err(MediaWorkerError::Exited {
                status,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            });
        }
        let output_bytes = output
            .metadata()
            .map_err(|error| {
                remove_incomplete(&output);
                if error.kind() == io::ErrorKind::NotFound {
                    MediaWorkerError::MissingOutput
                } else {
                    MediaWorkerError::Supervision(error)
                }
            })?
            .len();
        if output_bytes > self.limits.max_output_bytes {
            remove_incomplete(&output);
            return Err(MediaWorkerError::ResourceLimit("output-size"));
        }
        Ok(MediaWorkerOutput {
            output_bytes,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            stderr_truncated,
        })
    }
}

/// Builds the exact `FFmpeg` argument vector after canonicalizing every local path.
///
/// # Errors
/// Returns an error for invalid limits, remote/relative paths, missing inputs, output overwrite,
/// or an input/output alias.
pub fn build_ffmpeg_arguments(
    request: &FfmpegMuxRequest,
    limits: MediaWorkerLimits,
) -> Result<FfmpegArguments, MediaWorkerError> {
    build_arguments(request, limits).map(|(arguments, _)| arguments)
}

fn build_arguments(
    request: &FfmpegMuxRequest,
    limits: MediaWorkerLimits,
) -> Result<(FfmpegArguments, PathBuf), MediaWorkerError> {
    validate_limits(limits)?;
    if request.inputs.is_empty() || request.inputs.len() > limits.max_inputs {
        return Err(MediaWorkerError::InvalidInputCount {
            actual: request.inputs.len(),
            maximum: limits.max_inputs,
        });
    }
    let inputs = request
        .inputs
        .iter()
        .map(|input| canonical_input(input))
        .collect::<Result<Vec<_>, _>>()?;
    let output = canonical_output(&request.output)?;
    if inputs.iter().any(|input| input == &output) {
        return Err(MediaWorkerError::InputIsOutput(output));
    }

    let mut arguments = vec![
        "-nostdin".into(),
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
    ];
    for input in &inputs {
        arguments.push("-i".into());
        arguments.push(input.as_os_str().to_owned());
    }
    for index in 0..inputs.len() {
        arguments.push("-map".into());
        arguments.push(format!("{index}:0").into());
    }
    arguments.extend([
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
        "-c".into(),
        "copy".into(),
        "-f".into(),
        request.container.muxer().into(),
        "-n".into(),
        output.as_os_str().to_owned(),
    ]);
    Ok((FfmpegArguments(arguments), output))
}

fn validate_limits(limits: MediaWorkerLimits) -> Result<(), MediaWorkerError> {
    if limits.timeout.is_zero() || limits.timeout > MAX_TIMEOUT {
        return Err(MediaWorkerError::InvalidLimit("timeout"));
    }
    if limits.max_inputs == 0 || limits.max_inputs > 32 {
        return Err(MediaWorkerError::InvalidLimit("input-count"));
    }
    if limits.max_output_bytes == 0 {
        return Err(MediaWorkerError::InvalidLimit("output-size"));
    }
    if limits.max_stderr_bytes == 0 || limits.max_stderr_bytes > MAX_STDERR_LIMIT {
        return Err(MediaWorkerError::InvalidLimit("stderr-size"));
    }
    Ok(())
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

fn canonical_input(path: &Path) -> Result<PathBuf, MediaWorkerError> {
    if !path.is_absolute() {
        return Err(MediaWorkerError::NonAbsolutePath {
            role: "input",
            path: path.to_owned(),
        });
    }
    let path = path.canonicalize().map_err(MediaWorkerError::Path)?;
    if !path.is_file() {
        return Err(MediaWorkerError::InvalidInput(path));
    }
    Ok(path)
}

fn canonical_output(path: &Path) -> Result<PathBuf, MediaWorkerError> {
    if !path.is_absolute() {
        return Err(MediaWorkerError::NonAbsolutePath {
            role: "output",
            path: path.to_owned(),
        });
    }
    if path.exists() {
        return Err(MediaWorkerError::OutputExists(path.to_owned()));
    }
    let parent = path
        .parent()
        .ok_or_else(|| MediaWorkerError::InvalidOutputParent(path.to_owned()))?;
    let parent = parent.canonicalize().map_err(MediaWorkerError::Path)?;
    if !parent.is_dir() {
        return Err(MediaWorkerError::InvalidOutputParent(parent));
    }
    let name = path
        .file_name()
        .ok_or_else(|| MediaWorkerError::InvalidOutputParent(path.to_owned()))?;
    Ok(parent.join(name))
}

fn read_bounded(mut stderr: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stderr.read(&mut buffer)?;
        if read == 0 {
            return Ok((captured, truncated));
        }
        let remaining = limit.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
}

fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn remove_incomplete(path: &Path) {
    let _ = fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::{
        FfmpegContainer, FfmpegMuxRequest, FfmpegWorker, MediaWorkerError, MediaWorkerLimits,
        build_ffmpeg_arguments,
    };
    use std::{
        ffi::OsString,
        fs,
        path::{Path, PathBuf},
        time::Duration,
    };
    use tempfile::tempdir;

    fn request(input: PathBuf, output: PathBuf) -> FfmpegMuxRequest {
        FfmpegMuxRequest {
            inputs: vec![input],
            output,
            container: FfmpegContainer::Matroska,
        }
    }

    fn fixture_worker(script: &str, directory: &Path, limits: MediaWorkerLimits) -> FfmpegWorker {
        #[cfg(windows)]
        {
            let executable = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let script_path = directory.join("fake-ffmpeg.ps1");
            fs::write(&script_path, script).expect("fixture script");
            FfmpegWorker::with_prefix(
                &executable,
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-File".into(),
                    script_path.into_os_string(),
                ],
                limits,
            )
            .expect("fixture worker")
        }
        #[cfg(not(windows))]
        {
            FfmpegWorker::with_prefix(
                std::path::Path::new("/bin/sh"),
                vec!["-c".into(), script.into(), "fixture".into()],
                limits,
            )
            .expect("fixture worker")
        }
    }

    #[test]
    fn metacharacters_remain_one_literal_argument_without_shell_tokens() {
        let directory = tempdir().expect("temp directory");
        let input = directory.path().join("track;echo-owned.mp4");
        fs::write(&input, b"track").expect("input");
        let output = directory.path().join("muxed $(owned).mkv");
        let arguments = build_ffmpeg_arguments(
            &request(input.clone(), output),
            MediaWorkerLimits::default(),
        )
        .expect("arguments");

        let position = arguments
            .as_slice()
            .iter()
            .position(|argument| argument == "-i")
            .expect("input flag");
        assert_eq!(
            arguments.as_slice()[position + 1],
            input.canonicalize().unwrap()
        );
        assert!(!arguments.as_slice().contains(&OsString::from("cmd")));
        assert!(!arguments.as_slice().contains(&OsString::from("/C")));
    }

    #[test]
    fn crash_is_reported_and_incomplete_output_is_removed() {
        let directory = tempdir().expect("temp directory");
        let input = directory.path().join("video.mp4");
        let output = directory.path().join("output.mkv");
        fs::write(&input, b"original").expect("input");
        #[cfg(windows)]
        let script = "[IO.File]::WriteAllText($args[-1], 'partial'); exit 23";
        #[cfg(not(windows))]
        let script = "for out do :; done; printf partial > \"$out\"; exit 23";
        let worker = fixture_worker(script, directory.path(), MediaWorkerLimits::default());

        assert!(matches!(
            worker.run(&request(input.clone(), output.clone())),
            Err(MediaWorkerError::Exited { .. })
        ));
        assert_eq!(fs::read(input).expect("preserved input"), b"original");
        assert!(!output.exists());
    }

    #[test]
    fn timeout_kills_worker_and_removes_incomplete_output() {
        let directory = tempdir().expect("temp directory");
        let input = directory.path().join("audio.webm");
        let output = directory.path().join("output.webm");
        fs::write(&input, b"original").expect("input");
        let limits = MediaWorkerLimits {
            timeout: Duration::from_millis(100),
            ..MediaWorkerLimits::default()
        };
        #[cfg(windows)]
        let script = "[IO.File]::WriteAllText($args[-1], 'partial'); Start-Sleep -Seconds 5";
        #[cfg(not(windows))]
        let script = "for out do :; done; printf partial > \"$out\"; sleep 5";
        let worker = fixture_worker(script, directory.path(), limits);

        assert!(matches!(
            worker.run(&request(input, output.clone())),
            Err(MediaWorkerError::TimedOut)
        ));
        assert!(!output.exists());
    }

    #[test]
    fn input_and_output_resource_limits_are_enforced() {
        let directory = tempdir().expect("temp directory");
        let input = directory.path().join("audio.webm");
        let output = directory.path().join("output.webm");
        fs::write(&input, b"original").expect("input");
        let limits = MediaWorkerLimits {
            max_inputs: 1,
            max_output_bytes: 8,
            ..MediaWorkerLimits::default()
        };
        let excessive = FfmpegMuxRequest {
            inputs: vec![input.clone(), input.clone()],
            output: output.clone(),
            container: FfmpegContainer::WebM,
        };
        assert!(matches!(
            build_ffmpeg_arguments(&excessive, limits),
            Err(MediaWorkerError::InvalidInputCount { .. })
        ));

        #[cfg(windows)]
        let script = "[IO.File]::WriteAllBytes($args[-1], [byte[]](0..31)); exit 0";
        #[cfg(not(windows))]
        let script = "for out do :; done; printf 12345678901234567890123456789012 > \"$out\"";
        let worker = fixture_worker(script, directory.path(), limits);
        let result = worker.run(&request(input, output.clone()));
        assert!(
            matches!(result, Err(MediaWorkerError::ResourceLimit("output-size"))),
            "unexpected worker result: {result:?}"
        );
        assert!(!output.exists());
    }
}
