//! Parallel, resumable transfer of separately addressed MPEG-DASH audio and video tracks.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    thread,
    time::SystemTime,
};

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header,
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    dash::{DashSegmentPlan, DashSegmentResource},
    domain::{JobId, Segment, SegmentId, UnixMillis},
    retry::{RetryDecision, RetryPolicy},
    storage::{Database, StorageError},
};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DashTrackKind {
    Video,
    Audio,
}

/// One independently checkpointed DASH track. Audio and video must use distinct job IDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashTrackRequest {
    pub kind: DashTrackKind,
    pub job_id: JobId,
    pub plan: DashSegmentPlan,
    pub temporary_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashTrackOutcome {
    pub kind: DashTrackKind,
    pub job_id: JobId,
    pub downloaded_bytes: u64,
    pub skipped_units: usize,
    pub downloaded_units: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DashTrackPairOutcome {
    pub video: DashTrackOutcome,
    pub audio: DashTrackOutcome,
}

#[derive(Debug, Error)]
pub enum DashTrackDownloadError {
    #[error("audio and video tracks require distinct job IDs and output paths")]
    InvalidPair,
    #[error("DASH track has no initialization or media resources")]
    EmptyTrack,
    #[error("DASH track resource {unit} has an invalid byte range")]
    InvalidByteRange { unit: usize },
    #[error("DASH track resource {unit} request failed: {source}")]
    Request {
        unit: usize,
        #[source]
        source: reqwest::Error,
    },
    #[error("DASH track resource {unit} response read failed: {source}")]
    ResponseRead {
        unit: usize,
        #[source]
        source: std::io::Error,
    },
    #[error("DASH track resource {unit} returned HTTP status {status}")]
    HttpStatus { unit: usize, status: u16 },
    #[error("DASH track resource {unit} returned {actual} bytes, expected {expected}")]
    LengthMismatch {
        unit: usize,
        expected: u64,
        actual: u64,
    },
    #[error("durable DASH checkpoint does not match the track or partial file: {0}")]
    CheckpointMismatch(&'static str),
    #[error("DASH track worker panicked")]
    WorkerPanicked,
    #[error("durable DASH state failed: {0}")]
    Storage(#[from] StorageError),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Downloads separate audio and video plans concurrently, checkpointing each under its own job.
#[derive(Debug, Clone)]
pub struct DashTrackDownloader {
    client: Client,
    retry_policy: RetryPolicy,
}

impl DashTrackDownloader {
    /// Creates a downloader with the repository retry defaults.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, DashTrackDownloadError> {
        let client = Client::builder()
            .connect_timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|source| DashTrackDownloadError::Request { unit: 0, source })?;
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

    /// Downloads/resumes video and audio concurrently using independent SQLite connections.
    ///
    /// The caller creates one durable job per track in `database_path`. Each completed object is
    /// synced before its identity/content hash and contiguous output range are checkpointed.
    ///
    /// # Errors
    /// Returns an error for aliased track state, corrupt checkpoints, exhausted HTTP retry,
    /// invalid ranges, storage failures, or worker panic.
    pub fn resume_pair(
        &self,
        database_path: &Path,
        mut video: DashTrackRequest,
        mut audio: DashTrackRequest,
        at: UnixMillis,
    ) -> Result<DashTrackPairOutcome, DashTrackDownloadError> {
        if video.kind != DashTrackKind::Video
            || audio.kind != DashTrackKind::Audio
            || video.job_id == audio.job_id
        {
            return Err(DashTrackDownloadError::InvalidPair);
        }
        video.temporary_path = normalized_output_path(&video.temporary_path)?;
        audio.temporary_path = normalized_output_path(&audio.temporary_path)?;
        let database_identity = database_path.canonicalize()?;
        if video.temporary_path == audio.temporary_path
            || video.temporary_path == database_identity
            || audio.temporary_path == database_identity
        {
            return Err(DashTrackDownloadError::InvalidPair);
        }
        let database_path = database_path.to_owned();
        let video_worker = self.clone();
        let audio_worker = self.clone();
        let (video_result, audio_result) = thread::scope(|scope| {
            let video_path = database_path.clone();
            let video_handle =
                scope.spawn(move || video_worker.resume_track(&video_path, &video, at));
            let audio_handle =
                scope.spawn(move || audio_worker.resume_track(&database_path, &audio, at));
            (video_handle.join(), audio_handle.join())
        });
        let video = video_result.map_err(|_| DashTrackDownloadError::WorkerPanicked)??;
        let audio = audio_result.map_err(|_| DashTrackDownloadError::WorkerPanicked)??;
        Ok(DashTrackPairOutcome { video, audio })
    }

    fn resume_track(
        &self,
        database_path: &Path,
        request: &DashTrackRequest,
        at: UnixMillis,
    ) -> Result<DashTrackOutcome, DashTrackDownloadError> {
        let units = ordered_units(&request.plan);
        if units.is_empty() {
            return Err(DashTrackDownloadError::EmptyTrack);
        }
        if let Some(parent) = request.temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut database = Database::open(database_path)?;
        let mut segments = database.segments(request.job_id)?;
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&request.temporary_path)?;
        let (mut downloaded_bytes, mut hasher) =
            reconcile_durable_prefix(&mut output, &segments, &units)?;
        output.set_len(downloaded_bytes)?;
        output.seek(SeekFrom::Start(downloaded_bytes))?;
        let skipped_units = segments.len();

        for (unit_index, unit) in units.iter().enumerate().skip(skipped_units) {
            let response = self.fetch(unit_index, unit)?;
            let copied = copy_response(unit_index, response, unit, &mut output, &mut hasher)?;
            if copied.bytes == 0 {
                return Err(DashTrackDownloadError::CheckpointMismatch(
                    "empty track resource",
                ));
            }
            output.sync_all()?;
            let start = downloaded_bytes;
            downloaded_bytes = downloaded_bytes.checked_add(copied.bytes).ok_or(
                DashTrackDownloadError::CheckpointMismatch("output length overflow"),
            )?;
            segments.push(Segment {
                id: SegmentId::new(),
                job_id: request.job_id,
                start,
                end: downloaded_bytes - 1,
                durable_offset: downloaded_bytes,
                worker_state: "completed".to_owned(),
                checksum: Some(checkpoint_checksum(unit, &copied.sha256)),
                checkpoint_at: at,
            });
            database.checkpoint_segments(request.job_id, &segments, at)?;
        }
        output.sync_all()?;
        Ok(DashTrackOutcome {
            kind: request.kind,
            job_id: request.job_id,
            downloaded_bytes,
            skipped_units,
            downloaded_units: units.len() - skipped_units,
            sha256: format!("{:x}", hasher.finalize()),
        })
    }

    fn fetch(
        &self,
        unit_index: usize,
        unit: &DashTrackUnit<'_>,
    ) -> Result<Response, DashTrackDownloadError> {
        let mut last_request_error = None;
        for attempt in 1..=self.retry_policy.maximum_attempts.max(1) {
            let mut request = self.client.get(unit.resource.url.clone());
            if let Some(range) = unit.resource.byte_range {
                if range.start > range.end {
                    return Err(DashTrackDownloadError::InvalidByteRange { unit: unit_index });
                }
                request = request.header(
                    header::RANGE,
                    format!("bytes={}-{}", range.start, range.end),
                );
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
                            return Err(DashTrackDownloadError::HttpStatus {
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
        Err(DashTrackDownloadError::Request {
            unit: unit_index,
            source: last_request_error.expect("request loop records its final error"),
        })
    }
}

#[derive(Debug)]
struct DashTrackUnit<'a> {
    resource: &'a DashSegmentResource,
    number: Option<u64>,
    time: Option<u64>,
    duration: Option<u64>,
}

fn ordered_units(plan: &DashSegmentPlan) -> Vec<DashTrackUnit<'_>> {
    let mut units =
        Vec::with_capacity(plan.media.len() + usize::from(plan.initialization.is_some()));
    if let Some(initialization) = &plan.initialization {
        units.push(DashTrackUnit {
            resource: initialization,
            number: None,
            time: None,
            duration: None,
        });
    }
    units.extend(plan.media.iter().map(|segment| DashTrackUnit {
        resource: &segment.resource,
        number: Some(segment.number),
        time: Some(segment.time),
        duration: Some(segment.duration),
    }));
    units
}

fn normalized_output_path(path: &Path) -> Result<PathBuf, std::io::Error> {
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir()?.join(path)
    };
    if absolute.exists() {
        return absolute.canonicalize();
    }
    let parent = absolute
        .parent()
        .ok_or_else(|| std::io::Error::other("DASH output has no parent directory"))?;
    fs::create_dir_all(parent)?;
    let parent = parent.canonicalize()?;
    let name = absolute
        .file_name()
        .ok_or_else(|| std::io::Error::other("DASH output has no filename"))?;
    Ok(parent.join(name))
}

fn reconcile_durable_prefix(
    output: &mut File,
    segments: &[Segment],
    units: &[DashTrackUnit<'_>],
) -> Result<(u64, Sha256), DashTrackDownloadError> {
    if segments.len() > units.len() {
        return Err(DashTrackDownloadError::CheckpointMismatch(
            "too many durable resources",
        ));
    }
    let durable_bytes = segments.last().map_or(0, |segment| segment.durable_offset);
    if output.metadata()?.len() < durable_bytes {
        return Err(DashTrackDownloadError::CheckpointMismatch(
            "database is ahead of the partial file",
        ));
    }
    output.seek(SeekFrom::Start(0))?;
    let mut hasher = Sha256::new();
    let mut expected_start = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    for (segment, unit) in segments.iter().zip(units) {
        let after_end =
            segment
                .end
                .checked_add(1)
                .ok_or(DashTrackDownloadError::CheckpointMismatch(
                    "segment range overflow",
                ))?;
        if segment.start != expected_start
            || segment.durable_offset != after_end
            || segment.worker_state != "completed"
        {
            return Err(DashTrackDownloadError::CheckpointMismatch(
                "durable resources are not a completed contiguous prefix",
            ));
        }
        let mut remaining = after_end - segment.start;
        let mut unit_hasher = Sha256::new();
        while remaining > 0 {
            let requested = usize::try_from(remaining.min(BUFFER_SIZE as u64))
                .expect("bounded read length fits usize");
            let count = output.read(&mut buffer[..requested])?;
            if count == 0 {
                return Err(DashTrackDownloadError::CheckpointMismatch(
                    "partial file ended before its durable prefix",
                ));
            }
            hasher.update(&buffer[..count]);
            unit_hasher.update(&buffer[..count]);
            remaining -= u64::try_from(count).expect("buffer length fits u64");
        }
        let content_hash = format!("{:x}", unit_hasher.finalize());
        if segment.checksum.as_deref() != Some(checkpoint_checksum(unit, &content_hash).as_str()) {
            return Err(DashTrackDownloadError::CheckpointMismatch(
                "resource identity or content hash changed",
            ));
        }
        expected_start = after_end;
    }
    Ok((durable_bytes, hasher))
}

fn checkpoint_checksum(unit: &DashTrackUnit<'_>, content_hash: &str) -> String {
    let mut identity = Sha256::new();
    identity.update(unit.resource.url.as_str().as_bytes());
    identity.update([u8::from(unit.number.is_some())]);
    for value in [unit.number, unit.time, unit.duration] {
        identity.update(value.unwrap_or(0).to_le_bytes());
    }
    identity.update([u8::from(unit.resource.byte_range.is_some())]);
    if let Some(range) = unit.resource.byte_range {
        identity.update(range.start.to_le_bytes());
        identity.update(range.end.to_le_bytes());
    }
    format!("dash-v1:{:x}:{content_hash}", identity.finalize())
}

fn validate_response(
    unit_index: usize,
    unit: &DashTrackUnit<'_>,
    response: &Response,
) -> Result<(), DashTrackDownloadError> {
    let Some(range) = unit.resource.byte_range else {
        return (response.status() == StatusCode::OK)
            .then_some(())
            .ok_or(DashTrackDownloadError::InvalidByteRange { unit: unit_index });
    };
    if range.start > range.end || response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(DashTrackDownloadError::InvalidByteRange { unit: unit_index });
    }
    let expected_window = format!("{}-{}", range.start, range.end);
    let actual_window = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("bytes "))
        .and_then(|value| value.split_once('/'))
        .map(|(window, _)| window);
    (actual_window == Some(expected_window.as_str()))
        .then_some(())
        .ok_or(DashTrackDownloadError::InvalidByteRange { unit: unit_index })
}

struct CopiedUnit {
    bytes: u64,
    sha256: String,
}

fn copy_response(
    unit_index: usize,
    mut response: Response,
    unit: &DashTrackUnit<'_>,
    output: &mut File,
    hasher: &mut Sha256,
) -> Result<CopiedUnit, DashTrackDownloadError> {
    let mut copied = 0_u64;
    let mut unit_hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count =
            response
                .read(&mut buffer)
                .map_err(|source| DashTrackDownloadError::ResponseRead {
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
            .ok_or(DashTrackDownloadError::CheckpointMismatch(
                "resource length overflow",
            ))?;
    }
    if let Some(expected) = unit
        .resource
        .byte_range
        .and_then(|range| range.end.checked_sub(range.start))
        .and_then(|difference| difference.checked_add(1))
        && copied != expected
    {
        return Err(DashTrackDownloadError::LengthMismatch {
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
    };

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;
    use url::Url;

    use super::{DashTrackDownloader, DashTrackKind, DashTrackRequest};
    use crate::{
        dash::{DashMediaSegment, DashSegmentPlan, DashSegmentResource},
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
        state::DownloadState,
        storage::Database,
        test_support::{Fixture, FixtureServer},
    };

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("timestamp")
    }

    fn track_plan(server: &FixtureServer, prefix: &str, media_count: usize) -> DashSegmentPlan {
        DashSegmentPlan {
            initialization: Some(DashSegmentResource {
                url: Url::parse(&server.url(&format!("/{prefix}-init"))).expect("init URL"),
                byte_range: None,
            }),
            media: (1..=media_count)
                .map(|number| DashMediaSegment {
                    number: number as u64,
                    time: (number as u64 - 1) * 2,
                    duration: 2,
                    resource: DashSegmentResource {
                        url: Url::parse(&server.url(&format!("/{prefix}-{number}")))
                            .expect("media URL"),
                        byte_range: None,
                    },
                })
                .collect(),
        }
    }

    fn metadata(id: JobId, source: String, destination: &Path) -> JobMetadata {
        let source_url = HttpUrl::parse(source).expect("source URL");
        JobMetadata {
            id,
            display_url: source_url.as_str().to_owned(),
            source_url,
            final_url: None,
            filename: destination
                .file_name()
                .expect("filename")
                .to_string_lossy()
                .into_owned(),
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

    fn create_track_jobs(
        database_path: &Path,
        video: (JobId, &FixtureServer, &Path),
        audio: (JobId, &FixtureServer, &Path),
    ) {
        let mut database = Database::open(database_path).expect("database");
        database
            .create_job(
                &metadata(video.0, video.1.url("/manifest.mpd"), video.2),
                None,
            )
            .expect("video job");
        database
            .create_job(
                &metadata(audio.0, audio.1.url("/manifest.mpd"), audio.2),
                None,
            )
            .expect("audio job");
    }

    fn request(
        kind: DashTrackKind,
        job_id: JobId,
        plan: DashSegmentPlan,
        temporary_path: &Path,
    ) -> DashTrackRequest {
        DashTrackRequest {
            kind,
            job_id,
            plan,
            temporary_path: temporary_path.to_owned(),
        }
    }

    #[test]
    fn separate_audio_video_tracks_have_exact_bytes_hashes_and_jobs() {
        let video_server =
            FixtureServer::start(vec![Fixture::ok("VIDEO-INIT"), Fixture::ok("VIDEO")]);
        let audio_server =
            FixtureServer::start(vec![Fixture::ok("AUDIO-INIT"), Fixture::ok("AUDIO")]);
        let directory = tempdir().expect("temp directory");
        let database_path = directory.path().join("state.sqlite");
        let video_path = directory.path().join("video.part");
        let audio_path = directory.path().join("audio.part");
        let video_id = JobId::new();
        let audio_id = JobId::new();
        create_track_jobs(
            &database_path,
            (video_id, &video_server, &video_path),
            (audio_id, &audio_server, &audio_path),
        );

        let outcome = DashTrackDownloader::new()
            .expect("downloader")
            .resume_pair(
                &database_path,
                request(
                    DashTrackKind::Video,
                    video_id,
                    track_plan(&video_server, "video", 1),
                    &video_path,
                ),
                request(
                    DashTrackKind::Audio,
                    audio_id,
                    track_plan(&audio_server, "audio", 1),
                    &audio_path,
                ),
                timestamp(2),
            )
            .expect("track pair");

        let video_bytes = b"VIDEO-INITVIDEO";
        let audio_bytes = b"AUDIO-INITAUDIO";
        assert_eq!(fs::read(&video_path).expect("video output"), video_bytes);
        assert_eq!(fs::read(&audio_path).expect("audio output"), audio_bytes);
        assert_eq!(
            outcome.video.sha256,
            format!("{:x}", Sha256::digest(video_bytes))
        );
        assert_eq!(
            outcome.audio.sha256,
            format!("{:x}", Sha256::digest(audio_bytes))
        );
        assert_eq!(outcome.video.downloaded_units, 2);
        assert_eq!(outcome.audio.downloaded_units, 2);
        let database = Database::open(&database_path).expect("reopen database");
        assert_eq!(
            database.segments(video_id).expect("video segments").len(),
            2
        );
        assert_eq!(
            database.segments(audio_id).expect("audio segments").len(),
            2
        );
        assert_eq!(
            database
                .get_job(video_id)
                .expect("video job")
                .expect("video exists")
                .metadata
                .downloaded_bytes,
            video_bytes.len() as u64
        );
        assert_eq!(
            database
                .get_job(audio_id)
                .expect("audio job")
                .expect("audio exists")
                .metadata
                .downloaded_bytes,
            audio_bytes.len() as u64
        );
    }

    #[test]
    fn restart_skips_both_durable_track_prefixes_and_truncates_crash_tails() {
        let video_server = FixtureServer::start(vec![
            Fixture::ok("VI"),
            Fixture::ok("V1"),
            Fixture::ok("V2"),
        ]);
        let audio_server = FixtureServer::start(vec![
            Fixture::ok("AI"),
            Fixture::ok("A1"),
            Fixture::ok("A2"),
        ]);
        let directory = tempdir().expect("temp directory");
        let database_path = directory.path().join("state.sqlite");
        let video_path = directory.path().join("video.part");
        let audio_path = directory.path().join("audio.part");
        let video_id = JobId::new();
        let audio_id = JobId::new();
        create_track_jobs(
            &database_path,
            (video_id, &video_server, &video_path),
            (audio_id, &audio_server, &audio_path),
        );
        let downloader = DashTrackDownloader::new().expect("downloader");

        downloader
            .resume_pair(
                &database_path,
                request(
                    DashTrackKind::Video,
                    video_id,
                    track_plan(&video_server, "video", 1),
                    &video_path,
                ),
                request(
                    DashTrackKind::Audio,
                    audio_id,
                    track_plan(&audio_server, "audio", 1),
                    &audio_path,
                ),
                timestamp(2),
            )
            .expect("initial pair");
        for path in [&video_path, &audio_path] {
            OpenOptions::new()
                .append(true)
                .open(path)
                .expect("partial output")
                .write_all(b"crash-tail")
                .expect("crash tail");
        }

        let resumed = downloader
            .resume_pair(
                &database_path,
                request(
                    DashTrackKind::Video,
                    video_id,
                    track_plan(&video_server, "video", 2),
                    &video_path,
                ),
                request(
                    DashTrackKind::Audio,
                    audio_id,
                    track_plan(&audio_server, "audio", 2),
                    &audio_path,
                ),
                timestamp(3),
            )
            .expect("resumed pair");

        assert_eq!(resumed.video.skipped_units, 2);
        assert_eq!(resumed.audio.skipped_units, 2);
        assert_eq!(resumed.video.downloaded_units, 1);
        assert_eq!(resumed.audio.downloaded_units, 1);
        assert_eq!(fs::read(&video_path).expect("video output"), b"VIV1V2");
        assert_eq!(fs::read(&audio_path).expect("audio output"), b"AIA1A2");
        assert_eq!(
            video_server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            ["/video-init", "/video-1", "/video-2"]
        );
        assert_eq!(
            audio_server
                .requests()
                .iter()
                .map(|request| request.target.as_str())
                .collect::<Vec<_>>(),
            ["/audio-init", "/audio-1", "/audio-2"]
        );
    }
}
