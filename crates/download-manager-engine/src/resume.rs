//! Identity-validated single-stream range resume.

use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
    sync::mpsc::{Receiver, RecvTimeoutError, sync_channel},
    thread,
    time::Duration,
};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{CONTENT_RANGE, IF_RANGE, RANGE},
};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::{HttpUrl, RangeCapability, RemoteIdentity},
    http_range::parse_content_range,
    probe::{HttpProbe, ProbeError},
    transfer::{TransferControl, TransferOutcome, TransferProgress, TransferStatus},
};

const BUFFER_SIZE: usize = 64 * 1024;
const CONTROL_POLL_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum IdentityMismatch {
    #[error("ETag changed or disappeared")]
    Etag,
    #[error("Last-Modified changed or disappeared")]
    LastModified,
    #[error("content length changed from {expected} to {actual:?}")]
    ContentLength { expected: u64, actual: Option<u64> },
}

#[derive(Debug, Error)]
pub enum ResumeError {
    #[error("remote probe failed: {0}")]
    Probe(#[from] ProbeError),
    #[error("remote object identity changed: {0}")]
    IdentityChanged(#[from] IdentityMismatch),
    #[error("server does not support safe range resume; restart required")]
    RestartRequired,
    #[error("HTTP resume failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("HTTP resume returned status {0}; restart required")]
    UnexpectedStatus(u16),
    #[error("invalid Content-Range for requested offset {offset}: {value:?}")]
    InvalidContentRange { offset: u64, value: Option<String> },
    #[error("partial file is {actual} bytes but durable offset is {expected}")]
    LocalOffsetMismatch { expected: u64, actual: u64 },
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("resumed file ended at {actual} bytes, expected {expected}")]
    LengthMismatch { expected: u64, actual: u64 },
}

/// Validates that the current remote identity can safely extend a stored partial file.
///
/// # Errors
/// Returns the first strong validator or size mismatch.
pub fn validate_remote_identity(
    expected: &RemoteIdentity,
    actual: &RemoteIdentity,
) -> Result<(), IdentityMismatch> {
    if let Some(etag) = &expected.etag {
        if actual.etag.as_ref() != Some(etag) {
            return Err(IdentityMismatch::Etag);
        }
    } else if let Some(last_modified) = &expected.last_modified
        && actual.last_modified.as_ref() != Some(last_modified)
    {
        return Err(IdentityMismatch::LastModified);
    }
    if let Some(length) = expected.content_length
        && actual.content_length != Some(length)
    {
        return Err(IdentityMismatch::ContentLength {
            expected: length,
            actual: actual.content_length,
        });
    }
    Ok(())
}

/// Probes identity and appends only the requested missing byte range.
///
/// The partial file is not opened for writing until identity and range support are confirmed.
///
/// # Errors
/// Returns a safe refusal when identity, range support, local length, response status, or range
/// metadata cannot prove an append is correct.
///
/// # Panics
/// Panics only on a platform where an in-memory buffer length cannot fit in `u64`.
pub fn resume_single_stream<F>(
    url: &HttpUrl,
    partial_path: &Path,
    durable_offset: u64,
    expected_identity: &RemoteIdentity,
    on_progress: F,
) -> Result<TransferOutcome, ResumeError>
where
    F: FnMut(TransferProgress),
{
    match resume_single_stream_with_control(
        url,
        partial_path,
        durable_offset,
        expected_identity,
        &TransferControl::default(),
        on_progress,
    )? {
        TransferStatus::Completed(outcome) => Ok(outcome),
        TransferStatus::Paused { .. } => unreachable!("private transfer control cannot pause"),
    }
}

/// Probes identity and appends the requested missing byte range until complete or paused.
///
/// A paused result is returned only after all accepted bytes are flushed and synced. The partial
/// file is not opened for writing until identity and range support are confirmed.
///
/// # Errors
/// Returns a safe refusal when identity, range support, local length, response status, or range
/// metadata cannot prove an append is correct.
///
pub fn resume_single_stream_with_control<F>(
    url: &HttpUrl,
    partial_path: &Path,
    durable_offset: u64,
    expected_identity: &RemoteIdentity,
    control: &TransferControl,
    mut on_progress: F,
) -> Result<TransferStatus, ResumeError>
where
    F: FnMut(TransferProgress),
{
    let probe = match HttpProbe::new().and_then(|probe| probe.probe(url)) {
        Ok(probe) => probe,
        Err(_) if control.is_pause_requested() => {
            return Ok(TransferStatus::Paused {
                downloaded_bytes: durable_offset,
                total_bytes: expected_identity.content_length,
            });
        }
        Err(error) => return Err(error.into()),
    };
    validate_remote_identity(expected_identity, &probe.identity)?;
    if probe.identity.range_capability != RangeCapability::Supported {
        return Err(ResumeError::RestartRequired);
    }
    let local_length = partial_path.metadata()?.len();
    if local_length != durable_offset {
        return Err(ResumeError::LocalOffsetMismatch {
            expected: durable_offset,
            actual: local_length,
        });
    }
    if expected_identity.content_length == Some(durable_offset) {
        if control.is_pause_requested() {
            return Ok(TransferStatus::Paused {
                downloaded_bytes: durable_offset,
                total_bytes: expected_identity.content_length,
            });
        }
        return Ok(TransferStatus::Completed(TransferOutcome {
            downloaded_bytes: durable_offset,
            total_bytes: expected_identity.content_length,
            sha256: hash_file(partial_path)?,
        }));
    }
    let validator = expected_identity
        .etag
        .as_ref()
        .or(expected_identity.last_modified.as_ref())
        .cloned();
    let receiver = start_resume_reader(
        probe.final_url.as_str().to_owned(),
        durable_offset,
        expected_identity.content_length,
        validator,
    )?;
    if !wait_for_resume_ready(&receiver, control)? {
        return Ok(TransferStatus::Paused {
            downloaded_bytes: durable_offset,
            total_bytes: expected_identity.content_length,
        });
    }
    let mut file = OpenOptions::new().append(true).open(partial_path)?;
    let mut downloaded_bytes = durable_offset;
    let paused = loop {
        match next_resume_event(&receiver, control)? {
            None => break true,
            Some(ResumeEvent::Chunk(bytes)) => {
                file.write_all(&bytes)?;
                let chunk_length = u64::try_from(bytes.len())
                    .map_err(|_| unexpected_resume_event("response chunk is too large"))?;
                downloaded_bytes = downloaded_bytes
                    .checked_add(chunk_length)
                    .ok_or_else(|| unexpected_resume_event("download byte counter overflow"))?;
                on_progress(TransferProgress {
                    downloaded_bytes,
                    total_bytes: expected_identity.content_length,
                });
            }
            Some(ResumeEvent::Finished) => break false,
            Some(ResumeEvent::Ready) => {
                return Err(unexpected_resume_event("duplicate response headers"));
            }
            Some(ResumeEvent::Failed(error)) => return Err(error),
        }
    };
    file.flush()?;
    file.sync_all()?;
    if paused {
        return Ok(TransferStatus::Paused {
            downloaded_bytes,
            total_bytes: expected_identity.content_length,
        });
    }
    if let Some(expected) = expected_identity.content_length
        && downloaded_bytes != expected
    {
        return Err(ResumeError::LengthMismatch {
            expected,
            actual: downloaded_bytes,
        });
    }
    Ok(TransferStatus::Completed(TransferOutcome {
        downloaded_bytes,
        total_bytes: expected_identity.content_length,
        sha256: hash_file(partial_path)?,
    }))
}

enum ResumeEvent {
    Ready,
    Chunk(Vec<u8>),
    Finished,
    Failed(ResumeError),
}

fn start_resume_reader(
    url: String,
    offset: u64,
    total: Option<u64>,
    validator: Option<String>,
) -> Result<Receiver<ResumeEvent>, ResumeError> {
    let (sender, receiver) = sync_channel(1);
    thread::Builder::new()
        .name("download-manager-resume-reader".to_owned())
        .spawn(move || {
            let client = match Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .build()
            {
                Ok(client) => client,
                Err(error) => {
                    let _ = sender.send(ResumeEvent::Failed(error.into()));
                    return;
                }
            };
            let mut request = client.get(url).header(RANGE, format!("bytes={offset}-"));
            if let Some(validator) = validator {
                request = request.header(IF_RANGE, validator);
            }
            let mut response = match request.send() {
                Ok(response) => response,
                Err(error) => {
                    let _ = sender.send(ResumeEvent::Failed(error.into()));
                    return;
                }
            };
            if response.status() != StatusCode::PARTIAL_CONTENT {
                let _ = sender.send(ResumeEvent::Failed(ResumeError::UnexpectedStatus(
                    response.status().as_u16(),
                )));
                return;
            }
            let content_range = response
                .headers()
                .get(CONTENT_RANGE)
                .and_then(|value| value.to_str().ok())
                .map(str::to_owned);
            if !content_range
                .as_deref()
                .is_some_and(|value| valid_content_range(value, offset, total))
            {
                let _ = sender.send(ResumeEvent::Failed(ResumeError::InvalidContentRange {
                    offset,
                    value: content_range,
                }));
                return;
            }
            if sender.send(ResumeEvent::Ready).is_err() {
                return;
            }
            let mut buffer = vec![0_u8; BUFFER_SIZE];
            loop {
                match response.read(&mut buffer) {
                    Ok(0) => {
                        let _ = sender.send(ResumeEvent::Finished);
                        return;
                    }
                    Ok(count) => {
                        if sender
                            .send(ResumeEvent::Chunk(buffer[..count].to_vec()))
                            .is_err()
                        {
                            return;
                        }
                    }
                    Err(error) => {
                        let _ = sender.send(ResumeEvent::Failed(error.into()));
                        return;
                    }
                }
            }
        })?;
    Ok(receiver)
}

fn next_resume_event(
    receiver: &Receiver<ResumeEvent>,
    control: &TransferControl,
) -> Result<Option<ResumeEvent>, ResumeError> {
    loop {
        if control.is_pause_requested() {
            return Ok(None);
        }
        match receiver.recv_timeout(CONTROL_POLL_INTERVAL) {
            Ok(ResumeEvent::Failed(_)) if control.is_pause_requested() => return Ok(None),
            Ok(ResumeEvent::Failed(error)) => return Err(error),
            Ok(event) => return Ok(Some(event)),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return Err(unexpected_resume_event("HTTP resume reader disconnected"));
            }
        }
    }
}

fn wait_for_resume_ready(
    receiver: &Receiver<ResumeEvent>,
    control: &TransferControl,
) -> Result<bool, ResumeError> {
    match next_resume_event(receiver, control)? {
        Some(ResumeEvent::Ready) => Ok(true),
        None => Ok(false),
        Some(ResumeEvent::Failed(error)) => Err(error),
        Some(ResumeEvent::Chunk(_) | ResumeEvent::Finished) => Err(unexpected_resume_event(
            "response headers were not received",
        )),
    }
}

fn unexpected_resume_event(message: &str) -> ResumeError {
    std::io::Error::new(std::io::ErrorKind::InvalidData, message).into()
}

fn valid_content_range(value: &str, offset: u64, total: Option<u64>) -> bool {
    parse_content_range(value).is_some_and(|range| range.matches_resume(offset, total))
}

fn hash_file(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
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
    use std::{fs, thread, time::Duration, time::Instant};

    use tempfile::tempdir;

    use super::{
        IdentityMismatch, ResumeError, resume_single_stream, resume_single_stream_with_control,
    };
    use crate::{
        domain::{HttpUrl, RangeCapability, RemoteIdentity},
        test_support::{Fixture, FixtureServer},
        transfer::{TransferControl, TransferStatus},
    };

    fn identity(etag: &str, length: u64) -> RemoteIdentity {
        RemoteIdentity {
            etag: Some(etag.to_owned()),
            last_modified: Some("Sun, 06 Nov 1994 08:49:37 GMT".to_owned()),
            content_length: Some(length),
            mime_type: Some("application/octet-stream".to_owned()),
            content_fingerprint: None,
            range_capability: RangeCapability::Supported,
        }
    }

    #[test]
    fn same_object_resumes_only_the_missing_range() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("ETag", "\"v1\"")
                .header("Content-Length", "11")
                .header("Accept-Ranges", "bytes"),
            Fixture::ok(b" world".to_vec())
                .status(206)
                .header("Content-Range", "bytes 5-10/11"),
        ]);
        let directory = tempdir().expect("directory");
        let partial = directory.path().join("file.part");
        fs::write(&partial, b"hello").expect("partial");
        let outcome = resume_single_stream(
            &HttpUrl::parse(server.url("/file")).expect("URL"),
            &partial,
            5,
            &identity("\"v1\"", 11),
            |_| {},
        )
        .expect("resume");
        assert_eq!(fs::read(&partial).expect("completed"), b"hello world");
        assert_eq!(outcome.downloaded_bytes, 11);
        let requests = server.requests();
        assert_eq!(requests[1].header("range"), Some("bytes=5-"));
        assert_eq!(requests[1].header("if-range"), Some("\"v1\""));
    }

    #[test]
    fn resumed_transfer_pauses_only_after_syncing_accepted_bytes() {
        let tail = vec![0x5a; 100_000];
        let total = 5 + u64::try_from(tail.len()).expect("tail length");
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("ETag", "\"v1\"")
                .header("Content-Length", &total.to_string())
                .header("Accept-Ranges", "bytes"),
            Fixture::ok(tail.clone())
                .status(206)
                .header("Content-Range", &format!("bytes 5-{}/{total}", total - 1)),
        ]);
        let directory = tempdir().expect("directory");
        let partial = directory.path().join("file.part");
        fs::write(&partial, b"hello").expect("partial");
        let control = TransferControl::default();
        let progress_control = control.clone();
        let status = resume_single_stream_with_control(
            &HttpUrl::parse(server.url("/file")).expect("URL"),
            &partial,
            5,
            &identity("\"v1\"", total),
            &control,
            move |_| progress_control.request_pause(),
        )
        .expect("pause resumed transfer");
        let TransferStatus::Paused {
            downloaded_bytes,
            total_bytes,
        } = status
        else {
            panic!("resume should pause");
        };
        assert_eq!(total_bytes, Some(total));
        assert!(downloaded_bytes > 5);
        assert!(downloaded_bytes < total);
        let bytes = fs::read(&partial).expect("synced partial");
        assert_eq!(bytes.len() as u64, downloaded_bytes);
        assert_eq!(&bytes[..5], b"hello");
        assert_eq!(&bytes[5..], &tail[..bytes.len() - 5]);
    }

    #[test]
    fn resumed_transfer_pauses_while_the_peer_stalls_before_the_body() {
        let total = 11;
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("ETag", "\"v1\"")
                .header("Content-Length", &total.to_string())
                .header("Accept-Ranges", "bytes"),
            Fixture::ok(b" world".to_vec())
                .status(206)
                .header("Content-Range", "bytes 5-10/11")
                .body_delay(Duration::from_secs(1)),
        ]);
        let directory = tempdir().expect("directory");
        let partial = directory.path().join("file.part");
        fs::write(&partial, b"hello").expect("partial");
        let control = TransferControl::default();
        let resume_control = control.clone();
        let resume_url = HttpUrl::parse(server.url("/file")).expect("URL");
        let resume_partial = partial.clone();
        let resume_worker = thread::spawn(move || {
            resume_single_stream_with_control(
                &resume_url,
                &resume_partial,
                5,
                &identity("\"v1\"", total),
                &resume_control,
                |_| {},
            )
            .expect("pause stalled resume")
        });

        let request_deadline = Instant::now() + Duration::from_secs(2);
        while server.requests().len() < 2 {
            assert!(
                Instant::now() < request_deadline,
                "the ranged GET was not issued before the fixture deadline"
            );
            thread::sleep(Duration::from_millis(5));
        }
        let pause_started = Instant::now();
        control.request_pause();
        let status = resume_worker.join().expect("resume worker");

        assert!(matches!(
            status,
            TransferStatus::Paused {
                downloaded_bytes: 5,
                total_bytes: Some(11)
            }
        ));
        assert!(
            pause_started.elapsed() < Duration::from_millis(900),
            "pause should not wait for the stalled response body"
        );
        assert_eq!(fs::read(&partial).expect("unchanged partial"), b"hello");
        assert_eq!(server.requests().len(), 2, "the ranged GET was issued");
    }

    #[test]
    fn fully_durable_partial_completes_after_identity_probe_without_empty_range() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("ETag", "\"v1\"")
                .header("Content-Length", "5")
                .header("Accept-Ranges", "bytes"),
        ]);
        let directory = tempdir().expect("directory");
        let partial = directory.path().join("file.part");
        fs::write(&partial, b"hello").expect("partial");

        let outcome = resume_single_stream(
            &HttpUrl::parse(server.url("/file")).expect("URL"),
            &partial,
            5,
            &identity("\"v1\"", 5),
            |_| {},
        )
        .expect("complete durable partial");

        assert_eq!(outcome.downloaded_bytes, 5);
        assert_eq!(outcome.total_bytes, Some(5));
        assert_eq!(fs::read(partial).expect("unchanged partial"), b"hello");
        assert_eq!(
            server.requests().len(),
            1,
            "only the identity probe is needed"
        );
    }

    #[test]
    fn changed_etag_or_size_refuses_without_touching_partial() {
        for (etag, length, mismatch) in [
            ("\"v2\"", 5, IdentityMismatch::Etag),
            (
                "\"v1\"",
                6,
                IdentityMismatch::ContentLength {
                    expected: 5,
                    actual: Some(6),
                },
            ),
        ] {
            let server = FixtureServer::start(vec![
                Fixture::ok(Vec::new())
                    .header("ETag", etag)
                    .header("Content-Length", &length.to_string())
                    .header("Accept-Ranges", "bytes"),
            ]);
            let directory = tempdir().expect("directory");
            let partial = directory.path().join("file.part");
            fs::write(&partial, b"hello").expect("partial");
            let error = resume_single_stream(
                &HttpUrl::parse(server.url("/file")).expect("URL"),
                &partial,
                5,
                &identity("\"v1\"", 5),
                |_| {},
            )
            .expect_err("identity mismatch");
            assert!(matches!(error, ResumeError::IdentityChanged(found) if found == mismatch));
            assert_eq!(fs::read(&partial).expect("unchanged"), b"hello");
            assert_eq!(server.requests().len(), 1);
        }
    }

    #[test]
    fn server_without_range_support_requires_restart_without_appending() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("ETag", "\"v1\"")
                .header("Content-Length", "11"),
        ]);
        let directory = tempdir().expect("directory");
        let partial = directory.path().join("file.part");
        fs::write(&partial, b"hello").expect("partial");
        let error = resume_single_stream(
            &HttpUrl::parse(server.url("/file")).expect("URL"),
            &partial,
            5,
            &identity("\"v1\"", 11),
            |_| {},
        )
        .expect_err("restart required");
        assert!(matches!(error, ResumeError::RestartRequired));
        assert_eq!(fs::read(&partial).expect("unchanged"), b"hello");
        assert_eq!(server.requests().len(), 1);
    }
}
