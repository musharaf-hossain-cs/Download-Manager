//! Correctness-first single-stream HTTP transfer primitives.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
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
        mut on_progress: F,
    ) -> Result<TransferOutcome, TransferError>
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
        let (downloaded_bytes, sha256) =
            stream_response(&mut response, &mut file, total_bytes, &mut on_progress)?;
        file.flush()?;
        file.sync_all()?;
        if let Some(expected) = total_bytes
            && downloaded_bytes != expected
        {
            return Err(TransferError::LengthMismatch {
                expected,
                actual: downloaded_bytes,
            });
        }
        Ok(TransferOutcome {
            downloaded_bytes,
            total_bytes,
            sha256,
        })
    }
}

impl Default for SingleStreamDownloader {
    fn default() -> Self {
        Self::new().expect("default transfer client")
    }
}

fn stream_response<F>(
    response: &mut impl Read,
    file: &mut File,
    total_bytes: Option<u64>,
    on_progress: &mut F,
) -> Result<(u64, String), TransferError>
where
    F: FnMut(TransferProgress),
{
    let mut downloaded_bytes = 0_u64;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
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
    Ok((downloaded_bytes, format!("{:x}", hasher.finalize())))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{SingleStreamDownloader, TransferProgress};
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
}
