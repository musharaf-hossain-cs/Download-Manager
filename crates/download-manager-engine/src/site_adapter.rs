//! Authentication and bounded interpretation for remote site-adapter rules.
//!
//! Remote adapters are deliberately data-only. The signed schema can match an exact page host
//! and classify a media URL by path suffix; it contains no script, selector, expression, regular
//! expression, module, or executable hook.

use std::collections::HashSet;

use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::Deserialize;
use thiserror::Error;
use url::Url;

const SITE_ADAPTER_SCHEMA_VERSION: u32 = 1;
const MAX_SITE_ADAPTER_BYTES: usize = 64 * 1024;
const MAX_ADAPTER_ID_BYTES: usize = 64;
const MAX_PAGE_HOSTS: usize = 32;
const MAX_RULES: usize = 64;
const MAX_PATH_SUFFIX_BYTES: usize = 512;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SiteAdapterDocument {
    schema_version: u32,
    adapter_id: String,
    adapter_version: String,
    page_hosts: Vec<String>,
    rules: Vec<SiteAdapterRule>,
}

#[derive(Debug, Clone, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
struct SiteAdapterRule {
    path_suffix: String,
    media_kind: SiteAdapterMediaKind,
}

/// Media classification emitted by an authenticated site adapter.
#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SiteAdapterMediaKind {
    Progressive,
    Hls,
    Dash,
}

/// A data-only site adapter whose exact source bytes passed pinned-key authentication.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedSiteAdapter {
    adapter_id: String,
    adapter_version: Version,
    page_hosts: Vec<String>,
    rules: Vec<SiteAdapterRule>,
}

impl VerifiedSiteAdapter {
    #[must_use]
    pub fn adapter_id(&self) -> &str {
        &self.adapter_id
    }

    #[must_use]
    pub const fn adapter_version(&self) -> &Version {
        &self.adapter_version
    }

    /// Classifies a candidate only when the page uses HTTP(S), its host is explicitly listed,
    /// and the candidate uses HTTP(S) with a matching path suffix.
    #[must_use]
    pub fn classify(&self, page_url: &str, candidate_url: &str) -> Option<SiteAdapterMediaKind> {
        let page = parse_http_url(page_url)?;
        let page_host = page.host_str()?.to_ascii_lowercase();
        if !self.page_hosts.iter().any(|host| host == &page_host) {
            return None;
        }

        let candidate = parse_http_url(candidate_url)?;
        self.rules
            .iter()
            .find(|rule| candidate.path().ends_with(&rule.path_suffix))
            .map(|rule| rule.media_kind)
    }
}

#[derive(Debug, Error)]
pub enum SiteAdapterError {
    #[error("the trusted site-adapter public key is invalid")]
    InvalidPublicKey,
    #[error("site-adapter document is {actual} bytes; maximum is {maximum}")]
    DocumentTooLarge { actual: usize, maximum: usize },
    #[error("the site-adapter signature must be exactly 64 bytes")]
    InvalidSignatureLength,
    #[error("the site-adapter signature is not trusted")]
    UntrustedSignature,
    #[error("site-adapter document is not valid data-only JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("site-adapter schema version {actual} is unsupported; expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("site-adapter metadata has an invalid {0}")]
    InvalidMetadata(&'static str),
}

/// Authenticates remote adapter bytes against an application-pinned Ed25519 key.
#[derive(Debug, Clone)]
pub struct SiteAdapterVerifier {
    trusted_key: VerifyingKey,
}

impl SiteAdapterVerifier {
    /// Constructs a verifier from the site-adapter release key compiled into the caller.
    ///
    /// # Errors
    /// Returns [`SiteAdapterError::InvalidPublicKey`] for a non-canonical public key.
    pub fn new(trusted_public_key: [u8; 32]) -> Result<Self, SiteAdapterError> {
        let trusted_key = VerifyingKey::from_bytes(&trusted_public_key)
            .map_err(|_| SiteAdapterError::InvalidPublicKey)?;
        Ok(Self { trusted_key })
    }

    /// Authenticates exact remote bytes before decoding their strict data-only schema.
    ///
    /// # Errors
    /// Returns [`SiteAdapterError`] for an oversized document, invalid signature, unsupported
    /// schema, unknown field, or unsafe/unbounded rule value.
    pub fn verify(
        &self,
        document_bytes: &[u8],
        signature_bytes: &[u8],
    ) -> Result<VerifiedSiteAdapter, SiteAdapterError> {
        if document_bytes.len() > MAX_SITE_ADAPTER_BYTES {
            return Err(SiteAdapterError::DocumentTooLarge {
                actual: document_bytes.len(),
                maximum: MAX_SITE_ADAPTER_BYTES,
            });
        }
        let signature = Signature::from_slice(signature_bytes)
            .map_err(|_| SiteAdapterError::InvalidSignatureLength)?;
        self.trusted_key
            .verify_strict(document_bytes, &signature)
            .map_err(|_| SiteAdapterError::UntrustedSignature)?;

        let document: SiteAdapterDocument = serde_json::from_slice(document_bytes)?;
        validate_document(document)
    }
}

fn validate_document(
    document: SiteAdapterDocument,
) -> Result<VerifiedSiteAdapter, SiteAdapterError> {
    if document.schema_version != SITE_ADAPTER_SCHEMA_VERSION {
        return Err(SiteAdapterError::UnsupportedSchema {
            actual: document.schema_version,
            expected: SITE_ADAPTER_SCHEMA_VERSION,
        });
    }
    if !valid_adapter_id(&document.adapter_id) {
        return Err(SiteAdapterError::InvalidMetadata("adapter ID"));
    }
    let adapter_version = Version::parse(&document.adapter_version)
        .map_err(|_| SiteAdapterError::InvalidMetadata("adapter version"))?;
    if document.page_hosts.is_empty() || document.page_hosts.len() > MAX_PAGE_HOSTS {
        return Err(SiteAdapterError::InvalidMetadata("page hosts"));
    }
    if document.rules.is_empty() || document.rules.len() > MAX_RULES {
        return Err(SiteAdapterError::InvalidMetadata("rules"));
    }

    let mut unique_hosts = HashSet::with_capacity(document.page_hosts.len());
    for host in &document.page_hosts {
        if !valid_hostname(host) || !unique_hosts.insert(host) {
            return Err(SiteAdapterError::InvalidMetadata("page host"));
        }
    }
    for rule in &document.rules {
        if !valid_path_suffix(&rule.path_suffix) {
            return Err(SiteAdapterError::InvalidMetadata("rule path suffix"));
        }
    }

    Ok(VerifiedSiteAdapter {
        adapter_id: document.adapter_id,
        adapter_version,
        page_hosts: document.page_hosts,
        rules: document.rules,
    })
}

fn valid_adapter_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_ADAPTER_ID_BYTES
        && value.bytes().enumerate().all(|(index, byte)| {
            byte.is_ascii_lowercase()
                || byte.is_ascii_digit()
                || (byte == b'-' && index > 0 && index + 1 < value.len())
        })
}

fn valid_hostname(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 253
        && value.is_ascii()
        && value.bytes().all(|byte| !byte.is_ascii_uppercase())
        && value.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
        })
}

fn valid_path_suffix(value: &str) -> bool {
    value.starts_with('/')
        && value.len() <= MAX_PATH_SUFFIX_BYTES
        && !value.contains(['?', '#', '\\'])
        && value.chars().all(|character| !character.is_control())
}

fn parse_http_url(value: &str) -> Option<Url> {
    let parsed = Url::parse(value).ok()?;
    matches!(parsed.scheme(), "http" | "https").then_some(parsed)
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};

    use super::{SiteAdapterError, SiteAdapterMediaKind, SiteAdapterVerifier};

    const TEST_SIGNING_KEY: [u8; 32] = [0x6c; 32];

    fn document() -> Vec<u8> {
        serde_json::to_vec(&json!({
            "schema_version": 1,
            "adapter_id": "example-video",
            "adapter_version": "2.1.0",
            "page_hosts": ["video.example.test", "embed.example.test"],
            "rules": [
                {"path_suffix": "/master.m3u8", "media_kind": "hls"},
                {"path_suffix": "/manifest.mpd", "media_kind": "dash"},
                {"path_suffix": "/download.mp4", "media_kind": "progressive"}
            ]
        }))
        .expect("adapter document")
    }

    fn fixture() -> (SigningKey, SiteAdapterVerifier) {
        let signing_key = SigningKey::from_bytes(&TEST_SIGNING_KEY);
        let verifier =
            SiteAdapterVerifier::new(signing_key.verifying_key().to_bytes()).expect("trusted key");
        (signing_key, verifier)
    }

    fn verify_bytes(
        signing_key: &SigningKey,
        verifier: &SiteAdapterVerifier,
        bytes: &[u8],
    ) -> Result<super::VerifiedSiteAdapter, SiteAdapterError> {
        verifier.verify(bytes, &signing_key.sign(bytes).to_bytes())
    }

    #[test]
    fn signed_versioned_data_rules_classify_only_explicit_sites_and_suffixes() {
        let (signing_key, verifier) = fixture();
        let adapter = verify_bytes(&signing_key, &verifier, &document()).expect("valid adapter");

        assert_eq!(adapter.adapter_id(), "example-video");
        assert_eq!(adapter.adapter_version().to_string(), "2.1.0");
        assert_eq!(
            adapter.classify(
                "https://video.example.test/watch/1",
                "https://cdn.example.test/media/master.m3u8?token=secret"
            ),
            Some(SiteAdapterMediaKind::Hls)
        );
        assert_eq!(
            adapter.classify(
                "https://other.example.test/watch/1",
                "https://cdn.example.test/media/master.m3u8"
            ),
            None
        );
        assert_eq!(
            adapter.classify(
                "file:///C:/page.html",
                "https://cdn.example.test/media/master.m3u8"
            ),
            None
        );
        assert_eq!(
            adapter.classify("https://video.example.test/watch/1", "javascript:alert(1)"),
            None
        );
    }

    #[test]
    fn wrong_key_tampering_and_malformed_signatures_fail_before_use() {
        let (signing_key, verifier) = fixture();
        let bytes = document();
        let signature = signing_key.sign(&bytes).to_bytes();
        let wrong_key = SigningKey::from_bytes(&[0x22; 32]);
        let wrong_verifier = SiteAdapterVerifier::new(wrong_key.verifying_key().to_bytes())
            .expect("other trusted key");
        assert!(matches!(
            wrong_verifier.verify(&bytes, &signature),
            Err(SiteAdapterError::UntrustedSignature)
        ));

        let mut tampered: Value = serde_json::from_slice(&bytes).expect("JSON");
        tampered["adapter_version"] = json!("9.9.9");
        let tampered = serde_json::to_vec(&tampered).expect("tampered document");
        assert!(matches!(
            verifier.verify(&tampered, &signature),
            Err(SiteAdapterError::UntrustedSignature)
        ));
        assert!(matches!(
            verifier.verify(&bytes, &[0_u8; 63]),
            Err(SiteAdapterError::InvalidSignatureLength)
        ));
    }

    #[test]
    fn signed_executable_or_unbounded_shapes_are_rejected() {
        let (signing_key, verifier) = fixture();
        for (field, value, expected) in [
            ("script", json!("fetch('https://evil.test/code')"), "decode"),
            ("schema_version", json!(2), "schema"),
            ("adapter_version", json!("latest"), "metadata"),
            ("page_hosts", json!(["*.example.test"]), "metadata"),
            ("rules", json!([]), "metadata"),
            (
                "rules",
                json!([{"path_suffix": "/media.m3u8?eval=1", "media_kind": "hls"}]),
                "metadata",
            ),
        ] {
            let mut changed: Value = serde_json::from_slice(&document()).expect("document JSON");
            changed[field] = value;
            let changed = serde_json::to_vec(&changed).expect("changed document");
            let result = verify_bytes(&signing_key, &verifier, &changed);
            assert!(
                matches!(
                    (&result, expected),
                    (Err(SiteAdapterError::Decode(_)), "decode")
                        | (Err(SiteAdapterError::UnsupportedSchema { .. }), "schema")
                        | (Err(SiteAdapterError::InvalidMetadata(_)), "metadata")
                ),
                "unexpected result for {field}: {result:?}"
            );
        }
    }
}
