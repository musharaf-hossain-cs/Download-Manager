//! Ordered HLS video-on-demand fragment transfer.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::{Duration, SystemTime},
};

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::{HttpUrl, JobId, Segment, SegmentId, UnixMillis},
    media::{HlsByteRange, HlsInitSegment, HlsMediaPlaylist, HlsMediaSegment},
    media_refresh::{RefreshError, ValidatedRefresh, validate_hls_refresh},
    retry::{RetryDecision, RetryPolicy},
    storage::{Database, StorageError},
};

const BUFFER_SIZE: usize = 64 * 1024;

/// Result of writing one complete HLS VOD stream in playback order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsDownloadOutcome {
    pub downloaded_bytes: u64,
    pub media_segments: usize,
    pub init_segments: usize,
    pub sha256: String,
}

/// Result of reconciling durable HLS fragments and downloading only missing units.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsResumeOutcome {
    pub downloaded_bytes: u64,
    pub skipped_units: usize,
    pub downloaded_units: usize,
    pub sha256: String,
}

/// Cooperative cancellation shared with a running live HLS recording.
#[derive(Debug, Clone, Default)]
pub struct HlsLiveControl {
    cancelled: Arc<AtomicBool>,
}

impl HlsLiveControl {
    /// Requests a clean stop after the current network unit completes.
    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    /// Reports whether cancellation has been requested.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

/// User-selected boundary for a clear live HLS recording.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsLiveStopCondition {
    /// Continue until the source publishes `EXT-X-ENDLIST`.
    EndList,
    /// Stop on the first complete segment that reaches or exceeds this duration.
    Duration(Duration),
}

/// Why a live recording stopped without a transfer failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HlsLiveStopReason {
    EndList,
    DurationReached,
    Cancelled,
    /// The caller stopped supplying playlist snapshots before another boundary was reached.
    SnapshotsExhausted,
}

/// Inclusive media-sequence range that rolled out before it could be recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HlsLiveGap {
    pub first_sequence: u64,
    pub last_sequence: u64,
}

/// Final, bounded accounting for a clear live HLS recording.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HlsLiveRecordingReport {
    pub stop_reason: HlsLiveStopReason,
    pub downloaded_bytes: u64,
    pub media_segments: usize,
    pub init_segments: usize,
    pub recorded_duration: Duration,
    pub gaps: Vec<HlsLiveGap>,
    pub sha256: String,
}

#[derive(Debug, Error)]
pub enum HlsLiveRecordingError {
    #[error("live recording duration must be greater than zero")]
    InvalidStopDuration,
    #[error("HLS segment {sequence} has an invalid duration")]
    InvalidSegmentDuration { sequence: u64 },
    #[error("live HLS sequence or duration accounting overflowed")]
    Overflow,
    #[error(transparent)]
    Download(#[from] HlsDownloadError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Original and refreshed playlist pair authorized by the source browser session.
#[derive(Debug)]
pub struct HlsRefreshRequest<'a> {
    pub authorization: ValidatedRefresh,
    pub original: &'a HlsMediaPlaylist,
    pub refreshed: &'a HlsMediaPlaylist,
}

#[derive(Debug, Error)]
pub enum HlsDownloadError {
    #[error("live HLS playlists are outside the VOD downloader contract")]
    NotVod,
    #[error("HLS fragment {unit} request failed: {source}")]
    Request {
        unit: usize,
        #[source]
        source: reqwest::Error,
    },
    #[error("HLS fragment {unit} response read failed: {source}")]
    ResponseRead {
        unit: usize,
        #[source]
        source: std::io::Error,
    },
    #[error("HLS fragment {unit} returned HTTP status {status}")]
    HttpStatus { unit: usize, status: u16 },
    #[error("HLS fragment {unit} returned an invalid byte-range response")]
    InvalidByteRange { unit: usize },
    #[error("HLS fragment {unit} returned {actual} bytes, expected {expected}")]
    LengthMismatch {
        unit: usize,
        expected: u64,
        actual: u64,
    },
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum HlsResumeError {
    #[error(transparent)]
    Download(#[from] HlsDownloadError),
    #[error("durable HLS checkpoint does not match the playlist or partial file: {0}")]
    CheckpointMismatch(&'static str),
    #[error("durable HLS state failed: {0}")]
    Storage(#[from] StorageError),
    #[error("signed-URL refresh was refused: {0}")]
    Refresh(#[from] RefreshError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Downloads all objects referenced by an HLS VOD playlist into one ordered temporary file.
#[derive(Debug, Clone)]
pub struct HlsVodDownloader {
    client: Client,
    retry_policy: RetryPolicy,
}

/// Records ordered media from successive clear live HLS playlist windows.
///
/// Playlist retrieval remains with the caller so refresh credentials and polling policy stay at
/// the request-context boundary. Repeated segments are ignored, rolled-out sequence ranges are
/// reported, and cancellation is observed between network units.
#[derive(Debug, Clone)]
pub struct HlsLiveRecorder {
    downloader: HlsVodDownloader,
}

impl HlsLiveRecorder {
    /// Builds a recorder with the repository HTTP and retry defaults.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, HlsDownloadError> {
        Ok(Self {
            downloader: HlsVodDownloader::new()?,
        })
    }

    /// Overrides fragment retry behavior, primarily for deterministic orchestration and tests.
    #[must_use]
    pub fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.downloader = self.downloader.with_retry_policy(retry_policy);
        self
    }

    /// Writes unseen segments from chronological playlist snapshots in playback order.
    ///
    /// A duration boundary is rounded up to a complete segment. An `EXT-X-ENDLIST`, cooperative
    /// cancellation, or exhausted snapshot source also produces a successful report. Transfer and
    /// filesystem failures preserve the partial recording for explicit recovery or cleanup.
    ///
    /// # Errors
    /// Returns an error for a zero duration boundary, invalid segment duration, arithmetic
    /// overflow, transfer failure, or output failure.
    pub fn record<I>(
        &self,
        snapshots: I,
        temporary_path: &Path,
        stop_condition: HlsLiveStopCondition,
        control: &HlsLiveControl,
    ) -> Result<HlsLiveRecordingReport, HlsLiveRecordingError>
    where
        I: IntoIterator<Item = HlsMediaPlaylist>,
    {
        if matches!(stop_condition, HlsLiveStopCondition::Duration(value) if value.is_zero()) {
            return Err(HlsLiveRecordingError::InvalidStopDuration);
        }
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut recording = HlsLiveRecordingState::new(temporary_path)?;
        let mut stop_reason = HlsLiveStopReason::SnapshotsExhausted;

        'snapshots: for playlist in snapshots {
            if control.is_cancelled() {
                stop_reason = HlsLiveStopReason::Cancelled;
                break;
            }
            for segment in &playlist.segments {
                if control.is_cancelled() {
                    stop_reason = HlsLiveStopReason::Cancelled;
                    break 'snapshots;
                }
                if !recording.record_segment(&self.downloader, segment, control)? {
                    stop_reason = HlsLiveStopReason::Cancelled;
                    break 'snapshots;
                }

                if let HlsLiveStopCondition::Duration(limit) = stop_condition
                    && recording.recorded_duration >= limit
                {
                    stop_reason = HlsLiveStopReason::DurationReached;
                    break 'snapshots;
                }
            }
            if playlist.has_end_list {
                stop_reason = HlsLiveStopReason::EndList;
                break;
            }
        }
        recording.finish(stop_reason)
    }
}

struct HlsLiveRecordingState {
    output: File,
    hasher: Sha256,
    downloaded_bytes: u64,
    media_segments: usize,
    init_segments: usize,
    recorded_duration: Duration,
    gaps: Vec<HlsLiveGap>,
    next_sequence: Option<u64>,
    active_init: Option<HlsInitSegment>,
    unit_index: usize,
}

impl HlsLiveRecordingState {
    fn new(path: &Path) -> Result<Self, std::io::Error> {
        Ok(Self {
            output: File::create(path)?,
            hasher: Sha256::new(),
            downloaded_bytes: 0,
            media_segments: 0,
            init_segments: 0,
            recorded_duration: Duration::ZERO,
            gaps: Vec::new(),
            next_sequence: None,
            active_init: None,
            unit_index: 0,
        })
    }

    /// Returns `false` when cancellation arrives after an initialization object.
    fn record_segment(
        &mut self,
        downloader: &HlsVodDownloader,
        segment: &HlsMediaSegment,
        control: &HlsLiveControl,
    ) -> Result<bool, HlsLiveRecordingError> {
        let expected = self.next_sequence.unwrap_or(segment.sequence);
        if segment.sequence < expected {
            return Ok(true);
        }
        if segment.sequence > expected {
            push_live_gap(&mut self.gaps, expected, segment.sequence - 1);
        }
        let segment_duration =
            Duration::try_from_secs_f64(segment.duration_seconds).map_err(|_| {
                HlsLiveRecordingError::InvalidSegmentDuration {
                    sequence: segment.sequence,
                }
            })?;

        if segment.init_segment != self.active_init {
            if let Some(init) = &segment.init_segment {
                self.record_unit(
                    downloader,
                    &HlsUnit {
                        uri: init.uri.clone(),
                        byte_range: init.byte_range,
                        is_init: true,
                    },
                )?;
                self.init_segments = self
                    .init_segments
                    .checked_add(1)
                    .ok_or(HlsLiveRecordingError::Overflow)?;
            }
            self.active_init.clone_from(&segment.init_segment);
        }
        if control.is_cancelled() {
            return Ok(false);
        }

        self.record_unit(
            downloader,
            &HlsUnit {
                uri: segment.uri.clone(),
                byte_range: segment.byte_range,
                is_init: false,
            },
        )?;
        self.media_segments = self
            .media_segments
            .checked_add(1)
            .ok_or(HlsLiveRecordingError::Overflow)?;
        self.recorded_duration = self
            .recorded_duration
            .checked_add(segment_duration)
            .ok_or(HlsLiveRecordingError::Overflow)?;
        self.next_sequence = Some(
            segment
                .sequence
                .checked_add(1)
                .ok_or(HlsLiveRecordingError::Overflow)?,
        );
        Ok(true)
    }

    fn record_unit(
        &mut self,
        downloader: &HlsVodDownloader,
        unit: &HlsUnit,
    ) -> Result<(), HlsLiveRecordingError> {
        let response = downloader.fetch(self.unit_index, unit)?;
        let bytes = copy_response(
            self.unit_index,
            response,
            unit,
            &mut self.output,
            &mut self.hasher,
        )?
        .bytes;
        self.downloaded_bytes = self
            .downloaded_bytes
            .checked_add(bytes)
            .ok_or(HlsLiveRecordingError::Overflow)?;
        self.unit_index = self
            .unit_index
            .checked_add(1)
            .ok_or(HlsLiveRecordingError::Overflow)?;
        Ok(())
    }

    fn finish(
        self,
        stop_reason: HlsLiveStopReason,
    ) -> Result<HlsLiveRecordingReport, HlsLiveRecordingError> {
        self.output.sync_all()?;
        Ok(HlsLiveRecordingReport {
            stop_reason,
            downloaded_bytes: self.downloaded_bytes,
            media_segments: self.media_segments,
            init_segments: self.init_segments,
            recorded_duration: self.recorded_duration,
            gaps: self.gaps,
            sha256: format!("{:x}", self.hasher.finalize()),
        })
    }
}

fn push_live_gap(gaps: &mut Vec<HlsLiveGap>, first_sequence: u64, last_sequence: u64) {
    if let Some(previous) = gaps.last_mut()
        && previous.last_sequence.checked_add(1) == Some(first_sequence)
    {
        previous.last_sequence = last_sequence;
        return;
    }
    gaps.push(HlsLiveGap {
        first_sequence,
        last_sequence,
    });
}

impl HlsVodDownloader {
    /// Builds an HLS downloader with the repository retry defaults.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, HlsDownloadError> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|source| HlsDownloadError::Request { unit: 0, source })?;
        Ok(Self {
            client,
            retry_policy: RetryPolicy::default(),
        })
    }

    /// Overrides retry behavior, primarily for deterministic orchestration and tests.
    #[must_use]
    pub const fn with_retry_policy(mut self, retry_policy: RetryPolicy) -> Self {
        self.retry_policy = retry_policy;
        self
    }

    /// Fetches initialization objects when they change, then media fragments in declared order.
    ///
    /// The destination is removed on any failure so a basic VOD transfer cannot be mistaken for a
    /// resumable checkpoint. Durable partial output is introduced by the S109 contract.
    ///
    /// # Errors
    /// Returns an error for live input, exhausted retry, missing/truncated fragments, invalid range
    /// responses, or output failures.
    pub fn download(
        &self,
        playlist: &HlsMediaPlaylist,
        temporary_path: &Path,
    ) -> Result<HlsDownloadOutcome, HlsDownloadError> {
        if !playlist.has_end_list {
            return Err(HlsDownloadError::NotVod);
        }
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let result = self.download_to_file(playlist, temporary_path);
        if result.is_err() {
            let _ = fs::remove_file(temporary_path);
        }
        result
    }

    /// Reconciles the partial file with durable fragment checkpoints and fetches missing units.
    ///
    /// Every completed unit is synced before its contiguous output range and identity/content hash
    /// are committed. File bytes beyond the last database checkpoint are truncated on restart.
    ///
    /// # Errors
    /// Returns an error for non-VOD input, corrupt/mismatched checkpoints, transfer failures, or
    /// storage/filesystem failures.
    pub fn resume_persisted(
        &self,
        database: &mut Database,
        job_id: JobId,
        playlist: &HlsMediaPlaylist,
        temporary_path: &Path,
        at: UnixMillis,
    ) -> Result<HlsResumeOutcome, HlsResumeError> {
        self.resume_with_checkpoint_playlist(
            database,
            job_id,
            playlist,
            playlist,
            temporary_path,
            at,
        )
    }

    /// Resumes a user-refreshed VOD playlist after proving it is the same logical media object.
    ///
    /// Durable bytes are reconciled against the original playlist that created their checkpoints;
    /// only missing units are fetched from the refreshed signed URLs. Validation completes before
    /// the partial file or database is opened.
    ///
    /// # Errors
    /// Returns a safe refusal when the refreshed playlist changes resource identity or topology,
    /// or the normal persisted-resume checks fail.
    pub fn resume_refreshed(
        &self,
        database: &mut Database,
        job_id: JobId,
        refresh: HlsRefreshRequest<'_>,
        temporary_path: &Path,
        at: UnixMillis,
    ) -> Result<HlsResumeOutcome, HlsResumeError> {
        let job = database
            .get_job(job_id)?
            .ok_or_else(|| StorageError::Corrupt(format!("job {job_id} does not exist")))?;
        refresh.authorization.permits(&job.metadata)?;
        validate_hls_refresh(refresh.original, refresh.refreshed)?;
        self.resume_with_checkpoint_playlist(
            database,
            job_id,
            refresh.original,
            refresh.refreshed,
            temporary_path,
            at,
        )
    }

    fn resume_with_checkpoint_playlist(
        &self,
        database: &mut Database,
        job_id: JobId,
        checkpoint_playlist: &HlsMediaPlaylist,
        download_playlist: &HlsMediaPlaylist,
        temporary_path: &Path,
        at: UnixMillis,
    ) -> Result<HlsResumeOutcome, HlsResumeError> {
        if !download_playlist.has_end_list {
            return Err(HlsDownloadError::NotVod.into());
        }
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let checkpoint_units = ordered_units(checkpoint_playlist);
        let units = ordered_units(download_playlist);
        let mut segments = database.segments(job_id)?;
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(temporary_path)?;
        let (mut downloaded_bytes, mut hasher) =
            reconcile_durable_prefix(&mut output, &segments, &checkpoint_units)?;
        output.set_len(downloaded_bytes)?;
        output.seek(SeekFrom::Start(downloaded_bytes))?;
        let skipped_units = segments.len();

        for (unit_index, unit) in units.iter().enumerate().skip(skipped_units) {
            let response = self.fetch(unit_index, unit)?;
            let copied = copy_response(unit_index, response, unit, &mut output, &mut hasher)?;
            if copied.bytes == 0 {
                return Err(HlsResumeError::CheckpointMismatch("empty fragment"));
            }
            output.sync_all()?;
            let start = downloaded_bytes;
            downloaded_bytes = downloaded_bytes
                .checked_add(copied.bytes)
                .ok_or(HlsResumeError::CheckpointMismatch("output length overflow"))?;
            segments.push(Segment {
                id: SegmentId::new(),
                job_id,
                start,
                end: downloaded_bytes - 1,
                durable_offset: downloaded_bytes,
                worker_state: "completed".to_owned(),
                checksum: Some(checkpoint_checksum(unit, &copied.sha256)),
                checkpoint_at: at,
            });
            database.checkpoint_segments(job_id, &segments, at)?;
        }
        output.sync_all()?;
        Ok(HlsResumeOutcome {
            downloaded_bytes,
            skipped_units,
            downloaded_units: units.len() - skipped_units,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }

    fn download_to_file(
        &self,
        playlist: &HlsMediaPlaylist,
        temporary_path: &Path,
    ) -> Result<HlsDownloadOutcome, HlsDownloadError> {
        let units = ordered_units(playlist);
        let init_segments = units.iter().filter(|unit| unit.is_init).count();
        let mut output = File::create(temporary_path)?;
        let mut hasher = Sha256::new();
        let mut downloaded_bytes = 0_u64;
        for (index, unit) in units.iter().enumerate() {
            let response = self.fetch(index, unit)?;
            downloaded_bytes = downloaded_bytes
                .checked_add(copy_response(index, response, unit, &mut output, &mut hasher)?.bytes)
                .ok_or_else(|| std::io::Error::other("HLS output length overflow"))?;
        }
        output.sync_all()?;
        Ok(HlsDownloadOutcome {
            downloaded_bytes,
            media_segments: playlist.segments.len(),
            init_segments,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }

    fn fetch(&self, unit_index: usize, unit: &HlsUnit) -> Result<Response, HlsDownloadError> {
        let mut last_request_error = None;
        for attempt in 1..=self.retry_policy.maximum_attempts.max(1) {
            let mut request = self.client.get(unit.uri.as_str());
            if let Some(range) = unit.byte_range {
                let end = range
                    .offset
                    .checked_add(range.length - 1)
                    .ok_or(HlsDownloadError::InvalidByteRange { unit: unit_index })?;
                request = request.header(header::RANGE, format!("bytes={}-{}", range.offset, end));
            }
            match request.send() {
                Ok(response) if response.status().is_success() => {
                    validate_response(unit_index, unit, &response)?;
                    return Ok(response);
                }
                Ok(response) => {
                    let status = response.status().as_u16();
                    let retry_after = response
                        .headers()
                        .get(header::RETRY_AFTER)
                        .and_then(|value| value.to_str().ok());
                    match self.retry_policy.decide_http(
                        status,
                        retry_after,
                        attempt,
                        SystemTime::now(),
                        unit_index as u64,
                    ) {
                        RetryDecision::RetryAfter(delay)
                            if attempt < self.retry_policy.maximum_attempts.max(1) =>
                        {
                            thread::sleep(delay);
                        }
                        RetryDecision::RetryAfter(_) | RetryDecision::Stop => {
                            return Err(HlsDownloadError::HttpStatus {
                                unit: unit_index,
                                status,
                            });
                        }
                    }
                }
                Err(source) => {
                    last_request_error = Some(source);
                    if attempt < self.retry_policy.maximum_attempts.max(1) {
                        thread::sleep(self.retry_policy.backoff(attempt, unit_index as u64));
                    }
                }
            }
        }
        Err(HlsDownloadError::Request {
            unit: unit_index,
            source: last_request_error.expect("request loop records its final error"),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct HlsUnit {
    uri: HttpUrl,
    byte_range: Option<HlsByteRange>,
    is_init: bool,
}

fn ordered_units(playlist: &HlsMediaPlaylist) -> Vec<HlsUnit> {
    let mut units = Vec::with_capacity(playlist.segments.len() + 1);
    let mut active_init: Option<&HlsInitSegment> = None;
    for segment in &playlist.segments {
        if segment.init_segment.as_ref() != active_init {
            if let Some(init) = &segment.init_segment {
                units.push(HlsUnit {
                    uri: init.uri.clone(),
                    byte_range: init.byte_range,
                    is_init: true,
                });
            }
            active_init = segment.init_segment.as_ref();
        }
        units.push(HlsUnit {
            uri: segment.uri.clone(),
            byte_range: segment.byte_range,
            is_init: false,
        });
    }
    units
}

fn reconcile_durable_prefix(
    output: &mut File,
    segments: &[Segment],
    units: &[HlsUnit],
) -> Result<(u64, Sha256), HlsResumeError> {
    if segments.len() > units.len() {
        return Err(HlsResumeError::CheckpointMismatch("too many durable units"));
    }
    let durable_bytes = segments.last().map_or(0, |segment| segment.durable_offset);
    if output.metadata()?.len() < durable_bytes {
        return Err(HlsResumeError::CheckpointMismatch(
            "database is ahead of the partial file",
        ));
    }
    output.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut expected_start = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    for (segment, unit) in segments.iter().zip(units) {
        let after_end = segment
            .end
            .checked_add(1)
            .ok_or(HlsResumeError::CheckpointMismatch("segment range overflow"))?;
        if segment.start != expected_start
            || segment.durable_offset != after_end
            || segment.worker_state != "completed"
        {
            return Err(HlsResumeError::CheckpointMismatch(
                "durable units are not a completed contiguous prefix",
            ));
        }
        let mut remaining = after_end - segment.start;
        let mut unit_hasher = Sha256::new();
        while remaining > 0 {
            let requested = usize::try_from(remaining.min(BUFFER_SIZE as u64))
                .expect("bounded read length fits usize");
            let count = output.read(&mut buffer[..requested])?;
            if count == 0 {
                return Err(HlsResumeError::CheckpointMismatch(
                    "partial file ended before its durable prefix",
                ));
            }
            hasher.update(&buffer[..count]);
            unit_hasher.update(&buffer[..count]);
            remaining -= u64::try_from(count).expect("buffer length fits u64");
        }
        let content_hash = format!("{:x}", unit_hasher.finalize());
        if segment.checksum.as_deref() != Some(checkpoint_checksum(unit, &content_hash).as_str()) {
            return Err(HlsResumeError::CheckpointMismatch(
                "unit identity or content hash changed",
            ));
        }
        expected_start = after_end;
    }
    Ok((durable_bytes, hasher))
}

fn checkpoint_checksum(unit: &HlsUnit, content_hash: &str) -> String {
    let mut identity = Sha256::new();
    identity.update(unit.uri.as_str().as_bytes());
    identity.update([u8::from(unit.is_init)]);
    if let Some(range) = unit.byte_range {
        identity.update(range.offset.to_le_bytes());
        identity.update(range.length.to_le_bytes());
    }
    format!("hls-v1:{:x}:{content_hash}", identity.finalize())
}

fn validate_response(
    unit_index: usize,
    unit: &HlsUnit,
    response: &Response,
) -> Result<(), HlsDownloadError> {
    let Some(range) = unit.byte_range else {
        return (response.status() == StatusCode::OK)
            .then_some(())
            .ok_or(HlsDownloadError::InvalidByteRange { unit: unit_index });
    };
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(HlsDownloadError::InvalidByteRange { unit: unit_index });
    }
    let expected_end = range
        .offset
        .checked_add(range.length - 1)
        .ok_or(HlsDownloadError::InvalidByteRange { unit: unit_index })?;
    let actual = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    let expected_window = format!("{}-{expected_end}", range.offset);
    let actual_window = actual
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('/'))
        .map(|(window, _)| window);
    if actual_window != Some(expected_window.as_str()) {
        return Err(HlsDownloadError::InvalidByteRange { unit: unit_index });
    }
    Ok(())
}

struct CopiedUnit {
    bytes: u64,
    sha256: String,
}

fn copy_response(
    unit_index: usize,
    mut response: Response,
    unit: &HlsUnit,
    output: &mut File,
    hasher: &mut Sha256,
) -> Result<CopiedUnit, HlsDownloadError> {
    let mut copied = 0_u64;
    let mut unit_hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count =
            response
                .read(&mut buffer)
                .map_err(|source| HlsDownloadError::ResponseRead {
                    unit: unit_index,
                    source,
                })?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        unit_hasher.update(&buffer[..count]);
        copied = copied
            .checked_add(u64::try_from(count).expect("buffer length fits u64"))
            .ok_or_else(|| std::io::Error::other("HLS fragment length overflow"))?;
    }
    if let Some(expected) = unit.byte_range.map(|range| range.length)
        && copied != expected
    {
        return Err(HlsDownloadError::LengthMismatch {
            unit: unit_index,
            expected,
            actual: copied,
        });
    }
    Ok(CopiedUnit {
        bytes: copied,
        sha256: format!("{:x}", unit_hasher.finalize()),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, OpenOptions},
        io::Write,
        path::Path,
        time::Duration,
    };

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        HlsDownloadError, HlsLiveControl, HlsLiveGap, HlsLiveRecorder, HlsLiveStopCondition,
        HlsLiveStopReason, HlsRefreshRequest, HlsVodDownloader,
    };
    use crate::{
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
        media::parse_hls_media_playlist,
        media_refresh::ValidatedRefresh,
        retry::RetryPolicy,
        state::DownloadState,
        storage::Database,
        test_support::{Fixture, FixtureServer},
    };

    fn playlist(server: &FixtureServer, body: &str) -> crate::media::HlsMediaPlaylist {
        parse_hls_media_playlist(
            body,
            &HttpUrl::parse(server.url("/playlist.m3u8")).expect("playlist URL"),
        )
        .expect("media playlist")
    }

    fn no_delay_retry(maximum_attempts: u32) -> RetryPolicy {
        RetryPolicy {
            maximum_attempts,
            base_delay: Duration::ZERO,
            maximum_delay: Duration::ZERO,
            jitter_per_mille: 0,
        }
    }

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("timestamp")
    }

    fn test_job(id: JobId, source_url: HttpUrl, destination: &Path) -> JobMetadata {
        JobMetadata {
            id,
            display_url: source_url.as_str().to_owned(),
            source_url,
            final_url: None,
            filename: "video.mp4".to_owned(),
            destination: destination.to_owned(),
            state: DownloadState::Downloading,
            kind: DownloadKind::Media,
            total_bytes: None,
            downloaded_bytes: 0,
            category: None,
            queue: None,
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: timestamp(1),
            updated_at: timestamp(1),
            completed_at: None,
        }
    }

    #[test]
    fn vod_init_and_fragments_are_written_in_playlist_order_with_final_hash() {
        let server = FixtureServer::start(vec![
            Fixture::ok("INIT"),
            Fixture::ok("first"),
            Fixture::ok("second"),
        ]);
        let parsed = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4,\none.m4s\n#EXTINF:4,\ntwo.m4s\n#EXT-X-ENDLIST",
        );
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("video.part");

        let outcome = HlsVodDownloader::new()
            .expect("downloader")
            .download(&parsed, &output)
            .expect("ordered download");

        let expected = b"INITfirstsecond";
        assert_eq!(fs::read(&output).expect("output"), expected);
        assert_eq!(outcome.downloaded_bytes, expected.len() as u64);
        assert_eq!(outcome.media_segments, 2);
        assert_eq!(outcome.init_segments, 1);
        assert_eq!(outcome.sha256, format!("{:x}", Sha256::digest(expected)));
        assert_eq!(
            server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            ["/init.mp4", "/one.m4s", "/two.m4s"]
        );
    }

    #[test]
    fn retryable_fragment_recovers_within_budget() {
        let server =
            FixtureServer::start(vec![Fixture::ok("").status(503), Fixture::ok("recovered")]);
        let parsed = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nretry.ts\n#EXT-X-ENDLIST",
        );
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("retry.part");

        HlsVodDownloader::new()
            .expect("downloader")
            .with_retry_policy(no_delay_retry(2))
            .download(&parsed, &output)
            .expect("retry succeeds");

        assert_eq!(fs::read(&output).expect("output"), b"recovered");
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn missing_fragment_stops_and_removes_partial_output() {
        let server = FixtureServer::start(vec![Fixture::ok("").status(404)]);
        let parsed = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\nmissing.ts\n\
             #EXTINF:4,\nnever-requested.ts\n#EXT-X-ENDLIST",
        );
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("missing.part");

        let error = HlsVodDownloader::new()
            .expect("downloader")
            .with_retry_policy(no_delay_retry(2))
            .download(&parsed, &output)
            .expect_err("missing fragment");

        assert!(matches!(
            error,
            HlsDownloadError::HttpStatus {
                unit: 0,
                status: 404
            }
        ));
        assert!(!output.exists());
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn byte_range_fragment_requires_exact_partial_response() {
        let server = FixtureServer::start(vec![
            Fixture::ok("cde")
                .status(206)
                .header("Content-Range", "bytes 2-4/6"),
        ]);
        let parsed = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXTINF:4,\n\
             #EXT-X-BYTERANGE:3@2\nblob.ts\n#EXT-X-ENDLIST",
        );
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("range.part");

        HlsVodDownloader::new()
            .expect("downloader")
            .download(&parsed, &output)
            .expect("range download");

        let requests = server.requests();
        assert_eq!(requests[0].header("range"), Some("bytes=2-4"));
        assert_eq!(fs::read(output).expect("output"), b"cde");
    }

    #[test]
    fn restart_skips_durable_units_and_truncates_uncheckpointed_tail() {
        let server = FixtureServer::start(vec![
            Fixture::ok("INIT"),
            Fixture::ok("first"),
            Fixture::ok("second"),
        ]);
        let first_playlist = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4,\none.m4s\n#EXT-X-ENDLIST",
        );
        let full_playlist = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4\"\n\
             #EXTINF:4,\none.m4s\n#EXTINF:4,\ntwo.m4s\n#EXT-X-ENDLIST",
        );
        let directory = tempdir().expect("tempdir");
        let database_path = directory.path().join("state.sqlite");
        let output = directory.path().join("video.part");
        let job_id = JobId::new();
        let source_url = HttpUrl::parse(server.url("/playlist.m3u8")).expect("source URL");
        let mut database = Database::open(&database_path).expect("database");
        database
            .create_job(&test_job(job_id, source_url, &output), None)
            .expect("job");
        let downloader = HlsVodDownloader::new().expect("downloader");

        let initial = downloader
            .resume_persisted(
                &mut database,
                job_id,
                &first_playlist,
                &output,
                timestamp(2),
            )
            .expect("initial durable units");
        assert_eq!(initial.downloaded_units, 2);
        OpenOptions::new()
            .append(true)
            .open(&output)
            .expect("partial output")
            .write_all(b"uncheckpointed-crash-tail")
            .expect("crash tail");
        drop(database);

        let mut reopened = Database::open(&database_path).expect("reopen database");
        let resumed = downloader
            .resume_persisted(&mut reopened, job_id, &full_playlist, &output, timestamp(3))
            .expect("restart resume");

        let expected = b"INITfirstsecond";
        assert_eq!(resumed.skipped_units, 2);
        assert_eq!(resumed.downloaded_units, 1);
        assert_eq!(resumed.downloaded_bytes, expected.len() as u64);
        assert_eq!(resumed.sha256, format!("{:x}", Sha256::digest(expected)));
        assert_eq!(fs::read(&output).expect("output"), expected);
        assert_eq!(reopened.segments(job_id).expect("segments").len(), 3);
        assert_eq!(
            reopened
                .get_job(job_id)
                .expect("job read")
                .expect("job exists")
                .metadata
                .downloaded_bytes,
            expected.len() as u64
        );
        assert_eq!(
            server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            ["/init.mp4", "/one.m4s", "/two.m4s"]
        );
    }

    #[test]
    fn refreshed_signed_urls_resume_same_hls_object_and_refuse_mismatch() {
        let server = FixtureServer::start(vec![
            Fixture::ok("INIT"),
            Fixture::ok("first"),
            Fixture::ok("second"),
        ]);
        let initial = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4?token=old\"\n\
             #EXTINF:4,\none.m4s?token=old\n#EXT-X-ENDLIST",
        );
        let original = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4?token=old\"\n\
             #EXTINF:4,\none.m4s?token=old\n#EXTINF:4,\ntwo.m4s?token=old\n#EXT-X-ENDLIST",
        );
        let refreshed = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MAP:URI=\"init.mp4?token=new\"\n\
             #EXTINF:4,\none.m4s?token=new\n#EXTINF:4,\ntwo.m4s?token=new\n#EXT-X-ENDLIST",
        );
        let mut mismatched = refreshed.clone();
        mismatched.segments[2 - 1].uri =
            HttpUrl::parse(server.url("/different.m4s?token=new")).expect("mismatch URL");
        let directory = tempdir().expect("tempdir");
        let database_path = directory.path().join("state.sqlite");
        let output = directory.path().join("video.part");
        let job_id = JobId::new();
        let source_url = HttpUrl::parse(server.url("/playlist.m3u8")).expect("source URL");
        let mut database = Database::open(&database_path).expect("database");
        database
            .create_job(&test_job(job_id, source_url, &output), None)
            .expect("job");
        let downloader = HlsVodDownloader::new().expect("downloader");
        downloader
            .resume_persisted(&mut database, job_id, &initial, &output, timestamp(2))
            .expect("initial prefix");
        let mut stored_job = database
            .get_job(job_id)
            .expect("job read")
            .expect("job exists")
            .metadata;
        stored_job.state = DownloadState::NeedsRefresh;
        database
            .update_job(&stored_job)
            .expect("mark needs refresh");

        let before_bytes = fs::read(&output).expect("prefix");
        let before_segments = database.segments(job_id).expect("segments");
        let error = downloader
            .resume_refreshed(
                &mut database,
                job_id,
                HlsRefreshRequest {
                    authorization: ValidatedRefresh::for_test(job_id),
                    original: &original,
                    refreshed: &mismatched,
                },
                &output,
                timestamp(3),
            )
            .expect_err("changed object must be refused");
        assert!(matches!(error, super::HlsResumeError::Refresh(_)));
        assert_eq!(fs::read(&output).expect("unchanged prefix"), before_bytes);
        assert_eq!(
            database.segments(job_id).expect("unchanged segments"),
            before_segments
        );

        let resumed = downloader
            .resume_refreshed(
                &mut database,
                job_id,
                HlsRefreshRequest {
                    authorization: ValidatedRefresh::for_test(job_id),
                    original: &original,
                    refreshed: &refreshed,
                },
                &output,
                timestamp(4),
            )
            .expect("refreshed resume");
        assert_eq!(resumed.skipped_units, 2);
        assert_eq!(resumed.downloaded_units, 1);
        assert_eq!(fs::read(&output).expect("output"), b"INITfirstsecond");
        assert_eq!(
            server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            [
                "/init.mp4?token=old",
                "/one.m4s?token=old",
                "/two.m4s?token=new"
            ]
        );
    }

    #[test]
    fn live_windows_deduplicate_rollover_and_report_dropped_sequences_at_end() {
        let server = FixtureServer::start(vec![
            Fixture::ok("one"),
            Fixture::ok("two"),
            Fixture::ok("three"),
            Fixture::ok("five"),
        ]);
        let snapshots = [
            playlist(
                &server,
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:10\n\
                 #EXTINF:4,\none.ts\n#EXTINF:4,\ntwo.ts",
            ),
            playlist(
                &server,
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:11\n\
                 #EXTINF:4,\ntwo.ts\n#EXTINF:4,\nthree.ts",
            ),
            playlist(
                &server,
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:14\n\
                 #EXTINF:4,\nfive.ts\n#EXT-X-ENDLIST",
            ),
        ];
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("live.part");

        let report = HlsLiveRecorder::new()
            .expect("recorder")
            .record(
                snapshots,
                &output,
                HlsLiveStopCondition::EndList,
                &HlsLiveControl::default(),
            )
            .expect("live recording");

        let expected = b"onetwothreefive";
        assert_eq!(fs::read(&output).expect("output"), expected);
        assert_eq!(report.stop_reason, HlsLiveStopReason::EndList);
        assert_eq!(report.media_segments, 4);
        assert_eq!(report.recorded_duration, Duration::from_secs(16));
        assert_eq!(
            report.gaps,
            [HlsLiveGap {
                first_sequence: 13,
                last_sequence: 13
            }]
        );
        assert_eq!(report.sha256, format!("{:x}", Sha256::digest(expected)));
        assert_eq!(
            server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            ["/one.ts", "/two.ts", "/three.ts", "/five.ts"]
        );
    }

    #[test]
    fn live_duration_stops_at_a_complete_segment_boundary() {
        let server = FixtureServer::start(vec![Fixture::ok("one"), Fixture::ok("two")]);
        let snapshot = playlist(
            &server,
            "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:20\n\
             #EXTINF:4,\none.ts\n#EXTINF:4,\ntwo.ts\n#EXTINF:4,\nnever.ts",
        );
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("duration.part");

        let report = HlsLiveRecorder::new()
            .expect("recorder")
            .record(
                [snapshot],
                &output,
                HlsLiveStopCondition::Duration(Duration::from_secs(7)),
                &HlsLiveControl::default(),
            )
            .expect("duration recording");

        assert_eq!(report.stop_reason, HlsLiveStopReason::DurationReached);
        assert_eq!(report.media_segments, 2);
        assert_eq!(report.recorded_duration, Duration::from_secs(8));
        assert_eq!(fs::read(output).expect("output"), b"onetwo");
        assert_eq!(server.requests().len(), 2);
    }

    #[test]
    fn live_cancellation_stops_before_the_next_window_request() {
        struct CancellingSnapshots {
            control: HlsLiveControl,
            snapshots: std::vec::IntoIter<crate::media::HlsMediaPlaylist>,
            yielded: usize,
        }

        impl Iterator for CancellingSnapshots {
            type Item = crate::media::HlsMediaPlaylist;

            fn next(&mut self) -> Option<Self::Item> {
                if self.yielded == 1 {
                    self.control.cancel();
                }
                self.yielded += 1;
                self.snapshots.next()
            }
        }

        let server = FixtureServer::start(vec![Fixture::ok("one")]);
        let snapshots = vec![
            playlist(
                &server,
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:30\n\
                 #EXTINF:4,\none.ts",
            ),
            playlist(
                &server,
                "#EXTM3U\n#EXT-X-TARGETDURATION:4\n#EXT-X-MEDIA-SEQUENCE:31\n\
                 #EXTINF:4,\ntwo.ts",
            ),
        ];
        let control = HlsLiveControl::default();
        let source = CancellingSnapshots {
            control: control.clone(),
            snapshots: snapshots.into_iter(),
            yielded: 0,
        };
        let directory = tempdir().expect("tempdir");
        let output = directory.path().join("cancelled.part");

        let report = HlsLiveRecorder::new()
            .expect("recorder")
            .record(source, &output, HlsLiveStopCondition::EndList, &control)
            .expect("cancelled recording report");

        assert_eq!(report.stop_reason, HlsLiveStopReason::Cancelled);
        assert_eq!(report.media_segments, 1);
        assert_eq!(report.recorded_duration, Duration::from_secs(4));
        assert_eq!(fs::read(output).expect("output"), b"one");
        assert_eq!(server.requests().len(), 1);
    }
}
