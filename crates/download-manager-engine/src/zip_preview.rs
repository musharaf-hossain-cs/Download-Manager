//! Bounded ZIP central-directory previews over validated HTTP byte ranges.

use std::{io::Read, time::Duration};

use reqwest::{
    StatusCode,
    blocking::Client,
    header::{CONTENT_RANGE, IF_RANGE, RANGE},
    redirect::Policy,
};
use thiserror::Error;

use crate::{
    domain::{HttpUrl, RangeCapability, RemoteIdentity},
    http_range::{ContentRange, parse_content_range},
};

const EOCD_MIN_SIZE: u64 = 22;
const MAX_ZIP_COMMENT: u64 = u16::MAX as u64;
const MAX_EOCD_SEARCH: u64 = EOCD_MIN_SIZE + MAX_ZIP_COMMENT;
const MAX_CENTRAL_DIRECTORY_SIZE: u64 = 4 * 1024 * 1024;
const MAX_ENTRIES: usize = 2_048;
const MAX_REDIRECTS: usize = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

const EOCD_SIGNATURE: &[u8; 4] = b"PK\x05\x06";
const CENTRAL_FILE_SIGNATURE: &[u8; 4] = b"PK\x01\x02";

/// Metadata shown for one file in a remote ZIP preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipPreviewEntry {
    pub path: String,
    pub compressed_size: u64,
    pub uncompressed_size: u64,
    pub compression_method: u16,
    pub encrypted: bool,
    pub directory: bool,
}

/// A bounded listing read from a classic, single-disk ZIP central directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ZipPreview {
    pub entries: Vec<ZipPreviewEntry>,
    pub archive_comment: String,
}

/// Fail-closed reasons why a remote archive cannot be previewed safely.
#[derive(Debug, Error)]
pub enum ZipPreviewError {
    #[error("ZIP preview requires a known representation length")]
    UnknownLength,
    #[error("ZIP preview requires confirmed HTTP byte-range support")]
    RangeUnsupported,
    #[error("remote object is too small to be a ZIP archive")]
    TooSmall,
    #[error("HTTP range request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("server did not honor the requested byte range (status {0})")]
    RangeNotHonored(u16),
    #[error("server returned invalid range metadata")]
    InvalidContentRange,
    #[error("server returned {actual} bytes for an expected {expected}-byte range")]
    RangeLengthMismatch { expected: u64, actual: usize },
    #[error("range response could not be read: {0}")]
    Read(#[from] std::io::Error),
    #[error("ZIP end-of-central-directory record was not found")]
    MissingEndRecord,
    #[error("multi-disk ZIP archives are not supported for preview")]
    MultiDiskUnsupported,
    #[error("ZIP64 archives are not supported for preview")]
    Zip64Unsupported,
    #[error("ZIP central directory exceeds the {MAX_CENTRAL_DIRECTORY_SIZE}-byte preview limit")]
    CentralDirectoryTooLarge,
    #[error("ZIP contains more than the {MAX_ENTRIES}-entry preview limit")]
    TooManyEntries,
    #[error("ZIP central directory is malformed")]
    MalformedCentralDirectory,
}

/// Blocking client for listing a remote ZIP without downloading the represented object.
#[derive(Debug, Clone)]
pub struct ZipPreviewClient {
    client: Client,
}

impl ZipPreviewClient {
    /// Builds a preview client with bounded redirects and request timeouts.
    ///
    /// # Errors
    /// Returns an error if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, ZipPreviewError> {
        Ok(Self {
            client: Client::builder()
                .redirect(Policy::limited(MAX_REDIRECTS))
                .timeout(REQUEST_TIMEOUT)
                .build()?,
        })
    }

    /// Lists a classic, single-disk ZIP central directory using bounded byte ranges.
    ///
    /// The caller must supply identity metadata obtained from the normal HTTP probe. Preview is
    /// refused unless the representation length is known and byte-range support was confirmed.
    /// ZIP64 and multi-disk archives are deliberately out of scope rather than partially parsed.
    ///
    /// # Errors
    /// Returns a fail-closed error for unavailable range support, an uncooperative origin, a
    /// malformed/unsupported ZIP layout, or a listing that exceeds the fixed preview limits.
    pub fn preview(
        &self,
        url: &HttpUrl,
        identity: &RemoteIdentity,
    ) -> Result<ZipPreview, ZipPreviewError> {
        if identity.range_capability != RangeCapability::Supported {
            return Err(ZipPreviewError::RangeUnsupported);
        }
        let length = identity
            .content_length
            .ok_or(ZipPreviewError::UnknownLength)?;
        if length < EOCD_MIN_SIZE {
            return Err(ZipPreviewError::TooSmall);
        }

        let tail_start = length.saturating_sub(MAX_EOCD_SEARCH);
        let tail = self.read_range(url, identity, tail_start, length - 1, length)?;
        let end = parse_end_record(&tail, tail_start, length)?;
        if end.central_size > MAX_CENTRAL_DIRECTORY_SIZE {
            return Err(ZipPreviewError::CentralDirectoryTooLarge);
        }
        if usize::from(end.entries) > MAX_ENTRIES {
            return Err(ZipPreviewError::TooManyEntries);
        }

        let central_end = end
            .central_offset
            .checked_add(end.central_size)
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        if central_end > end.absolute_offset {
            return Err(ZipPreviewError::MalformedCentralDirectory);
        }
        let central = if end.central_size == 0 {
            Vec::new()
        } else if end.central_offset >= tail_start && central_end <= length {
            let start = usize::try_from(end.central_offset - tail_start)
                .map_err(|_| ZipPreviewError::MalformedCentralDirectory)?;
            let finish = usize::try_from(central_end - tail_start)
                .map_err(|_| ZipPreviewError::MalformedCentralDirectory)?;
            tail.get(start..finish)
                .ok_or(ZipPreviewError::MalformedCentralDirectory)?
                .to_vec()
        } else {
            self.read_range(url, identity, end.central_offset, central_end - 1, length)?
        };

        Ok(ZipPreview {
            entries: parse_entries(&central, usize::from(end.entries))?,
            archive_comment: String::from_utf8_lossy(&end.comment).into_owned(),
        })
    }

    fn read_range(
        &self,
        url: &HttpUrl,
        identity: &RemoteIdentity,
        start: u64,
        end: u64,
        complete_length: u64,
    ) -> Result<Vec<u8>, ZipPreviewError> {
        let expected = end
            .checked_sub(start)
            .and_then(|span| span.checked_add(1))
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        let mut request = self
            .client
            .get(url.as_str())
            .header(RANGE, format!("bytes={start}-{end}"));
        if let Some(validator) = identity
            .etag
            .as_deref()
            .filter(|etag| !etag.starts_with("W/"))
            .or(identity.last_modified.as_deref())
        {
            request = request.header(IF_RANGE, validator);
        }
        let mut response = request.send()?;
        if response.status() != StatusCode::PARTIAL_CONTENT {
            return Err(ZipPreviewError::RangeNotHonored(response.status().as_u16()));
        }
        let range = response
            .headers()
            .get(CONTENT_RANGE)
            .and_then(|value| value.to_str().ok())
            .and_then(parse_content_range)
            .ok_or(ZipPreviewError::InvalidContentRange)?;
        if !matches!(
            range,
            ContentRange::Satisfied {
                start: actual_start,
                end: actual_end,
                complete_length: Some(actual_length),
            } if actual_start == start && actual_end == end && actual_length == complete_length
        ) {
            return Err(ZipPreviewError::InvalidContentRange);
        }

        let limit = expected
            .checked_add(1)
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        let mut bytes = Vec::new();
        response.by_ref().take(limit).read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).ok() != Some(expected) {
            return Err(ZipPreviewError::RangeLengthMismatch {
                expected,
                actual: bytes.len(),
            });
        }
        Ok(bytes)
    }
}

impl Default for ZipPreviewClient {
    fn default() -> Self {
        Self::new().expect("default ZIP preview client")
    }
}

#[derive(Debug)]
struct EndRecord {
    entries: u16,
    central_size: u64,
    central_offset: u64,
    absolute_offset: u64,
    comment: Vec<u8>,
}

fn parse_end_record(
    tail: &[u8],
    tail_start: u64,
    complete_length: u64,
) -> Result<EndRecord, ZipPreviewError> {
    let offset = tail
        .windows(4)
        .rposition(|window| window == EOCD_SIGNATURE)
        .ok_or(ZipPreviewError::MissingEndRecord)?;
    let fixed = tail
        .get(offset..offset + 22)
        .ok_or(ZipPreviewError::MissingEndRecord)?;
    let comment_length = usize::from(le_u16(fixed, 20)?);
    if offset + 22 + comment_length != tail.len() {
        return Err(ZipPreviewError::MissingEndRecord);
    }

    let disk = le_u16(fixed, 4)?;
    let central_disk = le_u16(fixed, 6)?;
    let disk_entries = le_u16(fixed, 8)?;
    let entries = le_u16(fixed, 10)?;
    if disk != 0 || central_disk != 0 || disk_entries != entries {
        return Err(ZipPreviewError::MultiDiskUnsupported);
    }
    let central_size = le_u32(fixed, 12)?;
    let central_offset = le_u32(fixed, 16)?;
    if entries == u16::MAX || central_size == u32::MAX || central_offset == u32::MAX {
        return Err(ZipPreviewError::Zip64Unsupported);
    }
    let absolute_offset = tail_start
        .checked_add(u64::try_from(offset).map_err(|_| ZipPreviewError::MalformedCentralDirectory)?)
        .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
    let comment_length_u64 =
        u64::try_from(comment_length).map_err(|_| ZipPreviewError::MalformedCentralDirectory)?;
    if absolute_offset
        .checked_add(EOCD_MIN_SIZE)
        .and_then(|value| value.checked_add(comment_length_u64))
        != Some(complete_length)
    {
        return Err(ZipPreviewError::MissingEndRecord);
    }
    Ok(EndRecord {
        entries,
        central_size: u64::from(central_size),
        central_offset: u64::from(central_offset),
        absolute_offset,
        comment: tail[offset + 22..].to_vec(),
    })
}

fn parse_entries(
    central: &[u8],
    expected_entries: usize,
) -> Result<Vec<ZipPreviewEntry>, ZipPreviewError> {
    let mut entries = Vec::with_capacity(expected_entries);
    let mut offset = 0_usize;
    while offset < central.len() {
        if entries.len() >= expected_entries {
            return Err(ZipPreviewError::MalformedCentralDirectory);
        }
        let fixed = central
            .get(offset..offset + 46)
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        if fixed.get(..4) != Some(CENTRAL_FILE_SIGNATURE) {
            return Err(ZipPreviewError::MalformedCentralDirectory);
        }
        let flags = le_u16(fixed, 8)?;
        let compression_method = le_u16(fixed, 10)?;
        let compressed_size = le_u32(fixed, 20)?;
        let uncompressed_size = le_u32(fixed, 24)?;
        let name_length = usize::from(le_u16(fixed, 28)?);
        let extra_length = usize::from(le_u16(fixed, 30)?);
        let comment_length = usize::from(le_u16(fixed, 32)?);
        let local_offset = le_u32(fixed, 42)?;
        if compressed_size == u32::MAX || uncompressed_size == u32::MAX || local_offset == u32::MAX
        {
            return Err(ZipPreviewError::Zip64Unsupported);
        }
        let record_length = 46_usize
            .checked_add(name_length)
            .and_then(|value| value.checked_add(extra_length))
            .and_then(|value| value.checked_add(comment_length))
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        let record = central
            .get(offset..offset + record_length)
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        let name = record
            .get(46..46 + name_length)
            .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
        let path = String::from_utf8_lossy(name).into_owned();
        entries.push(ZipPreviewEntry {
            directory: path.ends_with('/'),
            path,
            compressed_size: u64::from(compressed_size),
            uncompressed_size: u64::from(uncompressed_size),
            compression_method,
            encrypted: flags & 1 != 0,
        });
        offset += record_length;
    }
    if entries.len() != expected_entries {
        return Err(ZipPreviewError::MalformedCentralDirectory);
    }
    Ok(entries)
}

fn le_u16(bytes: &[u8], offset: usize) -> Result<u16, ZipPreviewError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn le_u32(bytes: &[u8], offset: usize) -> Result<u32, ZipPreviewError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(ZipPreviewError::MalformedCentralDirectory)?;
    Ok(u32::from_le_bytes([value[0], value[1], value[2], value[3]]))
}

#[cfg(test)]
mod tests {
    use super::{
        MAX_CENTRAL_DIRECTORY_SIZE, MAX_ENTRIES, MAX_EOCD_SEARCH, ZipPreviewClient, ZipPreviewError,
    };
    use crate::{
        domain::{HttpUrl, RangeCapability, RemoteIdentity},
        test_support::{Fixture, FixtureServer},
    };

    #[test]
    fn lists_entries_and_comment_from_one_bounded_tail_range() {
        let archive = classic_zip(
            &[("docs/", 0, 0, 0), ("docs/readme.txt", 8, 8, 10)],
            b"preview",
        );
        let end = archive.len() - 1;
        let server = FixtureServer::start(vec![
            Fixture::ok(archive.clone())
                .status(206)
                .header("Content-Range", &format!("bytes 0-{end}/{}", archive.len())),
        ]);
        let identity = ranged_identity(archive.len());

        let preview = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(server.url("/archive.zip")).expect("URL"),
                &identity,
            )
            .expect("preview");

        assert_eq!(preview.archive_comment, "preview");
        assert_eq!(preview.entries.len(), 2);
        assert!(preview.entries[0].directory);
        assert_eq!(preview.entries[1].path, "docs/readme.txt");
        assert_eq!(preview.entries[1].compressed_size, 8);
        assert_eq!(preview.entries[1].uncompressed_size, 10);
        assert_eq!(preview.entries[1].compression_method, 8);
        assert!(!preview.entries[1].encrypted);
        let requests = server.requests();
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0].header("range"),
            Some(format!("bytes=0-{end}").as_str())
        );
        assert_eq!(requests[0].header("if-range"), Some("\"zip-v1\""));
    }

    #[test]
    fn large_directory_uses_exact_bounded_tail_and_central_ranges() {
        let names = (0..1_200)
            .map(|index| format!("folder/{index:04}-{}.bin", "x".repeat(48)))
            .collect::<Vec<_>>();
        let entries = names
            .iter()
            .map(|name| (name.as_str(), 0, 1, 1))
            .collect::<Vec<_>>();
        let archive = classic_zip(&entries, b"");
        let central_size = archive.len() - 22;
        let tail_start = archive.len() - usize::try_from(MAX_EOCD_SEARCH).expect("search size");
        let tail_end = archive.len() - 1;
        let server = FixtureServer::start(vec![
            Fixture::ok(archive[tail_start..].to_vec())
                .status(206)
                .header(
                    "Content-Range",
                    &format!("bytes {tail_start}-{tail_end}/{}", archive.len()),
                ),
            Fixture::ok(archive[..central_size].to_vec())
                .status(206)
                .header(
                    "Content-Range",
                    &format!("bytes 0-{}/{}", central_size - 1, archive.len()),
                ),
        ]);

        let preview = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(server.url("/large.zip")).expect("URL"),
                &ranged_identity(archive.len()),
            )
            .expect("preview");

        assert_eq!(preview.entries.len(), entries.len());
        assert_eq!(preview.entries[0].path, names[0]);
        assert_eq!(preview.entries.last().expect("last").path, names[1_199]);
        let requests = server.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[0].header("range"),
            Some(format!("bytes={tail_start}-{tail_end}").as_str())
        );
        assert_eq!(
            requests[1].header("range"),
            Some(format!("bytes=0-{}", central_size - 1).as_str())
        );
    }

    #[test]
    fn refuses_unknown_or_unconfirmed_remote_layout_without_a_request() {
        let url = HttpUrl::parse("https://example.test/archive.zip").expect("URL");
        let client = ZipPreviewClient::new().expect("client");
        let mut identity = ranged_identity(100);
        identity.range_capability = RangeCapability::Unknown;
        assert!(matches!(
            client.preview(&url, &identity),
            Err(ZipPreviewError::RangeUnsupported)
        ));
        identity.range_capability = RangeCapability::Supported;
        identity.content_length = None;
        assert!(matches!(
            client.preview(&url, &identity),
            Err(ZipPreviewError::UnknownLength)
        ));
    }

    #[test]
    fn rejects_an_origin_that_ignores_the_range_header() {
        let archive = classic_zip(&[("file.txt", 0, 1, 1)], b"");
        let server = FixtureServer::start(vec![Fixture::ok(archive.clone())]);
        let error = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(server.url("/archive.zip")).expect("URL"),
                &ranged_identity(archive.len()),
            )
            .expect_err("range must be honored");
        assert!(matches!(error, ZipPreviewError::RangeNotHonored(200)));
    }

    #[test]
    fn rejects_zip64_and_malformed_central_directory_metadata() {
        let mut zip64 = classic_zip(&[("file.txt", 0, 1, 1)], b"");
        let eocd = zip64.len() - 22;
        zip64[eocd + 16..eocd + 20].copy_from_slice(&u32::MAX.to_le_bytes());
        let zip64_end = zip64.len() - 1;
        let zip64_server =
            FixtureServer::start(vec![Fixture::ok(zip64.clone()).status(206).header(
                "Content-Range",
                &format!("bytes 0-{zip64_end}/{}", zip64.len()),
            )]);
        let error = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(zip64_server.url("/archive.zip")).expect("URL"),
                &ranged_identity(zip64.len()),
            )
            .expect_err("ZIP64 is out of scope");
        assert!(matches!(error, ZipPreviewError::Zip64Unsupported));

        let mut malformed = classic_zip(&[("file.txt", 0, 1, 1)], b"");
        malformed[0..4].copy_from_slice(b"NOPE");
        let malformed_end = malformed.len() - 1;
        let malformed_server =
            FixtureServer::start(vec![Fixture::ok(malformed.clone()).status(206).header(
                "Content-Range",
                &format!("bytes 0-{malformed_end}/{}", malformed.len()),
            )]);
        let error = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(malformed_server.url("/archive.zip")).expect("URL"),
                &ranged_identity(malformed.len()),
            )
            .expect_err("bad central signature");
        assert!(matches!(error, ZipPreviewError::MalformedCentralDirectory));
    }

    #[test]
    fn rejects_declared_directory_size_and_entry_count_above_limits() {
        let mut oversized = classic_zip(&[], b"");
        oversized[12..16].copy_from_slice(
            &u32::try_from(MAX_CENTRAL_DIRECTORY_SIZE + 1)
                .expect("limit fits u32")
                .to_le_bytes(),
        );
        let oversized_server = ranged_server(oversized.clone());
        let error = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(oversized_server.url("/oversized.zip")).expect("URL"),
                &ranged_identity(oversized.len()),
            )
            .expect_err("directory limit");
        assert!(matches!(error, ZipPreviewError::CentralDirectoryTooLarge));

        let mut excessive_entries = classic_zip(&[], b"");
        let count = u16::try_from(MAX_ENTRIES + 1).expect("entry limit fits u16");
        excessive_entries[8..10].copy_from_slice(&count.to_le_bytes());
        excessive_entries[10..12].copy_from_slice(&count.to_le_bytes());
        let entries_server = ranged_server(excessive_entries.clone());
        let error = ZipPreviewClient::new()
            .expect("client")
            .preview(
                &HttpUrl::parse(entries_server.url("/entries.zip")).expect("URL"),
                &ranged_identity(excessive_entries.len()),
            )
            .expect_err("entry limit");
        assert!(matches!(error, ZipPreviewError::TooManyEntries));
    }

    fn ranged_identity(length: usize) -> RemoteIdentity {
        RemoteIdentity {
            etag: Some("\"zip-v1\"".to_owned()),
            last_modified: None,
            content_length: Some(u64::try_from(length).expect("fixture length")),
            mime_type: Some("application/zip".to_owned()),
            content_fingerprint: None,
            range_capability: RangeCapability::Supported,
        }
    }

    fn ranged_server(body: Vec<u8>) -> FixtureServer {
        let end = body.len() - 1;
        let length = body.len();
        FixtureServer::start(vec![
            Fixture::ok(body)
                .status(206)
                .header("Content-Range", &format!("bytes 0-{end}/{length}")),
        ])
    }

    fn classic_zip(entries: &[(&str, u16, u32, u32)], comment: &[u8]) -> Vec<u8> {
        let mut central = Vec::new();
        for (name, method, compressed, uncompressed) in entries {
            central.extend_from_slice(b"PK\x01\x02");
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&20_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&compressed.to_le_bytes());
            central.extend_from_slice(&uncompressed.to_le_bytes());
            central.extend_from_slice(
                &u16::try_from(name.len())
                    .expect("fixture name length")
                    .to_le_bytes(),
            );
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u16.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(&0_u32.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let central_size = u32::try_from(central.len()).expect("fixture central size");
        let count = u16::try_from(entries.len()).expect("fixture entry count");
        central.extend_from_slice(b"PK\x05\x06");
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&0_u16.to_le_bytes());
        central.extend_from_slice(&count.to_le_bytes());
        central.extend_from_slice(&count.to_le_bytes());
        central.extend_from_slice(&central_size.to_le_bytes());
        central.extend_from_slice(&0_u32.to_le_bytes());
        central.extend_from_slice(
            &u16::try_from(comment.len())
                .expect("fixture comment length")
                .to_le_bytes(),
        );
        central.extend_from_slice(comment);
        central
    }
}
