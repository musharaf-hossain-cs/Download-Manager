//! Versioned, non-secret settings and history export archives.

use std::{fs::OpenOptions, io::Write, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{domain::UnixMillis, redaction::redact_json_value};

pub const EXPORT_ARCHIVE_FORMAT_VERSION: u32 = 1;
const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;
const MAX_JSON_DEPTH: usize = 64;

/// Local state selected for export. Section schemas remain owned by their components.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportArchiveInput {
    pub exported_at: UnixMillis,
    pub settings: Value,
    pub categories: Value,
    pub queues: Value,
    pub history: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExportArchiveDocument {
    pub format_version: u32,
    pub exported_at: UnixMillis,
    pub payload_sha256: String,
    pub settings: Value,
    pub categories: Value,
    pub queues: Value,
    pub history: Value,
}

#[derive(Debug, Error)]
pub enum ExportArchiveError {
    #[error("export archive {section} section must be a JSON {expected}")]
    InvalidSection {
        section: &'static str,
        expected: &'static str,
    },
    #[error("export archive JSON exceeds the maximum depth of {maximum}")]
    JsonTooDeep { maximum: usize },
    #[error("serialized export archive is {actual} bytes; maximum is {maximum}")]
    ArchiveTooLarge { actual: usize, maximum: usize },
    #[error("export archive path must name a file")]
    InvalidExportPath,
    #[error("export archive serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("export archive filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Immutable sanitized document and the exact bytes represented by it.
#[derive(Debug, Clone)]
pub struct PreparedExportArchive {
    document: ExportArchiveDocument,
    bytes: Vec<u8>,
}

impl PreparedExportArchive {
    /// Validates section shapes and removes recognized secrets before serialization and hashing.
    ///
    /// # Errors
    /// Returns [`ExportArchiveError`] for invalid shapes, excessive nesting or size, or a
    /// serialization failure.
    pub fn prepare(input: &ExportArchiveInput) -> Result<Self, ExportArchiveError> {
        validate_section("settings", &input.settings, "object", Value::is_object)?;
        for (name, value) in [
            ("categories", &input.categories),
            ("queues", &input.queues),
            ("history", &input.history),
        ] {
            validate_section(name, value, "array", Value::is_array)?;
        }

        let settings = sanitized(&input.settings)?;
        let categories = sanitized(&input.categories)?;
        let queues = sanitized(&input.queues)?;
        let history = sanitized(&input.history)?;
        let payload = serde_json::to_vec(&(&settings, &categories, &queues, &history))?;
        let payload_sha256 = format!("{:x}", Sha256::digest(payload));
        let document = ExportArchiveDocument {
            format_version: EXPORT_ARCHIVE_FORMAT_VERSION,
            exported_at: input.exported_at,
            payload_sha256,
            settings,
            categories,
            queues,
            history,
        };
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err(ExportArchiveError::ArchiveTooLarge {
                actual: bytes.len(),
                maximum: MAX_ARCHIVE_BYTES,
            });
        }
        Ok(Self { document, bytes })
    }

    #[must_use]
    pub const fn preview(&self) -> &ExportArchiveDocument {
        &self.document
    }

    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Writes the prepared archive without replacing an existing file.
    ///
    /// # Errors
    /// Returns [`ExportArchiveError`] when the destination is invalid, already exists, or cannot
    /// be written and synchronized.
    pub fn export(&self, path: impl AsRef<Path>) -> Result<(), ExportArchiveError> {
        let path = path.as_ref();
        if path.file_name().is_none() {
            return Err(ExportArchiveError::InvalidExportPath);
        }
        let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
        file.write_all(&self.bytes)?;
        file.sync_all()?;
        Ok(())
    }
}

fn validate_section(
    section: &'static str,
    value: &Value,
    expected: &'static str,
    predicate: fn(&Value) -> bool,
) -> Result<(), ExportArchiveError> {
    if predicate(value) {
        Ok(())
    } else {
        Err(ExportArchiveError::InvalidSection { section, expected })
    }
}

fn sanitized(value: &Value) -> Result<Value, ExportArchiveError> {
    let mut pending = vec![(value, 0usize)];
    while let Some((current, depth)) = pending.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(ExportArchiveError::JsonTooDeep {
                maximum: MAX_JSON_DEPTH,
            });
        }
        match current {
            Value::Array(values) => pending.extend(values.iter().map(|value| (value, depth + 1))),
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)));
            }
            _ => {}
        }
    }
    Ok(redact_json_value(value))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{EXPORT_ARCHIVE_FORMAT_VERSION, ExportArchiveInput, PreparedExportArchive};
    use crate::domain::UnixMillis;

    fn input() -> ExportArchiveInput {
        ExportArchiveInput {
            exported_at: UnixMillis::new(1_798_646_400_000).expect("timestamp"),
            settings: json!({
                "general": {"theme": "dark", "access_token": "settings-secret"}
            }),
            categories: json!([{
                "id": "documents", "name": "Documents",
                "destination": "C:\\Downloads\\Documents"
            }]),
            queues: json!([{"name": "Night", "position": 0}]),
            history: json!([{
                "id": "018f47ce-8e51-7f31-a1ec-4a97f84a12cd",
                "url": "https://user:url-secret@files.test/a?token=query-secret&safe=kept",
                "filename": "a.zip", "state": "completed"
            }]),
        }
    }

    #[test]
    fn versioned_archive_is_deterministic_and_secret_free_before_disk() {
        let first = PreparedExportArchive::prepare(&input()).expect("archive");
        let second = PreparedExportArchive::prepare(&input()).expect("archive");
        assert_eq!(first.bytes(), second.bytes());
        assert_eq!(
            first.preview().format_version,
            EXPORT_ARCHIVE_FORMAT_VERSION
        );
        assert_eq!(first.preview().payload_sha256.len(), 64);

        let encoded = String::from_utf8(first.bytes().to_vec()).expect("UTF-8");
        for secret in ["settings-secret", "url-secret", "query-secret", "user"] {
            assert!(!encoded.contains(secret), "secret remained: {secret}");
        }
        assert!(encoded.contains("[REDACTED]"));
        assert!(encoded.contains("safe=kept"));

        let directory = tempdir().expect("directory");
        let path = directory.path().join("download-manager-export.json");
        first.export(&path).expect("export");
        assert_eq!(fs::read(&path).expect("archive on disk"), first.bytes());
        assert!(
            first.export(&path).is_err(),
            "existing archive must survive"
        );
        assert_eq!(fs::read(&path).expect("preserved archive"), first.bytes());
    }

    #[test]
    fn section_shapes_and_depth_are_bounded() {
        let mut invalid = input();
        invalid.history = json!({});
        assert!(PreparedExportArchive::prepare(&invalid).is_err());

        let mut nested = Value::Null;
        for _ in 0..=65 {
            nested = json!([nested]);
        }
        let mut too_deep = input();
        too_deep.categories = json!([nested]);
        assert!(PreparedExportArchive::prepare(&too_deep).is_err());
    }
}
