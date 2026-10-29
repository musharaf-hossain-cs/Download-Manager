//! Deterministic, user-previewable diagnostics and health bundles.

use std::{
    collections::{BTreeMap, HashSet},
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::{
    domain::UnixMillis,
    redaction::{redact_json_value, redact_secrets},
    structured_log::{MAX_LOG_RECORD_BYTES, StructuredLogEntry},
};

const FORMAT_VERSION: u32 = 1;
const MAX_COMPONENTS: usize = 32;
const MAX_SETTINGS_SECTIONS: usize = 128;
const MAX_SETTINGS_FIELDS: usize = 2_048;
const MAX_CRASH_IDS: usize = 256;
const MAX_CONNECTIVITY_CHECKS: usize = 128;
const MAX_LOG_RECORDS: usize = 10_000;
const MAX_SHORT_TEXT_BYTES: usize = 256;
const MAX_DETAIL_BYTES: usize = 16 * 1024;
const MAX_LOG_COMPONENT_BYTES: usize = 64;
const MAX_LOG_EVENT_TYPE_BYTES: usize = 128;
const MAX_JSON_DEPTH: usize = 64;
const MAX_BUNDLE_BYTES: usize = 32 * 1024 * 1024;
const MAX_STAGING_ATTEMPTS: usize = 16;

/// Role of one versioned component included in a health bundle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsComponentRole {
    Application,
    Engine,
    NativeHost,
    Extension,
    Browser,
}

/// A component identity shown in the preview and exported bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsComponentVersion {
    pub role: DiagnosticsComponentRole,
    pub name: String,
    pub version: String,
}

/// Settings metadata only. Settings values and credentials are intentionally unsupported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsSettingsSchema {
    pub version: u32,
    pub fields_by_section: BTreeMap<String, Vec<String>>,
}

/// Result of one local connectivity or integration health check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticsCheckStatus {
    Pass,
    Fail,
    Blocked,
}

/// Display-ready connectivity check included in the preview and bundle.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsConnectivityCheck {
    pub id: String,
    pub title: String,
    pub status: DiagnosticsCheckStatus,
    pub detail: String,
}

/// Typed inputs collected locally before preparing a bundle.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosticsBundleInput {
    pub generated_at: UnixMillis,
    pub components: Vec<DiagnosticsComponentVersion>,
    pub settings_schema: DiagnosticsSettingsSchema,
    pub crash_ids: Vec<String>,
    pub connectivity_checks: Vec<DiagnosticsConnectivityCheck>,
    pub logs: Vec<StructuredLogEntry>,
}

/// Complete sanitized document presented to the user before export.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiagnosticsBundleDocument {
    pub format_version: u32,
    pub generated_at: UnixMillis,
    pub components: Vec<DiagnosticsComponentVersion>,
    pub settings_schema: DiagnosticsSettingsSchema,
    pub crash_ids: Vec<String>,
    pub connectivity_checks: Vec<DiagnosticsConnectivityCheck>,
    pub logs: Vec<StructuredLogEntry>,
}

/// Preparation or export failure. Invalid input is rejected before an output file is opened.
#[derive(Debug, Error)]
pub enum DiagnosticsBundleError {
    #[error("diagnostics {section} contains {actual} items; maximum is {maximum}")]
    TooManyItems {
        section: &'static str,
        actual: usize,
        maximum: usize,
    },
    #[error("diagnostics {field} must contain 1..={maximum} bytes")]
    InvalidText { field: &'static str, maximum: usize },
    #[error("diagnostics crash ID must be a short opaque identifier")]
    InvalidCrashId,
    #[error("diagnostics settings schema version must be greater than zero")]
    InvalidSettingsSchemaVersion,
    #[error("diagnostics bundle requires an application and at least one browser version")]
    MissingRequiredVersion,
    #[error("duplicate diagnostics component identity")]
    DuplicateComponent,
    #[error("duplicate diagnostics connectivity check ID")]
    DuplicateConnectivityCheck,
    #[error("duplicate diagnostics settings section after sanitization")]
    DuplicateSettingsSection,
    #[error("serialized diagnostics bundle is {actual} bytes; maximum is {maximum}")]
    BundleTooLarge { actual: usize, maximum: usize },
    #[error("diagnostics log record is {actual} estimated bytes; maximum is {maximum}")]
    LogRecordTooLarge { actual: usize, maximum: usize },
    #[error("diagnostics log detail exceeds the maximum JSON depth of {maximum}")]
    LogDetailTooDeep { maximum: usize },
    #[error("diagnostics export path must name a file")]
    InvalidExportPath,
    #[error("diagnostics bundle serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("diagnostics bundle filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Immutable sanitized document plus the exact bytes that may be exported after preview.
#[derive(Debug, Clone)]
pub struct PreparedDiagnosticsBundle {
    document: DiagnosticsBundleDocument,
    bytes: Vec<u8>,
}

impl PreparedDiagnosticsBundle {
    /// Validates, normalizes, and redacts a local health snapshot.
    ///
    /// # Errors
    /// Returns [`DiagnosticsBundleError`] when required data is missing, duplicated, invalid, or
    /// exceeds a bundle bound.
    pub fn prepare(input: DiagnosticsBundleInput) -> Result<Self, DiagnosticsBundleError> {
        validate_counts(&input)?;
        let components = normalize_components(input.components)?;
        if !components
            .iter()
            .any(|component| component.role == DiagnosticsComponentRole::Application)
            || !components
                .iter()
                .any(|component| component.role == DiagnosticsComponentRole::Browser)
        {
            return Err(DiagnosticsBundleError::MissingRequiredVersion);
        }
        let settings_schema = normalize_settings_schema(input.settings_schema)?;
        let crash_ids = normalize_crash_ids(input.crash_ids)?;
        let connectivity_checks = normalize_checks(input.connectivity_checks)?;
        let logs = normalize_logs(input.logs)?;
        let document = DiagnosticsBundleDocument {
            format_version: FORMAT_VERSION,
            generated_at: input.generated_at,
            components,
            settings_schema,
            crash_ids,
            connectivity_checks,
            logs,
        };
        let mut bytes = serde_json::to_vec_pretty(&document)?;
        bytes.push(b'\n');
        if bytes.len() > MAX_BUNDLE_BYTES {
            return Err(DiagnosticsBundleError::BundleTooLarge {
                actual: bytes.len(),
                maximum: MAX_BUNDLE_BYTES,
            });
        }
        Ok(Self { document, bytes })
    }

    /// Returns the complete sanitized payload for explicit user review.
    #[must_use]
    pub const fn preview(&self) -> &DiagnosticsBundleDocument {
        &self.document
    }

    /// Returns the exact bytes represented by [`Self::preview`].
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }

    /// Exports the already-previewed bytes without overwriting an existing destination.
    ///
    /// # Errors
    /// Returns [`DiagnosticsBundleError::Io`] if the destination exists or cannot be written.
    pub fn export(&self, path: impl AsRef<Path>) -> Result<(), DiagnosticsBundleError> {
        let path = path.as_ref();
        if path.file_name().is_none() {
            return Err(DiagnosticsBundleError::InvalidExportPath);
        }
        let (staging_path, mut file) = create_staging_file(path)?;
        if let Err(error) = file.write_all(&self.bytes).and_then(|()| file.sync_all()) {
            drop(file);
            let _ = fs::remove_file(&staging_path);
            return Err(error.into());
        }
        drop(file);
        if let Err(error) = commit_staging_file(&staging_path, path) {
            let _ = fs::remove_file(&staging_path);
            if path.exists() {
                return Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into());
            }
            return Err(error.into());
        }
        Ok(())
    }
}

fn validate_counts(input: &DiagnosticsBundleInput) -> Result<(), DiagnosticsBundleError> {
    check_count("components", input.components.len(), MAX_COMPONENTS)?;
    check_count("crash IDs", input.crash_ids.len(), MAX_CRASH_IDS)?;
    check_count(
        "connectivity checks",
        input.connectivity_checks.len(),
        MAX_CONNECTIVITY_CHECKS,
    )?;
    check_count("log records", input.logs.len(), MAX_LOG_RECORDS)?;
    validate_log_bounds(&input.logs)
}

fn check_count(
    section: &'static str,
    actual: usize,
    maximum: usize,
) -> Result<(), DiagnosticsBundleError> {
    if actual > maximum {
        Err(DiagnosticsBundleError::TooManyItems {
            section,
            actual,
            maximum,
        })
    } else {
        Ok(())
    }
}

fn normalize_components(
    components: Vec<DiagnosticsComponentVersion>,
) -> Result<Vec<DiagnosticsComponentVersion>, DiagnosticsBundleError> {
    let mut components = components
        .into_iter()
        .map(|component| {
            Ok(DiagnosticsComponentVersion {
                role: component.role,
                name: clean_text("component name", &component.name, MAX_SHORT_TEXT_BYTES)?,
                version: clean_text(
                    "component version",
                    &component.version,
                    MAX_SHORT_TEXT_BYTES,
                )?,
            })
        })
        .collect::<Result<Vec<_>, DiagnosticsBundleError>>()?;
    components.sort_by(|left, right| {
        (&left.role, &left.name, &left.version).cmp(&(&right.role, &right.name, &right.version))
    });
    let mut identities = HashSet::new();
    if components
        .iter()
        .any(|component| !identities.insert((component.role, component.name.clone())))
    {
        return Err(DiagnosticsBundleError::DuplicateComponent);
    }
    Ok(components)
}

fn normalize_settings_schema(
    schema: DiagnosticsSettingsSchema,
) -> Result<DiagnosticsSettingsSchema, DiagnosticsBundleError> {
    if schema.version == 0 {
        return Err(DiagnosticsBundleError::InvalidSettingsSchemaVersion);
    }
    check_count(
        "settings sections",
        schema.fields_by_section.len(),
        MAX_SETTINGS_SECTIONS,
    )?;
    let field_count = schema.fields_by_section.values().map(Vec::len).sum();
    check_count("settings fields", field_count, MAX_SETTINGS_FIELDS)?;
    let mut fields_by_section = BTreeMap::new();
    for (section, fields) in schema.fields_by_section {
        let section = clean_text("settings section", &section, MAX_SHORT_TEXT_BYTES)?;
        let mut fields = fields
            .into_iter()
            .map(|field| clean_text("settings field", &field, MAX_SHORT_TEXT_BYTES))
            .collect::<Result<Vec<_>, _>>()?;
        fields.sort();
        fields.dedup();
        if fields_by_section.insert(section, fields).is_some() {
            return Err(DiagnosticsBundleError::DuplicateSettingsSection);
        }
    }
    Ok(DiagnosticsSettingsSchema {
        version: schema.version,
        fields_by_section,
    })
}

fn normalize_crash_ids(mut crash_ids: Vec<String>) -> Result<Vec<String>, DiagnosticsBundleError> {
    if crash_ids.iter().any(|id| {
        id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    }) {
        return Err(DiagnosticsBundleError::InvalidCrashId);
    }
    crash_ids.sort();
    crash_ids.dedup();
    Ok(crash_ids)
}

fn normalize_checks(
    checks: Vec<DiagnosticsConnectivityCheck>,
) -> Result<Vec<DiagnosticsConnectivityCheck>, DiagnosticsBundleError> {
    let mut checks = checks
        .into_iter()
        .map(|check| {
            if check.id.is_empty()
                || check.id.len() > 128
                || !check.id.bytes().all(|byte| {
                    byte.is_ascii_lowercase()
                        || byte.is_ascii_digit()
                        || matches!(byte, b'.' | b'_' | b'-')
                })
            {
                return Err(DiagnosticsBundleError::InvalidText {
                    field: "connectivity check ID",
                    maximum: 128,
                });
            }
            Ok(DiagnosticsConnectivityCheck {
                id: check.id,
                title: clean_text("connectivity title", &check.title, MAX_SHORT_TEXT_BYTES)?,
                status: check.status,
                detail: clean_text("connectivity detail", &check.detail, MAX_DETAIL_BYTES)?,
            })
        })
        .collect::<Result<Vec<_>, DiagnosticsBundleError>>()?;
    checks.sort_by(|left, right| left.id.cmp(&right.id));
    if checks.windows(2).any(|pair| pair[0].id == pair[1].id) {
        return Err(DiagnosticsBundleError::DuplicateConnectivityCheck);
    }
    Ok(checks)
}

fn normalize_logs(
    logs: Vec<StructuredLogEntry>,
) -> Result<Vec<StructuredLogEntry>, DiagnosticsBundleError> {
    let mut keyed = logs
        .into_iter()
        .map(|mut entry| {
            entry.component =
                clean_text("log component", &entry.component, MAX_LOG_COMPONENT_BYTES)?;
            entry.event_type = clean_text(
                "log event type",
                &entry.event_type,
                MAX_LOG_EVENT_TYPE_BYTES,
            )?;
            entry.detail = redact_json_value(&entry.detail);
            let key = (
                entry.timestamp,
                entry.correlation_id.to_string(),
                entry.component.clone(),
                entry.event_type.clone(),
                serde_json::to_string(&entry)?,
            );
            Ok((key, entry))
        })
        .collect::<Result<Vec<_>, DiagnosticsBundleError>>()?;
    keyed.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(keyed.into_iter().map(|(_, entry)| entry).collect())
}

fn validate_log_bounds(logs: &[StructuredLogEntry]) -> Result<(), DiagnosticsBundleError> {
    let mut bundle_estimate = 0usize;
    for entry in logs {
        let detail_estimate = estimated_json_bytes(&entry.detail)?;
        let record_estimate = detail_estimate
            .saturating_add(entry.component.len().saturating_mul(6))
            .saturating_add(entry.event_type.len().saturating_mul(6))
            .saturating_add(512);
        if record_estimate > MAX_LOG_RECORD_BYTES {
            return Err(DiagnosticsBundleError::LogRecordTooLarge {
                actual: record_estimate,
                maximum: MAX_LOG_RECORD_BYTES,
            });
        }
        bundle_estimate = bundle_estimate.saturating_add(record_estimate);
        if bundle_estimate > MAX_BUNDLE_BYTES {
            return Err(DiagnosticsBundleError::BundleTooLarge {
                actual: bundle_estimate,
                maximum: MAX_BUNDLE_BYTES,
            });
        }
    }
    Ok(())
}

fn estimated_json_bytes(value: &Value) -> Result<usize, DiagnosticsBundleError> {
    let mut estimate = 0usize;
    let mut pending = vec![(value, 0usize)];
    while let Some((value, depth)) = pending.pop() {
        if depth > MAX_JSON_DEPTH {
            return Err(DiagnosticsBundleError::LogDetailTooDeep {
                maximum: MAX_JSON_DEPTH,
            });
        }
        estimate = estimate.saturating_add(match value {
            Value::Null | Value::Bool(_) | Value::Number(_) => 32,
            Value::String(value) => value.len().saturating_mul(6).saturating_add(2),
            Value::Array(values) => {
                pending.extend(values.iter().map(|value| (value, depth + 1)));
                values.len().saturating_add(2)
            }
            Value::Object(values) => {
                pending.extend(values.values().map(|value| (value, depth + 1)));
                values
                    .keys()
                    .map(|key| key.len().saturating_mul(6).saturating_add(4))
                    .sum::<usize>()
                    .saturating_add(2)
            }
        });
    }
    Ok(estimate)
}

fn create_staging_file(path: &Path) -> Result<(PathBuf, File), DiagnosticsBundleError> {
    for attempt in 0..MAX_STAGING_ATTEMPTS {
        let mut name = OsString::from(".");
        name.push(
            path.file_name()
                .ok_or(DiagnosticsBundleError::InvalidExportPath)?,
        );
        name.push(format!(".{}.{attempt}.tmp", std::process::id()));
        let staging_path = path.with_file_name(name);
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&staging_path)
        {
            Ok(file) => return Ok((staging_path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error.into()),
        }
    }
    Err(std::io::Error::from(std::io::ErrorKind::AlreadyExists).into())
}

fn commit_staging_file(staging_path: &Path, path: &Path) -> Result<(), std::io::Error> {
    fs::hard_link(staging_path, path)?;
    fs::remove_file(staging_path)
}

fn clean_text(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<String, DiagnosticsBundleError> {
    let value = redact_secrets(value);
    if value.is_empty() || value.len() > maximum {
        Err(DiagnosticsBundleError::InvalidText { field, maximum })
    } else {
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use std::{collections::BTreeMap, fs};

    use serde_json::json;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::{
        DiagnosticsBundleDocument, DiagnosticsBundleError, DiagnosticsBundleInput,
        DiagnosticsCheckStatus, DiagnosticsComponentRole, DiagnosticsComponentVersion,
        DiagnosticsConnectivityCheck, DiagnosticsSettingsSchema, PreparedDiagnosticsBundle,
    };
    use crate::{
        domain::{RequestId, UnixMillis},
        structured_log::{LogSeverity, MAX_LOG_RECORD_BYTES, StructuredLogEntry},
    };

    fn input() -> DiagnosticsBundleInput {
        DiagnosticsBundleInput {
            generated_at: UnixMillis::new(1_798_000_000_000).expect("timestamp"),
            components: vec![
                DiagnosticsComponentVersion {
                    role: DiagnosticsComponentRole::Browser,
                    name: "Microsoft Edge".to_owned(),
                    version: "141.0.0".to_owned(),
                },
                DiagnosticsComponentVersion {
                    role: DiagnosticsComponentRole::Application,
                    name: "Download Manager".to_owned(),
                    version: "0.1.0".to_owned(),
                },
            ],
            settings_schema: DiagnosticsSettingsSchema {
                version: 4,
                fields_by_section: BTreeMap::from([
                    ("network".to_owned(), vec!["proxyMode".to_owned()]),
                    (
                        "downloads".to_owned(),
                        vec!["destination".to_owned(), "conflictPolicy".to_owned()],
                    ),
                ]),
            },
            crash_ids: vec!["crash-b".to_owned(), "crash-a".to_owned()],
            connectivity_checks: vec![
                DiagnosticsConnectivityCheck {
                    id: "network.dns".to_owned(),
                    title: "DNS resolution".to_owned(),
                    status: DiagnosticsCheckStatus::Pass,
                    detail: "Resolved files.test".to_owned(),
                },
                DiagnosticsConnectivityCheck {
                    id: "browser.native_host".to_owned(),
                    title: "Native host".to_owned(),
                    status: DiagnosticsCheckStatus::Blocked,
                    detail: "Browser is closed".to_owned(),
                },
            ],
            logs: vec![
                log_entry(2, "request_complete", json!({ "result": "ok" })),
                log_entry(1, "request_started", json!({ "method": "GET" })),
            ],
        }
    }

    fn log_entry(sequence: i64, event_type: &str, detail: serde_json::Value) -> StructuredLogEntry {
        StructuredLogEntry::for_request(
            UnixMillis::new(sequence).expect("timestamp"),
            "engine",
            LogSeverity::Info,
            event_type,
            RequestId::from_uuid(
                Uuid::parse_str(&format!("00000000-0000-4000-8000-{sequence:012}"))
                    .expect("request ID"),
            ),
            detail,
        )
    }

    #[test]
    fn permuted_inputs_produce_identical_bundle_bytes() {
        let first = PreparedDiagnosticsBundle::prepare(input()).expect("first bundle");
        let mut permuted = input();
        permuted.components.reverse();
        permuted.crash_ids.reverse();
        permuted.connectivity_checks.reverse();
        permuted.logs.reverse();
        for fields in permuted.settings_schema.fields_by_section.values_mut() {
            fields.reverse();
        }
        let second = PreparedDiagnosticsBundle::prepare(permuted).expect("second bundle");

        assert_eq!(first.preview(), second.preview());
        assert_eq!(first.bytes(), second.bytes());
    }

    #[test]
    fn preview_matches_exact_exported_health_bundle() {
        let bundle = PreparedDiagnosticsBundle::prepare(input()).expect("bundle");
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("health-bundle.json");
        bundle.export(&path).expect("export");

        assert_eq!(fs::read(&path).expect("read export"), bundle.bytes());
        let document: DiagnosticsBundleDocument =
            serde_json::from_slice(bundle.bytes()).expect("bundle JSON");
        assert_eq!(&document, bundle.preview());
        assert_eq!(document.format_version, 1);
        assert_eq!(document.components.len(), 2);
        assert_eq!(document.settings_schema.version, 4);
        assert_eq!(document.crash_ids, ["crash-a", "crash-b"]);
        assert_eq!(document.connectivity_checks.len(), 2);
        assert_eq!(document.logs.len(), 2);
        assert_eq!(
            fs::read_dir(directory.path())
                .expect("bundle directory")
                .count(),
            1
        );
    }

    #[test]
    fn secret_bearing_bundle_is_clean_before_preview_and_disk() {
        let mut value = input();
        value.components[0].version =
            "141 Authorization: Bearer component-auth-sentinel".to_owned();
        value.connectivity_checks[0].detail = "Cookie: sid=check-cookie-sentinel\n\
            proxy=http://proxy-user:proxy-password-sentinel@proxy.test:8080/\n\
            url=https://files.test/archive?token=generic-token-sentinel&safe=visible"
            .to_owned();
        value.logs[0].detail = json!({
            "authorization": "Bearer log-auth-sentinel",
            "set-cookie": "log-cookie-sentinel",
            "password": "password-sentinel",
            "apiKey": "api-key-sentinel",
            "aws": "https://s3.test/object?X-Amz-Signature=aws-sentinel",
            "gcs": "https://storage.test/object?X-Goog-Signature=gcs-sentinel",
            "azure": "https://blob.test/object?sv=1&sig=azure-sentinel",
            "cloudfront": "https://cdn.test/object?Policy=policy-sentinel&Signature=cloudfront-sentinel&Key-Pair-Id=pair-sentinel"
        });
        let bundle = PreparedDiagnosticsBundle::prepare(value).expect("bundle");
        let serialized = String::from_utf8(bundle.bytes().to_vec()).expect("UTF-8");

        for secret in [
            "component-auth-sentinel",
            "check-cookie-sentinel",
            "proxy-user",
            "proxy-password-sentinel",
            "generic-token-sentinel",
            "log-auth-sentinel",
            "log-cookie-sentinel",
            "password-sentinel",
            "api-key-sentinel",
            "aws-sentinel",
            "gcs-sentinel",
            "azure-sentinel",
            "policy-sentinel",
            "cloudfront-sentinel",
            "pair-sentinel",
        ] {
            assert!(!serialized.contains(secret), "secret remained: {secret}");
        }
        assert!(serialized.contains("safe=visible"));
        assert!(serialized.contains("[REDACTED]"));

        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("health-bundle.json");
        bundle.export(&path).expect("export");
        assert_eq!(fs::read(&path).expect("disk bundle"), bundle.bytes());
    }

    #[test]
    fn invalid_bundle_refuses_before_creating_or_overwriting_output() {
        let mut invalid = input();
        invalid.crash_ids = vec!["not an opaque crash ID".to_owned()];
        assert!(matches!(
            PreparedDiagnosticsBundle::prepare(invalid),
            Err(DiagnosticsBundleError::InvalidCrashId)
        ));

        let mut oversized = input();
        oversized.crash_ids = (0..257).map(|index| format!("crash-{index}")).collect();
        assert!(matches!(
            PreparedDiagnosticsBundle::prepare(oversized),
            Err(DiagnosticsBundleError::TooManyItems {
                section: "crash IDs",
                actual: 257,
                maximum: 256
            })
        ));

        let mut oversized_log = input();
        oversized_log.logs[0].detail = json!({
            "message": "x".repeat(MAX_LOG_RECORD_BYTES)
        });
        assert!(matches!(
            PreparedDiagnosticsBundle::prepare(oversized_log),
            Err(DiagnosticsBundleError::LogRecordTooLarge { .. })
        ));

        let mut overly_deep_log = input();
        let mut detail = serde_json::Value::Null;
        for _ in 0..=65 {
            detail = json!([detail]);
        }
        overly_deep_log.logs[0].detail = detail;
        assert!(matches!(
            PreparedDiagnosticsBundle::prepare(overly_deep_log),
            Err(DiagnosticsBundleError::LogDetailTooDeep { maximum: 64 })
        ));

        let bundle = PreparedDiagnosticsBundle::prepare(input()).expect("bundle");
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("existing.json");
        fs::write(&path, b"preserve-me").expect("seed existing file");
        assert!(matches!(
            bundle.export(&path),
            Err(DiagnosticsBundleError::Io(error))
                if error.kind() == std::io::ErrorKind::AlreadyExists
        ));
        assert_eq!(fs::read(&path).expect("existing file"), b"preserve-me");
    }
}
