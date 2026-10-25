//! Disk-capacity preflight and runtime disk-full classification.

use std::{io::Write, path::Path};

use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiskCapacity {
    pub available_bytes: u64,
    pub required_bytes: u64,
    pub reserve_bytes: u64,
}

#[derive(Debug, Error)]
pub enum DiskError {
    #[error(
        "not enough disk space: {available_bytes} bytes available, {required_bytes} required plus {reserve_bytes} reserve"
    )]
    InsufficientSpace {
        available_bytes: u64,
        required_bytes: u64,
        reserve_bytes: u64,
    },
    #[error(
        "disk became full after safely writing through offset {safe_offset}; pause and free space before resuming"
    )]
    DiskFull { safe_offset: u64 },
    #[error("disk operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Checks free space at the nearest existing destination ancestor.
///
/// # Errors
/// Returns an actionable insufficient-space error or an I/O error querying the volume.
pub fn precheck_disk_space(
    destination: &Path,
    required_bytes: u64,
    reserve_bytes: u64,
) -> Result<DiskCapacity, DiskError> {
    let query_path = nearest_existing_ancestor(destination)?;
    let available_bytes = fs2::available_space(query_path)?;
    if available_bytes < required_bytes.saturating_add(reserve_bytes) {
        return Err(DiskError::InsufficientSpace {
            available_bytes,
            required_bytes,
            reserve_bytes,
        });
    }
    Ok(DiskCapacity {
        available_bytes,
        required_bytes,
        reserve_bytes,
    })
}

/// Writes a chunk while tracking the exact byte offset safe to persist after disk exhaustion.
///
/// # Errors
/// Maps Windows and Unix disk-full errors to an actionable pause result; preserves other I/O errors.
///
/// # Panics
/// Panics only on a platform where a single write count cannot fit in `u64`, or if a file offset
/// exceeds `u64::MAX`.
pub fn write_all_with_disk_guard<W: Write>(
    writer: &mut W,
    mut bytes: &[u8],
    starting_offset: u64,
) -> Result<u64, DiskError> {
    let mut safe_offset = starting_offset;
    while !bytes.is_empty() {
        match writer.write(bytes) {
            Ok(0) => return Err(DiskError::DiskFull { safe_offset }),
            Ok(count) => {
                safe_offset = safe_offset
                    .checked_add(u64::try_from(count).expect("write count fits u64"))
                    .expect("file offset overflow");
                bytes = &bytes[count..];
            }
            Err(error) if is_disk_full(&error) => {
                return Err(DiskError::DiskFull { safe_offset });
            }
            Err(error) => return Err(DiskError::Io(error)),
        }
    }
    Ok(safe_offset)
}

fn nearest_existing_ancestor(path: &Path) -> Result<&Path, std::io::Error> {
    let mut candidate = if path.is_dir() {
        Some(path)
    } else {
        path.parent()
    };
    while let Some(current) = candidate {
        if current.exists() {
            return Ok(current);
        }
        candidate = current.parent();
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::NotFound,
        format!("no existing ancestor for {}", path.display()),
    ))
}

fn is_disk_full(error: &std::io::Error) -> bool {
    matches!(error.raw_os_error(), Some(28 | 39 | 112))
}

#[cfg(test)]
mod tests {
    use std::io::{self, Write};

    use tempfile::tempdir;

    use super::{DiskError, precheck_disk_space, write_all_with_disk_guard};

    #[test]
    fn precheck_blocks_impossible_size_and_accepts_small_file() {
        let directory = tempdir().expect("directory");
        let destination = directory.path().join("nested").join("file.bin");
        let capacity = precheck_disk_space(&destination, 1, 0).expect("small capacity");
        assert!(capacity.available_bytes >= 1);
        assert!(matches!(
            precheck_disk_space(&destination, u64::MAX, 1),
            Err(DiskError::InsufficientSpace { .. })
        ));
    }

    #[test]
    fn runtime_disk_full_reports_exact_safe_paused_offset() {
        let mut writer = FullAfter {
            remaining: 4,
            bytes: Vec::new(),
        };
        let error =
            write_all_with_disk_guard(&mut writer, b"abcdefghij", 100).expect_err("disk full");
        assert!(matches!(error, DiskError::DiskFull { safe_offset: 104 }));
        assert_eq!(writer.bytes, b"abcd");
    }

    struct FullAfter {
        remaining: usize,
        bytes: Vec<u8>,
    }

    impl Write for FullAfter {
        fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
            if self.remaining == 0 {
                return Err(io::Error::from_raw_os_error(112));
            }
            let count = buffer.len().min(self.remaining);
            self.bytes.extend_from_slice(&buffer[..count]);
            self.remaining -= count;
            Ok(count)
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
}
