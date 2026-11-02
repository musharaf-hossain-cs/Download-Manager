use std::{
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
};

use download_manager_engine::{
    disk::{DiskError, write_all_with_disk_guard},
    domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
    state::DownloadState,
    storage::Database,
    update_staging::{UpdateStageError, UpdateStageStore},
    update_verification::UpdateVerifier,
};
use ed25519_dalek::{Signer, SigningKey};
use rusqlite::{Connection, params};
use serde_json::json;
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const CAMPAIGN_RUNS: u32 = 50;
const CAMPAIGN_SEED: u64 = 0x5176_d15c_db00_0001;
const SIGNING_KEY: [u8; 32] = [0x76; 32];

#[test]
fn fifty_seeded_disk_db_and_update_interruptions_preserve_invariants() {
    let campaign = TempDir::new().expect("campaign directory");
    let mut random = CAMPAIGN_SEED;

    for run in 0..CAMPAIGN_RUNS {
        random = next_random(random);
        exercise_disk_interruption(run, random);

        random = next_random(random);
        exercise_database_interruption(campaign.path(), run, random);

        random = next_random(random);
        exercise_update_interruption(campaign.path(), run, random);
    }
}

fn exercise_disk_interruption(run: u32, random: u64) {
    let payload = pattern_bytes(4_096, random);
    let fail_after = 1 + usize::try_from(random % 4_095).expect("bounded failure offset");
    let disk_full = random & 1 == 0;
    let mut writer = InterruptingWriter {
        fail_after,
        disk_full,
        bytes: Vec::new(),
    };

    let result = write_all_with_disk_guard(&mut writer, &payload, 0);
    if disk_full {
        assert!(
            matches!(
                result,
                Err(DiskError::DiskFull { safe_offset })
                    if safe_offset == u64::try_from(fail_after).expect("offset fits u64")
            ),
            "run {run}: disk-full interruption must report the exact durable prefix"
        );
    } else {
        assert!(
            matches!(result, Err(DiskError::Io(ref error)) if error.kind() == io::ErrorKind::PermissionDenied),
            "run {run}: access interruption must remain an ordinary I/O failure"
        );
    }
    assert_eq!(
        writer.bytes,
        payload[..fail_after],
        "run {run}: interrupted output must contain only the acknowledged prefix"
    );
}

fn exercise_database_interruption(root: &Path, run: u32, random: u64) {
    let database_path = root.join(format!("database-{run}.sqlite"));
    let job = campaign_job(run, random);
    let mut database = Database::open(&database_path).expect("create campaign database");
    database
        .create_job(&job, None)
        .expect("insert baseline job");
    drop(database);

    let connection = Connection::open(&database_path).expect("open raw interruption connection");
    connection
        .execute_batch("BEGIN IMMEDIATE;")
        .expect("start interrupted transaction");
    connection
        .execute(
            "UPDATE jobs SET state='completed', downloaded_bytes=total_bytes, \
             updated_at_ms=?2, completed_at_ms=?2 WHERE id=?1",
            params![job.id.to_string(), job.updated_at.get() + 1],
        )
        .expect("write uncommitted completion");
    connection
        .execute(
            "INSERT INTO event_log \
             (id, job_id, timestamp_ms, component, severity, event_type, correlation_id, redacted_detail) \
             VALUES (?1, ?2, ?3, 'campaign', 'info', 'completed', NULL, 'uncommitted')",
            params![
                format!("00000000-0000-0000-0000-{run:012}"),
                job.id.to_string(),
                job.updated_at.get() + 1
            ],
        )
        .expect("write uncommitted event");
    drop(connection);

    let restarted = Database::open(&database_path).expect("restart campaign database");
    let recovered = restarted
        .get_job(job.id)
        .expect("read recovered job")
        .expect("baseline job remains");
    assert_eq!(
        recovered.metadata, job,
        "run {run}: interrupted transaction must not publish a false completion"
    );
    assert!(
        restarted
            .events(job.id)
            .expect("read recovered events")
            .is_empty(),
        "run {run}: event and job changes must roll back atomically"
    );
}

fn exercise_update_interruption(root: &Path, run: u32, random: u64) {
    let run_root = root.join(format!("update-{run}"));
    fs::create_dir(&run_root).expect("create update campaign directory");
    let old_app = run_root.join("download-manager-old.exe");
    let old_bytes = pattern_bytes(8_192, random.rotate_left(13));
    fs::write(&old_app, &old_bytes).expect("write old application fixture");
    let old_hash = Sha256::digest(&old_bytes);
    let (verifier, authenticated_update) = verified_update_fixture(&run_root, random);
    let update_root = run_root.join("updates");
    let store = UpdateStageStore::open(&update_root).expect("open update store");

    match random % 5 {
        0..=2 => {
            let staging = update_root.join(".staging");
            fs::create_dir(&staging).expect("create interrupted staging directory");
            if random % 5 >= 1 {
                fs::write(staging.join("artifact.tmp"), pattern_bytes(257, random))
                    .expect("write interrupted artifact");
            }
            if random % 5 == 2 {
                fs::rename(staging.join("artifact.tmp"), staging.join("artifact.zip"))
                    .expect("publish interrupted artifact name");
                fs::write(
                    staging.join("metadata.json"),
                    b"incomplete authenticated state",
                )
                .expect("write interrupted metadata");
            }
            assert!(
                store
                    .recover(&verifier)
                    .expect("recover incomplete staging")
                    .is_none(),
                "run {run}: incomplete private staging must not become pending"
            );
            assert!(
                !staging.exists(),
                "run {run}: incomplete private staging must be removed"
            );
        }
        3 => {
            let staged = store
                .stage(&authenticated_update)
                .expect("stage authenticated update");
            let staged_hash = *staged.artifact_sha256();
            drop(store);
            let restarted = UpdateStageStore::open(&update_root).expect("restart update store");
            let recovered = restarted
                .recover(&verifier)
                .expect("recover pending update")
                .expect("pending update remains available");
            assert_eq!(
                *recovered.artifact_sha256(),
                staged_hash,
                "run {run}: durable pending update identity must survive interruption"
            );
        }
        4 => {
            let staged = store
                .stage(&authenticated_update)
                .expect("stage update before activation");
            let artifact_hash = Sha256::digest(
                fs::read(staged.artifact_path()).expect("read pending artifact before marker"),
            );
            fs::rename(update_root.join("pending"), update_root.join(".completed"))
                .expect("simulate durable activation marker");
            assert!(
                matches!(
                    store.recover(&verifier),
                    Err(UpdateStageError::InvalidAuthenticatedState(
                        "activation outcome"
                    ))
                ),
                "run {run}: unresolved activation outcome must fail closed"
            );
            let marked_artifact = update_root.join(".completed").join("artifact.zip");
            assert_eq!(
                Sha256::digest(fs::read(marked_artifact).expect("preserved marked artifact")),
                artifact_hash,
                "run {run}: unresolved activation evidence must be preserved"
            );
        }
        _ => unreachable!("modulo five is exhaustive"),
    }

    assert_eq!(
        Sha256::digest(fs::read(&old_app).expect("read old app after interruption")),
        old_hash,
        "run {run}: update interruption must not alter the launchable old application"
    );
}

fn campaign_job(run: u32, random: u64) -> JobMetadata {
    let at = UnixMillis::new(10_000 + i64::from(run)).expect("campaign timestamp");
    JobMetadata {
        id: JobId::new(),
        source_url: HttpUrl::parse(format!("https://example.test/campaign/{run}.bin"))
            .expect("campaign URL"),
        final_url: None,
        display_url: format!("https://example.test/campaign/{run}.bin"),
        filename: format!("campaign-{run}.bin"),
        destination: PathBuf::from(format!(r"C:\Downloads\campaign-{run}.bin")),
        state: DownloadState::Paused,
        kind: DownloadKind::File,
        total_bytes: Some(1_024 + random % 8_192),
        downloaded_bytes: random % 1_024,
        category: Some("Fault campaign".to_owned()),
        queue: None,
        priority: Priority::Normal,
        source_browser: None,
        source_profile: None,
        retry_count: 0,
        error: Some("interrupted safely".to_owned()),
        created_at: at,
        updated_at: at,
        completed_at: None,
    }
}

fn verified_update_fixture(
    root: &Path,
    random: u64,
) -> (
    UpdateVerifier,
    download_manager_engine::update_verification::VerifiedUpdate,
) {
    let artifact_path = root.join("download-manager-2.3.4-windows-x64.zip");
    let artifact = pattern_bytes(4_096, random.rotate_right(7));
    fs::write(&artifact_path, &artifact).expect("write update artifact");
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
    .expect("serialize signed update metadata");
    let signing_key = SigningKey::from_bytes(&SIGNING_KEY);
    let signature = signing_key.sign(&metadata).to_bytes();
    let verifier =
        UpdateVerifier::new(signing_key.verifying_key().to_bytes()).expect("test verifier");
    let authenticated_update = verifier
        .verify(&metadata, &signature, &artifact_path)
        .expect("verify update fixture");
    (verifier, authenticated_update)
}

fn pattern_bytes(length: usize, seed: u64) -> Vec<u8> {
    (0..length)
        .map(|index| {
            let mixed = seed.wrapping_add(
                u64::try_from(index)
                    .expect("pattern index fits u64")
                    .wrapping_mul(0x9e37_79b9),
            );
            mixed.to_le_bytes()[0]
        })
        .collect()
}

const fn next_random(value: u64) -> u64 {
    value
        .wrapping_mul(6_364_136_223_846_793_005)
        .wrapping_add(1_442_695_040_888_963_407)
}

struct InterruptingWriter {
    fail_after: usize,
    disk_full: bool,
    bytes: Vec<u8>,
}

impl Write for InterruptingWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if self.bytes.len() == self.fail_after {
            return if self.disk_full {
                Err(io::Error::from_raw_os_error(112))
            } else {
                Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "injected access denial",
                ))
            };
        }
        let count = buffer.len().min(self.fail_after - self.bytes.len());
        self.bytes.extend_from_slice(&buffer[..count]);
        Ok(count)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
