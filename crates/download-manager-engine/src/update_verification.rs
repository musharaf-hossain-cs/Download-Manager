//! Authentication and integrity verification for downloaded application updates.

use std::{
    fs::File,
    io::{self, Read},
    path::{Path, PathBuf},
};

use ed25519_dalek::{Signature, VerifyingKey};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

const UPDATE_METADATA_SCHEMA_VERSION: u32 = 1;
const MAX_UPDATE_METADATA_BYTES: usize = 64 * 1024;
const MAX_ARTIFACT_NAME_BYTES: usize = 255;
const READ_BUFFER_BYTES: usize = 128 * 1024;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMetadata {
    schema_version: u32,
    product: String,
    channel: String,
    version: String,
    architecture: String,
    artifact: UpdateArtifact,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateArtifact {
    file_name: String,
    size: u64,
    sha256: String,
}

/// Immutable proof that update metadata and its exact artifact passed verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedUpdate {
    version: Version,
    artifact_path: PathBuf,
    artifact_size: u64,
    artifact_sha256: [u8; 32],
    authenticated_metadata: Vec<u8>,
    metadata_signature: [u8; 64],
}

impl VerifiedUpdate {
    #[must_use]
    pub const fn version(&self) -> &Version {
        &self.version
    }

    #[must_use]
    pub fn artifact_path(&self) -> &Path {
        &self.artifact_path
    }

    #[must_use]
    pub const fn artifact_size(&self) -> u64 {
        self.artifact_size
    }

    #[must_use]
    pub const fn artifact_sha256(&self) -> &[u8; 32] {
        &self.artifact_sha256
    }

    pub(crate) fn authenticated_metadata(&self) -> &[u8] {
        &self.authenticated_metadata
    }

    pub(crate) const fn metadata_signature(&self) -> &[u8; 64] {
        &self.metadata_signature
    }
}

#[derive(Debug, Error)]
pub enum UpdateVerificationError {
    #[error("the trusted update public key is invalid")]
    InvalidPublicKey,
    #[error("update metadata is {actual} bytes; maximum is {maximum}")]
    MetadataTooLarge { actual: usize, maximum: usize },
    #[error("the update metadata signature must be exactly 64 bytes")]
    InvalidSignatureLength,
    #[error("the update metadata signature is not trusted")]
    UntrustedSignature,
    #[error("update metadata is not valid versioned JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("update metadata schema version {actual} is unsupported; expected {expected}")]
    UnsupportedSchema { actual: u32, expected: u32 },
    #[error("update metadata has an invalid {0}")]
    InvalidMetadata(&'static str),
    #[error("the update artifact could not be opened: {0}")]
    OpenArtifact(#[source] io::Error),
    #[error("the update artifact could not be read: {0}")]
    ReadArtifact(#[source] io::Error),
    #[error("update artifact size is {actual} bytes; expected {expected}")]
    ArtifactSizeMismatch { actual: u64, expected: u64 },
    #[error("the update artifact SHA-256 does not match signed metadata")]
    ArtifactHashMismatch,
}

/// Verifies update metadata against one application-pinned Ed25519 public key.
#[derive(Debug, Clone)]
pub struct UpdateVerifier {
    trusted_key: VerifyingKey,
}

impl UpdateVerifier {
    /// Constructs a verifier from the release public key compiled into the caller.
    ///
    /// # Errors
    /// Returns [`UpdateVerificationError::InvalidPublicKey`] for a non-canonical key.
    pub fn new(trusted_public_key: [u8; 32]) -> Result<Self, UpdateVerificationError> {
        let trusted_key = VerifyingKey::from_bytes(&trusted_public_key)
            .map_err(|_| UpdateVerificationError::InvalidPublicKey)?;
        Ok(Self { trusted_key })
    }

    /// Authenticates exact metadata bytes, validates their schema, then streams and hashes the
    /// named update artifact. No artifact is returned unless every check succeeds.
    ///
    /// # Errors
    /// Returns [`UpdateVerificationError`] when metadata, signature, artifact identity, size, or
    /// SHA-256 validation fails.
    pub fn verify(
        &self,
        metadata_bytes: &[u8],
        signature_bytes: &[u8],
        artifact_path: &Path,
    ) -> Result<VerifiedUpdate, UpdateVerificationError> {
        self.verify_artifact(metadata_bytes, signature_bytes, artifact_path, true)
    }

    pub(crate) fn verify_staged(
        &self,
        metadata_bytes: &[u8],
        signature_bytes: &[u8],
        artifact_path: &Path,
    ) -> Result<VerifiedUpdate, UpdateVerificationError> {
        self.verify_artifact(metadata_bytes, signature_bytes, artifact_path, false)
    }

    fn verify_artifact(
        &self,
        metadata_bytes: &[u8],
        signature_bytes: &[u8],
        artifact_path: &Path,
        require_release_file_name: bool,
    ) -> Result<VerifiedUpdate, UpdateVerificationError> {
        if metadata_bytes.len() > MAX_UPDATE_METADATA_BYTES {
            return Err(UpdateVerificationError::MetadataTooLarge {
                actual: metadata_bytes.len(),
                maximum: MAX_UPDATE_METADATA_BYTES,
            });
        }
        let signature = Signature::from_slice(signature_bytes)
            .map_err(|_| UpdateVerificationError::InvalidSignatureLength)?;
        self.trusted_key
            .verify_strict(metadata_bytes, &signature)
            .map_err(|_| UpdateVerificationError::UntrustedSignature)?;

        let metadata: UpdateMetadata = serde_json::from_slice(metadata_bytes)?;
        let (version, expected_hash) =
            validate_metadata(&metadata, artifact_path, require_release_file_name)?;
        let (actual_size, actual_hash) = hash_artifact(artifact_path)?;
        if actual_size != metadata.artifact.size {
            return Err(UpdateVerificationError::ArtifactSizeMismatch {
                actual: actual_size,
                expected: metadata.artifact.size,
            });
        }
        if actual_hash != expected_hash {
            return Err(UpdateVerificationError::ArtifactHashMismatch);
        }

        Ok(VerifiedUpdate {
            version,
            artifact_path: artifact_path.to_path_buf(),
            artifact_size: actual_size,
            artifact_sha256: actual_hash,
            authenticated_metadata: metadata_bytes.to_vec(),
            metadata_signature: signature.to_bytes(),
        })
    }
}

fn validate_metadata(
    metadata: &UpdateMetadata,
    artifact_path: &Path,
    require_release_file_name: bool,
) -> Result<(Version, [u8; 32]), UpdateVerificationError> {
    if metadata.schema_version != UPDATE_METADATA_SCHEMA_VERSION {
        return Err(UpdateVerificationError::UnsupportedSchema {
            actual: metadata.schema_version,
            expected: UPDATE_METADATA_SCHEMA_VERSION,
        });
    }
    if metadata.product != "Download Manager" {
        return Err(UpdateVerificationError::InvalidMetadata("product"));
    }
    if metadata.channel != "stable" {
        return Err(UpdateVerificationError::InvalidMetadata("channel"));
    }
    if metadata.architecture != "x86_64" {
        return Err(UpdateVerificationError::InvalidMetadata("architecture"));
    }
    let version = Version::parse(&metadata.version)
        .map_err(|_| UpdateVerificationError::InvalidMetadata("version"))?;
    let expected_name = format!("download-manager-{version}-windows-x64.zip");
    if metadata.artifact.file_name != expected_name
        || metadata.artifact.file_name.is_empty()
        || metadata.artifact.file_name.len() > MAX_ARTIFACT_NAME_BYTES
        || (require_release_file_name
            && artifact_path.file_name().and_then(|name| name.to_str())
                != Some(metadata.artifact.file_name.as_str()))
    {
        return Err(UpdateVerificationError::InvalidMetadata(
            "artifact file name",
        ));
    }
    if metadata.artifact.size == 0 {
        return Err(UpdateVerificationError::InvalidMetadata("artifact size"));
    }
    let expected_hash = decode_sha256(&metadata.artifact.sha256)
        .ok_or(UpdateVerificationError::InvalidMetadata("artifact SHA-256"))?;
    Ok((version, expected_hash))
}

fn decode_sha256(value: &str) -> Option<[u8; 32]> {
    if value.len() != 64
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return None;
    }
    let mut result = [0_u8; 32];
    let (pairs, _) = value.as_bytes().as_chunks::<2>();
    for (index, pair) in pairs.iter().enumerate() {
        let text = std::str::from_utf8(pair).ok()?;
        result[index] = u8::from_str_radix(text, 16).ok()?;
    }
    Some(result)
}

fn hash_artifact(path: &Path) -> Result<(u64, [u8; 32]), UpdateVerificationError> {
    let mut file = File::open(path).map_err(UpdateVerificationError::OpenArtifact)?;
    let mut hasher = Sha256::new();
    let mut size = 0_u64;
    let mut buffer = vec![0_u8; READ_BUFFER_BYTES];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(UpdateVerificationError::ReadArtifact)?;
        if count == 0 {
            break;
        }
        size = size
            .checked_add(count as u64)
            .ok_or(UpdateVerificationError::InvalidMetadata("artifact size"))?;
        hasher.update(&buffer[..count]);
    }
    Ok((size, hasher.finalize().into()))
}

#[cfg(test)]
mod tests {
    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{UpdateVerificationError, UpdateVerifier};

    const TEST_SIGNING_KEY: [u8; 32] = [0x5a; 32];

    struct Fixture {
        _directory: TempDir,
        path: std::path::PathBuf,
        metadata: Vec<u8>,
        verifier: UpdateVerifier,
        signing_key: SigningKey,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("temporary directory");
            let path = directory
                .path()
                .join("download-manager-1.2.3-windows-x64.zip");
            let artifact = b"signed update artifact";
            std::fs::write(&path, artifact).expect("write artifact");
            let metadata = serde_json::to_vec(&json!({
                "schema_version": 1,
                "product": "Download Manager",
                "channel": "stable",
                "version": "1.2.3",
                "architecture": "x86_64",
                "artifact": {
                    "file_name": "download-manager-1.2.3-windows-x64.zip",
                    "size": artifact.len(),
                    "sha256": format!("{:x}", Sha256::digest(artifact))
                }
            }))
            .expect("metadata");
            let signing_key = SigningKey::from_bytes(&TEST_SIGNING_KEY);
            let verifier =
                UpdateVerifier::new(signing_key.verifying_key().to_bytes()).expect("trusted key");
            Self {
                _directory: directory,
                path,
                metadata,
                verifier,
                signing_key,
            }
        }

        fn signature(&self, metadata: &[u8]) -> Vec<u8> {
            self.signing_key.sign(metadata).to_bytes().to_vec()
        }
    }

    #[test]
    fn valid_signed_metadata_and_exact_artifact_are_accepted() {
        let fixture = Fixture::new();
        let verified = fixture
            .verifier
            .verify(
                &fixture.metadata,
                &fixture.signature(&fixture.metadata),
                &fixture.path,
            )
            .expect("verified update");

        assert_eq!(verified.version().to_string(), "1.2.3");
        assert_eq!(verified.artifact_path(), fixture.path);
        assert_eq!(verified.artifact_size(), 22);
        assert_eq!(verified.artifact_sha256().len(), 32);
    }

    #[test]
    fn invalid_key_signature_and_tampered_metadata_are_rejected() {
        let fixture = Fixture::new();
        let other_key = SigningKey::from_bytes(&[0x33; 32]);
        let wrong_verifier =
            UpdateVerifier::new(other_key.verifying_key().to_bytes()).expect("other key");
        let signature = fixture.signature(&fixture.metadata);
        assert!(matches!(
            wrong_verifier.verify(&fixture.metadata, &signature, &fixture.path),
            Err(UpdateVerificationError::UntrustedSignature)
        ));
        assert!(matches!(
            fixture
                .verifier
                .verify(&fixture.metadata, &[0_u8; 63], &fixture.path),
            Err(UpdateVerificationError::InvalidSignatureLength)
        ));

        let mut tampered: Value = serde_json::from_slice(&fixture.metadata).expect("JSON");
        tampered["version"] = json!("9.9.9");
        let tampered = serde_json::to_vec(&tampered).expect("tampered metadata");
        assert!(matches!(
            fixture
                .verifier
                .verify(&tampered, &signature, &fixture.path),
            Err(UpdateVerificationError::UntrustedSignature)
        ));
    }

    #[test]
    fn signed_metadata_cannot_authorize_tampered_or_truncated_artifacts() {
        let fixture = Fixture::new();
        let signature = fixture.signature(&fixture.metadata);
        std::fs::write(&fixture.path, b"signed update artifacX").expect("tamper artifact");
        assert!(matches!(
            fixture
                .verifier
                .verify(&fixture.metadata, &signature, &fixture.path),
            Err(UpdateVerificationError::ArtifactHashMismatch)
        ));

        std::fs::write(&fixture.path, b"short").expect("truncate artifact");
        assert!(matches!(
            fixture
                .verifier
                .verify(&fixture.metadata, &signature, &fixture.path),
            Err(UpdateVerificationError::ArtifactSizeMismatch { .. })
        ));
    }

    #[test]
    fn signed_but_unsupported_or_unsafe_metadata_is_rejected() {
        let fixture = Fixture::new();
        for (field, value, expected) in [
            ("schema_version", json!(2), "schema"),
            ("channel", json!("beta"), "metadata"),
            ("architecture", json!("arm64"), "metadata"),
            ("version", json!("not-semver"), "metadata"),
        ] {
            let mut document: Value =
                serde_json::from_slice(&fixture.metadata).expect("metadata JSON");
            document[field] = value;
            let metadata = serde_json::to_vec(&document).expect("changed metadata");
            let result =
                fixture
                    .verifier
                    .verify(&metadata, &fixture.signature(&metadata), &fixture.path);
            assert!(
                matches!(
                    (&result, expected),
                    (
                        Err(UpdateVerificationError::UnsupportedSchema { .. }),
                        "schema"
                    ) | (Err(UpdateVerificationError::InvalidMetadata(_)), "metadata")
                ),
                "unexpected result for {field}: {result:?}"
            );
        }
    }
}
