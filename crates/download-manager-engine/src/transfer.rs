//! Correctness-first single-stream HTTP transfer primitives.

use std::{
    fs::{self, OpenOptions},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, sync_channel},
    },
    thread,
    time::Duration,
};

use reqwest::{StatusCode, blocking::Client, redirect::Policy};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    disk::{DiskError, write_all_with_disk_guard},
    domain::HttpUrl,
};

const BUFFER_SIZE: usize = 64 * 1024;
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);

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
    #[error("disk write failed safely: {0}")]
    Disk(#[from] DiskError),
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
        let receiver = start_response_reader(self.client.clone(), url.as_str().to_owned())?;
        let total_bytes = match next_event(&receiver, control)? {
            Some(NetworkEvent::Headers { total_bytes }) => total_bytes,
            None => {
                sync_empty_partial(temporary_path)?;
                return Ok(TransferStatus::Paused {
                    downloaded_bytes: 0,
                    total_bytes: None,
                });
            }
            Some(NetworkEvent::Failed(error)) => return Err(error),
            Some(NetworkEvent::Chunk(_) | NetworkEvent::Finished) => {
                return Err(unexpected_stream_event(
                    "response headers were not received",
                ));
            }
        };
        let mut file = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(temporary_path)?;
        let mut downloaded_bytes = 0_u64;
        let mut hasher = Sha256::new();
        let paused = loop {
            match next_event(&receiver, control)? {
                None => break true,
                Some(NetworkEvent::Chunk(bytes)) => {
                    if let Err(error) =
                        write_all_with_disk_guard(&mut file, &bytes, downloaded_bytes)
                    {
                        file.flush()?;
                        file.sync_all()?;
                        return Err(error.into());
                    }
                    hasher.update(&bytes);
                    let chunk_length = u64::try_from(bytes.len())
                        .map_err(|_| unexpected_stream_event("response chunk is too large"))?;
                    downloaded_bytes = downloaded_bytes
                        .checked_add(chunk_length)
                        .ok_or_else(|| unexpected_stream_event("download byte counter overflow"))?;
                    on_progress(TransferProgress {
                        downloaded_bytes,
                        total_bytes,
                    });
                }
                Some(NetworkEvent::Finished) => break false,
                Some(NetworkEvent::Headers { .. }) => {
                    return Err(unexpected_stream_event("duplicate response headers"));
                }
                Some(NetworkEvent::Failed(error)) => return Err(error),
            }
        };
        file.flush()?;
        file.sync_all()?;
        if paused {
            return Ok(TransferStatus::Paused {
                downloaded_bytes,
                total_bytes,
            });
        }
        if let Some(expected) = total_bytes
            && downloaded_bytes != expected
        {
            return Err(TransferError::LengthMismatch {
                expected,
                actual: downloaded_bytes,
            });
        }
        Ok(TransferStatus::Completed(TransferOutcome {
            downloaded_bytes,
            total_bytes,
            sha256: format!("{:x}", hasher.finalize()),
        }))
    }
}

impl Default for SingleStreamDownloader {
    fn default() -> Self {
        Self::new().expect("default transfer client")
    }
}

enum NetworkEvent {
    Headers { total_bytes: Option<u64> },
    Chunk(Vec<u8>),
    Finished,
    Failed(TransferError),
}

fn start_response_reader(
    client: Client,
    url: String,
) -> Result<Receiver<NetworkEvent>, TransferError> {
    let (sender, receiver) = sync_channel(1);
    thread::Builder::new()
        .name("download-manager-http-reader".to_owned())
        .spawn(move || {
            let mut response = match client.get(url).send() {
                Ok(response) => response,
                Err(error) => {
                    let _ = sender.send(NetworkEvent::Failed(error.into()));
                    return;
                }
            };
            if !response.status().is_success() || response.status() == StatusCode::PARTIAL_CONTENT {
                let _ = sender.send(NetworkEvent::Failed(TransferError::HttpStatus(
                    response.status().as_u16(),
                )));
                return;
            }
            if sender
                .send(NetworkEvent::Headers {
                    total_bytes: response.content_length(),
                })
                .is_err()
            {
                return;
            }
            let mut buffer = vec![0_u8; BUFFER_SIZE];
            loop {
                match response.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.send(NetworkEvent::Finished);
                        return;
                    }
                    Ok(count) => {
                        if sender
                            .send(NetworkEvent::Chunk(buffer[..count].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(NetworkEvent::Failed(error.into()));
                        return;
                    }
                }
            }
        })?;
    Ok(receiver)
}

fn next_event(
    receiver: &Receiver<NetworkEvent>,
    control: &TransferControl,
) -> Result<Option<NetworkEvent>, TransferError> {
    loop {
        if control.is_pause_requested() {
            return Ok(None);
        }
        match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(NetworkEvent::Failed(_)) if control.is_pause_requested() => return Ok(None),
            Ok(NetworkEvent::Failed(error)) => return Err(error),
            Ok(event) => return Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(unexpected_stream_event("HTTP reader disconnected"));
            }
        }
    }
}

fn sync_empty_partial(path: &Path) -> Result<(), TransferError> {
    let mut file = OpenOptions::new()
        .create(true)
        .truncate(true)
        .write(true)
        .open(path)?;
    file.flush()?;
    file.sync_all()?;
    Ok(())
}

fn unexpected_stream_event(message: &str) -> TransferError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
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
