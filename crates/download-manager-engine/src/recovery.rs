//! Startup reconciliation for durable offsets and partial files.

use std::{
    cmp::Ordering,
    collections::HashSet,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReconciliationAction {
    Consistent,
    FileTruncatedToDatabase,
    DatabaseMustRewind,
    MissingPartial,
    OrphanedPartial,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reconciliation {
    pub path: PathBuf,
    pub action: ReconciliationAction,
    pub durable_offset: Option<u64>,
}

#[derive(Debug, Error)]
pub enum RecoveryError {
    #[error("filesystem reconciliation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Reconciles one known partial file against the database's last durable offset.
///
/// Uncheckpointed file bytes are truncated. If the database is ahead, the returned offset is the
/// real file length so the caller can atomically rewind persistence before resuming.
///
/// # Errors
/// Returns an I/O error when metadata or truncation cannot be completed.
pub fn reconcile_partial(
    path: &Path,
    database_offset: u64,
) -> Result<Reconciliation, RecoveryError> {
    if !path.exists() {
        return Ok(Reconciliation {
            path: path.to_owned(),
            action: ReconciliationAction::MissingPartial,
            durable_offset: None,
        });
    }
    let file_length = path.metadata()?.len();
    match file_length.cmp(&database_offset) {
        Ordering::Greater => {
            OpenOptions::new()
                .write(true)
                .open(path)?
                .set_len(database_offset)?;
            Ok(Reconciliation {
                path: path.to_owned(),
                action: ReconciliationAction::FileTruncatedToDatabase,
                durable_offset: Some(database_offset),
            })
        }
        Ordering::Less => Ok(Reconciliation {
            path: path.to_owned(),
            action: ReconciliationAction::DatabaseMustRewind,
            durable_offset: Some(file_length),
        }),
        Ordering::Equal => Ok(Reconciliation {
            path: path.to_owned(),
            action: ReconciliationAction::Consistent,
            durable_offset: Some(database_offset),
        }),
    }
}

/// Finds unowned `.part` files without deleting or modifying them.
///
/// # Errors
/// Returns an I/O error when the partial directory cannot be enumerated.
pub fn find_orphaned_partials<S: std::hash::BuildHasher>(
    directory: &Path,
    known_paths: &HashSet<PathBuf, S>,
) -> Result<Vec<Reconciliation>, RecoveryError> {
    let mut orphans = fs::read_dir(directory)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "part")
        })
        .filter(|path| !known_paths.contains(path))
        .map(|path| Reconciliation {
            durable_offset: path.metadata().ok().map(|metadata| metadata.len()),
            path,
            action: ReconciliationAction::OrphanedPartial,
        })
        .collect::<Vec<_>>();
    orphans.sort_by(|left, right| left.path.cmp(&right.path));
    Ok(orphans)
}

#[cfg(test)]
mod tests {
    use std::{
        collections::HashSet,
        fs::{self, OpenOptions},
        io::Write,
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };

    use tempfile::tempdir;

    use super::{ReconciliationAction, find_orphaned_partials, reconcile_partial};

    #[test]
    fn database_ahead_behind_and_orphans_repair_safely() {
        let directory = tempdir().expect("directory");
        let file_ahead = directory.path().join("ahead.part");
        fs::write(&file_ahead, vec![1_u8; 100]).expect("partial");
        let repaired = reconcile_partial(&file_ahead, 60).expect("reconcile file ahead");
        assert_eq!(
            repaired.action,
            ReconciliationAction::FileTruncatedToDatabase
        );
        assert_eq!(file_ahead.metadata().expect("metadata").len(), 60);

        let database_ahead = directory.path().join("behind.part");
        fs::write(&database_ahead, vec![2_u8; 40]).expect("partial");
        let repaired = reconcile_partial(&database_ahead, 80).expect("reconcile DB ahead");
        assert_eq!(repaired.action, ReconciliationAction::DatabaseMustRewind);
        assert_eq!(repaired.durable_offset, Some(40));

        let missing =
            reconcile_partial(&directory.path().join("missing.part"), 10).expect("missing partial");
        assert_eq!(missing.action, ReconciliationAction::MissingPartial);

        let orphan = directory.path().join("orphan.part");
        fs::write(&orphan, b"unowned").expect("orphan");
        let known = HashSet::from([file_ahead, database_ahead]);
        let orphans = find_orphaned_partials(directory.path(), &known).expect("orphans");
        assert_eq!(orphans.len(), 1);
        assert_eq!(orphans[0].path, orphan);
        assert_eq!(orphans[0].action, ReconciliationAction::OrphanedPartial);
        assert_eq!(fs::read(&orphan).expect("orphan preserved"), b"unowned");
    }

    #[test]
    fn twenty_seeded_process_kills_recover_without_corrupt_completion() {
        const TOTAL: u64 = 2 * 1024 * 1024;
        const CHECKPOINT: u64 = 128 * 1024;
        let directory = tempdir().expect("directory");
        let executable = std::env::current_exe().expect("test executable");
        let mut random = 0x5eed_cafe_u64;
        for run in 0..20_u32 {
            random = random
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1);
            let kill_after = 64 * 1024 + random % (1024 * 1024);
            let path = directory.path().join(format!("kill-{run}.part"));
            let mut child = Command::new(&executable)
                .args([
                    "--ignored",
                    "--exact",
                    "recovery::tests::process_kill_writer_child",
                ])
                .env("DOWNLOAD_MANAGER_KILL_PATH", &path)
                .env("DOWNLOAD_MANAGER_KILL_TOTAL", TOTAL.to_string())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .expect("spawn writer child");
            let deadline = Instant::now() + Duration::from_secs(5);
            loop {
                let length = path.metadata().map_or(0, |metadata| metadata.len());
                if length >= kill_after {
                    break;
                }
                assert!(
                    Instant::now() < deadline,
                    "writer child did not make progress"
                );
                thread::sleep(Duration::from_millis(2));
            }
            child.kill().expect("force kill writer");
            child.wait().expect("reap writer");
            let observed = path.metadata().expect("partial metadata").len();
            let durable = (observed / CHECKPOINT) * CHECKPOINT;
            reconcile_partial(&path, durable).expect("startup reconcile");
            append_pattern(&path, durable, TOTAL);
            let completed = fs::read(&path).expect("completed file");
            assert_eq!(completed.len(), usize::try_from(TOTAL).expect("total size"));
            assert!(completed.iter().enumerate().all(|(index, value)| {
                *value == u8::try_from(index % 251).expect("pattern byte")
            }));
        }
    }

    #[test]
    #[ignore = "helper process launched by the forced-kill recovery test"]
    fn process_kill_writer_child() {
        let path = std::env::var_os("DOWNLOAD_MANAGER_KILL_PATH").expect("child path");
        let total = std::env::var("DOWNLOAD_MANAGER_KILL_TOTAL")
            .expect("child total")
            .parse::<u64>()
            .expect("numeric child total");
        append_pattern(std::path::Path::new(&path), 0, total);
    }

    fn append_pattern(path: &std::path::Path, start: u64, end: u64) {
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open pattern file");
        let mut position = start;
        while position < end {
            let count = usize::try_from((end - position).min(16 * 1024)).expect("chunk size");
            let bytes = (0..count)
                .map(|index| u8::try_from((position + index as u64) % 251).expect("pattern byte"))
                .collect::<Vec<_>>();
            file.write_all(&bytes).expect("write pattern");
            file.flush().expect("flush pattern");
            position += count as u64;
            if std::env::var_os("DOWNLOAD_MANAGER_KILL_PATH").is_some() {
                thread::sleep(Duration::from_millis(2));
            }
        }
        file.sync_all().expect("sync pattern");
    }
}
