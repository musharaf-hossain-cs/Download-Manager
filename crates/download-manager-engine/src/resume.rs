//! Identity-validated single-stream range resume.

use std::{
    fs::OpenOptions,
    io::{Read, Write},
    path::Path,
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
    probe::{HttpProbe, ProbeError},
    transfer::{TransferOutcome, TransferProgress},
};

const BUFFER_SIZE: usize = 64 * 1024;

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
    mut on_progress: F,
) -> Result<TransferOutcome, ResumeError>
where
    F: FnMut(TransferProgress),
{
    let probe = HttpProbe::new()?.probe(url)?;
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
    let client = Client::new();
    let mut request = client
        .get(probe.final_url.as_str())
        .header(RANGE, format!("bytes={durable_offset}-"));
    if let Some(validator) = expected_identity
        .etag
        .as_ref()
        .or(expected_identity.last_modified.as_ref())
    {
        request = request.header(IF_RANGE, validator);
    }
    let mut response = request.send()?;
    if response.status() != StatusCode::PARTIAL_CONTENT {
        return Err(ResumeError::UnexpectedStatus(response.status().as_u16()));
    }
    let content_range = response
        .headers()
        .get(CONTENT_RANGE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned);
    if !content_range.as_deref().is_some_and(|value| {
        valid_content_range(value, durable_offset, expected_identity.content_length)
    }) {
        return Err(ResumeError::InvalidContentRange {
            offset: durable_offset,
            value: content_range,
        });
    }
    let mut file = OpenOptions::new().append(true).open(partial_path)?;
    let mut downloaded_bytes = durable_offset;
    let mut buffer = vec![0_u8; BUFFER_SIZE];
    loop {
        let count = response.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        file.write_all(&buffer[..count])?;
        downloaded_bytes += u64::try_from(count).expect("buffer count fits u64");
        on_progress(TransferProgress {
            downloaded_bytes,
            total_bytes: expected_identity.content_length,
        });
    }
    file.flush()?;
    file.sync_all()?;
    if let Some(expected) = expected_identity.content_length
        && downloaded_bytes != expected
    {
        return Err(ResumeError::LengthMismatch {
            expected,
            actual: downloaded_bytes,
        });
    }
    Ok(TransferOutcome {
        downloaded_bytes,
        total_bytes: expected_identity.content_length,
        sha256: hash_file(partial_path)?,
    })
}

fn valid_content_range(value: &str, offset: u64, total: Option<u64>) -> bool {
    let Some(value) = value.strip_prefix("bytes ") else {
        return false;
    };
    let Some((range, represented_total)) = value.split_once('/') else {
        return false;
    };
    let Some((start, end)) = range.split_once('-') else {
        return false;
    };
    let Ok(start) = start.parse::<u64>() else {
        return false;
    };
    let Ok(end) = end.parse::<u64>() else {
        return false;
    };
    let parsed_total = represented_total.parse::<u64>().ok();
    start == offset && start <= end && total.is_none_or(|expected| parsed_total == Some(expected))
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
    use std::fs;

    use tempfile::tempdir;

    use super::{IdentityMismatch, ResumeError, resume_single_stream};
    use crate::{
        domain::{HttpUrl, RangeCapability, RemoteIdentity},
        test_support::{Fixture, FixtureServer},
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
