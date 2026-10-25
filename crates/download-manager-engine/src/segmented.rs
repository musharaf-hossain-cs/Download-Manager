//! Parallel HTTP range transfer with positional file writes.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::Path,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};

use reqwest::{StatusCode, blocking::Client, header};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::{HttpUrl, JobId, UnixMillis},
    storage::{Database, StorageError},
    transfer::{SingleStreamDownloader, TransferError},
};

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

impl ByteRange {
    #[must_use]
    pub const fn len(self) -> u64 {
        self.end - self.start + 1
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.start > self.end
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentedOutcome {
    pub downloaded_bytes: u64,
    pub sha256: String,
    pub ranges: [ByteRange; 2],
    pub used_single_stream_fallback: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResumeOutcome {
    pub downloaded_bytes: u64,
    pub sha256: String,
    pub requested_ranges: Vec<ByteRange>,
}

#[derive(Debug, Error)]
pub enum SegmentedError {
    #[error("object length must be greater than zero")]
    EmptyObject,
    #[error("object length must be at least two bytes")]
    ObjectTooSmall,
    #[error("connection count must be between 1 and 32")]
    InvalidConnectionCount,
    #[error("HTTP range request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("range response returned status {0}")]
    HttpStatus(u16),
    #[error("server ignored the requested byte range")]
    RangeIgnored,
    #[error("response Content-Range does not match requested bytes {start}-{end}")]
    InvalidContentRange { start: u64, end: u64 },
    #[error("range {start}-{end} returned {actual} bytes, expected {expected}")]
    LengthMismatch {
        start: u64,
        end: u64,
        expected: u64,
        actual: u64,
    },
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("durable segment state failed: {0}")]
    Storage(#[from] StorageError),
    #[error("single-stream fallback failed: {0}")]
    Fallback(#[from] TransferError),
    #[error("range worker panicked")]
    WorkerPanicked,
}

/// Plans between one and 32 adjacent, non-empty ranges over an object.
///
/// Objects smaller than the requested connection count receive one worker per byte. Any remainder
/// is distributed over the earliest ranges so range sizes differ by at most one byte.
///
/// # Errors
/// Returns an error for an empty object or a connection count outside 1..=32.
pub fn plan_fixed_ranges(
    object_length: u64,
    connection_count: u8,
) -> Result<Vec<ByteRange>, SegmentedError> {
    if object_length == 0 {
        return Err(SegmentedError::EmptyObject);
    }
    if !(1..=32).contains(&connection_count) {
        return Err(SegmentedError::InvalidConnectionCount);
    }
    let range_count = object_length.min(u64::from(connection_count));
    let base_size = object_length / range_count;
    let oversized_count = object_length % range_count;
    let mut ranges = Vec::with_capacity(usize::from(connection_count));
    let mut start = 0_u64;
    for index in 0..range_count {
        let size = base_size + u64::from(index < oversized_count);
        let end = start + size - 1;
        ranges.push(ByteRange { start, end });
        start = end + 1;
    }
    debug_assert_eq!(start, object_length);
    Ok(ranges)
}

#[derive(Debug, Clone)]
pub struct TwoRangeDownloader {
    client: Client,
}

impl TwoRangeDownloader {
    /// Builds a shared HTTP client used by both range workers.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, SegmentedError> {
        Ok(Self {
            client: Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()?,
        })
    }

    /// Downloads exactly two adjacent ranges into a preallocated temporary file.
    ///
    /// # Errors
    /// Returns an error if the object is too small, either range is rejected/truncated, or the
    /// output cannot be written and verified.
    ///
    /// # Panics
    /// Panics only if an internal range worker poisons its result mutex.
    pub fn download(
        &self,
        url: &HttpUrl,
        object_length: u64,
        temporary_path: &Path,
    ) -> Result<SegmentedOutcome, SegmentedError> {
        if object_length < 2 {
            return Err(SegmentedError::ObjectTooSmall);
        }
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let output = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(temporary_path)?;
        output.set_len(object_length)?;
        let midpoint = object_length / 2;
        let ranges = [
            ByteRange {
                start: 0,
                end: midpoint - 1,
            },
            ByteRange {
                start: midpoint,
                end: object_length - 1,
            },
        ];
        debug_assert_eq!(ranges[0].end + 1, ranges[1].start);

        if let Err(SegmentedError::RangeIgnored | SegmentedError::InvalidContentRange { .. }) =
            download_ranges(&self.client, url, temporary_path, &ranges)
        {
            drop(output);
            let fallback = SingleStreamDownloader::new()?.download(url, temporary_path, |_| {})?;
            return Ok(SegmentedOutcome {
                downloaded_bytes: fallback.downloaded_bytes,
                sha256: fallback.sha256,
                ranges,
                used_single_stream_fallback: true,
            });
        }
        output.sync_all()?;
        let sha256 = hash_file(temporary_path)?;
        Ok(SegmentedOutcome {
            downloaded_bytes: object_length,
            sha256,
            ranges,
            used_single_stream_fallback: false,
        })
    }

    /// Resumes the missing suffix of every durable segment and checkpoints completion atomically.
    ///
    /// # Errors
    /// Returns an error if durable state cannot be loaded/saved or any missing range fails.
    ///
    /// # Panics
    /// Panics only if an internal range worker poisons its result mutex.
    pub fn resume_persisted(
        &self,
        database: &mut Database,
        job_id: JobId,
        url: &HttpUrl,
        object_length: u64,
        temporary_path: &Path,
        at: UnixMillis,
    ) -> Result<ResumeOutcome, SegmentedError> {
        let mut segments = database.segments(job_id)?;
        let requested_ranges: Vec<_> = segments
            .iter()
            .filter(|segment| segment.durable_offset <= segment.end)
            .map(|segment| ByteRange {
                start: segment.durable_offset,
                end: segment.end,
            })
            .collect();
        let output = OpenOptions::new()
            .create(false)
            .read(true)
            .write(true)
            .open(temporary_path)?;
        if output.metadata()?.len() != object_length {
            return Err(SegmentedError::LengthMismatch {
                start: 0,
                end: object_length.saturating_sub(1),
                expected: object_length,
                actual: output.metadata()?.len(),
            });
        }
        download_ranges(&self.client, url, temporary_path, &requested_ranges)?;
        output.sync_all()?;
        for segment in &mut segments {
            if segment.durable_offset <= segment.end {
                segment.durable_offset = segment.end + 1;
                "completed".clone_into(&mut segment.worker_state);
                segment.checkpoint_at = at;
            }
        }
        database.checkpoint_segments(job_id, &segments, at)?;
        Ok(ResumeOutcome {
            downloaded_bytes: object_length,
            sha256: hash_file(temporary_path)?,
            requested_ranges,
        })
    }
}

impl Default for TwoRangeDownloader {
    fn default() -> Self {
        Self::new().expect("default segmented client")
    }
}

fn download_range(
    client: &Client,
    url: &HttpUrl,
    range: ByteRange,
    file: &mut File,
) -> Result<(), SegmentedError> {
    let mut response = client
        .get(url.as_str())
        .header(
            header::RANGE,
            format!("bytes={}-{}", range.start, range.end),
        )
        .send()?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        if response.status().is_success() {
            return Err(SegmentedError::RangeIgnored);
        }
        return Err(SegmentedError::HttpStatus(response.status().as_u16()));
    }
    let content_range = response
        .headers()
        .get(header::CONTENT_RANGE)
        .and_then(|value| value.to_str().ok());
    if !content_range.is_some_and(|value| content_range_matches(value, range)) {
        return Err(SegmentedError::InvalidContentRange {
            start: range.start,
            end: range.end,
        });
    }
    file.seek(SeekFrom::Start(range.start))?;
    let expected = range.len();
    let mut written = 0_u64;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    while written < expected {
        let remaining = usize::try_from((expected - written).min(BUFFER_SIZE as u64))
            .expect("bounded read size");
        let count = response.read(&mut buffer[..remaining])?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        written += u64::try_from(count).expect("buffer count fits u64");
    }
    if written != expected {
        return Err(SegmentedError::LengthMismatch {
            start: range.start,
            end: range.end,
            expected,
            actual: written,
        });
    }
    file.flush()?;
    Ok(())
}

fn content_range_matches(value: &str, expected: ByteRange) -> bool {
    let Some(value) = value.strip_prefix("bytes ") else {
        return false;
    };
    let Some((bounds, total)) = value.split_once('/') else {
        return false;
    };
    if total != "*" && total.parse::<u64>().is_err() {
        return false;
    }
    let Some((start, end)) = bounds.split_once('-') else {
        return false;
    };
    start.parse::<u64>() == Ok(expected.start) && end.parse::<u64>() == Ok(expected.end)
}

fn download_ranges(
    client: &Client,
    url: &HttpUrl,
    output_path: &Path,
    ranges: &[ByteRange],
) -> Result<(), SegmentedError> {
    let error = Arc::new(Mutex::new(None));
    thread::scope(|scope| {
        let mut workers = Vec::with_capacity(ranges.len());
        for &range in ranges {
            let client = client.clone();
            let url = url.clone();
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(output_path)?;
            let first_error = Arc::clone(&error);
            workers.push(scope.spawn(move || {
                let result = download_range(&client, &url, range, &mut file);
                if let Err(failure) = result {
                    let mut slot = first_error.lock().expect("range error lock");
                    if slot.is_none() {
                        *slot = Some(failure);
                    }
                }
            }));
        }
        for worker in workers {
            worker.join().map_err(|_| SegmentedError::WorkerPanicked)?;
        }
        Ok::<(), SegmentedError>(())
    })?;
    if let Some(failure) = error.lock().expect("range error lock").take() {
        return Err(failure);
    }
    Ok(())
}

fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        fs::OpenOptions,
        io::{Read, Seek, SeekFrom, Write},
        net::{TcpListener, TcpStream},
        path::PathBuf,
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    use tempfile::tempdir;

    use super::{ByteRange, SegmentedError, TwoRangeDownloader, plan_fixed_ranges};
    use crate::{
        domain::{
            DownloadKind, HttpUrl, JobId, JobMetadata, Priority, Segment, SegmentId, UnixMillis,
        },
        state::DownloadState,
        storage::Database,
    };

    #[test]
    fn delayed_ranges_write_without_overlap_or_gap_and_match_final_hash() {
        let body: Vec<u8> = (0_u32..200_003)
            .map(|value| u8::try_from(value % 251).expect("byte"))
            .collect();
        let (url, requests, server) = range_server(Arc::new(body.clone()), 2);
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("segmented.part");
        let outcome = TwoRangeDownloader::new()
            .expect("downloader")
            .download(&HttpUrl::parse(url).expect("URL"), body.len() as u64, &path)
            .expect("segmented download");
        server.join().expect("server");

        assert_eq!(fs::read(path).expect("output"), body);
        assert_eq!(outcome.downloaded_bytes, 200_003);
        assert!(!outcome.used_single_stream_fallback);
        assert_eq!(
            outcome.ranges,
            [
                ByteRange {
                    start: 0,
                    end: 100_000
                },
                ByteRange {
                    start: 100_001,
                    end: 200_002
                }
            ]
        );
        assert_eq!(outcome.ranges[0].end + 1, outcome.ranges[1].start);
        assert_eq!(
            outcome.sha256,
            "49cbf04ab31e40bccff20650404805fc4a1f508e56d891f406ec592a2176d2e4"
        );
        let mut captured = requests.lock().expect("requests").clone();
        captured.sort_by_key(|range| range.start);
        assert_eq!(captured, outcome.ranges);
    }

    #[test]
    fn fixed_planner_covers_boundaries_small_objects_and_large_files() {
        assert!(matches!(
            plan_fixed_ranges(0, 1),
            Err(SegmentedError::EmptyObject)
        ));
        for invalid in [0, 33, u8::MAX] {
            assert!(matches!(
                plan_fixed_ranges(100, invalid),
                Err(SegmentedError::InvalidConnectionCount)
            ));
        }
        assert_eq!(
            plan_fixed_ranges(1, 32).expect("one byte"),
            vec![ByteRange { start: 0, end: 0 }]
        );
        assert_eq!(
            plan_fixed_ranges(7, 3).expect("uneven"),
            vec![
                ByteRange { start: 0, end: 2 },
                ByteRange { start: 3, end: 4 },
                ByteRange { start: 5, end: 6 }
            ]
        );

        for length in [2, 3, 31, 32, 33, 1_048_577, 1_u64 << 40] {
            for connections in 1..=32 {
                let ranges = plan_fixed_ranges(length, connections).expect("valid plan");
                assert_eq!(ranges.len() as u64, length.min(u64::from(connections)));
                assert_eq!(ranges.first().expect("first").start, 0);
                assert_eq!(ranges.last().expect("last").end, length - 1);
                assert!(
                    ranges
                        .windows(2)
                        .all(|pair| pair[0].end + 1 == pair[1].start)
                );
                let minimum = ranges.iter().map(|range| range.len()).min().expect("min");
                let maximum = ranges.iter().map(|range| range.len()).max().expect("max");
                assert!(maximum - minimum <= 1);
                assert_eq!(ranges.iter().map(|range| range.len()).sum::<u64>(), length);
            }
        }
    }

    #[test]
    fn restart_requests_only_persisted_missing_segment_suffixes() {
        let body: Vec<u8> = (0_u8..100).collect();
        let directory = tempdir().expect("temp directory");
        let database_path = directory.path().join("state.sqlite");
        let partial_path = directory.path().join("multi.part");
        let job_id = JobId::new();
        let mut database = Database::open(&database_path).expect("database");
        database
            .create_job(&test_job(job_id, &partial_path), None)
            .expect("job");
        database
            .replace_segments(
                job_id,
                &[
                    test_segment(job_id, 0, 49, 20),
                    test_segment(job_id, 50, 99, 70),
                ],
            )
            .expect("segments");
        drop(database);

        let mut partial = OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(&partial_path)
            .expect("partial");
        partial.set_len(100).expect("preallocate");
        partial.write_all(&body[..20]).expect("first prefix");
        partial.seek(SeekFrom::Start(50)).expect("second segment");
        partial.write_all(&body[50..70]).expect("second prefix");
        partial.sync_all().expect("durable prefixes");
        drop(partial);

        let (url, requests, server) = range_server(Arc::new(body.clone()), 2);
        let mut reopened = Database::open(&database_path).expect("reopen database");
        let outcome = TwoRangeDownloader::new()
            .expect("downloader")
            .resume_persisted(
                &mut reopened,
                job_id,
                &HttpUrl::parse(url).expect("URL"),
                100,
                &partial_path,
                timestamp(2),
            )
            .expect("resume");
        server.join().expect("server");

        assert_eq!(fs::read(&partial_path).expect("output"), body);
        let mut expected = vec![
            ByteRange { start: 20, end: 49 },
            ByteRange { start: 70, end: 99 },
        ];
        let mut captured = requests.lock().expect("requests").clone();
        captured.sort_by_key(|range| range.start);
        expected.sort_by_key(|range| range.start);
        assert_eq!(captured, expected);
        assert_eq!(outcome.requested_ranges, expected);
        assert!(
            reopened
                .segments(job_id)
                .expect("stored segments")
                .iter()
                .all(|segment| segment.durable_offset == segment.end + 1
                    && segment.worker_state == "completed")
        );
        assert_eq!(
            reopened
                .get_job(job_id)
                .expect("stored job")
                .expect("job")
                .metadata
                .downloaded_bytes,
            100
        );
    }

    #[test]
    fn ignored_or_invalid_content_range_falls_back_without_corruption() {
        let body: Vec<u8> = (0_u32..131_077)
            .map(|value| u8::try_from(value % 239).expect("byte"))
            .collect();
        for fault in [FallbackFault::Ignored, FallbackFault::InvalidHeader] {
            let (url, server) = fallback_server(Arc::new(body.clone()), fault);
            let directory = tempdir().expect("temp directory");
            let path = directory.path().join("fallback.part");
            fs::write(&path, vec![0xff; body.len()]).expect("poisoned partial");
            let outcome = TwoRangeDownloader::new()
                .expect("downloader")
                .download(&HttpUrl::parse(url).expect("URL"), body.len() as u64, &path)
                .expect("fallback download");
            server.join().expect("server");
            assert!(outcome.used_single_stream_fallback);
            assert_eq!(outcome.downloaded_bytes, body.len() as u64);
            assert_eq!(fs::read(&path).expect("output"), body);
        }
    }

    #[derive(Clone, Copy)]
    enum FallbackFault {
        Ignored,
        InvalidHeader,
    }

    fn fallback_server(
        body: Arc<Vec<u8>>,
        fault: FallbackFault,
    ) -> (String, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            for request_index in 0..3 {
                let (mut stream, _) = listener.accept().expect("accept");
                let request = read_headers(&mut stream);
                if request_index < 2 {
                    let range = parse_range(&request);
                    match fault {
                        FallbackFault::Ignored => {
                            write!(
                                stream,
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            )
                            .expect("ignored headers");
                            stream.write_all(&body).expect("ignored body");
                        }
                        FallbackFault::InvalidHeader => {
                            let start = usize::try_from(range.start).expect("start");
                            let end = usize::try_from(range.end).expect("end");
                            let bytes = &body[start..=end];
                            write!(
                                stream,
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes 0-0/{}\r\nConnection: close\r\n\r\n",
                                bytes.len(),
                                body.len()
                            )
                            .expect("invalid headers");
                            stream.write_all(bytes).expect("range body");
                        }
                    }
                } else {
                    assert!(
                        !request
                            .lines()
                            .any(|line| line.to_ascii_lowercase().starts_with("range:"))
                    );
                    write!(
                        stream,
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    )
                    .expect("fallback headers");
                    stream.write_all(&body).expect("fallback body");
                }
            }
        });
        (format!("http://{address}/file"), server)
    }

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("timestamp")
    }

    fn test_job(id: JobId, destination: &std::path::Path) -> JobMetadata {
        JobMetadata {
            id,
            source_url: HttpUrl::parse("https://example.test/file").expect("URL"),
            final_url: None,
            display_url: "https://example.test/file".to_owned(),
            filename: "file.bin".to_owned(),
            destination: PathBuf::from(destination),
            state: DownloadState::Downloading,
            kind: DownloadKind::File,
            total_bytes: Some(100),
            downloaded_bytes: 40,
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

    fn test_segment(job_id: JobId, start: u64, end: u64, offset: u64) -> Segment {
        Segment {
            id: SegmentId::new(),
            job_id,
            start,
            end,
            durable_offset: offset,
            worker_state: "paused".to_owned(),
            checksum: None,
            checkpoint_at: timestamp(1),
        }
    }

    fn range_server(
        body: Arc<Vec<u8>>,
        request_count: usize,
    ) -> (String, Arc<Mutex<Vec<ByteRange>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let server = thread::spawn(move || {
            let mut workers = Vec::new();
            for connection in listener.incoming().take(request_count) {
                let mut stream = connection.expect("accept");
                let body = Arc::clone(&body);
                let captured = Arc::clone(&captured);
                workers.push(thread::spawn(move || {
                    let range = read_range(&mut stream);
                    captured.lock().expect("requests").push(range);
                    if range.start == 0 {
                        thread::sleep(Duration::from_millis(35));
                    } else {
                        thread::sleep(Duration::from_millis(5));
                    }
                    let start = usize::try_from(range.start).expect("fixture start fits usize");
                    let end = usize::try_from(range.end).expect("fixture end fits usize");
                    let bytes = &body[start..=end];
                    write!(
                        stream,
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                        bytes.len(), range.start, range.end, body.len()
                    )
                    .expect("headers");
                    stream.write_all(bytes).expect("body");
                }));
            }
            for worker in workers {
                worker.join().expect("response worker");
            }
        });
        (format!("http://{address}/file"), requests, server)
    }

    fn read_range(stream: &mut TcpStream) -> ByteRange {
        parse_range(&read_headers(stream))
    }

    fn read_headers(stream: &mut TcpStream) -> String {
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request");
            bytes.extend_from_slice(&buffer[..count]);
        }
        String::from_utf8(bytes).expect("request text")
    }

    fn parse_range(request: &str) -> ByteRange {
        let value = request
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.eq_ignore_ascii_case("range")
                    .then(|| value.trim().strip_prefix("bytes="))
                    .flatten()
            })
            .expect("range header");
        let (start, end) = value.split_once('-').expect("range bounds");
        ByteRange {
            start: start.parse().expect("start"),
            end: end.parse().expect("end"),
        }
    }
}
