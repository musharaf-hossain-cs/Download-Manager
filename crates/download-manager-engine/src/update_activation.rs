//! Transactional activation and rollback for a cryptographically staged application update.

use std::{
    fmt,
    fs::{self, OpenOptions},
    io,
    path::{Path, PathBuf},
    time::Duration,
};

use rusqlite::{Connection, OpenFlags};
use semver::Version;
use thiserror::Error;

use crate::{
    domain::{JobId, Segment},
    request_context::RequestContext,
    state::DownloadState,
    storage::{AuditEvent, Database, JobRecord, QueueRecord, StorageError},
    update_staging::{StagedUpdate, UpdateStageError, UpdateStageStore},
    update_verification::UpdateVerifier,
};

/// Ordered phases of the external application replacement contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateActivationPhase {
    Prepare,
    Activate,
    HealthCheck,
    CompatibilityCheck,
    Commit,
}

impl fmt::Display for UpdateActivationPhase {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Prepare => "prepare",
            Self::Activate => "activate",
            Self::HealthCheck => "health_check",
            Self::CompatibilityCheck => "compatibility_check",
            Self::Commit => "commit",
        })
    }
}

/// Packaging boundary used by the engine while an external updater owns the application files.
///
/// `prepare` must create a rollback point without changing the launch target. After it succeeds,
/// `rollback` must remain available until `commit` returns successfully. Implementations must not
/// consume or rewrite the authenticated staged artifact. Adapter methods must not change durable
/// engine state directly; only candidate startup during `health_check` may run a schema migration.
pub trait UpdateActivationAdapter {
    type Error: fmt::Display;

    /// Creates the application rollback point without changing the launch target.
    ///
    /// # Errors
    /// Returns an adapter error if the previous application cannot be preserved safely.
    fn prepare(&mut self, update: &StagedUpdate) -> Result<(), Self::Error>;

    /// Makes the candidate application the launch target without consuming its staged artifact.
    ///
    /// # Errors
    /// Returns an adapter error if the candidate cannot be installed or selected completely.
    fn activate(&mut self, update: &StagedUpdate) -> Result<(), Self::Error>;

    /// Starts or probes the candidate strongly enough to accept it for normal launches.
    ///
    /// # Errors
    /// Returns an adapter error if candidate startup or its required health checks fail.
    fn health_check(&mut self, update: &StagedUpdate) -> Result<(), Self::Error>;

    /// Irreversibly accepts the healthy candidate and discards the application rollback point.
    ///
    /// # Errors
    /// Returns an adapter error while rollback is still possible if commit cannot finish.
    fn commit(&mut self, update: &StagedUpdate) -> Result<(), Self::Error>;

    /// Stops the candidate, closes its database handles, and prevents any launch during rollback.
    ///
    /// The launch inhibition must remain active until [`Self::rollback`] succeeds.
    ///
    /// # Errors
    /// Returns an adapter error if candidate activity or new launches cannot be stopped safely.
    fn quiesce_for_rollback(&mut self) -> Result<(), Self::Error>;

    /// Restores the exact application preserved by [`Self::prepare`] and re-enables launches.
    ///
    /// # Errors
    /// Returns an adapter error if the previous application cannot be restored completely.
    fn rollback(&mut self, update: &StagedUpdate) -> Result<(), Self::Error>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateActivationOutcome {
    Activated {
        version: Version,
    },
    RolledBack {
        version: Version,
        failed_phase: UpdateActivationPhase,
    },
}

#[derive(Debug, Error)]
pub enum UpdateActivationError {
    #[error("the engine database must be an existing regular file")]
    InvalidDatabase,
    #[error("the update rollback database path already exists")]
    BackupExists,
    #[error("the update rollback database path must be valid Unicode")]
    InvalidBackupPath,
    #[error("job {job_id} is still in non-quiescent state {state}")]
    NonQuiescentJob { job_id: JobId, state: DownloadState },
    #[error("durable update state failed: {0}")]
    Storage(#[from] StorageError),
    #[error("the authenticated staged update is unavailable")]
    NoPendingUpdate,
    #[error("staged update validation failed: {0}")]
    Stage(#[from] UpdateStageError),
    #[error("SQLite update backup failed: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("update rollback filesystem operation failed: {0}")]
    Io(#[from] io::Error),
    #[error("application update {phase} failed before activation: {detail}")]
    Adapter {
        phase: UpdateActivationPhase,
        detail: String,
    },
    #[error(
        "update failed during {failed_phase} ({activation_detail}) and rollback was incomplete: application={application_detail}; state={state_detail}"
    )]
    RollbackFailed {
        failed_phase: UpdateActivationPhase,
        activation_detail: String,
        application_detail: String,
        state_detail: String,
    },
    #[error("update {phase} succeeded but rollback-state cleanup failed: {detail}")]
    Cleanup {
        phase: UpdateActivationPhase,
        detail: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CompatibilitySnapshot {
    jobs: Vec<JobRecord>,
    queues: Vec<QueueRecord>,
    resumable: Vec<JobDurableState>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct JobDurableState {
    job_id: JobId,
    segments: Vec<Segment>,
    events: Vec<AuditEvent>,
    request_context: Option<RequestContext>,
}

impl CompatibilitySnapshot {
    fn load(database_path: &Path) -> Result<Self, StorageError> {
        let database = Database::open(database_path)?;
        let jobs = database.list_jobs()?;
        let queues = database.list_queues()?;
        let resumable = jobs
            .iter()
            .map(|job| {
                Ok(JobDurableState {
                    job_id: job.metadata.id,
                    segments: database.segments(job.metadata.id)?,
                    events: database.events(job.metadata.id)?,
                    request_context: database.request_context(job.metadata.id)?,
                })
            })
            .collect::<Result<_, StorageError>>()?;
        Ok(Self {
            jobs,
            queues,
            resumable,
        })
    }

    fn require_quiescent(&self) -> Result<(), UpdateActivationError> {
        for job in &self.jobs {
            let state = job.metadata.state;
            if matches!(
                state,
                DownloadState::Probing
                    | DownloadState::Connecting
                    | DownloadState::Downloading
                    | DownloadState::Pausing
                    | DownloadState::Verifying
                    | DownloadState::Assembling
            ) {
                return Err(UpdateActivationError::NonQuiescentJob {
                    job_id: job.metadata.id,
                    state,
                });
            }
        }
        Ok(())
    }
}

struct AttemptFailure {
    phase: UpdateActivationPhase,
    detail: String,
}

struct RestoredDatabase {
    displaced_path: PathBuf,
}

struct RollbackContext<'a, A> {
    stage_store: &'a UpdateStageStore,
    database_path: &'a Path,
    backup_path: &'a Path,
    adapter: &'a mut A,
    before: &'a CompatibilitySnapshot,
}

/// Activates one S157-authenticated update while preserving a rollback-compatible engine database.
///
/// The caller must stop transfer workers and close every database connection first. A candidate is
/// accepted only if its health check passes and all jobs, queue order, segments, durable offsets,
/// remote identities, and events still match. Any post-prepare failure invokes application rollback,
/// restores the consistent SQLite backup, and verifies the restored durable snapshot before success
/// is reported as [`UpdateActivationOutcome::RolledBack`].
///
/// # Errors
/// Returns [`UpdateActivationError`] for a live writer, invalid backup location, state failure,
/// pre-switch adapter failure, incomplete rollback, or terminal cleanup failure.
pub fn activate_update<A: UpdateActivationAdapter>(
    stage_store: &UpdateStageStore,
    verifier: &UpdateVerifier,
    database_path: impl AsRef<Path>,
    backup_path: impl AsRef<Path>,
    adapter: &mut A,
) -> Result<UpdateActivationOutcome, UpdateActivationError> {
    let update = stage_store
        .recover(verifier)?
        .ok_or(UpdateActivationError::NoPendingUpdate)?;
    let database_path = database_path.as_ref();
    let backup_path = backup_path.as_ref();
    validate_database(database_path)?;
    let before = CompatibilitySnapshot::load(database_path)?;
    before.require_quiescent()?;
    create_consistent_backup(database_path, backup_path)?;

    if let Err(error) = adapter.prepare(&update) {
        remove_backup(backup_path, UpdateActivationPhase::Prepare)?;
        return Err(UpdateActivationError::Adapter {
            phase: UpdateActivationPhase::Prepare,
            detail: error.to_string(),
        });
    }

    let attempt = run_attempt(&update, database_path, adapter, &before);
    if let Err(failure) = attempt {
        return rollback(
            &update,
            None,
            RollbackContext {
                stage_store,
                database_path,
                backup_path,
                adapter,
                before: &before,
            },
            failure,
        );
    }

    let completing = match stage_store.prepare_completion(&update) {
        Ok(completing) => completing,
        Err(error) => {
            return rollback(
                &update,
                None,
                RollbackContext {
                    stage_store,
                    database_path,
                    backup_path,
                    adapter,
                    before: &before,
                },
                AttemptFailure {
                    phase: UpdateActivationPhase::Commit,
                    detail: error.to_string(),
                },
            );
        }
    };
    if let Err(error) = adapter.commit(&completing) {
        return rollback(
            &update,
            Some(&completing),
            RollbackContext {
                stage_store,
                database_path,
                backup_path,
                adapter,
                before: &before,
            },
            AttemptFailure {
                phase: UpdateActivationPhase::Commit,
                detail: error.to_string(),
            },
        );
    }
    stage_store
        .finish_completion(&completing)
        .map_err(|error| UpdateActivationError::Cleanup {
            phase: UpdateActivationPhase::Commit,
            detail: error.to_string(),
        })?;
    remove_backup(backup_path, UpdateActivationPhase::Commit)?;
    Ok(UpdateActivationOutcome::Activated {
        version: update.version().clone(),
    })
}

fn run_attempt<A: UpdateActivationAdapter>(
    update: &StagedUpdate,
    database_path: &Path,
    adapter: &mut A,
    before: &CompatibilitySnapshot,
) -> Result<(), AttemptFailure> {
    adapter.activate(update).map_err(|error| AttemptFailure {
        phase: UpdateActivationPhase::Activate,
        detail: error.to_string(),
    })?;
    adapter
        .health_check(update)
        .map_err(|error| AttemptFailure {
            phase: UpdateActivationPhase::HealthCheck,
            detail: error.to_string(),
        })?;
    let after = CompatibilitySnapshot::load(database_path).map_err(|error| AttemptFailure {
        phase: UpdateActivationPhase::CompatibilityCheck,
        detail: error.to_string(),
    })?;
    if &after != before {
        return Err(AttemptFailure {
            phase: UpdateActivationPhase::CompatibilityCheck,
            detail: "durable jobs, queues, segments, or events changed".to_owned(),
        });
    }
    Ok(())
}

fn rollback<A: UpdateActivationAdapter>(
    update: &StagedUpdate,
    completion: Option<&StagedUpdate>,
    context: RollbackContext<'_, A>,
    failure: AttemptFailure,
) -> Result<UpdateActivationOutcome, UpdateActivationError> {
    let RollbackContext {
        stage_store,
        database_path,
        backup_path,
        adapter,
        before,
    } = context;
    let quiesced = adapter
        .quiesce_for_rollback()
        .map_err(|error| error.to_string());
    if let Err(application_detail) = quiesced {
        return Err(UpdateActivationError::RollbackFailed {
            failed_phase: failure.phase,
            activation_detail: failure.detail,
            application_detail,
            state_detail: "not restored because launch inhibition failed".to_owned(),
        });
    }

    let restored_database = match restore_backup(database_path, backup_path) {
        Ok(restored) => restored,
        Err(error) => {
            return Err(UpdateActivationError::RollbackFailed {
                failed_phase: failure.phase,
                activation_detail: failure.detail,
                application_detail: "candidate stopped and launches remain inhibited".to_owned(),
                state_detail: error.to_string(),
            });
        }
    };
    let restored_snapshot = CompatibilitySnapshot::load(database_path);
    if !matches!(restored_snapshot, Ok(ref restored) if restored == before) {
        let state_detail = restored_snapshot.map_or_else(
            |error| error.to_string(),
            |_| "restored database does not match its pre-update snapshot".to_owned(),
        );
        return Err(UpdateActivationError::RollbackFailed {
            failed_phase: failure.phase,
            activation_detail: failure.detail,
            application_detail: "candidate stopped and launches remain inhibited".to_owned(),
            state_detail,
        });
    }
    if let Some(completing) = completion
        && let Err(error) = stage_store.cancel_completion(completing)
    {
        return Err(UpdateActivationError::RollbackFailed {
            failed_phase: failure.phase,
            activation_detail: failure.detail,
            application_detail: "candidate stopped and launches remain inhibited".to_owned(),
            state_detail: format!("state restored, but pending update evidence was not: {error}"),
        });
    }
    if let Err(error) = adapter.rollback(update) {
        return Err(UpdateActivationError::RollbackFailed {
            failed_phase: failure.phase,
            activation_detail: failure.detail,
            application_detail: error.to_string(),
            state_detail: "restored".to_owned(),
        });
    }

    remove_cleanup_file(&restored_database.displaced_path, failure.phase)?;
    remove_backup(backup_path, failure.phase)?;
    Ok(UpdateActivationOutcome::RolledBack {
        version: update.version().clone(),
        failed_phase: failure.phase,
    })
}

fn validate_database(path: &Path) -> Result<(), UpdateActivationError> {
    let metadata =
        fs::symlink_metadata(path).map_err(|_| UpdateActivationError::InvalidDatabase)?;
    if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
        return Err(UpdateActivationError::InvalidDatabase);
    }
    Ok(())
}

fn create_consistent_backup(
    database_path: &Path,
    backup_path: &Path,
) -> Result<(), UpdateActivationError> {
    let parent = backup_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .ok_or(UpdateActivationError::InvalidBackupPath)?;
    fs::create_dir_all(parent)?;
    let parent_metadata = fs::symlink_metadata(parent)?;
    if !parent_metadata.file_type().is_dir() || parent_metadata.file_type().is_symlink() {
        return Err(UpdateActivationError::InvalidBackupPath);
    }
    if fs::symlink_metadata(backup_path).is_ok() {
        return Err(UpdateActivationError::BackupExists);
    }
    let resolved_database = fs::canonicalize(database_path)?;
    let resolved_parent = fs::canonicalize(parent)?;
    let resolved_backup = resolved_parent.join(
        backup_path
            .file_name()
            .ok_or(UpdateActivationError::InvalidBackupPath)?,
    );
    if reserved_database_paths(&resolved_database)
        .iter()
        .any(|reserved| paths_equal(reserved, &resolved_backup))
    {
        return Err(UpdateActivationError::InvalidBackupPath);
    }
    let backup = backup_path
        .to_str()
        .ok_or(UpdateActivationError::InvalidBackupPath)?;
    let connection = Connection::open_with_flags(
        database_path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
    connection.execute("VACUUM main INTO ?1", [backup])?;
    drop(connection);
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(backup_path)?
        .sync_all()?;
    Ok(())
}

fn restore_backup(
    database_path: &Path,
    backup_path: &Path,
) -> Result<RestoredDatabase, UpdateActivationError> {
    validate_database(backup_path)?;
    validate_database(database_path)?;
    let [_, _, temporary_path, displaced_path] = reserved_database_paths(database_path);
    for path in [&temporary_path, &displaced_path] {
        if fs::symlink_metadata(path).is_ok() {
            return Err(UpdateActivationError::BackupExists);
        }
    }
    fs::copy(backup_path, &temporary_path)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(&temporary_path)?
        .sync_all()?;
    for sidecar in sqlite_sidecars(database_path) {
        match fs::remove_file(sidecar) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    fs::rename(database_path, &displaced_path)?;
    if let Err(error) = fs::rename(&temporary_path, database_path) {
        if let Err(restore_error) = fs::rename(&displaced_path, database_path) {
            return Err(UpdateActivationError::Io(io::Error::new(
                restore_error.kind(),
                format!(
                    "failed to publish restored database ({error}) and failed to put candidate database back ({restore_error})"
                ),
            )));
        }
        return Err(error.into());
    }
    Ok(RestoredDatabase { displaced_path })
}

fn sqlite_sidecars(database_path: &Path) -> [PathBuf; 2] {
    let mut wal = database_path.as_os_str().to_owned();
    wal.push("-wal");
    let mut shared_memory = database_path.as_os_str().to_owned();
    shared_memory.push("-shm");
    [wal.into(), shared_memory.into()]
}

fn reserved_database_paths(database_path: &Path) -> [PathBuf; 4] {
    let [wal, shared_memory] = sqlite_sidecars(database_path);
    let mut temporary = database_path.as_os_str().to_owned();
    temporary.push(".update-restore.tmp");
    let mut displaced = database_path.as_os_str().to_owned();
    displaced.push(".update-displaced.tmp");
    [wal, shared_memory, temporary.into(), displaced.into()]
}

fn paths_equal(left: &Path, right: &Path) -> bool {
    if cfg!(windows) {
        left.to_string_lossy()
            .eq_ignore_ascii_case(&right.to_string_lossy())
    } else {
        left == right
    }
}

fn remove_backup(
    backup_path: &Path,
    phase: UpdateActivationPhase,
) -> Result<(), UpdateActivationError> {
    remove_cleanup_file(backup_path, phase)
}

fn remove_cleanup_file(
    path: &Path,
    phase: UpdateActivationPhase,
) -> Result<(), UpdateActivationError> {
    fs::remove_file(path).map_err(|error| UpdateActivationError::Cleanup {
        phase,
        detail: error.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::{Path, PathBuf},
        process::{Command, Stdio},
    };

    use ed25519_dalek::{Signer, SigningKey};
    use rusqlite::Connection;
    use serde_json::json;
    use sha2::{Digest, Sha256};
    use tempfile::TempDir;

    use super::{
        CompatibilitySnapshot, UpdateActivationAdapter, UpdateActivationError,
        UpdateActivationOutcome, UpdateActivationPhase, activate_update, create_consistent_backup,
        reserved_database_paths,
    };
    use crate::{
        credentials::CredentialRef,
        domain::{
            DownloadKind, HttpUrl, JobId, JobMetadata, Priority, Segment, SegmentId, UnixMillis,
        },
        request_context::RequestContext,
        state::DownloadState,
        storage::{AuditEvent, Database},
        update_staging::{StagedUpdate, UpdateStageError, UpdateStageStore},
        update_verification::{UpdateVerificationError, UpdateVerifier},
    };

    const TEST_SIGNING_KEY: [u8; 32] = [0x48; 32];

    struct Fixture {
        directory: TempDir,
        database_path: PathBuf,
        backup_path: PathBuf,
        partial_path: PathBuf,
    }

    struct StagedFixture {
        store: UpdateStageStore,
        verifier: UpdateVerifier,
        staged: StagedUpdate,
    }

    impl Fixture {
        fn new() -> Self {
            let directory = TempDir::new().expect("temporary directory");
            let database_path = directory.path().join("state/state.sqlite");
            let backup_path = directory.path().join("activation/state.pre-update.sqlite");
            let partial_path = directory.path().join("partials/job.part");
            fs::create_dir_all(partial_path.parent().expect("partial parent"))
                .expect("partial directory");
            fs::write(&partial_path, b"durable partial bytes").expect("partial bytes");
            seed_paused_jobs(&database_path, &partial_path);
            Self {
                directory,
                database_path,
                backup_path,
                partial_path,
            }
        }

        fn stage(&self, artifact: &[u8]) -> StagedFixture {
            let release = self
                .directory
                .path()
                .join("download-manager-2.3.4-windows-x64.zip");
            fs::write(&release, artifact).expect("release artifact");
            let metadata = serde_json::to_vec(&json!({
                "schema_version": 1,
                "product": "Download Manager",
                "channel": "stable",
                "version": "2.3.4",
                "architecture": "x86_64",
                "artifact": {
                    "file_name": "download-manager-2.3.4-windows-x64.zip",
                    "size": artifact.len(),
                    "sha256": format!("{:x}", Sha256::digest(artifact))
                }
            }))
            .expect("metadata");
            let key = SigningKey::from_bytes(&TEST_SIGNING_KEY);
            let verifier = UpdateVerifier::new(key.verifying_key().to_bytes()).expect("verifier");
            let authenticated_update = verifier
                .verify(&metadata, &key.sign(&metadata).to_bytes(), &release)
                .expect("verified update");
            let store =
                UpdateStageStore::open(self.directory.path().join("updates")).expect("stage store");
            let staged = store.stage(&authenticated_update).expect("staged update");
            StagedFixture {
                store,
                verifier,
                staged,
            }
        }
    }

    #[derive(Clone, Copy)]
    enum Health {
        Pass,
        FailAfterStateMutation,
    }

    struct FileAdapter {
        live_app: PathBuf,
        rollback_app: PathBuf,
        database_path: PathBuf,
        health: Health,
        fail_commit: bool,
        launches_inhibited: bool,
    }

    impl UpdateActivationAdapter for FileAdapter {
        type Error = &'static str;

        fn prepare(&mut self, _update: &StagedUpdate) -> Result<(), Self::Error> {
            fs::copy(&self.live_app, &self.rollback_app).map_err(|_| "backup old app")?;
            Ok(())
        }

        fn activate(&mut self, update: &StagedUpdate) -> Result<(), Self::Error> {
            fs::copy(update.artifact_path(), &self.live_app).map_err(|_| "replace app")?;
            Ok(())
        }

        fn health_check(&mut self, _update: &StagedUpdate) -> Result<(), Self::Error> {
            match self.health {
                Health::Pass => {
                    let connection = Connection::open(&self.database_path)
                        .map_err(|_| "open candidate database")?;
                    connection
                        .execute_batch(
                            "CREATE TABLE candidate_schema_probe (version INTEGER NOT NULL);",
                        )
                        .map_err(|_| "candidate migration")?;
                    drop(connection);
                    Command::new(&self.live_app)
                        .arg("--list")
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .status()
                        .map_err(|_| "launch candidate")?
                        .success()
                        .then_some(())
                        .ok_or("candidate launch failed")
                }
                Health::FailAfterStateMutation => {
                    let connection = Connection::open(&self.database_path)
                        .map_err(|_| "open candidate database")?;
                    connection
                        .execute_batch(
                            "UPDATE jobs SET state='failed', downloaded_bytes=0; \
                             UPDATE segments SET durable_offset=range_start, worker_state='idle'; \
                             DELETE FROM request_contexts;",
                        )
                        .map_err(|_| "mutate candidate state")?;
                    Err("injected candidate health failure")
                }
            }
        }

        fn commit(&mut self, _update: &StagedUpdate) -> Result<(), Self::Error> {
            if self.fail_commit {
                return Err("injected commit failure");
            }
            fs::remove_file(&self.rollback_app).map_err(|_| "commit app")
        }

        fn quiesce_for_rollback(&mut self) -> Result<(), Self::Error> {
            self.launches_inhibited = true;
            Ok(())
        }

        fn rollback(&mut self, _update: &StagedUpdate) -> Result<(), Self::Error> {
            if !self.launches_inhibited {
                return Err("launches were not inhibited");
            }
            fs::copy(&self.rollback_app, &self.live_app).map_err(|_| "restore old app")?;
            fs::remove_file(&self.rollback_app).map_err(|_| "remove app rollback")?;
            self.launches_inhibited = false;
            Ok(())
        }
    }

    #[test]
    fn forced_activation_failure_rolls_back_old_app_and_preserves_paused_jobs() {
        let fixture = Fixture::new();
        let old_app = fixture.directory.path().join(executable_name("old-app"));
        fs::copy(std::env::current_exe().expect("test executable"), &old_app)
            .expect("old application");
        let old_hash = hash(&old_app);
        let staged = fixture.stage(b"broken candidate application");
        let pending_hash = hash(staged.staged.artifact_path());
        let before = CompatibilitySnapshot::load(&fixture.database_path).expect("snapshot");
        let partial_hash = hash(&fixture.partial_path);
        let rollback_app = fixture.directory.path().join("old-app.rollback");
        let mut adapter = FileAdapter {
            live_app: old_app.clone(),
            rollback_app,
            database_path: fixture.database_path.clone(),
            health: Health::FailAfterStateMutation,
            fail_commit: false,
            launches_inhibited: false,
        };

        let outcome = activate_update(
            &staged.store,
            &staged.verifier,
            &fixture.database_path,
            &fixture.backup_path,
            &mut adapter,
        )
        .expect("successful rollback");

        assert_eq!(
            outcome,
            UpdateActivationOutcome::RolledBack {
                version: "2.3.4".parse().expect("version"),
                failed_phase: UpdateActivationPhase::HealthCheck,
            }
        );
        assert_eq!(hash(&old_app), old_hash);
        assert!(
            Command::new(&old_app)
                .arg("--list")
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .expect("launch restored app")
                .success()
        );
        assert_eq!(
            CompatibilitySnapshot::load(&fixture.database_path).expect("restored snapshot"),
            before
        );
        assert_eq!(hash(&fixture.partial_path), partial_hash);
        assert_eq!(hash(staged.staged.artifact_path()), pending_hash);
        assert!(!fixture.backup_path.exists());
    }

    #[test]
    fn successful_activation_preserves_paused_jobs_queues_and_offsets() {
        let fixture = Fixture::new();
        let old_app = fixture.directory.path().join(executable_name("live-app"));
        fs::write(&old_app, b"old application").expect("old app");
        let candidate = fs::read(std::env::current_exe().expect("test executable"))
            .expect("candidate executable");
        let staged = fixture.stage(&candidate);
        let candidate_hash = hash(staged.staged.artifact_path());
        let before = CompatibilitySnapshot::load(&fixture.database_path).expect("snapshot");
        let partial_hash = hash(&fixture.partial_path);
        let rollback_app = fixture.directory.path().join("live-app.rollback");
        let mut adapter = FileAdapter {
            live_app: old_app.clone(),
            rollback_app: rollback_app.clone(),
            database_path: fixture.database_path.clone(),
            health: Health::Pass,
            fail_commit: false,
            launches_inhibited: false,
        };

        let outcome = activate_update(
            &staged.store,
            &staged.verifier,
            &fixture.database_path,
            &fixture.backup_path,
            &mut adapter,
        )
        .expect("activate update");

        assert_eq!(
            outcome,
            UpdateActivationOutcome::Activated {
                version: "2.3.4".parse().expect("version"),
            }
        );
        assert_eq!(hash(&old_app), candidate_hash);
        assert_eq!(
            CompatibilitySnapshot::load(&fixture.database_path).expect("post-update snapshot"),
            before
        );
        assert_eq!(hash(&fixture.partial_path), partial_hash);
        assert!(!staged.staged.artifact_path().exists());
        assert!(!rollback_app.exists());
        assert!(!fixture.backup_path.exists());
        let next = fixture.stage(b"next signed update");
        assert!(next.staged.artifact_path().is_file());
    }

    #[test]
    fn commit_failure_restores_pending_marker_app_and_state() {
        let fixture = Fixture::new();
        let old_app = fixture.directory.path().join(executable_name("live-app"));
        fs::copy(std::env::current_exe().expect("test executable"), &old_app)
            .expect("old application");
        let old_hash = hash(&old_app);
        let candidate = fs::read(std::env::current_exe().expect("test executable"))
            .expect("candidate executable");
        let staged = fixture.stage(&candidate);
        let before = CompatibilitySnapshot::load(&fixture.database_path).expect("snapshot");
        let rollback_app = fixture.directory.path().join("live-app.rollback");
        let mut adapter = FileAdapter {
            live_app: old_app.clone(),
            rollback_app: rollback_app.clone(),
            database_path: fixture.database_path.clone(),
            health: Health::Pass,
            fail_commit: true,
            launches_inhibited: false,
        };

        let outcome = activate_update(
            &staged.store,
            &staged.verifier,
            &fixture.database_path,
            &fixture.backup_path,
            &mut adapter,
        )
        .expect("commit rollback");

        assert_eq!(
            outcome,
            UpdateActivationOutcome::RolledBack {
                version: "2.3.4".parse().expect("version"),
                failed_phase: UpdateActivationPhase::Commit,
            }
        );
        assert_eq!(hash(&old_app), old_hash);
        assert_eq!(
            CompatibilitySnapshot::load(&fixture.database_path).expect("restored snapshot"),
            before
        );
        let pending = staged
            .store
            .recover(&staged.verifier)
            .expect("recover pending")
            .expect("pending update restored");
        assert!(pending.artifact_path().is_file());
        assert!(!rollback_app.exists());
        assert!(!fixture.backup_path.exists());
    }

    #[test]
    fn tampered_pending_artifact_is_rejected_again_at_activation_boundary() {
        let fixture = Fixture::new();
        let old_app = fixture.directory.path().join(executable_name("live-app"));
        fs::write(&old_app, b"old application").expect("old app");
        let old_hash = hash(&old_app);
        let staged = fixture.stage(b"authenticated candidate");
        fs::write(staged.staged.artifact_path(), b"tampered candidate").expect("tamper pending");
        let rollback_app = fixture.directory.path().join("live-app.rollback");
        let mut adapter = FileAdapter {
            live_app: old_app.clone(),
            rollback_app: rollback_app.clone(),
            database_path: fixture.database_path.clone(),
            health: Health::Pass,
            fail_commit: false,
            launches_inhibited: false,
        };

        assert!(matches!(
            activate_update(
                &staged.store,
                &staged.verifier,
                &fixture.database_path,
                &fixture.backup_path,
                &mut adapter,
            ),
            Err(UpdateActivationError::Stage(
                UpdateStageError::Verification(
                    UpdateVerificationError::ArtifactSizeMismatch { .. }
                )
            ))
        ));
        assert_eq!(hash(&old_app), old_hash);
        assert!(!rollback_app.exists());
        assert!(!fixture.backup_path.exists());
    }

    #[test]
    fn rollback_backup_refuses_database_sidecar_and_swap_path_aliases() {
        let fixture = Fixture::new();
        for reserved in reserved_database_paths(&fixture.database_path) {
            if reserved.exists() {
                fs::remove_file(&reserved).expect("remove closed SQLite sidecar");
            }
            assert!(matches!(
                create_consistent_backup(&fixture.database_path, &reserved),
                Err(UpdateActivationError::InvalidBackupPath)
            ));
            assert!(!reserved.exists());
        }
    }

    fn seed_paused_jobs(database_path: &Path, partial_path: &Path) {
        let mut database = Database::open(database_path).expect("database");
        database
            .create_queue("Later", timestamp(1))
            .expect("later queue");
        database
            .create_queue("Primary", timestamp(2))
            .expect("primary queue");

        let mut first = job(partial_path, "Later", 1);
        first.state = DownloadState::Pausing;
        database.create_job(&first, None).expect("first job");
        database
            .replace_segments(
                first.id,
                &[Segment {
                    id: SegmentId::new(),
                    job_id: first.id,
                    start: 0,
                    end: 999,
                    durable_offset: 0,
                    worker_state: "active".to_owned(),
                    checksum: None,
                    checkpoint_at: timestamp(1),
                }],
            )
            .expect("first segment");
        database
            .checkpoint_single_stream_pause(first.id, 417, timestamp(3))
            .expect("pause first job");

        let mut second = job(partial_path, "Primary", 4);
        second.state = DownloadState::Paused;
        second.downloaded_bytes = 300;
        database.create_job(&second, None).expect("second job");
        database
            .replace_segments(
                second.id,
                &[
                    Segment {
                        id: SegmentId::new(),
                        job_id: second.id,
                        start: 0,
                        end: 499,
                        durable_offset: 200,
                        worker_state: "paused".to_owned(),
                        checksum: Some("left".to_owned()),
                        checkpoint_at: timestamp(5),
                    },
                    Segment {
                        id: SegmentId::new(),
                        job_id: second.id,
                        start: 500,
                        end: 999,
                        durable_offset: 600,
                        worker_state: "paused".to_owned(),
                        checksum: Some("right".to_owned()),
                        checkpoint_at: timestamp(5),
                    },
                ],
            )
            .expect("second segments");
        database
            .append_event(&AuditEvent {
                id: crate::domain::EventId::new(),
                job_id: second.id,
                sequence: 0,
                timestamp: timestamp(6),
                component: "update-test".to_owned(),
                severity: "info".to_owned(),
                event_type: "paused_fixture".to_owned(),
                correlation_id: Some("s158".to_owned()),
                redacted_detail: "paused before update".to_owned(),
            })
            .expect("second event");
        database
            .save_request_context(
                &RequestContext::new(
                    second.id,
                    Some(&HttpUrl::parse("https://example.test/watch?token=secret").expect("URL")),
                    Some(&HttpUrl::parse("https://example.test/").expect("origin")),
                    Some("Fixture Browser/1.0".to_owned()),
                    Some("media-default".to_owned()),
                    None,
                    Some(CredentialRef {
                        service: "DownloadManager/cookie/example.test".to_owned(),
                        account: "profile-one".to_owned(),
                    }),
                    timestamp(100),
                )
                .expect("request context"),
            )
            .expect("save request context");
    }

    fn job(destination: &Path, queue: &str, at: i64) -> JobMetadata {
        JobMetadata {
            id: JobId::new(),
            source_url: HttpUrl::parse("https://example.test/file.bin").expect("URL"),
            final_url: None,
            display_url: "https://example.test/file.bin".to_owned(),
            filename: "file.bin".to_owned(),
            destination: destination.to_path_buf(),
            state: DownloadState::Paused,
            kind: DownloadKind::File,
            total_bytes: Some(1_000),
            downloaded_bytes: 0,
            category: Some("General".to_owned()),
            queue: Some(queue.to_owned()),
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: timestamp(at),
            updated_at: timestamp(at),
            completed_at: None,
        }
    }

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("timestamp")
    }

    fn executable_name(stem: &str) -> String {
        if cfg!(windows) {
            format!("{stem}.exe")
        } else {
            stem.to_owned()
        }
    }

    fn hash(path: &Path) -> [u8; 32] {
        Sha256::digest(fs::read(path).expect("read fixture")).into()
    }
}
