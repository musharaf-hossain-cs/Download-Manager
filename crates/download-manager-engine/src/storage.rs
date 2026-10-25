//! SQLite-backed durable state owned exclusively by the engine.

use std::{
    fs,
    path::{Path, PathBuf},
    str::FromStr,
    time::Duration,
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    domain::{
        DownloadKind, EventId, JobId, JobMetadata, Priority, RangeCapability, RemoteIdentity,
        Segment, SegmentId, UnixMillis, ValidationError,
    },
    state::{DownloadEvent, DownloadState, StateTransition, TransitionError, reduce_state},
};

const MIGRATIONS: &[Migration] = &[
    Migration {
        version: 1,
        sql: include_str!("../migrations/001_jobs.sql"),
    },
    Migration {
        version: 2,
        sql: include_str!("../migrations/002_segments_events.sql"),
    },
];

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("SQLite error: {0}")]
    Sqlite(#[from] rusqlite::Error),
    #[error("filesystem error: {0}")]
    Io(#[from] std::io::Error),
    #[error("domain validation failed: {0}")]
    Validation(#[from] ValidationError),
    #[error("state transition failed: {0}")]
    Transition(#[from] TransitionError),
    #[error("stored data is invalid: {0}")]
    Corrupt(String),
    #[error("numeric value does not fit in SQLite INTEGER: {0}")]
    IntegerOverflow(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobRecord {
    pub metadata: JobMetadata,
    pub remote_identity: Option<RemoteIdentity>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuditEvent {
    pub id: EventId,
    pub job_id: JobId,
    pub sequence: i64,
    pub timestamp: UnixMillis,
    pub component: String,
    pub severity: String,
    pub event_type: String,
    pub correlation_id: Option<String>,
    pub redacted_detail: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetentionResult {
    pub jobs_deleted: usize,
    pub events_deleted: usize,
}

/// Configured connection to the durable engine database.
pub struct Database {
    connection: Connection,
    backup_path: Option<PathBuf>,
}

impl Database {
    /// Opens a database, creates a pre-migration backup, and applies pending migrations.
    ///
    /// # Errors
    /// Returns [`StorageError`] when backup, configuration, or migration fails.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        Self::open_with_migrations(path.as_ref(), MIGRATIONS)
    }

    /// Opens an isolated in-memory database, primarily for engine tests.
    ///
    /// # Errors
    /// Returns [`StorageError`] when configuration or migration fails.
    pub fn open_in_memory() -> Result<Self, StorageError> {
        let connection = Connection::open_in_memory()?;
        configure(&connection)?;
        apply_migrations(&connection, MIGRATIONS)?;
        Ok(Self {
            connection,
            backup_path: None,
        })
    }

    fn open_with_migrations(path: &Path, migrations: &[Migration]) -> Result<Self, StorageError> {
        if let Some(parent) = path.parent()
            && !parent.as_os_str().is_empty()
        {
            fs::create_dir_all(parent)?;
        }

        let target_version = migrations
            .iter()
            .map(|migration| migration.version)
            .max()
            .unwrap_or(0);
        let needs_backup = path.is_file()
            && fs::metadata(path)?.len() > 0
            && existing_schema_version(path)? < target_version;
        let backup_path = if needs_backup {
            let destination = migration_backup_path(path);
            fs::copy(path, &destination)?;
            Some(destination)
        } else {
            None
        };

        let connection = Connection::open(path)?;
        configure(&connection)?;
        apply_migrations(&connection, migrations)?;
        Ok(Self {
            connection,
            backup_path,
        })
    }

    #[must_use]
    pub fn migration_backup(&self) -> Option<&Path> {
        self.backup_path.as_deref()
    }

    /// Returns the highest applied schema version.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the schema table cannot be read.
    pub fn schema_version(&self) -> Result<u32, StorageError> {
        let version = self.connection.query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_versions",
            [],
            |row| row.get::<_, u32>(0),
        )?;
        Ok(version)
    }

    /// Inserts one job and its optional identity in a single transaction.
    ///
    /// # Errors
    /// Returns [`StorageError`] for validation, constraint, or transaction failures.
    pub fn create_job(
        &mut self,
        metadata: &JobMetadata,
        identity: Option<&RemoteIdentity>,
    ) -> Result<(), StorageError> {
        metadata.validate()?;
        if let Some(identity) = identity {
            identity.validate()?;
        }
        let transaction = self.connection.transaction()?;
        insert_job(&transaction, metadata)?;
        if let Some(identity) = identity {
            insert_remote_identity(&transaction, metadata.id, identity)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Replaces all mutable metadata for an existing job.
    ///
    /// # Errors
    /// Returns [`StorageError`] when validation fails or the job cannot be updated.
    pub fn update_job(&mut self, metadata: &JobMetadata) -> Result<(), StorageError> {
        metadata.validate()?;
        let values = job_params(metadata)?;
        let changed = self.connection.execute(
            "UPDATE jobs SET source_url=?2, final_url=?3, display_url=?4, filename=?5, \
             destination=?6, state=?7, kind=?8, total_bytes=?9, downloaded_bytes=?10, \
             category=?11, queue_name=?12, priority=?13, source_browser=?14, \
             source_profile=?15, retry_count=?16, error=?17, created_at_ms=?18, \
             updated_at_ms=?19, completed_at_ms=?20 WHERE id=?1",
            rusqlite::params_from_iter(values),
        )?;
        if changed == 0 {
            return Err(StorageError::Corrupt(format!(
                "job {} does not exist",
                metadata.id
            )));
        }
        Ok(())
    }

    /// Loads one job and its remote identity.
    ///
    /// # Errors
    /// Returns [`StorageError`] when stored data is invalid or cannot be read.
    pub fn get_job(&self, id: JobId) -> Result<Option<JobRecord>, StorageError> {
        let metadata = self
            .connection
            .query_row(
                "SELECT id, source_url, final_url, display_url, filename, destination, state, \
                 kind, total_bytes, downloaded_bytes, category, queue_name, priority, \
                 source_browser, source_profile, retry_count, error, created_at_ms, \
                 updated_at_ms, completed_at_ms FROM jobs WHERE id=?1",
                [id.to_string()],
                row_to_job,
            )
            .optional()?;
        let Some(metadata) = metadata else {
            return Ok(None);
        };
        metadata.validate()?;
        let remote_identity = self.get_remote_identity(id)?;
        Ok(Some(JobRecord {
            metadata,
            remote_identity,
        }))
    }

    /// Loads all jobs in stable creation order.
    ///
    /// # Errors
    /// Returns [`StorageError`] when stored data is invalid or cannot be read.
    pub fn list_jobs(&self) -> Result<Vec<JobRecord>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, source_url, final_url, display_url, filename, destination, state, \
             kind, total_bytes, downloaded_bytes, category, queue_name, priority, \
             source_browser, source_profile, retry_count, error, created_at_ms, \
             updated_at_ms, completed_at_ms FROM jobs ORDER BY created_at_ms, id",
        )?;
        let metadata = statement
            .query_map([], row_to_job)?
            .collect::<Result<Vec<_>, _>>()?;
        drop(statement);
        metadata
            .into_iter()
            .map(|metadata| {
                metadata.validate()?;
                let remote_identity = self.get_remote_identity(metadata.id)?;
                Ok(JobRecord {
                    metadata,
                    remote_identity,
                })
            })
            .collect()
    }

    /// Deletes a job record and all child rows without touching downloaded files.
    ///
    /// # Errors
    /// Returns [`StorageError`] when SQLite cannot complete the deletion.
    pub fn delete_job(&mut self, id: JobId) -> Result<bool, StorageError> {
        Ok(self
            .connection
            .execute("DELETE FROM jobs WHERE id=?1", [id.to_string()])?
            > 0)
    }

    /// Inserts or replaces the remote identity for an existing job.
    ///
    /// # Errors
    /// Returns [`StorageError`] for validation or persistence failures.
    pub fn set_remote_identity(
        &mut self,
        job_id: JobId,
        identity: &RemoteIdentity,
    ) -> Result<(), StorageError> {
        identity.validate()?;
        self.connection.execute(
            "INSERT INTO remote_identities \
             (job_id, etag, last_modified, content_length, mime_type, content_fingerprint, range_capability) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) \
             ON CONFLICT(job_id) DO UPDATE SET etag=excluded.etag, \
             last_modified=excluded.last_modified, content_length=excluded.content_length, \
             mime_type=excluded.mime_type, content_fingerprint=excluded.content_fingerprint, \
             range_capability=excluded.range_capability",
            params![
                job_id.to_string(),
                identity.etag,
                identity.last_modified,
                optional_u64_to_i64(identity.content_length)?,
                identity.mime_type,
                identity.content_fingerprint,
                range_capability_text(identity.range_capability),
            ],
        )?;
        Ok(())
    }

    /// Atomically replaces a job's durable segment map.
    ///
    /// # Errors
    /// Returns [`StorageError`] for invalid segments or transaction failures.
    pub fn replace_segments(
        &mut self,
        job_id: JobId,
        segments: &[Segment],
    ) -> Result<(), StorageError> {
        for segment in segments {
            segment.validate()?;
            if segment.job_id != job_id {
                return Err(StorageError::Corrupt(
                    "segment belongs to a different job".to_owned(),
                ));
            }
        }
        let transaction = self.connection.transaction()?;
        transaction.execute("DELETE FROM segments WHERE job_id=?1", [job_id.to_string()])?;
        for segment in segments {
            transaction.execute(
                "INSERT INTO segments \
                 (id, job_id, range_start, range_end, durable_offset, worker_state, checksum, checkpoint_at_ms) \
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    segment.id.to_string(),
                    segment.job_id.to_string(),
                    u64_to_i64(segment.start)?,
                    u64_to_i64(segment.end)?,
                    u64_to_i64(segment.durable_offset)?,
                    segment.worker_state,
                    segment.checksum,
                    segment.checkpoint_at.get(),
                ],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Loads a job's segments in ascending range order.
    ///
    /// # Errors
    /// Returns [`StorageError`] when stored segment data is invalid or unreadable.
    pub fn segments(&self, job_id: JobId) -> Result<Vec<Segment>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, range_start, range_end, durable_offset, worker_state, \
             checksum, checkpoint_at_ms FROM segments WHERE job_id=?1 ORDER BY range_start, id",
        )?;
        let segments = statement
            .query_map([job_id.to_string()], row_to_segment)?
            .collect::<Result<Vec<_>, _>>()?;
        for segment in &segments {
            segment.validate()?;
        }
        Ok(segments)
    }

    /// Appends a redacted audit event and returns its durable sequence number.
    ///
    /// # Errors
    /// Returns [`StorageError`] for invalid event data or persistence failures.
    pub fn append_event(&mut self, event: &AuditEvent) -> Result<i64, StorageError> {
        validate_event(event)?;
        let sequence = self.connection.query_row(
            "INSERT INTO event_log \
             (id, job_id, timestamp_ms, component, severity, event_type, correlation_id, redacted_detail) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) RETURNING sequence",
            params![
                event.id.to_string(),
                event.job_id.to_string(),
                event.timestamp.get(),
                event.component,
                event.severity,
                event.event_type,
                event.correlation_id,
                event.redacted_detail,
            ],
            |row| row.get(0),
        )?;
        Ok(sequence)
    }

    /// Loads events in durable insertion order.
    ///
    /// # Errors
    /// Returns [`StorageError`] when event data cannot be read.
    pub fn events(&self, job_id: JobId) -> Result<Vec<AuditEvent>, StorageError> {
        let mut statement = self.connection.prepare(
            "SELECT id, job_id, sequence, timestamp_ms, component, severity, event_type, \
             correlation_id, redacted_detail FROM event_log WHERE job_id=?1 ORDER BY sequence",
        )?;
        Ok(statement
            .query_map([job_id.to_string()], row_to_event)?
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// Atomically updates the authoritative state and journals the acknowledged transition.
    ///
    /// # Errors
    /// Returns [`StorageError`] for an illegal transition or transaction failure.
    pub fn transition_job(
        &mut self,
        job_id: JobId,
        next: DownloadState,
        at: UnixMillis,
        reason: Option<String>,
    ) -> Result<DownloadState, StorageError> {
        let transaction = self.connection.transaction()?;
        let current_text: String = transaction.query_row(
            "SELECT state FROM jobs WHERE id=?1",
            [job_id.to_string()],
            |row| row.get(0),
        )?;
        let current = parse_state(&current_text)?;
        let reduced = reduce_state(current, next)?;
        let completed_at = (next == DownloadState::Completed).then_some(at.get());
        transaction.execute(
            "UPDATE jobs SET state=?2, updated_at_ms=?3, \
             completed_at_ms=COALESCE(?4, completed_at_ms) WHERE id=?1",
            params![job_id.to_string(), next.as_str(), at.get(), completed_at],
        )?;
        let event = DownloadEvent::StateChanged {
            transition: StateTransition {
                job_id,
                from: current,
                to: next,
                at,
                reason,
            },
        };
        transaction.execute(
            "INSERT INTO event_log \
             (id, job_id, timestamp_ms, component, severity, event_type, redacted_detail) \
             VALUES (?1, ?2, ?3, 'engine', 'info', 'state_changed', ?4)",
            params![
                EventId::new().to_string(),
                job_id.to_string(),
                at.get(),
                serde_json::to_string(&event)
                    .map_err(|error| StorageError::Corrupt(error.to_string()))?,
            ],
        )?;
        transaction.commit()?;
        Ok(reduced)
    }

    /// Atomically acknowledges a graceful single-stream pause and its exact durable offset.
    ///
    /// # Errors
    /// Returns an error unless the job is pausing, has exactly one segment, and all values persist.
    pub fn checkpoint_single_stream_pause(
        &mut self,
        job_id: JobId,
        durable_offset: u64,
        at: UnixMillis,
    ) -> Result<(), StorageError> {
        let transaction = self.connection.transaction()?;
        let current_text: String = transaction.query_row(
            "SELECT state FROM jobs WHERE id=?1",
            [job_id.to_string()],
            |row| row.get(0),
        )?;
        let current = parse_state(&current_text)?;
        reduce_state(current, DownloadState::Paused)?;
        let changed = transaction.execute(
            "UPDATE segments SET durable_offset=?2, worker_state='paused', checkpoint_at_ms=?3 \
             WHERE job_id=?1",
            params![job_id.to_string(), u64_to_i64(durable_offset)?, at.get()],
        )?;
        if changed != 1 {
            return Err(StorageError::Corrupt(format!(
                "single-stream pause expected one segment, found {changed}"
            )));
        }
        transaction.execute(
            "UPDATE jobs SET state='paused', downloaded_bytes=?2, updated_at_ms=?3 WHERE id=?1",
            params![job_id.to_string(), u64_to_i64(durable_offset)?, at.get()],
        )?;
        let event = DownloadEvent::StateChanged {
            transition: StateTransition {
                job_id,
                from: current,
                to: DownloadState::Paused,
                at,
                reason: Some(format!("paused at durable offset {durable_offset}")),
            },
        };
        transaction.execute(
            "INSERT INTO event_log \
             (id, job_id, timestamp_ms, component, severity, event_type, redacted_detail) \
             VALUES (?1, ?2, ?3, 'engine', 'info', 'state_changed', ?4)",
            params![
                EventId::new().to_string(),
                job_id.to_string(),
                at.get(),
                serde_json::to_string(&event)
                    .map_err(|error| StorageError::Corrupt(error.to_string()))?,
            ],
        )?;
        transaction.commit()?;
        Ok(())
    }

    /// Removes only completed history and old events belonging to completed jobs.
    ///
    /// # Errors
    /// Returns [`StorageError`] when the cleanup transaction fails.
    pub fn apply_retention(
        &mut self,
        completed_before: UnixMillis,
        events_before: UnixMillis,
    ) -> Result<RetentionResult, StorageError> {
        let transaction = self.connection.transaction()?;
        let events_deleted = transaction.execute(
            "DELETE FROM event_log WHERE timestamp_ms < ?1 AND job_id IN \
             (SELECT id FROM jobs WHERE state='completed')",
            [events_before.get()],
        )?;
        let jobs_deleted = transaction.execute(
            "DELETE FROM jobs WHERE state='completed' AND completed_at_ms IS NOT NULL \
             AND completed_at_ms < ?1",
            [completed_before.get()],
        )?;
        transaction.commit()?;
        Ok(RetentionResult {
            jobs_deleted,
            events_deleted,
        })
    }

    fn get_remote_identity(&self, job_id: JobId) -> Result<Option<RemoteIdentity>, StorageError> {
        let identity = self
            .connection
            .query_row(
                "SELECT etag, last_modified, content_length, mime_type, content_fingerprint, \
                 range_capability FROM remote_identities WHERE job_id=?1",
                [job_id.to_string()],
                |row| {
                    Ok(RemoteIdentity {
                        etag: row.get(0)?,
                        last_modified: row.get(1)?,
                        content_length: optional_i64_to_u64(row.get(2)?)?,
                        mime_type: row.get(3)?,
                        content_fingerprint: row.get(4)?,
                        range_capability: parse_range_capability(&row.get::<_, String>(5)?)?,
                    })
                },
            )
            .optional()?;
        if let Some(identity) = &identity {
            identity.validate()?;
        }
        Ok(identity)
    }
}

#[derive(Clone, Copy)]
struct Migration {
    version: u32,
    sql: &'static str,
}

fn configure(connection: &Connection) -> Result<(), rusqlite::Error> {
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", true)?;
    connection.pragma_update(None, "journal_mode", "WAL")?;
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(())
}

fn apply_migrations(connection: &Connection, migrations: &[Migration]) -> Result<(), StorageError> {
    connection.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_versions (\
         version INTEGER PRIMARY KEY CHECK(version > 0), \
         applied_at_ms INTEGER NOT NULL CHECK(applied_at_ms >= 0)\
         ) STRICT;",
    )?;
    let current: u32 = connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_versions",
        [],
        |row| row.get(0),
    )?;
    for migration in migrations
        .iter()
        .filter(|migration| migration.version > current)
    {
        let transaction = connection.unchecked_transaction()?;
        transaction.execute_batch(migration.sql)?;
        transaction.execute(
            "INSERT INTO schema_versions (version, applied_at_ms) \
             VALUES (?1, CAST(unixepoch('subsec') * 1000 AS INTEGER))",
            [migration.version],
        )?;
        transaction.commit()?;
    }
    Ok(())
}

fn migration_backup_path(path: &Path) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(".pre-migration.bak");
    PathBuf::from(name)
}

fn existing_schema_version(path: &Path) -> Result<u32, StorageError> {
    let connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let has_version_table: bool = connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='schema_versions')",
        [],
        |row| row.get(0),
    )?;
    if !has_version_table {
        return Ok(0);
    }
    Ok(connection.query_row(
        "SELECT COALESCE(MAX(version), 0) FROM schema_versions",
        [],
        |row| row.get(0),
    )?)
}

fn insert_job(transaction: &Transaction<'_>, metadata: &JobMetadata) -> Result<(), StorageError> {
    let values = job_params(metadata)?;
    transaction.execute(
        "INSERT INTO jobs \
         (id, source_url, final_url, display_url, filename, destination, state, kind, \
          total_bytes, downloaded_bytes, category, queue_name, priority, source_browser, \
          source_profile, retry_count, error, created_at_ms, updated_at_ms, completed_at_ms) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, \
                 ?15, ?16, ?17, ?18, ?19, ?20)",
        rusqlite::params_from_iter(values),
    )?;
    Ok(())
}

fn job_params(metadata: &JobMetadata) -> Result<Vec<rusqlite::types::Value>, StorageError> {
    use rusqlite::types::Value;
    Ok(vec![
        Value::Text(metadata.id.to_string()),
        Value::Text(metadata.source_url.to_string()),
        optional_text(metadata.final_url.as_ref().map(ToString::to_string)),
        Value::Text(metadata.display_url.clone()),
        Value::Text(metadata.filename.clone()),
        Value::Text(metadata.destination.to_string_lossy().into_owned()),
        Value::Text(metadata.state.as_str().to_owned()),
        Value::Text(kind_text(metadata.kind).to_owned()),
        optional_integer(optional_u64_to_i64(metadata.total_bytes)?),
        Value::Integer(u64_to_i64(metadata.downloaded_bytes)?),
        optional_text(metadata.category.clone()),
        optional_text(metadata.queue.clone()),
        Value::Text(priority_text(metadata.priority).to_owned()),
        optional_text(metadata.source_browser.clone()),
        optional_text(metadata.source_profile.clone()),
        Value::Integer(i64::from(metadata.retry_count)),
        optional_text(metadata.error.clone()),
        Value::Integer(metadata.created_at.get()),
        Value::Integer(metadata.updated_at.get()),
        optional_integer(metadata.completed_at.map(UnixMillis::get)),
    ])
}

fn optional_text(value: Option<String>) -> rusqlite::types::Value {
    value.map_or(rusqlite::types::Value::Null, rusqlite::types::Value::Text)
}

fn optional_integer(value: Option<i64>) -> rusqlite::types::Value {
    value.map_or(
        rusqlite::types::Value::Null,
        rusqlite::types::Value::Integer,
    )
}

fn insert_remote_identity(
    transaction: &Transaction<'_>,
    job_id: JobId,
    identity: &RemoteIdentity,
) -> Result<(), StorageError> {
    transaction.execute(
        "INSERT INTO remote_identities \
         (job_id, etag, last_modified, content_length, mime_type, content_fingerprint, range_capability) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            job_id.to_string(),
            identity.etag,
            identity.last_modified,
            optional_u64_to_i64(identity.content_length)?,
            identity.mime_type,
            identity.content_fingerprint,
            range_capability_text(identity.range_capability),
        ],
    )?;
    Ok(())
}

fn row_to_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobMetadata> {
    let final_url: Option<String> = row.get(2)?;
    Ok(JobMetadata {
        id: parse_column(&row.get::<_, String>(0)?)?,
        source_url: parse_column(&row.get::<_, String>(1)?)?,
        final_url: final_url.as_deref().map(parse_column).transpose()?,
        display_url: row.get(3)?,
        filename: row.get(4)?,
        destination: PathBuf::from(row.get::<_, String>(5)?),
        state: parse_state_column(&row.get::<_, String>(6)?)?,
        kind: parse_kind(&row.get::<_, String>(7)?)?,
        total_bytes: optional_i64_to_u64(row.get(8)?)?,
        downloaded_bytes: i64_to_u64(row.get(9)?)?,
        category: row.get(10)?,
        queue: row.get(11)?,
        priority: parse_priority(&row.get::<_, String>(12)?)?,
        source_browser: row.get(13)?,
        source_profile: row.get(14)?,
        retry_count: row.get(15)?,
        error: row.get(16)?,
        created_at: parse_timestamp(row.get(17)?)?,
        updated_at: parse_timestamp(row.get(18)?)?,
        completed_at: row
            .get::<_, Option<i64>>(19)?
            .map(parse_timestamp)
            .transpose()?,
    })
}

fn row_to_segment(row: &rusqlite::Row<'_>) -> rusqlite::Result<Segment> {
    Ok(Segment {
        id: parse_column::<SegmentId>(&row.get::<_, String>(0)?)?,
        job_id: parse_column::<JobId>(&row.get::<_, String>(1)?)?,
        start: i64_to_u64(row.get(2)?)?,
        end: i64_to_u64(row.get(3)?)?,
        durable_offset: i64_to_u64(row.get(4)?)?,
        worker_state: row.get(5)?,
        checksum: row.get(6)?,
        checkpoint_at: parse_timestamp(row.get(7)?)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<AuditEvent> {
    Ok(AuditEvent {
        id: parse_column::<EventId>(&row.get::<_, String>(0)?)?,
        job_id: parse_column::<JobId>(&row.get::<_, String>(1)?)?,
        sequence: row.get(2)?,
        timestamp: parse_timestamp(row.get(3)?)?,
        component: row.get(4)?,
        severity: row.get(5)?,
        event_type: row.get(6)?,
        correlation_id: row.get(7)?,
        redacted_detail: row.get(8)?,
    })
}

fn validate_event(event: &AuditEvent) -> Result<(), StorageError> {
    for (name, value, maximum) in [
        ("component", event.component.as_str(), 128),
        ("severity", event.severity.as_str(), 32),
        ("event_type", event.event_type.as_str(), 128),
        ("redacted_detail", event.redacted_detail.as_str(), 16_384),
    ] {
        if value.is_empty() || value.len() > maximum {
            return Err(StorageError::Corrupt(format!("invalid event {name}")));
        }
    }
    Ok(())
}

fn u64_to_i64(value: u64) -> Result<i64, StorageError> {
    i64::try_from(value).map_err(|_| StorageError::IntegerOverflow(value))
}

fn optional_u64_to_i64(value: Option<u64>) -> Result<Option<i64>, StorageError> {
    value.map(u64_to_i64).transpose()
}

fn i64_to_u64(value: i64) -> rusqlite::Result<u64> {
    u64::try_from(value).map_err(sql_conversion_error)
}

fn optional_i64_to_u64(value: Option<i64>) -> rusqlite::Result<Option<u64>> {
    value.map(i64_to_u64).transpose()
}

fn parse_timestamp(value: i64) -> rusqlite::Result<UnixMillis> {
    UnixMillis::new(value).map_err(sql_conversion_error)
}

fn parse_column<T>(value: &str) -> rusqlite::Result<T>
where
    T: FromStr,
    T::Err: std::error::Error + Send + Sync + 'static,
{
    value.parse().map_err(sql_conversion_error)
}

fn sql_conversion_error(error: impl std::error::Error + Send + Sync + 'static) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(0, rusqlite::types::Type::Text, Box::new(error))
}

fn parse_state(value: &str) -> Result<DownloadState, StorageError> {
    value
        .parse::<DownloadState>()
        .map_err(|error| StorageError::Corrupt(error.to_string()))
}

fn parse_state_column(value: &str) -> rusqlite::Result<DownloadState> {
    value.parse().map_err(sql_conversion_error)
}

const fn kind_text(value: DownloadKind) -> &'static str {
    match value {
        DownloadKind::File => "file",
        DownloadKind::Media => "media",
    }
}

fn parse_kind(value: &str) -> rusqlite::Result<DownloadKind> {
    match value {
        "file" => Ok(DownloadKind::File),
        "media" => Ok(DownloadKind::Media),
        _ => Err(sql_conversion_error(StoredValueError(value.to_owned()))),
    }
}

const fn priority_text(value: Priority) -> &'static str {
    match value {
        Priority::Low => "low",
        Priority::Normal => "normal",
        Priority::High => "high",
    }
}

fn parse_priority(value: &str) -> rusqlite::Result<Priority> {
    match value {
        "low" => Ok(Priority::Low),
        "normal" => Ok(Priority::Normal),
        "high" => Ok(Priority::High),
        _ => Err(sql_conversion_error(StoredValueError(value.to_owned()))),
    }
}

const fn range_capability_text(value: RangeCapability) -> &'static str {
    match value {
        RangeCapability::Unknown => "unknown",
        RangeCapability::Supported => "supported",
        RangeCapability::Unsupported => "unsupported",
    }
}

fn parse_range_capability(value: &str) -> rusqlite::Result<RangeCapability> {
    match value {
        "unknown" => Ok(RangeCapability::Unknown),
        "supported" => Ok(RangeCapability::Supported),
        "unsupported" => Ok(RangeCapability::Unsupported),
        _ => Err(sql_conversion_error(StoredValueError(value.to_owned()))),
    }
}

#[derive(Debug, Error)]
#[error("invalid stored value: {0}")]
struct StoredValueError(String);

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{AuditEvent, Database, EventId, Migration, RetentionResult};
    use crate::{
        domain::{
            DownloadKind, HttpUrl, JobId, JobMetadata, Priority, RangeCapability, RemoteIdentity,
            Segment, SegmentId, UnixMillis,
        },
        state::DownloadState,
    };

    fn timestamp(value: i64) -> UnixMillis {
        UnixMillis::new(value).expect("valid timestamp")
    }

    fn job(state: DownloadState, at: i64) -> JobMetadata {
        JobMetadata {
            id: JobId::new(),
            source_url: HttpUrl::parse("https://example.test/file.bin").expect("URL"),
            final_url: None,
            display_url: "https://example.test/file.bin".to_owned(),
            filename: "file.bin".to_owned(),
            destination: PathBuf::from(r"C:\Downloads\file.bin"),
            state,
            kind: DownloadKind::File,
            total_bytes: Some(100),
            downloaded_bytes: 0,
            category: Some("General".to_owned()),
            queue: None,
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: timestamp(at),
            updated_at: timestamp(at),
            completed_at: (state == DownloadState::Completed).then(|| timestamp(at)),
        }
    }

    fn identity() -> RemoteIdentity {
        RemoteIdentity {
            etag: Some("\"abc\"".to_owned()),
            last_modified: Some("Wed, 21 Oct 2015 07:28:00 GMT".to_owned()),
            content_length: Some(100),
            mime_type: Some("application/octet-stream".to_owned()),
            content_fingerprint: None,
            range_capability: RangeCapability::Supported,
        }
    }

    #[test]
    fn database_creates_reopens_and_reports_schema_version() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("state.sqlite");
        let database = Database::open(&path).expect("create database");
        assert_eq!(database.schema_version().expect("version"), 2);
        drop(database);
        let reopened = Database::open(&path).expect("reopen database");
        assert_eq!(reopened.schema_version().expect("version"), 2);
        assert!(reopened.migration_backup().is_none());
    }

    #[test]
    fn job_and_remote_identity_crud_round_trip() {
        let mut database = Database::open_in_memory().expect("database");
        let mut metadata = job(DownloadState::Created, 1);
        database
            .create_job(&metadata, Some(&identity()))
            .expect("create job");
        let stored = database
            .get_job(metadata.id)
            .expect("read job")
            .expect("job exists");
        assert_eq!(stored.metadata, metadata);
        assert_eq!(stored.remote_identity, Some(identity()));

        metadata.filename = "renamed.bin".to_owned();
        metadata.updated_at = timestamp(2);
        database.update_job(&metadata).expect("update job");
        assert_eq!(
            database
                .list_jobs()
                .expect("list jobs")
                .first()
                .expect("one job")
                .metadata
                .filename,
            "renamed.bin"
        );
        assert!(database.delete_job(metadata.id).expect("delete job"));
        assert!(database.get_job(metadata.id).expect("read").is_none());
    }

    #[test]
    fn job_and_identity_insert_rolls_back_as_one_transaction() {
        let mut database = Database::open_in_memory().expect("database");
        database
            .connection
            .execute_batch(
                "CREATE TRIGGER reject_identity BEFORE INSERT ON remote_identities \
                 BEGIN SELECT RAISE(ABORT, 'injected failure'); END;",
            )
            .expect("trigger");
        let metadata = job(DownloadState::Created, 1);
        assert!(database.create_job(&metadata, Some(&identity())).is_err());
        assert!(database.get_job(metadata.id).expect("read").is_none());
    }

    #[test]
    fn segments_enforce_ranges_and_events_are_ordered() {
        let mut database = Database::open_in_memory().expect("database");
        let metadata = job(DownloadState::Created, 1);
        database.create_job(&metadata, None).expect("job");
        let segment = Segment {
            id: SegmentId::new(),
            job_id: metadata.id,
            start: 0,
            end: 99,
            durable_offset: 50,
            worker_state: "active".to_owned(),
            checksum: None,
            checkpoint_at: timestamp(2),
        };
        database
            .replace_segments(metadata.id, std::slice::from_ref(&segment))
            .expect("segments");
        assert_eq!(
            database.segments(metadata.id).expect("read segments"),
            vec![segment]
        );
        assert!(
            database
                .connection
                .execute(
                    "INSERT INTO segments \
                     (id, job_id, range_start, range_end, durable_offset, worker_state, checkpoint_at_ms) \
                     VALUES (?1, ?2, 10, 5, 10, 'idle', 3)",
                    rusqlite::params![SegmentId::new().to_string(), metadata.id.to_string()],
                )
                .is_err()
        );

        for at in [4, 3] {
            database
                .append_event(&AuditEvent {
                    id: EventId::new(),
                    job_id: metadata.id,
                    sequence: 0,
                    timestamp: timestamp(at),
                    component: "engine".to_owned(),
                    severity: "info".to_owned(),
                    event_type: "test".to_owned(),
                    correlation_id: None,
                    redacted_detail: format!("event-{at}"),
                })
                .expect("event");
        }
        let events = database.events(metadata.id).expect("events");
        assert_eq!(events[0].redacted_detail, "event-4");
        assert_eq!(events[1].redacted_detail, "event-3");
        assert!(events[0].sequence < events[1].sequence);
    }

    #[test]
    fn state_update_and_event_are_atomic() {
        let mut database = Database::open_in_memory().expect("database");
        let metadata = job(DownloadState::Created, 1);
        database.create_job(&metadata, None).expect("job");
        database
            .transition_job(metadata.id, DownloadState::Probing, timestamp(2), None)
            .expect("transition");
        assert_eq!(
            database
                .get_job(metadata.id)
                .expect("read")
                .expect("job")
                .metadata
                .state,
            DownloadState::Probing
        );
        assert_eq!(database.events(metadata.id).expect("events").len(), 1);
        assert!(
            database
                .transition_job(metadata.id, DownloadState::Completed, timestamp(3), None)
                .is_err()
        );
        assert_eq!(database.events(metadata.id).expect("events").len(), 1);
    }

    #[test]
    fn graceful_pause_persists_exact_single_offset_atomically() {
        let mut database = Database::open_in_memory().expect("database");
        let mut metadata = job(DownloadState::Pausing, 1);
        metadata.total_bytes = Some(1_000);
        database.create_job(&metadata, None).expect("job");
        database
            .replace_segments(
                metadata.id,
                &[Segment {
                    id: SegmentId::new(),
                    job_id: metadata.id,
                    start: 0,
                    end: 999,
                    durable_offset: 0,
                    worker_state: "active".to_owned(),
                    checksum: None,
                    checkpoint_at: timestamp(1),
                }],
            )
            .expect("segment");
        database
            .checkpoint_single_stream_pause(metadata.id, 417, timestamp(2))
            .expect("pause checkpoint");
        let job = database
            .get_job(metadata.id)
            .expect("read job")
            .expect("job");
        assert_eq!(job.metadata.state, DownloadState::Paused);
        assert_eq!(job.metadata.downloaded_bytes, 417);
        let segment = database
            .segments(metadata.id)
            .expect("segments")
            .into_iter()
            .next()
            .expect("segment");
        assert_eq!(segment.durable_offset, 417);
        assert_eq!(segment.worker_state, "paused");
        assert_eq!(database.events(metadata.id).expect("events").len(), 1);
    }

    #[test]
    fn failed_migration_rolls_back_and_preserves_backup() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("broken.sqlite");
        let original = Connection::open(&path).expect("database");
        original
            .execute_batch("CREATE TABLE marker(value TEXT); INSERT INTO marker VALUES ('safe');")
            .expect("marker");
        drop(original);
        let migrations = [Migration {
            version: 1,
            sql: "CREATE TABLE partial(value TEXT); THIS IS NOT SQL;",
        }];
        assert!(Database::open_with_migrations(&path, &migrations).is_err());
        let reopened = Connection::open(&path).expect("original remains usable");
        let marker: String = reopened
            .query_row("SELECT value FROM marker", [], |row| row.get(0))
            .expect("marker survives");
        assert_eq!(marker, "safe");
        let partial_exists: bool = reopened
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE name='partial')",
                [],
                |row| row.get(0),
            )
            .expect("schema query");
        assert!(!partial_exists);
        assert!(super::migration_backup_path(&path).exists());
    }

    #[test]
    fn retention_never_age_deletes_active_or_partial_jobs() {
        let mut database = Database::open_in_memory().expect("database");
        let completed = job(DownloadState::Completed, 1);
        let downloading = job(DownloadState::Downloading, 1);
        let paused = job(DownloadState::Paused, 1);
        for metadata in [&completed, &downloading, &paused] {
            database.create_job(metadata, None).expect("job");
            database
                .append_event(&AuditEvent {
                    id: EventId::new(),
                    job_id: metadata.id,
                    sequence: 0,
                    timestamp: timestamp(1),
                    component: "engine".to_owned(),
                    severity: "info".to_owned(),
                    event_type: "test".to_owned(),
                    correlation_id: None,
                    redacted_detail: "safe".to_owned(),
                })
                .expect("event");
        }
        assert_eq!(
            database
                .apply_retention(timestamp(10), timestamp(10))
                .expect("retention"),
            RetentionResult {
                jobs_deleted: 1,
                events_deleted: 1
            }
        );
        assert!(database.get_job(completed.id).expect("read").is_none());
        assert!(database.get_job(downloading.id).expect("read").is_some());
        assert!(database.get_job(paused.id).expect("read").is_some());
    }
}
