//! Durable, non-activating staging for cryptographically verified application updates.

use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
};

use semver::Version;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::update_verification::{UpdateVerificationError, UpdateVerifier, VerifiedUpdate};

const STAGING_DIRECTORY: &str = ".staging";
const PENDING_DIRECTORY: &str = "pending";
const COMPLETED_DIRECTORY: &str = ".completed";
const ARTIFACT_FILE: &str = "artifact.zip";
const TEMP_ARTIFACT_FILE: &str = "artifact.tmp";
const METADATA_FILE: &str = "metadata.json";
const SIGNATURE_FILE: &str = "metadata.sig";
const COPY_BUFFER_BYTES: usize = 128 * 1024;
const MAX_METADATA_BYTES: usize = 64 * 1024;
const SIGNATURE_BYTES: usize = 64;

/// One completely copied and revalidated update artifact that has not been activated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedUpdate {
    version: Version,
    artifact_path: PathBuf,
    artifact_size: u64,
    artifact_sha256: [u8; 32],
}

impl StagedUpdate {
    fn from_verified(verified: &VerifiedUpdate, artifact_path: PathBuf) -> Self {
        Self {
            version: verified.version().clone(),
            artifact_path,
            artifact_size: verified.artifact_size(),
            artifact_sha256: *verified.artifact_sha256(),
        }
    }

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
}

/// Owns the fixed staging and pending locations beneath one caller-selected state root.
#[derive(Debug, Clone)]
pub struct UpdateStageStore {
    root: PathBuf,
}

#[derive(Debug, Error)]
pub enum UpdateStageError {
    #[error("the update staging root must be a real directory")]
    InvalidRoot,
    #[error("the update {0} location is not a real directory")]
    UnsafeDirectory(&'static str),
    #[error("the update {location} contains an unexpected or unsafe entry: {name}")]
    UnsafeEntry {
        location: &'static str,
        name: String,
    },
    #[error("a complete update is already pending activation")]
    PendingExists,
    #[error("the staged update has an invalid {0}")]
    InvalidAuthenticatedState(&'static str),
    #[error("the verified update artifact changed before staging completed")]
    VerifiedArtifactChanged,
    #[error("the staged update failed signed verification: {0}")]
    Verification(#[from] UpdateVerificationError),
    #[error("update staging filesystem operation failed: {0}")]
    Io(#[from] io::Error),
}

impl UpdateStageStore {
    /// Opens or creates an isolated update-state directory.
    ///
    /// The store has no live-install path and therefore cannot replace an application. S158 owns
    /// activation and rollback.
    ///
    /// # Errors
    /// Returns [`UpdateStageError`] when the root cannot be created or is not a real directory.
    pub fn open(root: impl AsRef<Path>) -> Result<Self, UpdateStageError> {
        let root = root.as_ref();
        if root.file_name().is_none() {
            return Err(UpdateStageError::InvalidRoot);
        }
        fs::create_dir_all(root)?;
        let metadata = fs::symlink_metadata(root)?;
        if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
            return Err(UpdateStageError::InvalidRoot);
        }
        Ok(Self {
            root: root.to_path_buf(),
        })
    }

    /// Copies a verified artifact into durable pending storage and rechecks its exact identity.
    ///
    /// # Errors
    /// Returns [`UpdateStageError`] for an existing pending update, unsafe recovery state, source
    /// mutation after verification, malformed state, or a filesystem failure.
    pub fn stage(&self, verified: &VerifiedUpdate) -> Result<StagedUpdate, UpdateStageError> {
        let source = File::open(verified.artifact_path())?;
        self.stage_from_reader(verified, source)
    }

    /// Removes an incomplete private transaction and returns a validated ready update, if any.
    ///
    /// A ready but malformed or tampered pending update is preserved and reported as an error.
    ///
    /// # Errors
    /// Returns [`UpdateStageError`] for unsafe entries, invalid ready state, or filesystem errors.
    pub fn recover(
        &self,
        verifier: &UpdateVerifier,
    ) -> Result<Option<StagedUpdate>, UpdateStageError> {
        self.validate_root()?;
        if fs::symlink_metadata(self.root.join(COMPLETED_DIRECTORY)).is_ok() {
            return Err(UpdateStageError::InvalidAuthenticatedState(
                "activation outcome",
            ));
        }
        let staging = self.root.join(STAGING_DIRECTORY);
        if staging.exists() {
            remove_incomplete_staging(&staging)?;
        }

        let pending = self.root.join(PENDING_DIRECTORY);
        if pending.exists() {
            validate_pending(&pending, verifier).map(Some)
        } else {
            Ok(None)
        }
    }

    pub(crate) fn prepare_completion(
        &self,
        activated: &StagedUpdate,
    ) -> Result<StagedUpdate, UpdateStageError> {
        self.validate_root()?;
        if fs::symlink_metadata(self.root.join(COMPLETED_DIRECTORY)).is_ok() {
            return Err(UpdateStageError::InvalidAuthenticatedState(
                "activation outcome",
            ));
        }
        let pending = self.root.join(PENDING_DIRECTORY);
        validate_directory(
            &pending,
            "pending directory",
            &[ARTIFACT_FILE, METADATA_FILE, SIGNATURE_FILE],
        )?;
        if fs::canonicalize(pending.join(ARTIFACT_FILE))?
            != fs::canonicalize(activated.artifact_path())?
        {
            return Err(UpdateStageError::InvalidAuthenticatedState(
                "activation token",
            ));
        }
        let completed = self.root.join(COMPLETED_DIRECTORY);
        fs::rename(pending, &completed)?;
        Ok(StagedUpdate {
            version: activated.version.clone(),
            artifact_path: completed.join(ARTIFACT_FILE),
            artifact_size: activated.artifact_size,
            artifact_sha256: activated.artifact_sha256,
        })
    }

    pub(crate) fn cancel_completion(
        &self,
        completing: &StagedUpdate,
    ) -> Result<(), UpdateStageError> {
        self.validate_root()?;
        let completed = self.root.join(COMPLETED_DIRECTORY);
        validate_completion_token(&completed, completing)?;
        let pending = self.root.join(PENDING_DIRECTORY);
        if pending.exists() {
            return Err(UpdateStageError::PendingExists);
        }
        fs::rename(completed, pending)?;
        Ok(())
    }

    pub(crate) fn finish_completion(
        &self,
        completing: &StagedUpdate,
    ) -> Result<(), UpdateStageError> {
        self.validate_root()?;
        let completed = self.root.join(COMPLETED_DIRECTORY);
        validate_completion_token(&completed, completing)?;
        remove_completed(&completed)
    }

    fn stage_from_reader(
        &self,
        verified: &VerifiedUpdate,
        mut source: impl Read,
    ) -> Result<StagedUpdate, UpdateStageError> {
        self.validate_root()?;
        if fs::symlink_metadata(self.root.join(COMPLETED_DIRECTORY)).is_ok() {
            return Err(UpdateStageError::PendingExists);
        }
        let staging = self.root.join(STAGING_DIRECTORY);
        if staging.exists() {
            remove_incomplete_staging(&staging)?;
        }
        if self.root.join(PENDING_DIRECTORY).exists() {
            return Err(UpdateStageError::PendingExists);
        }

        fs::create_dir(&staging)?;
        let temporary_artifact = staging.join(TEMP_ARTIFACT_FILE);
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_artifact)?;
        let (size, hash) = copy_and_hash(&mut source, &mut output)?;
        output.sync_all()?;
        drop(output);

        if size != verified.artifact_size() || &hash != verified.artifact_sha256() {
            return Err(UpdateStageError::VerifiedArtifactChanged);
        }

        let artifact = staging.join(ARTIFACT_FILE);
        fs::rename(&temporary_artifact, &artifact)?;
        write_synced(
            &staging.join(METADATA_FILE),
            verified.authenticated_metadata(),
        )?;
        write_synced(&staging.join(SIGNATURE_FILE), verified.metadata_signature())?;

        let pending = self.root.join(PENDING_DIRECTORY);
        fs::rename(&staging, &pending)?;
        Ok(StagedUpdate::from_verified(
            verified,
            pending.join(ARTIFACT_FILE),
        ))
    }

    fn validate_root(&self) -> Result<(), UpdateStageError> {
        let metadata = fs::symlink_metadata(&self.root)?;
        if metadata.file_type().is_dir() && !metadata.file_type().is_symlink() {
            Ok(())
        } else {
            Err(UpdateStageError::InvalidRoot)
        }
    }
}

fn write_synced(path: &Path, bytes: &[u8]) -> Result<(), UpdateStageError> {
    let mut file = OpenOptions::new().write(true).create_new(true).open(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    Ok(())
}

fn copy_and_hash(
    source: &mut impl Read,
    output: &mut File,
) -> Result<(u64, [u8; 32]), UpdateStageError> {
    let mut buffer = vec![0_u8; COPY_BUFFER_BYTES];
    let mut size = 0_u64;
    let mut hasher = Sha256::new();
    loop {
        let count = source.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        size = size
            .checked_add(count as u64)
            .ok_or(UpdateStageError::VerifiedArtifactChanged)?;
        hasher.update(&buffer[..count]);
    }
    Ok((size, hasher.finalize().into()))
}

fn validate_pending(
    pending: &Path,
    verifier: &UpdateVerifier,
) -> Result<StagedUpdate, UpdateStageError> {
    validate_directory(
        pending,
        "pending directory",
        &[ARTIFACT_FILE, METADATA_FILE, SIGNATURE_FILE],
    )?;
    let metadata_path = pending.join(METADATA_FILE);
    let signature_path = pending.join(SIGNATURE_FILE);
    let metadata = read_bounded(&metadata_path, MAX_METADATA_BYTES, "metadata size")?;
    let signature = read_bounded(&signature_path, SIGNATURE_BYTES, "signature size")?;
    if signature.len() != SIGNATURE_BYTES {
        return Err(UpdateStageError::InvalidAuthenticatedState(
            "signature size",
        ));
    }
    let artifact_path = pending.join(ARTIFACT_FILE);
    let authenticated_update = verifier.verify_staged(&metadata, &signature, &artifact_path)?;
    Ok(StagedUpdate::from_verified(
        &authenticated_update,
        artifact_path,
    ))
}

fn read_bounded(
    path: &Path,
    maximum: usize,
    error_field: &'static str,
) -> Result<Vec<u8>, UpdateStageError> {
    let file = File::open(path)?;
    let limit = u64::try_from(maximum)
        .expect("bounded update state size fits u64")
        .saturating_add(1);
    let mut bytes = Vec::with_capacity(maximum.min(16 * 1024));
    file.take(limit).read_to_end(&mut bytes)?;
    if bytes.len() > maximum {
        return Err(UpdateStageError::InvalidAuthenticatedState(error_field));
    }
    Ok(bytes)
}

fn remove_incomplete_staging(staging: &Path) -> Result<(), UpdateStageError> {
    validate_directory(
        staging,
        "staging directory",
        &[
            TEMP_ARTIFACT_FILE,
            ARTIFACT_FILE,
            METADATA_FILE,
            SIGNATURE_FILE,
        ],
    )?;
    for name in [
        TEMP_ARTIFACT_FILE,
        ARTIFACT_FILE,
        METADATA_FILE,
        SIGNATURE_FILE,
    ] {
        let path = staging.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    fs::remove_dir(staging)?;
    Ok(())
}

fn remove_completed(completed: &Path) -> Result<(), UpdateStageError> {
    validate_directory(
        completed,
        "completed directory",
        &[ARTIFACT_FILE, METADATA_FILE, SIGNATURE_FILE],
    )?;
    for name in [ARTIFACT_FILE, METADATA_FILE, SIGNATURE_FILE] {
        let path = completed.join(name);
        if path.exists() {
            fs::remove_file(path)?;
        }
    }
    fs::remove_dir(completed)?;
    Ok(())
}

fn validate_completion_token(
    completed: &Path,
    completing: &StagedUpdate,
) -> Result<(), UpdateStageError> {
    validate_directory(
        completed,
        "completed directory",
        &[ARTIFACT_FILE, METADATA_FILE, SIGNATURE_FILE],
    )?;
    if fs::canonicalize(completed.join(ARTIFACT_FILE))?
        != fs::canonicalize(completing.artifact_path())?
    {
        return Err(UpdateStageError::InvalidAuthenticatedState(
            "completion token",
        ));
    }
    Ok(())
}

fn validate_directory(
    path: &Path,
    location: &'static str,
    allowed_names: &[&str],
) -> Result<(), UpdateStageError> {
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        return Err(UpdateStageError::UnsafeDirectory(location));
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let entry_type = entry.file_type()?;
        if !allowed_names.contains(&name.as_ref())
            || !entry_type.is_file()
            || entry_type.is_symlink()
        {
            return Err(UpdateStageError::UnsafeEntry {
                location,
                name: name.into_owned(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{
        fs::{self, File},
        io::{self, Read},
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use ed25519_dalek::{Signer, SigningKey};
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{
        COMPLETED_DIRECTORY, MAX_METADATA_BYTES, METADATA_FILE, PENDING_DIRECTORY, SIGNATURE_FILE,
        STAGING_DIRECTORY, TEMP_ARTIFACT_FILE, UpdateStageError, UpdateStageStore,
    };
    use crate::update_verification::{UpdateVerificationError, UpdateVerifier, VerifiedUpdate};

    const TEST_SIGNING_KEY: [u8; 32] = [0x27; 32];

    struct Fixture {
        directory: TempDir,
        verifier: UpdateVerifier,
        verified: VerifiedUpdate,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("temporary directory");
            let artifact_path = directory
                .path()
                .join("download-manager-2.3.4-windows-x64.zip");
            let artifact = vec![0x5a; 3 * super::COPY_BUFFER_BYTES];
            fs::write(&artifact_path, &artifact).expect("write artifact");
            let metadata = serde_json::to_vec(&json!({
                "schema_version": 1,
                "product": "Download Manager",
                "channel": "stable",
                "version": "2.3.4",
                "architecture": "x86_64",
                "artifact": {
                    "file_name": "download-manager-2.3.4-windows-x64.zip",
                    "size": artifact.len(),
                    "sha256": format!("{:x}", Sha256::digest(&artifact))
                }
            }))
            .expect("metadata");
            let signing_key = SigningKey::from_bytes(&TEST_SIGNING_KEY);
            let signature = signing_key.sign(&metadata).to_bytes();
            let update_verifier =
                UpdateVerifier::new(signing_key.verifying_key().to_bytes()).expect("test verifier");
            let verified_update = update_verifier
                .verify(&metadata, &signature, &artifact_path)
                .expect("verified update");
            Self {
                directory,
                verifier: update_verifier,
                verified: verified_update,
            }
        }

        fn store(&self) -> UpdateStageStore {
            UpdateStageStore::open(self.directory.path().join("updates")).expect("stage store")
        }
    }

    struct BlockingReader<R> {
        inner: R,
        signal_path: std::path::PathBuf,
        copied_first_chunk: bool,
    }

    impl<R: Read> Read for BlockingReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            if self.copied_first_chunk {
                fs::write(&self.signal_path, b"first chunk copied")?;
                loop {
                    thread::sleep(Duration::from_secs(1));
                }
            }
            let count = self.inner.read(buffer)?;
            self.copied_first_chunk = true;
            Ok(count)
        }
    }

    #[test]
    fn interrupted_copy_recovers_without_touching_or_breaking_old_app() {
        let fixture = Fixture::new();
        let update_root = fixture.directory.path().join("updates");
        let signal_path = fixture.directory.path().join("stage-blocked.signal");
        let live_root = fixture.directory.path().join("live-install");
        fs::create_dir(&live_root).expect("live root");
        let executable_name = if cfg!(windows) {
            "download-manager-old.exe"
        } else {
            "download-manager-old"
        };
        let old_app = live_root.join(executable_name);
        fs::copy(std::env::current_exe().expect("test executable"), &old_app)
            .expect("old application fixture");
        let old_hash = hash(&old_app);

        let mut child = Command::new(std::env::current_exe().expect("test executable"))
            .args([
                "--ignored",
                "--exact",
                "update_staging::tests::process_kill_stage_child",
            ])
            .env("DOWNLOAD_MANAGER_STAGE_ROOT", &update_root)
            .env(
                "DOWNLOAD_MANAGER_STAGE_ARTIFACT",
                fixture.verified.artifact_path(),
            )
            .env("DOWNLOAD_MANAGER_STAGE_SIGNAL", &signal_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn staging child");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !signal_path.is_file() {
            assert!(
                child.try_wait().expect("query staging child").is_none(),
                "staging child exited before blocking"
            );
            assert!(Instant::now() < deadline, "staging child did not block");
            thread::sleep(Duration::from_millis(5));
        }
        let partial = update_root.join(STAGING_DIRECTORY).join(TEMP_ARTIFACT_FILE);
        assert!(fs::metadata(&partial).expect("partial artifact").len() > 0);
        child.kill().expect("force kill staging child");
        child.wait().expect("reap staging child");

        let restarted = UpdateStageStore::open(&update_root).expect("fresh stage store");
        assert_eq!(restarted.recover(&fixture.verifier).expect("recover"), None);
        assert!(!restarted.root.join(STAGING_DIRECTORY).exists());
        assert_eq!(hash(&old_app), old_hash);
        assert!(
            Command::new(&old_app)
                .arg("--list")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("launch old application fixture")
                .success()
        );
    }

    #[test]
    #[ignore = "helper process launched by the forced-kill staging test"]
    fn process_kill_stage_child() {
        let update_root = std::env::var_os("DOWNLOAD_MANAGER_STAGE_ROOT").expect("stage root");
        let artifact_path =
            std::env::var_os("DOWNLOAD_MANAGER_STAGE_ARTIFACT").expect("artifact path");
        let signal_path = std::env::var_os("DOWNLOAD_MANAGER_STAGE_SIGNAL").expect("signal path");
        let artifact_path = std::path::PathBuf::from(artifact_path);
        let artifact = fs::read(&artifact_path).expect("artifact bytes");
        let metadata = serde_json::to_vec(&json!({
            "schema_version": 1,
            "product": "Download Manager",
            "channel": "stable",
            "version": "2.3.4",
            "architecture": "x86_64",
            "artifact": {
                "file_name": "download-manager-2.3.4-windows-x64.zip",
                "size": artifact.len(),
                "sha256": format!("{:x}", Sha256::digest(&artifact))
            }
        }))
        .expect("metadata");
        let signing_key = SigningKey::from_bytes(&TEST_SIGNING_KEY);
        let signature = signing_key.sign(&metadata).to_bytes();
        let update_verifier =
            UpdateVerifier::new(signing_key.verifying_key().to_bytes()).expect("test verifier");
        let verified_update = update_verifier
            .verify(&metadata, &signature, &artifact_path)
            .expect("verified update");
        let store = UpdateStageStore::open(update_root).expect("stage store");
        let source = File::open(&artifact_path).expect("artifact reader");
        let blocking = BlockingReader {
            inner: source,
            signal_path: signal_path.into(),
            copied_first_chunk: false,
        };
        store
            .stage_from_reader(&verified_update, blocking)
            .expect("parent must kill this process while blocked");
    }

    #[test]
    fn ready_stage_survives_restart_and_refuses_replacement() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let staged = store.stage(&fixture.verified).expect("stage update");
        assert_eq!(staged.version().to_string(), "2.3.4");
        assert_eq!(staged.artifact_size(), fixture.verified.artifact_size());
        assert_eq!(staged.artifact_sha256(), fixture.verified.artifact_sha256());
        assert_eq!(
            fs::read(staged.artifact_path()).expect("staged artifact"),
            fs::read(fixture.verified.artifact_path()).expect("source artifact")
        );
        drop(store);

        let restarted = fixture.store();
        let first = restarted
            .recover(&fixture.verifier)
            .expect("first recovery")
            .expect("ready update");
        let second = restarted
            .recover(&fixture.verifier)
            .expect("second recovery")
            .expect("ready update");
        assert_eq!(first, second);
        assert!(matches!(
            restarted.stage(&fixture.verified),
            Err(UpdateStageError::PendingExists)
        ));
    }

    #[test]
    fn unresolved_completion_marker_is_preserved_for_fault_reconciliation() {
        let fixture = Fixture::new();
        let store = fixture.store();
        let staged = store.stage(&fixture.verified).expect("stage update");
        let completing = store
            .prepare_completion(&staged)
            .expect("prepare completion marker");

        assert!(matches!(
            store.recover(&fixture.verifier),
            Err(UpdateStageError::InvalidAuthenticatedState(
                "activation outcome"
            ))
        ));
        assert!(completing.artifact_path().is_file());
        assert!(store.root.join(COMPLETED_DIRECTORY).is_dir());

        store
            .cancel_completion(&completing)
            .expect("restore pending marker");
        assert!(store.recover(&fixture.verifier).expect("recover").is_some());
    }

    #[test]
    fn tampered_or_rewritten_pending_state_is_preserved_and_rejected() {
        let tampered = Fixture::new();
        let store = tampered.store();
        let staged = store.stage(&tampered.verified).expect("stage update");
        let mut bytes = fs::read(staged.artifact_path()).expect("artifact");
        bytes[0] ^= 0xff;
        fs::write(staged.artifact_path(), &bytes).expect("tamper artifact");
        assert!(matches!(
            store.recover(&tampered.verifier),
            Err(UpdateStageError::Verification(
                UpdateVerificationError::ArtifactHashMismatch
            ))
        ));
        assert_eq!(
            fs::read(staged.artifact_path()).expect("preserved tampered artifact"),
            bytes
        );

        let rewritten = Fixture::new();
        let rewritten_store = rewritten.store();
        let rewritten_stage = rewritten_store
            .stage(&rewritten.verified)
            .expect("stage rewritten update");
        let replacement = b"coordinated artifact and metadata replacement";
        fs::write(rewritten_stage.artifact_path(), replacement).expect("replace artifact");
        let forged_metadata = serde_json::to_vec(&json!({
            "schema_version": 1,
            "product": "Download Manager",
            "channel": "stable",
            "version": "9.9.9",
            "architecture": "x86_64",
            "artifact": {
                "file_name": "download-manager-9.9.9-windows-x64.zip",
                "size": replacement.len(),
                "sha256": format!("{:x}", Sha256::digest(replacement))
            }
        }))
        .expect("forged metadata");
        let metadata_path = rewritten_store
            .root
            .join(PENDING_DIRECTORY)
            .join(METADATA_FILE);
        fs::write(&metadata_path, &forged_metadata).expect("replace metadata");
        assert!(matches!(
            rewritten_store.recover(&rewritten.verifier),
            Err(UpdateStageError::Verification(
                UpdateVerificationError::UntrustedSignature
            ))
        ));
        assert_eq!(
            fs::read(metadata_path).expect("preserved forged metadata"),
            forged_metadata
        );

        let oversized = Fixture::new();
        let oversized_store = oversized.store();
        oversized_store
            .stage(&oversized.verified)
            .expect("stage oversized-manifest fixture");
        let oversized_metadata = oversized_store
            .root
            .join(PENDING_DIRECTORY)
            .join(METADATA_FILE);
        let oversized_bytes = vec![b' '; MAX_METADATA_BYTES + 1];
        fs::write(&oversized_metadata, &oversized_bytes).expect("oversize metadata");
        assert!(matches!(
            oversized_store.recover(&oversized.verifier),
            Err(UpdateStageError::InvalidAuthenticatedState("metadata size"))
        ));
        assert_eq!(
            usize::try_from(
                fs::metadata(oversized_metadata)
                    .expect("preserved oversized metadata")
                    .len()
            )
            .expect("metadata length fits usize"),
            MAX_METADATA_BYTES + 1
        );

        let short_signature = Fixture::new();
        let signature_store = short_signature.store();
        signature_store
            .stage(&short_signature.verified)
            .expect("stage short-signature fixture");
        let signature_path = signature_store
            .root
            .join(PENDING_DIRECTORY)
            .join(SIGNATURE_FILE);
        fs::write(&signature_path, [0_u8; 63]).expect("shorten signature");
        assert!(matches!(
            signature_store.recover(&short_signature.verifier),
            Err(UpdateStageError::InvalidAuthenticatedState(
                "signature size"
            ))
        ));
    }

    #[test]
    fn source_mutation_after_verification_never_publishes_pending() {
        let fixture = Fixture::new();
        fs::write(
            fixture.verified.artifact_path(),
            b"changed after verification",
        )
        .expect("change source");
        let store = fixture.store();
        assert!(matches!(
            store.stage(&fixture.verified),
            Err(UpdateStageError::VerifiedArtifactChanged)
        ));
        assert!(!store.root.join(PENDING_DIRECTORY).exists());
        drop(store);
        let restarted = fixture.store();
        assert_eq!(
            restarted
                .recover(&fixture.verifier)
                .expect("recover partial"),
            None
        );
    }

    fn hash(path: &std::path::Path) -> [u8; 32] {
        Sha256::digest(fs::read(path).expect("read fixture")).into()
    }
}
