//! Safe final-file placement and explicit conflict handling.

use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};
use thiserror::Error;
use uuid::Uuid;

const COPY_BUFFER_SIZE: usize = 128 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictPolicy {
    Ask,
    Overwrite,
    Rename,
    ResumeIfSame,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FinalizeOutcome {
    pub final_path: PathBuf,
    pub reused_existing: bool,
}

#[derive(Debug, Error)]
pub enum FinalizeError {
    #[error("destination already exists and requires a user decision: {0}")]
    Conflict(PathBuf),
    #[error("existing destination is not the same completed object: {0}")]
    ExistingObjectDiffers(PathBuf),
    #[error("filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("cross-volume copy verification failed")]
    VerificationFailed,
}

/// Places a completed temporary file using a rename on the destination volume.
///
/// # Errors
/// Returns a conflict error when policy requires a decision, or an I/O error when placement fails.
pub fn finalize_same_volume(
    temporary_path: &Path,
    requested_path: &Path,
    policy: ConflictPolicy,
) -> Result<FinalizeOutcome, FinalizeError> {
    let destination = resolve_conflict(temporary_path, requested_path, policy)?;
    let Some(destination) = destination else {
        fs::remove_file(temporary_path)?;
        return Ok(FinalizeOutcome {
            final_path: requested_path.to_owned(),
            reused_existing: true,
        });
    };
    rename_with_optional_backup(
        temporary_path,
        &destination,
        policy == ConflictPolicy::Overwrite,
    )?;
    Ok(FinalizeOutcome {
        final_path: destination,
        reused_existing: false,
    })
}

/// Copies a completed temporary file to destination-local staging, verifies SHA-256, then renames.
///
/// The source is removed only after the verified destination rename succeeds.
///
/// # Errors
/// Returns a conflict, copy, verification, sync, rename, or cleanup error.
pub fn finalize_cross_volume(
    temporary_path: &Path,
    requested_path: &Path,
    policy: ConflictPolicy,
) -> Result<FinalizeOutcome, FinalizeError> {
    let destination = resolve_conflict(temporary_path, requested_path, policy)?;
    let Some(destination) = destination else {
        fs::remove_file(temporary_path)?;
        return Ok(FinalizeOutcome {
            final_path: requested_path.to_owned(),
            reused_existing: true,
        });
    };
    let parent = destination.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let staging = parent.join(format!(".download-manager-{}.copying", Uuid::new_v4()));
    let source_hash = copy_and_hash(temporary_path, &staging)?;
    let staging_hash = hash_file(&staging)?;
    if source_hash != staging_hash {
        let _ignored = fs::remove_file(&staging);
        return Err(FinalizeError::VerificationFailed);
    }
    if let Err(error) =
        rename_with_optional_backup(&staging, &destination, policy == ConflictPolicy::Overwrite)
    {
        let _ignored = fs::remove_file(&staging);
        return Err(error);
    }
    fs::remove_file(temporary_path)?;
    Ok(FinalizeOutcome {
        final_path: destination,
        reused_existing: false,
    })
}

fn resolve_conflict(
    temporary_path: &Path,
    requested_path: &Path,
    policy: ConflictPolicy,
) -> Result<Option<PathBuf>, FinalizeError> {
    if !requested_path.exists() {
        return Ok(Some(requested_path.to_owned()));
    }
    match policy {
        ConflictPolicy::Ask => Err(FinalizeError::Conflict(requested_path.to_owned())),
        ConflictPolicy::Overwrite => Ok(Some(requested_path.to_owned())),
        ConflictPolicy::Rename => Ok(Some(next_available_name(requested_path))),
        ConflictPolicy::ResumeIfSame => {
            if hash_file(temporary_path)? == hash_file(requested_path)? {
                Ok(None)
            } else {
                Err(FinalizeError::ExistingObjectDiffers(
                    requested_path.to_owned(),
                ))
            }
        }
    }
}

fn next_available_name(path: &Path) -> PathBuf {
    let parent = path.parent().unwrap_or_else(|| Path::new(""));
    let stem = path
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("download");
    let extension = path.extension().and_then(|value| value.to_str());
    for counter in 1_u32.. {
        let name = extension.map_or_else(
            || format!("{stem} ({counter})"),
            |extension| format!("{stem} ({counter}).{extension}"),
        );
        let candidate = parent.join(name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("u32 filename counter exhausted")
}

fn rename_with_optional_backup(
    source: &Path,
    destination: &Path,
    overwrite: bool,
) -> Result<(), FinalizeError> {
    if !overwrite || !destination.exists() {
        fs::rename(source, destination)?;
        return Ok(());
    }
    let backup = destination.with_file_name(format!(".download-manager-{}.old", Uuid::new_v4()));
    fs::rename(destination, &backup)?;
    if let Err(error) = fs::rename(source, destination) {
        fs::rename(&backup, destination)?;
        return Err(error.into());
    }
    fs::remove_file(backup)?;
    Ok(())
}

fn copy_and_hash(source: &Path, destination: &Path) -> Result<Vec<u8>, FinalizeError> {
    let mut input = File::open(source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        hasher.update(&buffer[..count]);
    }
    output.flush()?;
    output.sync_all()?;
    Ok(hasher.finalize().to_vec())
}

fn hash_file(path: &Path) -> Result<Vec<u8>, FinalizeError> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0_u8; COPY_BUFFER_SIZE];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    Ok(hasher.finalize().to_vec())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use tempfile::tempdir;

    use super::{ConflictPolicy, FinalizeError, finalize_cross_volume, finalize_same_volume};

    fn write(path: &std::path::Path, body: &[u8]) {
        fs::write(path, body).expect("write fixture");
    }

    #[test]
    fn same_volume_conflict_policies_are_explicit_and_safe() {
        let directory = tempdir().expect("directory");
        let destination = directory.path().join("file.bin");
        write(&destination, b"old");

        let ask = directory.path().join("ask.part");
        write(&ask, b"new");
        assert!(matches!(
            finalize_same_volume(&ask, &destination, ConflictPolicy::Ask),
            Err(FinalizeError::Conflict(_))
        ));
        assert_eq!(fs::read(&destination).expect("old remains"), b"old");

        let overwrite = directory.path().join("overwrite.part");
        write(&overwrite, b"new");
        finalize_same_volume(&overwrite, &destination, ConflictPolicy::Overwrite)
            .expect("overwrite");
        assert_eq!(fs::read(&destination).expect("new file"), b"new");

        let rename = directory.path().join("rename.part");
        write(&rename, b"renamed");
        let renamed =
            finalize_same_volume(&rename, &destination, ConflictPolicy::Rename).expect("rename");
        assert_eq!(renamed.final_path.file_name().unwrap(), "file (1).bin");

        let same = directory.path().join("same.part");
        write(&same, b"new");
        let reused = finalize_same_volume(&same, &destination, ConflictPolicy::ResumeIfSame)
            .expect("same object");
        assert!(reused.reused_existing);
        assert!(!same.exists());

        let different = directory.path().join("different.part");
        write(&different, b"different");
        assert!(matches!(
            finalize_same_volume(&different, &destination, ConflictPolicy::ResumeIfSame),
            Err(FinalizeError::ExistingObjectDiffers(_))
        ));
    }

    #[test]
    fn cross_volume_abstraction_copies_verifies_renames_and_cleans_source() {
        let source_directory = tempdir().expect("source directory");
        let destination_directory = tempdir().expect("destination directory");
        let source = source_directory.path().join("file.part");
        let destination = destination_directory.path().join("file.bin");
        let body = vec![0xa5; 300_000];
        write(&source, &body);
        let outcome = finalize_cross_volume(&source, &destination, ConflictPolicy::Ask)
            .expect("cross-volume finalize");
        assert_eq!(outcome.final_path, destination);
        assert_eq!(fs::read(&destination).expect("destination"), body);
        assert!(!source.exists());
        assert!(
            fs::read_dir(destination_directory.path())
                .expect("read directory")
                .all(|entry| !entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .ends_with(".copying"))
        );
    }

    #[test]
    fn cross_volume_failure_never_changes_existing_destination() {
        let directory = tempdir().expect("directory");
        let missing = directory.path().join("missing.part");
        let destination = directory.path().join("file.bin");
        write(&destination, b"safe");
        assert!(finalize_cross_volume(&missing, &destination, ConflictPolicy::Overwrite).is_err());
        assert_eq!(fs::read(destination).expect("destination"), b"safe");
    }
}
