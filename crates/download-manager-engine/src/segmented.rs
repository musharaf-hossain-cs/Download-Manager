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

use crate::domain::HttpUrl;

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
}

#[derive(Debug, Error)]
pub enum SegmentedError {
    #[error("object length must be at least two bytes")]
    ObjectTooSmall,
    #[error("HTTP range request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("range response returned status {0}")]
    HttpStatus(u16),
    #[error("range {start}-{end} returned {actual} bytes, expected {expected}")]
    LengthMismatch {
        start: u64,
        end: u64,
        expected: u64,
        actual: u64,
    },
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("range worker panicked")]
    WorkerPanicked,
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

        let error = Arc::new(Mutex::new(None));
        thread::scope(|scope| {
            let mut workers = Vec::with_capacity(2);
            for range in ranges {
                let client = self.client.clone();
                let url = url.clone();
                let mut file = output.try_clone()?;
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
        output.sync_all()?;
        let sha256 = hash_file(temporary_path)?;
        Ok(SegmentedOutcome {
            downloaded_bytes: object_length,
            sha256,
            ranges,
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
        return Err(SegmentedError::HttpStatus(response.status().as_u16()));
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
        io::{Read, Write},
        net::{TcpListener, TcpStream},
        sync::{Arc, Mutex},
        thread,
        time::Duration,
    };

    use tempfile::tempdir;

    use super::{ByteRange, TwoRangeDownloader};
    use crate::domain::HttpUrl;

    #[test]
    fn delayed_ranges_write_without_overlap_or_gap_and_match_final_hash() {
        let body: Vec<u8> = (0_u32..200_003)
            .map(|value| u8::try_from(value % 251).expect("byte"))
            .collect();
        let (url, requests, server) = range_server(Arc::new(body.clone()));
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("segmented.part");
        let outcome = TwoRangeDownloader::new()
            .expect("downloader")
            .download(&HttpUrl::parse(url).expect("URL"), body.len() as u64, &path)
            .expect("segmented download");
        server.join().expect("server");

        assert_eq!(fs::read(path).expect("output"), body);
        assert_eq!(outcome.downloaded_bytes, 200_003);
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

    fn range_server(
        body: Arc<Vec<u8>>,
    ) -> (String, Arc<Mutex<Vec<ByteRange>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let address = listener.local_addr().expect("address");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = Arc::clone(&requests);
        let server = thread::spawn(move || {
            let mut workers = Vec::new();
            for connection in listener.incoming().take(2) {
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
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).expect("request");
            bytes.extend_from_slice(&buffer[..count]);
        }
        let request = String::from_utf8(bytes).expect("request text");
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
