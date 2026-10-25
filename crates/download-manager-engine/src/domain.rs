//! Validated domain values shared by persistence and process boundaries.

use std::{fmt, path::PathBuf, str::FromStr};

use serde::{Deserialize, Serialize};
use thiserror::Error;
use url::Url;
use uuid::Uuid;

use crate::state::DownloadState;

/// Domain-level validation failures.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ValidationError {
    #[error("{field} must not be empty")]
    Empty { field: &'static str },
    #[error("{field} exceeds the maximum length of {maximum}")]
    TooLong { field: &'static str, maximum: usize },
    #[error("invalid UUID: {0}")]
    InvalidUuid(String),
    #[error("timestamp must not be negative")]
    NegativeTimestamp,
    #[error("URL is invalid: {0}")]
    InvalidUrl(String),
    #[error("only HTTP and HTTPS URLs are supported")]
    UnsupportedUrlScheme,
    #[error("URL must include a host")]
    MissingUrlHost,
    #[error("downloaded bytes cannot exceed total bytes")]
    DownloadedExceedsTotal,
    #[error("segment range or offset is invalid")]
    InvalidSegmentRange,
}

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }

            #[must_use]
            pub const fn from_uuid(value: Uuid) -> Self {
                Self(value)
            }

            #[must_use]
            pub const fn as_uuid(self) -> Uuid {
                self.0
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }

        impl FromStr for $name {
            type Err = ValidationError;

            fn from_str(value: &str) -> Result<Self, Self::Err> {
                Uuid::parse_str(value)
                    .map(Self)
                    .map_err(|_| ValidationError::InvalidUuid(value.to_owned()))
            }
        }
    };
}

id_type!(JobId);
id_type!(SegmentId);
id_type!(EventId);
id_type!(RequestId);

/// Milliseconds since the Unix epoch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "i64", into = "i64")]
pub struct UnixMillis(i64);

impl UnixMillis {
    /// Creates a non-negative timestamp.
    ///
    /// # Errors
    /// Returns [`ValidationError::NegativeTimestamp`] for a negative value.
    pub fn new(value: i64) -> Result<Self, ValidationError> {
        if value < 0 {
            Err(ValidationError::NegativeTimestamp)
        } else {
            Ok(Self(value))
        }
    }

    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl TryFrom<i64> for UnixMillis {
    type Error = ValidationError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<UnixMillis> for i64 {
    fn from(value: UnixMillis) -> Self {
        value.0
    }
}

/// An absolute, host-bearing HTTP(S) URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct HttpUrl(String);

impl HttpUrl {
    /// Parses and validates an absolute HTTP(S) URL.
    ///
    /// # Errors
    /// Returns a validation error for an oversized, malformed, hostless, or unsupported URL.
    pub fn parse(value: impl Into<String>) -> Result<Self, ValidationError> {
        let value = value.into();
        if value.len() > 8_192 {
            return Err(ValidationError::TooLong {
                field: "URL",
                maximum: 8_192,
            });
        }
        let parsed =
            Url::parse(&value).map_err(|error| ValidationError::InvalidUrl(error.to_string()))?;
        if !matches!(parsed.scheme(), "http" | "https") {
            return Err(ValidationError::UnsupportedUrlScheme);
        }
        if parsed.host().is_none() {
            return Err(ValidationError::MissingUrlHost);
        }
        Ok(Self(parsed.into()))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for HttpUrl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

impl From<HttpUrl> for String {
    fn from(value: HttpUrl) -> Self {
        value.0
    }
}

impl TryFrom<String> for HttpUrl {
    type Error = ValidationError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(value)
    }
}

impl FromStr for HttpUrl {
    type Err = ValidationError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::parse(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloadKind {
    File,
    Media,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    Low,
    Normal,
    High,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RangeCapability {
    Unknown,
    Supported,
    Unsupported,
}

/// Non-secret metadata for a durable download job.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobMetadata {
    pub id: JobId,
    pub source_url: HttpUrl,
    pub final_url: Option<HttpUrl>,
    pub display_url: String,
    pub filename: String,
    pub destination: PathBuf,
    pub state: DownloadState,
    pub kind: DownloadKind,
    pub total_bytes: Option<u64>,
    pub downloaded_bytes: u64,
    pub category: Option<String>,
    pub queue: Option<String>,
    pub priority: Priority,
    pub source_browser: Option<String>,
    pub source_profile: Option<String>,
    pub retry_count: u32,
    pub error: Option<String>,
    pub created_at: UnixMillis,
    pub updated_at: UnixMillis,
    pub completed_at: Option<UnixMillis>,
}

impl JobMetadata {
    /// Checks cross-field and bounded-text invariants.
    ///
    /// # Errors
    /// Returns a validation error when any invariant is violated.
    pub fn validate(&self) -> Result<(), ValidationError> {
        validate_text("filename", &self.filename, 1_024)?;
        validate_text("display_url", &self.display_url, 8_192)?;
        if let Some(total) = self.total_bytes
            && self.downloaded_bytes > total
        {
            return Err(ValidationError::DownloadedExceedsTotal);
        }
        if self.updated_at < self.created_at {
            return Err(ValidationError::NegativeTimestamp);
        }
        for (field, value, maximum) in [
            ("category", self.category.as_deref(), 256),
            ("queue", self.queue.as_deref(), 256),
            ("source_browser", self.source_browser.as_deref(), 128),
            ("source_profile", self.source_profile.as_deref(), 256),
            ("error", self.error.as_deref(), 4_096),
        ] {
            if let Some(value) = value {
                validate_text(field, value, maximum)?;
            }
        }
        Ok(())
    }
}

/// Validators used to prove a partial file still belongs to the same remote object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoteIdentity {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: Option<u64>,
    pub mime_type: Option<String>,
    pub content_fingerprint: Option<String>,
    pub range_capability: RangeCapability,
}

impl RemoteIdentity {
    /// Checks that optional remote validators are non-empty and bounded.
    ///
    /// # Errors
    /// Returns a validation error for an invalid validator value.
    pub fn validate(&self) -> Result<(), ValidationError> {
        for (field, value, maximum) in [
            ("etag", self.etag.as_deref(), 1_024),
            ("last_modified", self.last_modified.as_deref(), 256),
            ("mime_type", self.mime_type.as_deref(), 256),
            (
                "content_fingerprint",
                self.content_fingerprint.as_deref(),
                1_024,
            ),
        ] {
            if let Some(value) = value {
                validate_text(field, value, maximum)?;
            }
        }
        Ok(())
    }
}

/// Inclusive segment bounds plus the next byte that has not been durably written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Segment {
    pub id: SegmentId,
    pub job_id: JobId,
    pub start: u64,
    pub end: u64,
    pub durable_offset: u64,
    pub worker_state: String,
    pub checksum: Option<String>,
    pub checkpoint_at: UnixMillis,
}

impl Segment {
    /// Checks inclusive range and durable-offset invariants.
    ///
    /// # Errors
    /// Returns a validation error for an invalid range, offset, or text field.
    pub fn validate(&self) -> Result<(), ValidationError> {
        let after_end = self
            .end
            .checked_add(1)
            .ok_or(ValidationError::InvalidSegmentRange)?;
        if self.start > self.end
            || self.durable_offset < self.start
            || self.durable_offset > after_end
        {
            return Err(ValidationError::InvalidSegmentRange);
        }
        validate_text("worker_state", &self.worker_state, 64)?;
        if let Some(checksum) = &self.checksum {
            validate_text("checksum", checksum, 1_024)?;
        }
        Ok(())
    }
}

fn validate_text(field: &'static str, value: &str, maximum: usize) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        return Err(ValidationError::Empty { field });
    }
    if value.len() > maximum {
        return Err(ValidationError::TooLong { field, maximum });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{HttpUrl, JobId, Segment, SegmentId, UnixMillis, ValidationError};

    #[test]
    fn ids_serialize_as_stable_uuid_strings() {
        let id: JobId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd"
            .parse()
            .expect("valid ID");
        assert_eq!(
            serde_json::to_string(&id).expect("serialize"),
            "\"018f47ce-8e51-7f31-a1ec-4a97f84a12cd\""
        );
        assert!("not-an-id".parse::<JobId>().is_err());
    }

    #[test]
    fn http_url_rejects_untrusted_schemes_and_missing_hosts() {
        assert!(HttpUrl::parse("https://example.test/file").is_ok());
        assert_eq!(
            HttpUrl::parse("file:///windows/system.ini"),
            Err(ValidationError::UnsupportedUrlScheme)
        );
        assert!(HttpUrl::parse("https://?query=missing-host").is_err());
    }

    #[test]
    fn timestamp_and_segment_invariants_are_enforced() {
        assert_eq!(UnixMillis::new(-1), Err(ValidationError::NegativeTimestamp));
        let segment = Segment {
            id: SegmentId::new(),
            job_id: JobId::new(),
            start: 10,
            end: 19,
            durable_offset: 20,
            worker_state: "idle".to_owned(),
            checksum: None,
            checkpoint_at: UnixMillis::new(1).expect("timestamp"),
        };
        assert!(segment.validate().is_ok());
        assert_eq!(
            Segment {
                durable_offset: 21,
                ..segment
            }
            .validate(),
            Err(ValidationError::InvalidSegmentRange)
        );
    }
}
