//! Container selection and post-mux validation for downloaded audio/video tracks.

use std::{
    ffi::OsString,
    fs,
    io::{self, Read},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    str::FromStr,
    thread,
    time::{Duration, Instant},
};

use serde::Deserialize;
use thiserror::Error;

use crate::media_worker::{
    FfmpegContainer, FfmpegMuxRequest, FfmpegWorker, MediaWorkerError, MediaWorkerOutput,
};

const POLL_INTERVAL: Duration = Duration::from_millis(10);
const MAX_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_CAPTURE_BYTES: usize = 1024 * 1024;
const DEFAULT_START_TOLERANCE: Duration = Duration::from_millis(250);
const DEFAULT_DURATION_TOLERANCE: Duration = Duration::from_millis(500);
const MAX_PROBE_SECONDS: f64 = 31_556_952_000.0;

/// Video codecs supported by the copy-only mux pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Av1,
    H264,
    Hevc,
    Vp8,
    Vp9,
}

impl VideoCodec {
    #[must_use]
    pub const fn ffprobe_name(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::H264 => "h264",
            Self::Hevc => "hevc",
            Self::Vp8 => "vp8",
            Self::Vp9 => "vp9",
        }
    }
}

impl FromStr for VideoCodec {
    type Err = UnsupportedCodec;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let normalized = codec_prefix(value);
        match normalized.as_str() {
            "av1" | "av01" => Ok(Self::Av1),
            "h264" | "avc1" | "avc3" => Ok(Self::H264),
            "hevc" | "h265" | "hev1" | "hvc1" => Ok(Self::Hevc),
            "vp8" | "vp08" => Ok(Self::Vp8),
            "vp9" | "vp09" => Ok(Self::Vp9),
            _ => Err(UnsupportedCodec {
                kind: "video",
                value: value.to_owned(),
            }),
        }
    }
}

/// Audio codecs supported by the copy-only mux pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioCodec {
    Aac,
    Ac3,
    Eac3,
    Flac,
    Mp3,
    Opus,
    Vorbis,
}

impl AudioCodec {
    #[must_use]
    pub const fn ffprobe_name(self) -> &'static str {
        match self {
            Self::Aac => "aac",
            Self::Ac3 => "ac3",
            Self::Eac3 => "eac3",
            Self::Flac => "flac",
            Self::Mp3 => "mp3",
            Self::Opus => "opus",
            Self::Vorbis => "vorbis",
        }
    }
}

impl FromStr for AudioCodec {
    type Err = UnsupportedCodec;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let full = value.trim().to_ascii_lowercase();
        if matches!(full.as_str(), "mp4a.69" | "mp4a.6b") {
            return Ok(Self::Mp3);
        }
        let normalized = codec_prefix(value);
        match normalized.as_str() {
            "aac" | "mp4a" => Ok(Self::Aac),
            "ac3" | "ac-3" => Ok(Self::Ac3),
            "eac3" | "ec-3" => Ok(Self::Eac3),
            "flac" => Ok(Self::Flac),
            "mp3" => Ok(Self::Mp3),
            "opus" => Ok(Self::Opus),
            "vorbis" => Ok(Self::Vorbis),
            _ => Err(UnsupportedCodec {
                kind: "audio",
                value: value.to_owned(),
            }),
        }
    }
}

fn codec_prefix(value: &str) -> String {
    value
        .trim()
        .to_ascii_lowercase()
        .split_once('.')
        .map_or_else(
            || value.trim().to_ascii_lowercase(),
            |(prefix, _)| prefix.to_owned(),
        )
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
#[error("unsupported {kind} codec: {value}")]
pub struct UnsupportedCodec {
    kind: &'static str,
    value: String,
}

/// The deterministic copy-only mux decision for one video and one audio track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxPlan {
    pub container: FfmpegContainer,
    pub video: VideoCodec,
    pub audio: AudioCodec,
}

impl MuxPlan {
    /// Selects the narrowest broadly interoperable container without transcoding.
    ///
    /// Native `WebM` pairs remain `WebM`, native `MP4` pairs remain `MP4`, and mixed or `FLAC` pairs use
    /// Matroska as the lossless compatibility fallback.
    #[must_use]
    pub const fn select(video: VideoCodec, audio: AudioCodec) -> Self {
        let container = if webm_compatible(video, audio) {
            FfmpegContainer::WebM
        } else if mp4_compatible(video, audio) {
            FfmpegContainer::Mp4
        } else {
            FfmpegContainer::Matroska
        };
        Self {
            container,
            video,
            audio,
        }
    }

    /// Validates an explicitly requested container against the copy-only codec matrix.
    ///
    /// # Errors
    /// Returns an error when the pair cannot be represented by that muxer without transcoding.
    pub const fn for_container(
        container: FfmpegContainer,
        video: VideoCodec,
        audio: AudioCodec,
    ) -> Result<Self, IncompatibleContainer> {
        let compatible = match container {
            FfmpegContainer::Mp4 => mp4_compatible(video, audio),
            FfmpegContainer::Matroska => true,
            FfmpegContainer::WebM => webm_compatible(video, audio),
        };
        if compatible {
            Ok(Self {
                container,
                video,
                audio,
            })
        } else {
            Err(IncompatibleContainer {
                container,
                video,
                audio,
            })
        }
    }

    #[must_use]
    pub const fn extension(self) -> &'static str {
        match self.container {
            FfmpegContainer::Mp4 => "mp4",
            FfmpegContainer::Matroska => "mkv",
            FfmpegContainer::WebM => "webm",
        }
    }
}

const fn mp4_compatible(video: VideoCodec, audio: AudioCodec) -> bool {
    matches!(video, VideoCodec::Av1 | VideoCodec::H264 | VideoCodec::Hevc)
        && matches!(
            audio,
            AudioCodec::Aac | AudioCodec::Ac3 | AudioCodec::Eac3 | AudioCodec::Mp3
        )
}

const fn webm_compatible(video: VideoCodec, audio: AudioCodec) -> bool {
    matches!(video, VideoCodec::Av1 | VideoCodec::Vp8 | VideoCodec::Vp9)
        && matches!(audio, AudioCodec::Opus | AudioCodec::Vorbis)
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
#[error("{video:?}/{audio:?} cannot be copied into {container:?}")]
pub struct IncompatibleContainer {
    pub container: FfmpegContainer,
    pub video: VideoCodec,
    pub audio: AudioCodec,
}

/// Expected output identity and timestamp bounds used after muxing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxExpectation {
    pub plan: MuxPlan,
    pub duration: Duration,
    pub start_tolerance: Duration,
    pub duration_tolerance: Duration,
}

impl MuxExpectation {
    /// Creates an expectation with conservative container timestamp tolerances.
    ///
    /// # Errors
    /// Returns an error for a zero duration.
    pub fn new(plan: MuxPlan, duration: Duration) -> Result<Self, MuxValidationError> {
        Self::with_tolerances(
            plan,
            duration,
            DEFAULT_START_TOLERANCE,
            DEFAULT_DURATION_TOLERANCE,
        )
    }

    /// Creates an expectation with caller-selected nonzero tolerances.
    ///
    /// # Errors
    /// Returns an error for a zero duration or zero tolerance.
    pub fn with_tolerances(
        plan: MuxPlan,
        duration: Duration,
        start_tolerance: Duration,
        duration_tolerance: Duration,
    ) -> Result<Self, MuxValidationError> {
        if duration.is_zero() {
            return Err(MuxValidationError::InvalidExpectation("duration"));
        }
        if start_tolerance.is_zero() {
            return Err(MuxValidationError::InvalidExpectation("start-tolerance"));
        }
        if duration_tolerance.is_zero() {
            return Err(MuxValidationError::InvalidExpectation("duration-tolerance"));
        }
        Ok(Self {
            plan,
            duration,
            start_tolerance,
            duration_tolerance,
        })
    }
}

/// Timestamp measurements accepted from `ffprobe` after all checks pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MuxValidation {
    pub format_duration: Duration,
    pub video_start_offset: Duration,
    pub audio_start_offset: Duration,
    pub av_start_delta: Duration,
    pub av_end_delta: Duration,
}

#[derive(Debug, Error)]
pub enum MuxValidationError {
    #[error("invalid mux expectation: {0}")]
    InvalidExpectation(&'static str),
    #[error("ffprobe output is not valid JSON: {0}")]
    InvalidJson(#[source] serde_json::Error),
    #[error("ffprobe output is missing or has an invalid {0}")]
    InvalidField(&'static str),
    #[error("ffprobe reported {actual} {kind} streams; exactly one is required")]
    StreamCount { kind: &'static str, actual: usize },
    #[error("ffprobe container {actual:?} is incompatible with expected {expected:?}")]
    ContainerMismatch {
        expected: FfmpegContainer,
        actual: String,
    },
    #[error("ffprobe {kind} codec {actual:?} does not match expected {expected}")]
    CodecMismatch {
        kind: &'static str,
        expected: &'static str,
        actual: String,
    },
    #[error("{field} differs by {delta:?}, above the allowed {tolerance:?}")]
    TimestampMismatch {
        field: &'static str,
        delta: Duration,
        tolerance: Duration,
    },
}

#[derive(Debug, Deserialize)]
struct ProbeDocument {
    streams: Vec<ProbeStream>,
    format: ProbeFormat,
}

#[derive(Debug, Deserialize)]
struct ProbeStream {
    codec_type: String,
    codec_name: String,
    start_time: String,
    duration: String,
}

#[derive(Debug, Deserialize)]
struct ProbeFormat {
    format_name: String,
    start_time: String,
    duration: String,
}

/// Validates bounded `ffprobe` JSON without trusting filenames or stderr text.
///
/// # Errors
/// Returns a precise codec, container, duration, or A/V synchronization failure.
pub fn validate_ffprobe_json(
    json: &[u8],
    expected: MuxExpectation,
) -> Result<MuxValidation, MuxValidationError> {
    let document: ProbeDocument =
        serde_json::from_slice(json).map_err(MuxValidationError::InvalidJson)?;
    validate_container_name(&document.format.format_name, expected.plan.container)?;

    let video = exactly_one_stream(&document.streams, "video")?;
    let audio = exactly_one_stream(&document.streams, "audio")?;
    validate_codec(
        "video",
        video.codec_name.as_str(),
        expected.plan.video.ffprobe_name(),
    )?;
    validate_codec(
        "audio",
        audio.codec_name.as_str(),
        expected.plan.audio.ffprobe_name(),
    )?;

    let format_start = seconds(&document.format.start_time, "format start time")?;
    let format_duration = positive_seconds(&document.format.duration, "format duration")?;
    let video_start = seconds(&video.start_time, "video start time")?;
    let video_duration = positive_seconds(&video.duration, "video duration")?;
    let audio_start = seconds(&audio.start_time, "audio start time")?;
    let audio_duration = positive_seconds(&audio.duration, "audio duration")?;
    let expected_duration = duration_micros(expected.duration)?;
    let start_tolerance = duration_micros(expected.start_tolerance)?;
    let duration_tolerance = duration_micros(expected.duration_tolerance)?;

    require_delta(
        "format start timestamp",
        format_start,
        0,
        start_tolerance,
        expected.start_tolerance,
    )?;
    require_delta(
        "video start timestamp",
        video_start,
        format_start,
        start_tolerance,
        expected.start_tolerance,
    )?;
    require_delta(
        "audio start timestamp",
        audio_start,
        format_start,
        start_tolerance,
        expected.start_tolerance,
    )?;
    require_delta(
        "format duration",
        format_duration,
        expected_duration,
        duration_tolerance,
        expected.duration_tolerance,
    )?;
    require_delta(
        "video duration",
        video_duration,
        expected_duration,
        duration_tolerance,
        expected.duration_tolerance,
    )?;
    require_delta(
        "audio duration",
        audio_duration,
        expected_duration,
        duration_tolerance,
        expected.duration_tolerance,
    )?;

    let av_start_delta = absolute_delta(video_start, audio_start);
    require_delta(
        "A/V start synchronization",
        video_start,
        audio_start,
        start_tolerance,
        expected.start_tolerance,
    )?;
    let video_end = video_start
        .checked_add(video_duration)
        .ok_or(MuxValidationError::InvalidField("video end timestamp"))?;
    let audio_end = audio_start
        .checked_add(audio_duration)
        .ok_or(MuxValidationError::InvalidField("audio end timestamp"))?;
    let av_end_delta = absolute_delta(video_end, audio_end);
    require_delta(
        "A/V end synchronization",
        video_end,
        audio_end,
        duration_tolerance,
        expected.duration_tolerance,
    )?;

    Ok(MuxValidation {
        format_duration: micros_duration(format_duration)?,
        video_start_offset: micros_duration(absolute_delta(video_start, format_start))?,
        audio_start_offset: micros_duration(absolute_delta(audio_start, format_start))?,
        av_start_delta: micros_duration(av_start_delta)?,
        av_end_delta: micros_duration(av_end_delta)?,
    })
}

fn exactly_one_stream<'a>(
    streams: &'a [ProbeStream],
    kind: &'static str,
) -> Result<&'a ProbeStream, MuxValidationError> {
    let matching = streams
        .iter()
        .filter(|stream| stream.codec_type == kind)
        .collect::<Vec<_>>();
    if matching.len() == 1 {
        Ok(matching[0])
    } else {
        Err(MuxValidationError::StreamCount {
            kind,
            actual: matching.len(),
        })
    }
}

fn validate_container_name(
    actual: &str,
    expected: FfmpegContainer,
) -> Result<(), MuxValidationError> {
    let names = actual.split(',').map(str::trim).collect::<Vec<_>>();
    let matches = match expected {
        FfmpegContainer::Mp4 => names
            .iter()
            .any(|name| matches!(*name, "mov" | "mp4" | "m4a" | "3gp" | "3g2" | "mj2")),
        FfmpegContainer::Matroska | FfmpegContainer::WebM => names
            .iter()
            .any(|name| matches!(*name, "matroska" | "webm")),
    };
    if matches {
        Ok(())
    } else {
        Err(MuxValidationError::ContainerMismatch {
            expected,
            actual: actual.to_owned(),
        })
    }
}

fn validate_codec(
    kind: &'static str,
    actual: &str,
    expected: &'static str,
) -> Result<(), MuxValidationError> {
    if actual.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err(MuxValidationError::CodecMismatch {
            kind,
            expected,
            actual: actual.to_owned(),
        })
    }
}

fn seconds(value: &str, field: &'static str) -> Result<i64, MuxValidationError> {
    let seconds = value
        .parse::<f64>()
        .map_err(|_| MuxValidationError::InvalidField(field))?;
    let micros = seconds * 1_000_000.0;
    if !micros.is_finite() || seconds.abs() > MAX_PROBE_SECONDS {
        return Err(MuxValidationError::InvalidField(field));
    }
    #[allow(clippy::cast_possible_truncation)]
    Ok(micros.round() as i64)
}

fn positive_seconds(value: &str, field: &'static str) -> Result<i64, MuxValidationError> {
    let value = seconds(value, field)?;
    if value <= 0 {
        Err(MuxValidationError::InvalidField(field))
    } else {
        Ok(value)
    }
}

fn duration_micros(value: Duration) -> Result<i64, MuxValidationError> {
    i64::try_from(value.as_micros()).map_err(|_| MuxValidationError::InvalidField("duration"))
}

fn micros_duration(value: i64) -> Result<Duration, MuxValidationError> {
    u64::try_from(value)
        .map(Duration::from_micros)
        .map_err(|_| MuxValidationError::InvalidField("negative timestamp"))
}

fn absolute_delta(left: i64, right: i64) -> i64 {
    left.abs_diff(right).try_into().unwrap_or(i64::MAX)
}

fn require_delta(
    field: &'static str,
    left: i64,
    right: i64,
    tolerance_micros: i64,
    tolerance: Duration,
) -> Result<(), MuxValidationError> {
    let delta = absolute_delta(left, right);
    if delta <= tolerance_micros {
        Ok(())
    } else {
        Err(MuxValidationError::TimestampMismatch {
            field,
            delta: Duration::from_micros(delta.try_into().unwrap_or(u64::MAX)),
            tolerance,
        })
    }
}

/// Per-invocation limits for the local-only `ffprobe` verifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FfprobeLimits {
    pub timeout: Duration,
    pub max_stdout_bytes: usize,
    pub max_stderr_bytes: usize,
}

impl Default for FfprobeLimits {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            max_stdout_bytes: 256 * 1024,
            max_stderr_bytes: 64 * 1024,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfprobeArguments(Vec<OsString>);

impl FfprobeArguments {
    #[must_use]
    pub fn as_slice(&self) -> &[OsString] {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfprobeReport {
    pub validation: MuxValidation,
    pub stderr: String,
    pub stderr_truncated: bool,
}

#[derive(Debug, Error)]
pub enum FfprobeError {
    #[error("invalid ffprobe limit: {0}")]
    InvalidLimit(&'static str),
    #[error("ffprobe {role} path must be an absolute local path: {path}")]
    NonAbsolutePath { role: &'static str, path: PathBuf },
    #[error("ffprobe path is not a regular file: {0}")]
    InvalidFile(PathBuf),
    #[error("failed to inspect ffprobe path: {0}")]
    Path(#[source] io::Error),
    #[error("failed to start ffprobe: {0}")]
    Spawn(#[source] io::Error),
    #[error("failed while supervising ffprobe: {0}")]
    Supervision(#[source] io::Error),
    #[error("ffprobe exceeded its {0} limit")]
    ResourceLimit(&'static str),
    #[error("ffprobe exceeded its time limit")]
    TimedOut,
    #[error("ffprobe exited unsuccessfully ({status}): {stderr}")]
    Exited { status: ExitStatus, stderr: String },
    #[error(transparent)]
    Validation(#[from] MuxValidationError),
}

/// A shell-free, local-file-only `ffprobe` process with bounded output and runtime.
#[derive(Debug, Clone)]
pub struct FfprobeWorker {
    executable: PathBuf,
    prefix_arguments: Vec<OsString>,
    limits: FfprobeLimits,
}

impl FfprobeWorker {
    /// Creates a verifier for an explicitly selected `ffprobe` executable.
    ///
    /// # Errors
    /// Returns an error when the executable or limits are invalid.
    pub fn new(executable: impl AsRef<Path>, limits: FfprobeLimits) -> Result<Self, FfprobeError> {
        Self::with_prefix(executable.as_ref(), Vec::new(), limits)
    }

    fn with_prefix(
        executable: &Path,
        prefix_arguments: Vec<OsString>,
        limits: FfprobeLimits,
    ) -> Result<Self, FfprobeError> {
        validate_probe_limits(limits)?;
        if !executable.is_absolute() {
            return Err(FfprobeError::NonAbsolutePath {
                role: "executable",
                path: executable.to_owned(),
            });
        }
        let executable = executable.canonicalize().map_err(FfprobeError::Path)?;
        if !executable.is_file() {
            return Err(FfprobeError::InvalidFile(executable));
        }
        Ok(Self {
            executable,
            prefix_arguments,
            limits,
        })
    }

    /// Probes one completed local output and validates its container, codecs, and timestamps.
    ///
    /// # Errors
    /// Returns path, process, resource, or media-validation errors.
    pub fn validate(
        &self,
        output: &Path,
        expected: MuxExpectation,
    ) -> Result<FfprobeReport, FfprobeError> {
        let arguments = build_ffprobe_arguments(output, self.limits)?;
        let mut command = Command::new(&self.executable);
        command
            .args(&self.prefix_arguments)
            .args(arguments.as_slice())
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env_clear();
        copy_runtime_environment(&mut command);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let mut child = command.spawn().map_err(FfprobeError::Spawn)?;
        let stdout = child.stdout.take().ok_or_else(|| {
            FfprobeError::Supervision(io::Error::other("ffprobe stdout was not captured"))
        })?;
        let stderr = child.stderr.take().ok_or_else(|| {
            FfprobeError::Supervision(io::Error::other("ffprobe stderr was not captured"))
        })?;
        let stdout_limit = self.limits.max_stdout_bytes;
        let stderr_limit = self.limits.max_stderr_bytes;
        let stdout_reader = thread::spawn(move || read_bounded(stdout, stdout_limit));
        let stderr_reader = thread::spawn(move || read_bounded(stderr, stderr_limit));
        let deadline = Instant::now() + self.limits.timeout;
        let status = loop {
            if let Some(status) = child.try_wait().map_err(FfprobeError::Supervision)? {
                break status;
            }
            if Instant::now() >= deadline {
                terminate(&mut child);
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(FfprobeError::TimedOut);
            }
            thread::sleep(POLL_INTERVAL);
        };
        let (stdout, stdout_truncated) = join_reader(stdout_reader, "stdout")?;
        let (stderr, stderr_truncated) = join_reader(stderr_reader, "stderr")?;
        if stdout_truncated {
            return Err(FfprobeError::ResourceLimit("stdout"));
        }
        if !status.success() {
            return Err(FfprobeError::Exited {
                status,
                stderr: String::from_utf8_lossy(&stderr).into_owned(),
            });
        }
        Ok(FfprobeReport {
            validation: validate_ffprobe_json(&stdout, expected)?,
            stderr: String::from_utf8_lossy(&stderr).into_owned(),
            stderr_truncated,
        })
    }
}

/// Builds the fixed local-file-only `ffprobe` argument vector.
///
/// # Errors
/// Returns an error for invalid limits or a non-local/missing output.
pub fn build_ffprobe_arguments(
    output: &Path,
    limits: FfprobeLimits,
) -> Result<FfprobeArguments, FfprobeError> {
    validate_probe_limits(limits)?;
    if !output.is_absolute() {
        return Err(FfprobeError::NonAbsolutePath {
            role: "input",
            path: output.to_owned(),
        });
    }
    let output = output.canonicalize().map_err(FfprobeError::Path)?;
    if !output.is_file() {
        return Err(FfprobeError::InvalidFile(output));
    }
    Ok(FfprobeArguments(vec![
        "-v".into(),
        "error".into(),
        "-protocol_whitelist".into(),
        "file".into(),
        "-show_entries".into(),
        "format=format_name,start_time,duration:stream=codec_type,codec_name,start_time,duration"
            .into(),
        "-of".into(),
        "json".into(),
        output.into_os_string(),
    ]))
}

fn validate_probe_limits(limits: FfprobeLimits) -> Result<(), FfprobeError> {
    if limits.timeout.is_zero() || limits.timeout > MAX_TIMEOUT {
        return Err(FfprobeError::InvalidLimit("timeout"));
    }
    if limits.max_stdout_bytes == 0 || limits.max_stdout_bytes > MAX_CAPTURE_BYTES {
        return Err(FfprobeError::InvalidLimit("stdout"));
    }
    if limits.max_stderr_bytes == 0 || limits.max_stderr_bytes > MAX_CAPTURE_BYTES {
        return Err(FfprobeError::InvalidLimit("stderr"));
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

fn read_bounded(mut stream: impl Read, limit: usize) -> io::Result<(Vec<u8>, bool)> {
    let mut captured = Vec::with_capacity(limit.min(8192));
    let mut truncated = false;
    let mut buffer = [0_u8; 8192];
    loop {
        let read = stream.read(&mut buffer)?;
        if read == 0 {
            return Ok((captured, truncated));
        }
        let remaining = limit.saturating_sub(captured.len());
        captured.extend_from_slice(&buffer[..read.min(remaining)]);
        truncated |= read > remaining;
    }
}

fn join_reader(
    reader: thread::JoinHandle<io::Result<(Vec<u8>, bool)>>,
    name: &'static str,
) -> Result<(Vec<u8>, bool), FfprobeError> {
    reader
        .join()
        .map_err(|_| {
            FfprobeError::Supervision(io::Error::other(format!("{name} reader panicked")))
        })?
        .map_err(FfprobeError::Supervision)
}

fn terminate(child: &mut std::process::Child) {
    let _ = child.kill();
    let _ = child.wait();
}

#[derive(Debug)]
pub struct ValidatedMuxOutput {
    pub mux: MediaWorkerOutput,
    pub probe: FfprobeReport,
}

#[derive(Debug, Error)]
pub enum ValidatedMuxError {
    #[error("mux request container {requested:?} does not match validated plan {expected:?}")]
    PlanMismatch {
        requested: FfmpegContainer,
        expected: FfmpegContainer,
    },
    #[error(transparent)]
    Mux(#[from] MediaWorkerError),
    #[error("muxed output failed validation: {0}")]
    Probe(#[from] FfprobeError),
}

/// Runs the restricted mux worker and accepts its output only after an independent probe passes.
///
/// A failed validation removes the untrusted output while preserving both source tracks.
///
/// # Errors
/// Returns the underlying mux or probe failure.
pub fn mux_and_validate(
    muxer: &FfmpegWorker,
    probe: &FfprobeWorker,
    request: &FfmpegMuxRequest,
    expected: MuxExpectation,
) -> Result<ValidatedMuxOutput, ValidatedMuxError> {
    if request.container != expected.plan.container {
        return Err(ValidatedMuxError::PlanMismatch {
            requested: request.container,
            expected: expected.plan.container,
        });
    }
    let mux = muxer.run(request)?;
    match probe.validate(&request.output, expected) {
        Ok(probe) => Ok(ValidatedMuxOutput { mux, probe }),
        Err(error) => {
            let _ = fs::remove_file(&request.output);
            Err(ValidatedMuxError::Probe(error))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        AudioCodec, FfprobeLimits, FfprobeWorker, MuxExpectation, MuxPlan, MuxValidationError,
        ValidatedMuxError, VideoCodec, build_ffprobe_arguments, mux_and_validate,
        validate_ffprobe_json,
    };
    use crate::media_worker::{FfmpegContainer, FfmpegMuxRequest, FfmpegWorker, MediaWorkerLimits};
    use std::{ffi::OsString, fs, path::PathBuf, time::Duration};
    use tempfile::tempdir;

    fn expectation(
        container: FfmpegContainer,
        video: VideoCodec,
        audio: AudioCodec,
    ) -> MuxExpectation {
        MuxExpectation::new(
            MuxPlan::for_container(container, video, audio).expect("compatible plan"),
            Duration::from_secs(12),
        )
        .expect("expectation")
    }

    fn probe_json(container: &str, video: &str, audio: &str) -> Vec<u8> {
        format!(
            r#"{{"streams":[{{"codec_type":"video","codec_name":"{video}","start_time":"0.000000","duration":"12.000000"}},{{"codec_type":"audio","codec_name":"{audio}","start_time":"0.021000","duration":"11.979000"}}],"format":{{"format_name":"{container}","start_time":"0.000000","duration":"12.000000"}}}}"#
        )
        .into_bytes()
    }

    #[test]
    fn mp4_mkv_webm_compatibility_rules_are_deterministic() {
        assert_eq!(
            MuxPlan::select(VideoCodec::H264, AudioCodec::Aac).container,
            FfmpegContainer::Mp4
        );
        assert_eq!(
            MuxPlan::select(VideoCodec::Vp9, AudioCodec::Opus).container,
            FfmpegContainer::WebM
        );
        assert_eq!(
            MuxPlan::select(VideoCodec::H264, AudioCodec::Opus).container,
            FfmpegContainer::Matroska
        );
        assert_eq!(
            MuxPlan::select(VideoCodec::Vp9, AudioCodec::Aac).container,
            FfmpegContainer::Matroska
        );
        assert_eq!(
            MuxPlan::select(VideoCodec::Av1, AudioCodec::Opus).extension(),
            "webm"
        );
        assert!(
            MuxPlan::for_container(FfmpegContainer::Mp4, VideoCodec::Vp9, AudioCodec::Opus)
                .is_err()
        );
        assert!(
            MuxPlan::for_container(FfmpegContainer::WebM, VideoCodec::H264, AudioCodec::Aac)
                .is_err()
        );
        assert_eq!("avc1.640028".parse(), Ok(VideoCodec::H264));
        assert_eq!("mp4a.40.2".parse(), Ok(AudioCodec::Aac));
        assert_eq!("vp09.00.40.08".parse(), Ok(VideoCodec::Vp9));
    }

    #[test]
    fn ffprobe_duration_sync_and_codec_assertions_cover_all_containers() {
        for (container, format_name, video, audio) in [
            (
                FfmpegContainer::Mp4,
                "mov,mp4,m4a,3gp,3g2,mj2",
                VideoCodec::H264,
                AudioCodec::Aac,
            ),
            (
                FfmpegContainer::Matroska,
                "matroska,webm",
                VideoCodec::H264,
                AudioCodec::Opus,
            ),
            (
                FfmpegContainer::WebM,
                "matroska,webm",
                VideoCodec::Vp9,
                AudioCodec::Opus,
            ),
        ] {
            let validation = validate_ffprobe_json(
                &probe_json(format_name, video.ffprobe_name(), audio.ffprobe_name()),
                expectation(container, video, audio),
            )
            .expect("valid mux report");
            assert_eq!(validation.format_duration, Duration::from_secs(12));
            assert_eq!(validation.av_start_delta, Duration::from_millis(21));
            assert_eq!(validation.av_end_delta, Duration::ZERO);
        }
    }

    #[test]
    fn ffprobe_rejects_codec_duration_and_sync_mismatches() {
        let expected = expectation(FfmpegContainer::Mp4, VideoCodec::H264, AudioCodec::Aac);
        assert!(matches!(
            validate_ffprobe_json(&probe_json("mov,mp4", "hevc", "aac"), expected),
            Err(MuxValidationError::CodecMismatch { kind: "video", .. })
        ));

        let duration_mismatch = br#"{"streams":[{"codec_type":"video","codec_name":"h264","start_time":"0","duration":"12"},{"codec_type":"audio","codec_name":"aac","start_time":"0","duration":"10"}],"format":{"format_name":"mp4","start_time":"0","duration":"12"}}"#;
        assert!(matches!(
            validate_ffprobe_json(duration_mismatch, expected),
            Err(MuxValidationError::TimestampMismatch {
                field: "audio duration",
                ..
            })
        ));

        let sync_mismatch = br#"{"streams":[{"codec_type":"video","codec_name":"h264","start_time":"0","duration":"12"},{"codec_type":"audio","codec_name":"aac","start_time":"1","duration":"12"}],"format":{"format_name":"mp4","start_time":"0","duration":"12"}}"#;
        assert!(matches!(
            validate_ffprobe_json(sync_mismatch, expected),
            Err(MuxValidationError::TimestampMismatch {
                field: "audio start timestamp",
                ..
            })
        ));
    }

    #[test]
    fn ffprobe_worker_uses_fixed_local_arguments_and_validates_bounded_json() {
        let directory = tempdir().expect("temp directory");
        let output = directory.path().join("muxed $(literal).mp4");
        fs::write(&output, b"fixture").expect("output fixture");
        let arguments =
            build_ffprobe_arguments(&output, FfprobeLimits::default()).expect("probe arguments");
        assert_eq!(
            arguments.as_slice().last(),
            Some(&output.canonicalize().unwrap().into_os_string())
        );
        assert!(arguments.as_slice().contains(&OsString::from("file")));
        assert!(!arguments.as_slice().contains(&OsString::from("cmd")));

        let json = String::from_utf8(probe_json("mov,mp4", "h264", "aac")).unwrap();
        #[cfg(windows)]
        let (executable, prefix) = {
            let executable = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let script = directory.path().join("fake-ffprobe.ps1");
            fs::write(
                &script,
                format!("[Console]::Out.Write('{}')", json.replace('\'', "''")),
            )
            .expect("script");
            (
                executable,
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-File".into(),
                    script.into_os_string(),
                ],
            )
        };
        #[cfg(not(windows))]
        let (executable, prefix) = (
            PathBuf::from("/bin/sh"),
            vec![
                "-c".into(),
                format!("printf '%s' '{json}'").into(),
                "fixture".into(),
            ],
        );
        let worker = FfprobeWorker::with_prefix(&executable, prefix, FfprobeLimits::default())
            .expect("worker");
        let report = worker
            .validate(
                &output,
                expectation(FfmpegContainer::Mp4, VideoCodec::H264, AudioCodec::Aac),
            )
            .expect("validated report");
        assert_eq!(report.validation.av_start_delta, Duration::from_millis(21));
    }

    #[test]
    fn failed_post_mux_validation_removes_output_and_preserves_both_tracks() {
        let directory = tempdir().expect("temp directory");
        let video = directory.path().join("video.mp4");
        let audio = directory.path().join("audio.m4a");
        let output = directory.path().join("output.mp4");
        fs::write(&video, b"video-original").expect("video");
        fs::write(&audio, b"audio-original").expect("audio");

        #[cfg(windows)]
        let (executable, mux_prefix, probe_prefix) = {
            let executable = PathBuf::from(std::env::var_os("SystemRoot").expect("SystemRoot"))
                .join("System32/WindowsPowerShell/v1.0/powershell.exe");
            let mux_script = directory.path().join("fake-ffmpeg.ps1");
            fs::write(&mux_script, "[IO.File]::WriteAllText($args[-1], 'muxed')")
                .expect("mux script");
            let probe_script = directory.path().join("bad-ffprobe.ps1");
            let invalid = String::from_utf8(probe_json("mov,mp4", "hevc", "aac")).unwrap();
            fs::write(
                &probe_script,
                format!("[Console]::Out.Write('{}')", invalid.replace('\'', "''")),
            )
            .expect("probe script");
            (
                executable,
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-File".into(),
                    mux_script.into_os_string(),
                ],
                vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-File".into(),
                    probe_script.into_os_string(),
                ],
            )
        };
        #[cfg(not(windows))]
        let (executable, mux_prefix, probe_prefix) = {
            let invalid = String::from_utf8(probe_json("mov,mp4", "hevc", "aac")).unwrap();
            (
                PathBuf::from("/bin/sh"),
                vec![
                    "-c".into(),
                    "for out do :; done; printf muxed > \"$out\"".into(),
                    "fixture".into(),
                ],
                vec![
                    "-c".into(),
                    format!("printf '%s' '{invalid}'").into(),
                    "fixture".into(),
                ],
            )
        };
        let muxer =
            FfmpegWorker::with_prefix(&executable, mux_prefix, MediaWorkerLimits::default())
                .expect("mux worker");
        let probe = FfprobeWorker::with_prefix(&executable, probe_prefix, FfprobeLimits::default())
            .expect("probe worker");
        let request = FfmpegMuxRequest {
            inputs: vec![video.clone(), audio.clone()],
            output: output.clone(),
            container: FfmpegContainer::Mp4,
        };

        assert!(matches!(
            mux_and_validate(
                &muxer,
                &probe,
                &request,
                expectation(FfmpegContainer::Mp4, VideoCodec::H264, AudioCodec::Aac,),
            ),
            Err(ValidatedMuxError::Probe(_))
        ));
        assert!(!output.exists());
        assert_eq!(fs::read(video).expect("video preserved"), b"video-original");
        assert_eq!(fs::read(audio).expect("audio preserved"), b"audio-original");
    }
}
