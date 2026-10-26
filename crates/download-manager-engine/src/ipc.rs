//! Versioned and size-bounded schemas for local UI and browser IPC.

use std::{
    collections::{HashSet, VecDeque},
    path::PathBuf,
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use thiserror::Error;

use crate::{
    domain::{HttpUrl, JobId, JobMetadata, RequestId, ValidationError},
    state::{DownloadEvent, DownloadState},
};

pub const UI_PROTOCOL_VERSION: u16 = 1;
pub const EXTENSION_PROTOCOL_VERSION: u16 = 1;
pub const MAX_UI_MESSAGE_BYTES: usize = 1_048_576;
pub const MAX_EXTENSION_MESSAGE_BYTES: usize = 65_536;
pub const MAX_EXTENSION_BATCH_ITEMS: usize = 128;
pub const MAX_TRACKED_REQUEST_IDS: usize = 1_024;

#[derive(Debug, Error)]
pub enum ProtocolError {
    #[error("message size {actual} exceeds limit {maximum}")]
    MessageTooLarge { actual: usize, maximum: usize },
    #[error("malformed message: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("unknown field in strict protocol schema: {0}")]
    UnknownField(String),
    #[error("unsupported protocol version {received}; expected {expected}")]
    UnsupportedVersion { received: u16, expected: u16 },
    #[error("duplicate request ID: {0}")]
    DuplicateRequestId(RequestId),
    #[error("invalid request nonce")]
    InvalidNonce,
    #[error("extension ID must not be empty or exceed 256 bytes")]
    InvalidExtensionId,
    #[error("batch contains {actual} items; maximum is {maximum}")]
    BatchTooLarge { actual: usize, maximum: usize },
    #[error("invalid field: {0}")]
    InvalidField(#[from] ValidationError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Envelope<T> {
    pub version: u16,
    pub request_id: RequestId,
    pub payload: T,
}

impl<T> Envelope<T> {
    #[must_use]
    pub const fn new(version: u16, request_id: RequestId, payload: T) -> Self {
        Self {
            version,
            request_id,
            payload,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiRequest {
    Health,
    ListJobs,
    ProbeUrl {
        source_url: HttpUrl,
        destination: PathBuf,
    },
    AddJob {
        source_url: HttpUrl,
        final_url: HttpUrl,
        filename: String,
        destination: PathBuf,
        total_bytes: Option<u64>,
        start_now: bool,
        duplicate_action: DuplicateAction,
    },
    TransitionJob {
        job_id: JobId,
        state: DownloadState,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DuplicateAction {
    FocusExisting,
    AddDuplicate,
    ReplaceExisting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiResponse {
    Health {
        engine_version: String,
    },
    Accepted {
        job_id: JobId,
    },
    Jobs {
        jobs: Vec<JobMetadata>,
    },
    UrlProbe {
        final_url: HttpUrl,
        filename: String,
        destination: PathBuf,
        total_bytes: Option<u64>,
        mime_type: Option<String>,
    },
    Rejected {
        code: String,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum UiMessage {
    Response { response: UiResponse },
    Event { event: DownloadEvent },
}

/// Decodes one strict UI request envelope.
///
/// # Errors
/// Returns [`ProtocolError`] for size, JSON, schema, or version violations.
pub fn decode_ui_request(bytes: &[u8]) -> Result<Envelope<UiRequest>, ProtocolError> {
    if bytes.len() <= MAX_UI_MESSAGE_BYTES {
        reject_unknown_ui_fields(bytes)?;
    }
    decode_versioned(bytes, MAX_UI_MESSAGE_BYTES, UI_PROTOCOL_VERSION)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionEnvelope {
    pub version: u16,
    pub request_id: RequestId,
    pub nonce: String,
    pub extension_id: String,
    pub payload: ExtensionRequest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionRequest {
    Health,
    AddDownload {
        url: HttpUrl,
        referrer: Option<HttpUrl>,
        suggested_filename: Option<String>,
    },
    AddBatch {
        items: Vec<ExtensionDownloadItem>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtensionDownloadItem {
    pub url: HttpUrl,
    pub referrer: Option<HttpUrl>,
    pub suggested_filename: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ExtensionResponse {
    Healthy { host_version: String },
    Accepted { job_ids: Vec<JobId> },
    Rejected { code: String, message: String },
}

/// Bounded replay cache for one authenticated native-host connection.
#[derive(Debug)]
pub struct RequestIdTracker {
    seen: HashSet<RequestId>,
    order: VecDeque<RequestId>,
    capacity: usize,
}

impl Default for RequestIdTracker {
    fn default() -> Self {
        Self::new(MAX_TRACKED_REQUEST_IDS)
    }
}

impl RequestIdTracker {
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            seen: HashSet::with_capacity(capacity),
            order: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Records a request ID for replay protection.
    ///
    /// # Errors
    /// Returns [`ProtocolError::DuplicateRequestId`] when the ID is already tracked.
    pub fn accept(&mut self, request_id: RequestId) -> Result<(), ProtocolError> {
        if self.seen.contains(&request_id) {
            return Err(ProtocolError::DuplicateRequestId(request_id));
        }
        if self.capacity == 0 {
            return Ok(());
        }
        if self.order.len() == self.capacity
            && let Some(expired) = self.order.pop_front()
        {
            self.seen.remove(&expired);
        }
        self.order.push_back(request_id);
        self.seen.insert(request_id);
        Ok(())
    }
}

/// Decodes and semantically validates one extension request.
///
/// # Errors
/// Returns [`ProtocolError`] for size, JSON, schema, version, nonce, URL, or replay violations.
pub fn decode_extension_request(
    bytes: &[u8],
    tracker: &mut RequestIdTracker,
) -> Result<ExtensionEnvelope, ProtocolError> {
    reject_unknown_extension_fields(bytes)?;
    let envelope: ExtensionEnvelope = decode_versioned(
        bytes,
        MAX_EXTENSION_MESSAGE_BYTES,
        EXTENSION_PROTOCOL_VERSION,
    )?;
    validate_nonce(&envelope.nonce)?;
    if envelope.extension_id.is_empty() || envelope.extension_id.len() > 256 {
        return Err(ProtocolError::InvalidExtensionId);
    }
    validate_extension_payload(&envelope.payload)?;
    tracker.accept(envelope.request_id)?;
    Ok(envelope)
}

fn reject_unknown_ui_fields(bytes: &[u8]) -> Result<(), ProtocolError> {
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    validate_object_keys(&value, &["version", "request_id", "payload"])?;
    let payload = value
        .get("payload")
        .ok_or_else(|| ProtocolError::UnknownField("missing payload".to_owned()))?;
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("health" | "list_jobs") => validate_object_keys(payload, &["type"]),
        Some("probe_url") => validate_object_keys(payload, &["type", "source_url", "destination"]),
        Some("add_job") => validate_object_keys(
            payload,
            &[
                "type",
                "source_url",
                "final_url",
                "filename",
                "destination",
                "total_bytes",
                "start_now",
                "duplicate_action",
            ],
        ),
        Some("transition_job") => validate_object_keys(payload, &["type", "job_id", "state"]),
        _ => Ok(()),
    }
}

fn reject_unknown_extension_fields(bytes: &[u8]) -> Result<(), ProtocolError> {
    if bytes.len() > MAX_EXTENSION_MESSAGE_BYTES {
        return Ok(());
    }
    let value: serde_json::Value = serde_json::from_slice(bytes)?;
    validate_object_keys(
        &value,
        &["version", "request_id", "nonce", "extension_id", "payload"],
    )?;
    let payload = value
        .get("payload")
        .ok_or_else(|| ProtocolError::UnknownField("missing payload".to_owned()))?;
    match payload.get("type").and_then(serde_json::Value::as_str) {
        Some("health") => validate_object_keys(payload, &["type"]),
        Some("add_download") => {
            validate_object_keys(payload, &["type", "url", "referrer", "suggested_filename"])
        }
        Some("add_batch") => {
            validate_object_keys(payload, &["type", "items"])?;
            if let Some(items) = payload.get("items").and_then(serde_json::Value::as_array) {
                for item in items {
                    validate_object_keys(item, &["url", "referrer", "suggested_filename"])?;
                }
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

fn validate_object_keys(value: &serde_json::Value, allowed: &[&str]) -> Result<(), ProtocolError> {
    let Some(object) = value.as_object() else {
        return Ok(());
    };
    if let Some(unknown) = object.keys().find(|key| !allowed.contains(&key.as_str())) {
        Err(ProtocolError::UnknownField(unknown.clone()))
    } else {
        Ok(())
    }
}

fn decode_versioned<T>(
    bytes: &[u8],
    maximum: usize,
    expected_version: u16,
) -> Result<T, ProtocolError>
where
    T: DeserializeOwned + ProtocolVersion,
{
    if bytes.len() > maximum {
        return Err(ProtocolError::MessageTooLarge {
            actual: bytes.len(),
            maximum,
        });
    }
    let decoded: T = serde_json::from_slice(bytes)?;
    let received = decoded.version();
    if received != expected_version {
        return Err(ProtocolError::UnsupportedVersion {
            received,
            expected: expected_version,
        });
    }
    Ok(decoded)
}

trait ProtocolVersion {
    fn version(&self) -> u16;
}

impl<T> ProtocolVersion for Envelope<T> {
    fn version(&self) -> u16 {
        self.version
    }
}

impl ProtocolVersion for ExtensionEnvelope {
    fn version(&self) -> u16 {
        self.version
    }
}

fn validate_nonce(nonce: &str) -> Result<(), ProtocolError> {
    if (16..=128).contains(&nonce.len())
        && nonce
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        Ok(())
    } else {
        Err(ProtocolError::InvalidNonce)
    }
}

fn validate_extension_payload(payload: &ExtensionRequest) -> Result<(), ProtocolError> {
    let items: &[ExtensionDownloadItem] = match payload {
        ExtensionRequest::Health => return Ok(()),
        ExtensionRequest::AddDownload {
            url,
            referrer,
            suggested_filename,
        } => {
            validate_download_item(url, referrer.as_ref(), suggested_filename.as_deref())?;
            return Ok(());
        }
        ExtensionRequest::AddBatch { items } => items,
    };
    if items.len() > MAX_EXTENSION_BATCH_ITEMS {
        return Err(ProtocolError::BatchTooLarge {
            actual: items.len(),
            maximum: MAX_EXTENSION_BATCH_ITEMS,
        });
    }
    for item in items {
        validate_download_item(
            &item.url,
            item.referrer.as_ref(),
            item.suggested_filename.as_deref(),
        )?;
    }
    Ok(())
}

fn validate_download_item(
    _url: &HttpUrl,
    _referrer: Option<&HttpUrl>,
    suggested_filename: Option<&str>,
) -> Result<(), ProtocolError> {
    if let Some(filename) = suggested_filename
        && (filename.is_empty() || filename.len() > 1_024 || filename.contains(['\r', '\n', '\0']))
    {
        return Err(ProtocolError::InvalidField(ValidationError::TooLong {
            field: "suggested_filename",
            maximum: 1_024,
        }));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        EXTENSION_PROTOCOL_VERSION, ExtensionEnvelope, ExtensionRequest,
        MAX_EXTENSION_MESSAGE_BYTES, ProtocolError, RequestIdTracker, UI_PROTOCOL_VERSION,
        UiRequest, decode_extension_request, decode_ui_request,
    };
    use crate::domain::{HttpUrl, RequestId};

    #[test]
    fn ui_schema_has_a_stable_snapshot() {
        let request_id: RequestId = "018f47ce-8e51-7f31-a1ec-4a97f84a12cd".parse().expect("ID");
        let message = super::Envelope::new(UI_PROTOCOL_VERSION, request_id, UiRequest::Health);
        let json = serde_json::to_string(&message).expect("serialize");
        assert_eq!(
            json,
            r#"{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"health"}}"#
        );
        assert_eq!(decode_ui_request(json.as_bytes()).expect("decode"), message);
    }

    #[test]
    fn ui_rejects_unknown_versions_unknown_fields_and_malformed_messages() {
        let wrong_version = br#"{"version":2,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"health"}}"#;
        assert!(matches!(
            decode_ui_request(wrong_version),
            Err(ProtocolError::UnsupportedVersion { .. })
        ));
        let unknown_field = br#"{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"health","extra":true}}"#;
        assert!(matches!(
            decode_ui_request(unknown_field),
            Err(ProtocolError::UnknownField(field)) if field == "extra"
        ));
        assert!(matches!(
            decode_ui_request(b"not json"),
            Err(ProtocolError::Malformed(_))
        ));
    }

    fn extension_message(request_id: RequestId, url: &str) -> Vec<u8> {
        serde_json::to_vec(&ExtensionEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id,
            nonce: "abcdefghijklmnop".to_owned(),
            extension_id: "packaged-extension-id".to_owned(),
            payload: ExtensionRequest::AddDownload {
                url: HttpUrl::parse(url).expect("URL"),
                referrer: None,
                suggested_filename: None,
            },
        })
        .expect("serialize")
    }

    #[test]
    fn extension_rejects_duplicate_request_ids() {
        let id = RequestId::new();
        let message = extension_message(id, "https://example.test/file");
        let mut tracker = RequestIdTracker::default();
        decode_extension_request(&message, &mut tracker).expect("first request");
        assert!(matches!(
            decode_extension_request(&message, &mut tracker),
            Err(ProtocolError::DuplicateRequestId(duplicate)) if duplicate == id
        ));
    }

    #[test]
    fn extension_rejects_oversize_invalid_urls_and_bad_nonces() {
        let mut tracker = RequestIdTracker::default();
        assert!(matches!(
            decode_extension_request(&vec![b' '; MAX_EXTENSION_MESSAGE_BYTES + 1], &mut tracker),
            Err(ProtocolError::MessageTooLarge { .. })
        ));

        let invalid_url = format!(
            r#"{{"version":1,"request_id":"{}","nonce":"abcdefghijklmnop","extension_id":"id","payload":{{"type":"add_download","url":"file:///secret","referrer":null,"suggested_filename":null}}}}"#,
            RequestId::new()
        );
        assert!(matches!(
            decode_extension_request(invalid_url.as_bytes(), &mut tracker),
            Err(ProtocolError::Malformed(_))
        ));

        let mut bad_nonce: ExtensionEnvelope = serde_json::from_slice(&extension_message(
            RequestId::new(),
            "https://example.test/file",
        ))
        .expect("decode fixture");
        bad_nonce.nonce = "short".to_owned();
        assert!(matches!(
            decode_extension_request(
                &serde_json::to_vec(&bad_nonce).expect("serialize"),
                &mut tracker
            ),
            Err(ProtocolError::InvalidNonce)
        ));
    }
}
