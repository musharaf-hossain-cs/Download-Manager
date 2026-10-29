//! Validated preview and transactional application of exported local state.

use std::collections::{BTreeMap, HashMap};

use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    domain::UnixMillis,
    export_archive::{EXPORT_ARCHIVE_FORMAT_VERSION, ExportArchiveDocument},
    redaction::redact_json_value,
};

const MAX_ARCHIVE_BYTES: usize = 64 * 1024 * 1024;

/// User-selected behavior for records already present in local state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImportMode {
    /// Keep local records absent from the archive and replace matching records.
    Merge,
    /// Make every imported section exactly match the archive.
    Replace,
}

/// The four non-secret sections committed together by an import.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportArchiveState {
    pub settings: Value,
    pub categories: Value,
    pub queues: Value,
    pub history: Value,
}

/// Optimistic store snapshot used to bind preview to the state the user reviewed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportStoreSnapshot {
    pub revision: u64,
    pub state: ImportArchiveState,
}

/// Per-section effect shown before a transactional import is accepted.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ImportSectionChanges {
    pub added: usize,
    pub updated: usize,
    pub unchanged: usize,
    pub retained: usize,
    pub removed: usize,
}

/// Complete review document, including the exact state passed to the store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportPreview {
    pub mode: ImportMode,
    pub exported_at: UnixMillis,
    pub payload_sha256: String,
    pub expected_revision: u64,
    pub settings: ImportSectionChanges,
    pub categories: ImportSectionChanges,
    pub queues: ImportSectionChanges,
    pub history: ImportSectionChanges,
    pub replacement: ImportArchiveState,
}

#[derive(Debug, Error)]
pub enum ImportArchiveError {
    #[error("import archive is {actual} bytes; maximum is {maximum}")]
    ArchiveTooLarge { actual: usize, maximum: usize },
    #[error("import archive is not valid versioned JSON: {0}")]
    Decode(#[from] serde_json::Error),
    #[error("import archive format version {actual} is unsupported; expected {expected}")]
    UnsupportedVersion { actual: u32, expected: u32 },
    #[error("import archive {section} section must be a JSON {expected}")]
    InvalidSection {
        section: &'static str,
        expected: &'static str,
    },
    #[error("import archive payload checksum is invalid")]
    ChecksumMismatch,
    #[error("import archive contains secret-bearing values")]
    SecretBearingArchive,
    #[error("import archive {section} item must have a non-empty string {identity}")]
    InvalidIdentity {
        section: &'static str,
        identity: &'static str,
    },
    #[error("import archive {section} contains duplicate {identity} {value:?}")]
    DuplicateIdentity {
        section: &'static str,
        identity: &'static str,
        value: String,
    },
}

/// Storage boundary for one all-or-nothing, revision-checked import commit.
pub trait TransactionalImportStore {
    type Error;

    /// Atomically replaces all four sections if `expected_revision` is still current.
    /// Implementations must leave every section unchanged on any error.
    ///
    /// # Errors
    /// Returns a store-specific error if the revision is stale or the transaction cannot commit.
    fn commit_import(
        &mut self,
        expected_revision: u64,
        replacement: &ImportArchiveState,
    ) -> Result<u64, Self::Error>;
}

/// Validated archive plus the immutable import effect the user reviewed.
#[derive(Debug, Clone)]
pub struct PreparedImportArchive {
    preview: ImportPreview,
}

impl PreparedImportArchive {
    /// Parses and authenticates the archive, then computes its exact merge or replace effect.
    ///
    /// # Errors
    /// Returns [`ImportArchiveError`] for oversized, old, malformed, tampered, secret-bearing, or
    /// structurally invalid archives.
    pub fn prepare(
        bytes: &[u8],
        current: &ImportStoreSnapshot,
        mode: ImportMode,
    ) -> Result<Self, ImportArchiveError> {
        if bytes.len() > MAX_ARCHIVE_BYTES {
            return Err(ImportArchiveError::ArchiveTooLarge {
                actual: bytes.len(),
                maximum: MAX_ARCHIVE_BYTES,
            });
        }
        let document: ExportArchiveDocument = serde_json::from_slice(bytes)?;
        if document.format_version != EXPORT_ARCHIVE_FORMAT_VERSION {
            return Err(ImportArchiveError::UnsupportedVersion {
                actual: document.format_version,
                expected: EXPORT_ARCHIVE_FORMAT_VERSION,
            });
        }

        validate_shapes(&document)?;
        if payload_sha256(&document)? != document.payload_sha256 {
            return Err(ImportArchiveError::ChecksumMismatch);
        }
        let imported = state_from(&document);
        if redact_state(&imported) != imported {
            return Err(ImportArchiveError::SecretBearingArchive);
        }

        let current_categories = indexed(&current.state.categories, "categories", "id")?;
        let imported_categories = indexed(&imported.categories, "categories", "id")?;
        let current_queues = indexed(&current.state.queues, "queues", "name")?;
        let imported_queues = indexed(&imported.queues, "queues", "name")?;
        let current_history = indexed(&current.state.history, "history", "id")?;
        let imported_history = indexed(&imported.history, "history", "id")?;

        let settings = object_changes(&current.state.settings, &imported.settings, mode);
        let categories = indexed_changes(&current_categories, &imported_categories, mode);
        let queues = indexed_changes(&current_queues, &imported_queues, mode);
        let history = indexed_changes(&current_history, &imported_history, mode);
        let replacement = match mode {
            ImportMode::Replace => imported,
            ImportMode::Merge => ImportArchiveState {
                settings: merge_objects(&current.state.settings, &imported.settings),
                categories: merge_array(&current.state.categories, &imported.categories, "id"),
                queues: merge_array(&current.state.queues, &imported.queues, "name"),
                history: merge_array(&current.state.history, &imported.history, "id"),
            },
        };
        Ok(Self {
            preview: ImportPreview {
                mode,
                exported_at: document.exported_at,
                payload_sha256: document.payload_sha256,
                expected_revision: current.revision,
                settings,
                categories,
                queues,
                history,
                replacement,
            },
        })
    }

    #[must_use]
    pub const fn preview(&self) -> &ImportPreview {
        &self.preview
    }

    /// Commits the exact previewed state in one revision-checked store transaction.
    ///
    /// # Errors
    /// Returns the store error without changing the prepared preview. The store contract requires
    /// every failed call to leave all four sections unchanged.
    pub fn apply<S: TransactionalImportStore>(&self, store: &mut S) -> Result<u64, S::Error> {
        store.commit_import(self.preview.expected_revision, &self.preview.replacement)
    }
}

fn validate_shapes(document: &ExportArchiveDocument) -> Result<(), ImportArchiveError> {
    if !document.settings.is_object() {
        return Err(ImportArchiveError::InvalidSection {
            section: "settings",
            expected: "object",
        });
    }
    for (section, value) in [
        ("categories", &document.categories),
        ("queues", &document.queues),
        ("history", &document.history),
    ] {
        if !value.is_array() {
            return Err(ImportArchiveError::InvalidSection {
                section,
                expected: "array",
            });
        }
    }
    Ok(())
}

fn payload_sha256(document: &ExportArchiveDocument) -> Result<String, serde_json::Error> {
    let payload = serde_json::to_vec(&(
        &document.settings,
        &document.categories,
        &document.queues,
        &document.history,
    ))?;
    Ok(format!("{:x}", Sha256::digest(payload)))
}

fn state_from(document: &ExportArchiveDocument) -> ImportArchiveState {
    ImportArchiveState {
        settings: document.settings.clone(),
        categories: document.categories.clone(),
        queues: document.queues.clone(),
        history: document.history.clone(),
    }
}

fn redact_state(state: &ImportArchiveState) -> ImportArchiveState {
    ImportArchiveState {
        settings: redact_json_value(&state.settings),
        categories: redact_json_value(&state.categories),
        queues: redact_json_value(&state.queues),
        history: redact_json_value(&state.history),
    }
}

fn indexed<'a>(
    value: &'a Value,
    section: &'static str,
    identity: &'static str,
) -> Result<BTreeMap<&'a str, &'a Value>, ImportArchiveError> {
    let Some(items) = value.as_array() else {
        return Err(ImportArchiveError::InvalidSection {
            section,
            expected: "array",
        });
    };
    let mut result = BTreeMap::new();
    for item in items {
        let Some(key) = item
            .as_object()
            .and_then(|object| object.get(identity))
            .and_then(Value::as_str)
            .filter(|key| !key.trim().is_empty() && key.len() <= 1024)
        else {
            return Err(ImportArchiveError::InvalidIdentity { section, identity });
        };
        if result.insert(key, item).is_some() {
            return Err(ImportArchiveError::DuplicateIdentity {
                section,
                identity,
                value: key.to_owned(),
            });
        }
    }
    Ok(result)
}

fn indexed_changes(
    current: &BTreeMap<&str, &Value>,
    imported: &BTreeMap<&str, &Value>,
    mode: ImportMode,
) -> ImportSectionChanges {
    let mut changes = ImportSectionChanges::default();
    for (key, value) in imported {
        match current.get(key) {
            None => changes.added += 1,
            Some(existing) if existing == value => changes.unchanged += 1,
            Some(_) => changes.updated += 1,
        }
    }
    let absent = current
        .keys()
        .filter(|key| !imported.contains_key(*key))
        .count();
    match mode {
        ImportMode::Merge => changes.retained = absent,
        ImportMode::Replace => changes.removed = absent,
    }
    changes
}

fn object_changes(current: &Value, imported: &Value, mode: ImportMode) -> ImportSectionChanges {
    let current = current.as_object().expect("validated current settings");
    let imported = imported.as_object().expect("validated imported settings");
    let mut changes = ImportSectionChanges::default();
    for (key, value) in imported {
        match current.get(key) {
            None => changes.added += 1,
            Some(existing) if existing == value => changes.unchanged += 1,
            Some(_) => changes.updated += 1,
        }
    }
    let absent = current
        .keys()
        .filter(|key| !imported.contains_key(*key))
        .count();
    match mode {
        ImportMode::Merge => changes.retained = absent,
        ImportMode::Replace => changes.removed = absent,
    }
    changes
}

fn merge_objects(current: &Value, imported: &Value) -> Value {
    let mut result = current.clone();
    merge_value(&mut result, imported);
    result
}

fn merge_value(current: &mut Value, imported: &Value) {
    if let (Some(current), Some(imported)) = (current.as_object_mut(), imported.as_object()) {
        for (key, value) in imported {
            if let Some(existing) = current.get_mut(key) {
                merge_value(existing, value);
            } else {
                current.insert(key.clone(), value.clone());
            }
        }
    } else {
        *current = imported.clone();
    }
}

fn merge_array(current: &Value, imported: &Value, identity: &str) -> Value {
    let mut result = current.as_array().expect("validated current array").clone();
    let mut positions = result
        .iter()
        .enumerate()
        .map(|(index, item)| {
            (
                item.get(identity)
                    .and_then(Value::as_str)
                    .expect("validated identity")
                    .to_owned(),
                index,
            )
        })
        .collect::<HashMap<_, _>>();
    for item in imported.as_array().expect("validated imported array") {
        let key = item
            .get(identity)
            .and_then(Value::as_str)
            .expect("validated identity");
        if let Some(index) = positions.get(key).copied() {
            result[index] = item.clone();
        } else {
            positions.insert(key.to_owned(), result.len());
            result.push(item.clone());
        }
    }
    Value::Array(result)
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};
    use sha2::{Digest, Sha256};

    use super::{
        ImportArchiveError, ImportArchiveState, ImportMode, ImportStoreSnapshot,
        PreparedImportArchive, TransactionalImportStore,
    };
    use crate::{
        domain::UnixMillis,
        export_archive::{ExportArchiveInput, PreparedExportArchive},
    };

    #[derive(Debug, PartialEq, Eq)]
    enum StoreError {
        Stale,
        Injected,
    }

    struct MemoryStore {
        revision: u64,
        state: ImportArchiveState,
        fail: bool,
    }

    impl TransactionalImportStore for MemoryStore {
        type Error = StoreError;

        fn commit_import(
            &mut self,
            expected_revision: u64,
            replacement: &ImportArchiveState,
        ) -> Result<u64, Self::Error> {
            if expected_revision != self.revision {
                return Err(StoreError::Stale);
            }
            if self.fail {
                return Err(StoreError::Injected);
            }
            self.state = replacement.clone();
            self.revision += 1;
            Ok(self.revision)
        }
    }

    fn archive() -> PreparedExportArchive {
        PreparedExportArchive::prepare(&ExportArchiveInput {
            exported_at: UnixMillis::new(1_798_646_400_000).expect("timestamp"),
            settings: json!({"general": {"theme": "dark", "quiet": true}}),
            categories: json!([
                {"id": "general", "name": "General"},
                {"id": "docs", "name": "Documents"}
            ]),
            queues: json!([{"name": "Night", "limit": 2}]),
            history: json!([{"id": "job-1", "state": "completed"}]),
        })
        .expect("export")
    }

    fn current() -> ImportStoreSnapshot {
        ImportStoreSnapshot {
            revision: 7,
            state: ImportArchiveState {
                settings: json!({
                    "general": {"theme": "light", "language": "bn"},
                    "network": {"connections": 8}
                }),
                categories: json!([
                    {"id": "general", "name": "Local General"},
                    {"id": "music", "name": "Music"}
                ]),
                queues: json!([{"name": "Day", "limit": 1}]),
                history: json!([{"id": "job-0", "state": "completed"}]),
            },
        }
    }

    #[test]
    fn export_round_trip_previews_and_atomically_replaces_all_sections() {
        let export = archive();
        let prepared =
            PreparedImportArchive::prepare(export.bytes(), &current(), ImportMode::Replace)
                .expect("import");
        let preview = prepared.preview();
        assert_eq!(preview.expected_revision, 7);
        assert_eq!(preview.categories.added, 1);
        assert_eq!(preview.categories.updated, 1);
        assert_eq!(preview.categories.removed, 1);
        assert_eq!(preview.queues.added, 1);
        assert_eq!(preview.queues.removed, 1);
        assert_eq!(preview.replacement.settings, export.preview().settings);
        assert_eq!(preview.replacement.history, export.preview().history);

        let before = current();
        let mut failed = MemoryStore {
            revision: before.revision,
            state: before.state.clone(),
            fail: true,
        };
        assert_eq!(prepared.apply(&mut failed), Err(StoreError::Injected));
        assert_eq!(
            failed.state, before.state,
            "failed transaction mutated state"
        );

        failed.fail = false;
        assert_eq!(prepared.apply(&mut failed), Ok(8));
        assert_eq!(failed.state, preview.replacement);
        assert_eq!(prepared.apply(&mut failed), Err(StoreError::Stale));
    }

    #[test]
    fn merge_preserves_absent_records_and_overlays_matching_records() {
        let prepared =
            PreparedImportArchive::prepare(archive().bytes(), &current(), ImportMode::Merge)
                .expect("merge");
        let preview = prepared.preview();
        assert_eq!(preview.categories.retained, 1);
        assert_eq!(preview.settings.retained, 1);
        assert_eq!(preview.settings.updated, 1);
        assert_eq!(
            preview.replacement.settings,
            json!({
                "general": {"theme": "dark", "language": "bn", "quiet": true},
                "network": {"connections": 8}
            })
        );
        assert_eq!(preview.replacement.categories.as_array().unwrap().len(), 3);
        assert_eq!(preview.replacement.queues.as_array().unwrap().len(), 2);
        assert_eq!(preview.replacement.history.as_array().unwrap().len(), 2);
    }

    #[test]
    fn old_corrupt_tampered_and_duplicate_archives_are_rejected_before_preview() {
        let export = archive();
        let mut old: Value = serde_json::from_slice(export.bytes()).expect("JSON");
        old["format_version"] = json!(0);
        assert!(matches!(
            PreparedImportArchive::prepare(
                &serde_json::to_vec(&old).unwrap(),
                &current(),
                ImportMode::Merge
            ),
            Err(ImportArchiveError::UnsupportedVersion { .. })
        ));

        assert!(matches!(
            PreparedImportArchive::prepare(b"{broken", &current(), ImportMode::Merge),
            Err(ImportArchiveError::Decode(_))
        ));

        let mut tampered: Value = serde_json::from_slice(export.bytes()).expect("JSON");
        tampered["settings"]["general"]["theme"] = json!("tampered");
        assert!(matches!(
            PreparedImportArchive::prepare(
                &serde_json::to_vec(&tampered).unwrap(),
                &current(),
                ImportMode::Merge
            ),
            Err(ImportArchiveError::ChecksumMismatch)
        ));

        let mut duplicate: Value = serde_json::from_slice(export.bytes()).expect("JSON");
        duplicate["categories"] = json!([
            {"id": "same", "name": "One"},
            {"id": "same", "name": "Two"}
        ]);
        let payload = serde_json::to_vec(&(
            &duplicate["settings"],
            &duplicate["categories"],
            &duplicate["queues"],
            &duplicate["history"],
        ))
        .unwrap();
        duplicate["payload_sha256"] = json!(format!("{:x}", Sha256::digest(payload)));
        assert!(matches!(
            PreparedImportArchive::prepare(
                &serde_json::to_vec(&duplicate).unwrap(),
                &current(),
                ImportMode::Merge
            ),
            Err(ImportArchiveError::DuplicateIdentity { .. })
        ));
    }
}
