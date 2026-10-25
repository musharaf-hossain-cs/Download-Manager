//! Correctness-first single-stream HTTP transfer primitives.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use reqwest::{StatusCode, blocking::Client, redirect::Policy};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::domain::HttpUrl;

const BUFFER_SIZE: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferOutcome {
    pub downloaded_bytes: u64,
    pub total_bytes: Option<u64>,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransferStatus {
    Completed(TransferOutcome),
    Paused {
        downloaded_bytes: u64,
        total_bytes: Option<u64>,
    },
}

#[derive(Debug, Clone, Default)]
pub struct TransferControl {
    pause_requested: Arc<AtomicBool>,
}

impl TransferControl {
    pub fn request_pause(&self) {
        self.pause_requested.store(true, Ordering::Release);
    }

    #[must_use]
    pub fn is_pause_requested(&self) -> bool {
        self.pause_requested.load(Ordering::Acquire)
    }
}

#[derive(Debug, Error)]
pub enum TransferError {
    #[error("HTTP transfer failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("HTTP transfer returned status {0}")]
    HttpStatus(u16),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("response ended at {actual} bytes, expected {expected}")]
    LengthMismatch { expected: u64, actual: u64 },
}

#[derive(Debug, Clone)]
pub struct SingleStreamDownloader {
    client: Client,
}

impl SingleStreamDownloader {
    /// Builds a bounded HTTP(S) transfer client.
    ///
    /// # Errors
    /// Returns an error if the client cannot be initialized.
    pub fn new() -> Result<Self, TransferError> {
        Ok(Self {
            client: Client::builder()
                .redirect(Policy::limited(10))
                .connect_timeout(Duration::from_secs(30))
                .build()?,
        })
    }

    /// Streams one response into a newly truncated temporary file.
    ///
    /// The callback receives monotonically increasing byte counts. The file is flushed and synced
    /// before a successful outcome is returned.
    ///
    /// # Errors
    /// Returns an error for HTTP failures, local I/O failures, or a truncated known-length body.
    pub fn download<F>(
        &self,
        url: &HttpUrl,
        temporary_path: &Path,
        on_progress: F,
    ) -> Result<TransferOutcome, TransferError>
    where
        F: FnMut(TransferProgress),
    {
        match self.download_with_control(
            url,
            temporary_path,
            &TransferControl::default(),
            on_progress,
        )? {
            TransferStatus::Completed(outcome) => Ok(outcome),
            TransferStatus::Paused { .. } => unreachable!("private transfer control cannot pause"),
        }
    }

    /// Streams to a temporary file until complete or until pause is requested.
    ///
    /// A paused result is returned only after all accepted bytes are flushed and synced.
    ///
    /// # Errors
    /// Returns an error for HTTP failures, local I/O failures, or a truncated known-length body.
    pub fn download_with_control<F>(
        &self,
        url: &HttpUrl,
        temporary_path: &Path,
        control: &TransferControl,
        mut on_progress: F,
    ) -> Result<TransferStatus, TransferError>
    where
        F: FnMut(TransferProgress),
    {
        if let Some(parent) = temporary_path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        let mut response = self.client.get(url.as_str()).send()?;
        if !response.status().is_success() || response.status() == StatusCode::PARTIAL_CONTENT {
            return Err(TransferError::HttpStatus(response.status().as_u16()));
        }
        let total_bytes = response.content_length();
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temporary_path)?;
        let stream = stream_response(
            &mut response,
            &mut file,
            total_bytes,
            control,
            &mut on_progress,
        )?;
        file.flush()?;
        file.sync_all()?;
        if stream.paused {
            return Ok(TransferStatus::Paused {
                downloaded_bytes: stream.downloaded_bytes,
                total_bytes,
            });
        }
        if let Some(expected) = total_bytes
            && stream.downloaded_bytes != expected
        {
            return Err(TransferError::LengthMismatch {
                expected,
                actual: stream.downloaded_bytes,
            });
        }
        Ok(TransferStatus::Completed(TransferOutcome {
            downloaded_bytes: stream.downloaded_bytes,
            total_bytes,
            sha256: stream.sha256,
        }))
    }
}

impl Default for SingleStreamDownloader {
    fn default() -> Self {
        Self::new().expect("default transfer client")
    }
}

struct StreamResult {
    downloaded_bytes: u64,
    sha256: String,
    paused: bool,
}

fn stream_response<F>(
    response: &mut impl Read,
    file: &mut File,
    total_bytes: Option<u64>,
    control: &TransferControl,
    on_progress: &mut F,
) -> Result<StreamResult, TransferError>
where
    F: FnMut(TransferProgress),
{
    let mut downloaded_bytes = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        if control.is_pause_requested() {
            return Ok(StreamResult {
                downloaded_bytes,
                sha256: format!("{:x}", hasher.finalize()),
                paused: true,
            });
        }
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
        downloaded_bytes = downloaded_bytes
            .checked_add(u64::try_from(count).expect("buffer count fits u64"))
            .expect("download byte counter overflow");
        on_progress(TransferProgress {
            downloaded_bytes,
            total_bytes,
        });
    }
    Ok(StreamResult {
        downloaded_bytes,
        sha256: format!("{:x}", hasher.finalize()),
        paused: false,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{SingleStreamDownloader, TransferControl, TransferProgress, TransferStatus};
    use crate::{
        domain::HttpUrl,
        test_support::{Fixture, FixtureServer},
    };

    #[test]
    fn downloads_known_length_to_temp_with_progress_and_hash() {
        let body = vec![0x5a; 150_000];
        let server = FixtureServer::start(vec![Fixture::ok(body.clone())]);
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("download.part");
        let mut progress = Vec::new();
        let outcome = SingleStreamDownloader::new()
            .expect("downloader")
            .download(
                &HttpUrl::parse(server.url("/known")).expect("URL"),
                &path,
                |event| progress.push(event),
            )
            .expect("download");
        assert_eq!(fs::read(path).expect("read temp"), body);
        assert_eq!(outcome.downloaded_bytes, 150_000);
        assert_eq!(outcome.total_bytes, Some(150_000));
        assert_eq!(
            outcome.sha256,
            "a0a1de02f897cc655b5ecf7b9d9b80742bd2c4e6378af94b95acace452bd7e93"
        );
        assert_eq!(
            progress.last(),
            Some(&TransferProgress {
                downloaded_bytes: 150_000,
                total_bytes: Some(150_000)
            })
        );
        assert!(
            progress
                .windows(2)
                .all(|window| window[0].downloaded_bytes < window[1].downloaded_bytes)
        );
    }

    #[test]
    fn downloads_unknown_length_without_inventing_a_total() {
        let body = b"close-delimited body".to_vec();
        let server = FixtureServer::start(vec![Fixture::ok(body.clone()).without_content_length()]);
        let directory = tempdir().expect("temp directory");
        let mut progress = Vec::new();
        let outcome = SingleStreamDownloader::new()
            .expect("downloader")
            .download(
                &HttpUrl::parse(server.url("/unknown")).expect("URL"),
                &directory.path().join("unknown.part"),
                |event| progress.push(event),
            )
            .expect("download");
        assert_eq!(outcome.downloaded_bytes, body.len() as u64);
        assert_eq!(outcome.total_bytes, None);
        assert!(progress.iter().all(|event| event.total_bytes.is_none()));
    }

    #[test]
    fn pause_stops_the_stream_at_an_exact_synced_offset() {
        let body = vec![0x33; 400_000];
        let server = FixtureServer::start(vec![Fixture::ok(body.clone())]);
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("paused.part");
        let control = TransferControl::default();
        let callback_control = control.clone();
        let status = SingleStreamDownloader::new()
            .expect("downloader")
            .download_with_control(
                &HttpUrl::parse(server.url("/pause")).expect("URL"),
                &path,
                &control,
                move |_| callback_control.request_pause(),
            )
            .expect("pause");
        let TransferStatus::Paused {
            downloaded_bytes,
            total_bytes,
        } = status
        else {
            panic!("expected paused transfer");
        };
        assert!(downloaded_bytes > 0 && downloaded_bytes < body.len() as u64);
        assert_eq!(total_bytes, Some(body.len() as u64));
        assert_eq!(
            fs::metadata(&path).expect("temp metadata").len(),
            downloaded_bytes
        );
        assert_eq!(
            fs::read(&path).expect("temp body"),
            body[..usize::try_from(downloaded_bytes).expect("offset")]
        );
    }
}
