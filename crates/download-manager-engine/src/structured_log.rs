//! Size-bounded JSON-lines logging with request correlation and deterministic rotation.

use std::{
    ffi::OsString,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use thiserror::Error;

use crate::domain::{RequestId, UnixMillis};

/// Absolute ceiling for one configured log file.
pub const MAX_LOG_FILE_BYTES: u64 = 16 * 1024 * 1024;
/// Absolute ceiling for retained active and rotated files together.
pub const MAX_LOG_FILES: usize = 16;
/// Absolute ceiling for one serialized JSON-lines record, including its newline.
pub const MAX_LOG_RECORD_BYTES: usize = 64 * 1024;

const MAX_COMPONENT_BYTES: usize = 64;
const MAX_EVENT_TYPE_BYTES: usize = 128;

/// Severity stored in every structured log record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogSeverity {
    Debug,
    Info,
    Warn,
    Error,
}

/// One structured record. IPC request IDs are reused as correlation IDs end-to-end.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StructuredLogEntry {
    pub timestamp: UnixMillis,
    pub component: String,
    pub severity: LogSeverity,
    pub event_type: String,
    pub correlation_id: RequestId,
    pub detail: Value,
}

impl StructuredLogEntry {
    #[must_use]
    pub fn for_request(
        timestamp: UnixMillis,
        component: impl Into<String>,
        severity: LogSeverity,
        event_type: impl Into<String>,
        request_id: RequestId,
        detail: Value,
    ) -> Self {
        Self {
            timestamp,
            component: component.into(),
            severity,
            event_type: event_type.into(),
            correlation_id: request_id,
            detail,
        }
    }
}

/// Rotation and size limits for one log stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StructuredLogLimits {
    pub max_file_bytes: u64,
    pub max_files: usize,
}

impl Default for StructuredLogLimits {
    fn default() -> Self {
        Self {
            max_file_bytes: 4 * 1024 * 1024,
            max_files: 4,
        }
    }
}

#[derive(Debug, Error)]
pub enum StructuredLogError {
    #[error("log max_file_bytes must be in 1..={MAX_LOG_FILE_BYTES}")]
    InvalidFileLimit,
    #[error("log max_files must be in 1..={MAX_LOG_FILES}")]
    InvalidFileCount,
    #[error("log component must contain 1..={MAX_COMPONENT_BYTES} bytes")]
    InvalidComponent,
    #[error("log event_type must contain 1..={MAX_EVENT_TYPE_BYTES} bytes")]
    InvalidEventType,
    #[error("serialized log record is {actual} bytes; maximum is {maximum}")]
    RecordTooLarge { actual: usize, maximum: usize },
    #[error("structured log lock is poisoned")]
    Poisoned,
    #[error("structured log serialization failed: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("structured log filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Cloneable, thread-safe JSON-lines sink with bounded file rotation.
#[derive(Debug, Clone)]
pub struct StructuredLog {
    path: Arc<PathBuf>,
    limits: StructuredLogLimits,
    write_lock: Arc<Mutex<()>>,
}

impl StructuredLog {
    /// Opens a bounded log stream rooted at `path`.
    ///
    /// The active file is `path`; older generations use `.1` through
    /// `.max_files - 1`. All clones share the same append/rotation lock.
    ///
    /// # Errors
    /// Returns [`StructuredLogError`] for invalid limits or directory creation failure.
    pub fn open(
        path: impl AsRef<Path>,
        limits: StructuredLogLimits,
    ) -> Result<Self, StructuredLogError> {
        if !(1..=MAX_LOG_FILE_BYTES).contains(&limits.max_file_bytes) {
            return Err(StructuredLogError::InvalidFileLimit);
        }
        if !(1..=MAX_LOG_FILES).contains(&limits.max_files) {
            return Err(StructuredLogError::InvalidFileCount);
        }
        let path = path.as_ref();
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }
        Ok(Self {
            path: Arc::new(path.to_owned()),
            limits,
            write_lock: Arc::new(Mutex::new(())),
        })
    }

    /// Appends one complete JSON record, rotating before the configured file limit is crossed.
    ///
    /// # Errors
    /// Returns [`StructuredLogError`] without writing when the entry or its serialized form is
    /// invalid, or when rotation/append cannot complete.
    pub fn write(&self, entry: &StructuredLogEntry) -> Result<(), StructuredLogError> {
        validate_entry(entry)?;
        let mut record = serde_json::to_vec(entry)?;
        record.push(b'\n');
        let configured_max = usize::try_from(self.limits.max_file_bytes).unwrap_or(usize::MAX);
        let maximum = configured_max.min(MAX_LOG_RECORD_BYTES);
        if record.len() > maximum {
            return Err(StructuredLogError::RecordTooLarge {
                actual: record.len(),
                maximum,
            });
        }

        let _guard = self
            .write_lock
            .lock()
            .map_err(|_| StructuredLogError::Poisoned)?;
        let current_size = match fs::metadata(self.path.as_ref()) {
            Ok(metadata) => metadata.len(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
            Err(error) => return Err(error.into()),
        };
        let record_len = u64::try_from(record.len()).unwrap_or(u64::MAX);
        if current_size > 0 && current_size.saturating_add(record_len) > self.limits.max_file_bytes
        {
            self.rotate()?;
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.as_ref())?;
        file.write_all(&record)?;
        file.flush()?;
        Ok(())
    }

    fn rotate(&self) -> Result<(), StructuredLogError> {
        if self.limits.max_files == 1 {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(true)
                .open(self.path.as_ref())?;
            return Ok(());
        }

        let oldest = rotated_path(self.path.as_ref(), self.limits.max_files - 1);
        remove_if_present(&oldest)?;
        for generation in (1..self.limits.max_files - 1).rev() {
            let source = rotated_path(self.path.as_ref(), generation);
            if source.exists() {
                fs::rename(&source, rotated_path(self.path.as_ref(), generation + 1))?;
            }
        }
        if self.path.exists() {
            fs::rename(self.path.as_ref(), rotated_path(self.path.as_ref(), 1))?;
        }
        Ok(())
    }
}

fn validate_entry(entry: &StructuredLogEntry) -> Result<(), StructuredLogError> {
    if entry.component.is_empty() || entry.component.len() > MAX_COMPONENT_BYTES {
        return Err(StructuredLogError::InvalidComponent);
    }
    if entry.event_type.is_empty() || entry.event_type.len() > MAX_EVENT_TYPE_BYTES {
        return Err(StructuredLogError::InvalidEventType);
    }
    Ok(())
}

fn rotated_path(path: &Path, generation: usize) -> PathBuf {
    let mut value: OsString = path.as_os_str().to_owned();
    value.push(format!(".{generation}"));
    PathBuf::from(value)
}

fn remove_if_present(path: &Path) -> Result<(), std::io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, thread};

    use serde_json::{Value, json};
    use tempfile::tempdir;

    use super::{
        LogSeverity, StructuredLog, StructuredLogEntry, StructuredLogError, StructuredLogLimits,
        rotated_path,
    };
    use crate::domain::{RequestId, UnixMillis};

    fn entry(sequence: usize, detail: Value) -> StructuredLogEntry {
        StructuredLogEntry::for_request(
            UnixMillis::new(i64::try_from(sequence).expect("timestamp")).expect("timestamp"),
            "engine",
            LogSeverity::Info,
            "request_complete",
            RequestId::new(),
            detail,
        )
    }

    fn lines(path: &Path) -> Vec<StructuredLogEntry> {
        fs::read_to_string(path)
            .expect("read log")
            .lines()
            .map(|line| serde_json::from_str(line).expect("valid JSON line"))
            .collect()
    }

    use std::path::Path;

    #[test]
    fn rotation_bounds_each_file_and_retains_only_configured_generations() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("engine.jsonl");
        let limits = StructuredLogLimits {
            max_file_bytes: 420,
            max_files: 3,
        };
        let log = StructuredLog::open(&path, limits).expect("log");

        for sequence in 0..12 {
            log.write(&entry(
                sequence,
                json!({ "sequence": sequence, "padding": "x".repeat(40) }),
            ))
            .expect("write");
        }

        assert!(path.exists());
        assert!(rotated_path(&path, 1).exists());
        assert!(rotated_path(&path, 2).exists());
        assert!(!rotated_path(&path, 3).exists());
        for file in [&path, &rotated_path(&path, 1), &rotated_path(&path, 2)] {
            assert!(fs::metadata(file).expect("metadata").len() <= limits.max_file_bytes);
            assert_ne!(lines(file).len(), 0);
        }
        assert_eq!(
            lines(&path).last().expect("latest").detail["sequence"],
            json!(11)
        );
    }

    #[test]
    fn oversize_record_is_refused_without_creating_or_changing_a_file() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("engine.jsonl");
        let log = StructuredLog::open(
            &path,
            StructuredLogLimits {
                max_file_bytes: 300,
                max_files: 2,
            },
        )
        .expect("log");
        log.write(&entry(1, json!({ "safe": true })))
            .expect("initial write");
        let before = fs::read(&path).expect("before");

        assert!(matches!(
            log.write(&entry(2, json!({ "oversize": "x".repeat(1_000) }))),
            Err(StructuredLogError::RecordTooLarge { maximum: 300, .. })
        ));
        assert_eq!(fs::read(&path).expect("after"), before);
        assert!(!rotated_path(&path, 1).exists());
    }

    #[test]
    fn concurrent_writers_emit_complete_correlated_json_records() {
        let directory = tempdir().expect("temp directory");
        let path = directory.path().join("engine.jsonl");
        let log = StructuredLog::open(
            &path,
            StructuredLogLimits {
                max_file_bytes: 1_000_000,
                max_files: 2,
            },
        )
        .expect("log");
        let mut writers = Vec::new();
        for writer in 0..8 {
            let log = log.clone();
            writers.push(thread::spawn(move || {
                for sequence in 0..50 {
                    let request_id = RequestId::new();
                    let record = StructuredLogEntry::for_request(
                        UnixMillis::new(sequence).expect("timestamp"),
                        "engine",
                        LogSeverity::Debug,
                        "worker_progress",
                        request_id,
                        json!({ "writer": writer, "sequence": sequence }),
                    );
                    assert_eq!(record.correlation_id, request_id);
                    log.write(&record).expect("concurrent write");
                }
            }));
        }
        for writer in writers {
            writer.join().expect("writer thread");
        }

        let records = lines(&path);
        assert_eq!(records.len(), 400);
        let unique = records
            .iter()
            .map(|record| record.correlation_id)
            .collect::<std::collections::HashSet<_>>();
        assert_eq!(unique.len(), records.len());
    }
}
