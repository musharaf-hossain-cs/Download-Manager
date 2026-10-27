//! Ordered HLS video-on-demand fragment transfer.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
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
    domain::{HttpUrl, JobId, Segment, SegmentId, UnixMillis},
    media::{HlsByteRange, HlsInitSegment, HlsMediaPlaylist},
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
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Downloads all objects referenced by an HLS VOD playlist into one ordered temporary file.
#[derive(Debug, Clone)]
pub struct HlsVodDownloader {
    client: Client,
    retry_policy: RetryPolicy,
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
        if !playlist.has_end_list {
            return Err(HlsDownloadError::NotVod.into());
        }
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let units = ordered_units(playlist);
        let mut segments = database.segments(job_id)?;
        let mut output = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(temporary_path)?;
        let (mut downloaded_bytes, mut hasher) =
            reconcile_durable_prefix(&mut output, &segments, &units)?;
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

    use super::{HlsDownloadError, HlsVodDownloader};
    use crate::{
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
        media::parse_hls_media_playlist,
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
}
