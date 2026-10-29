//! Transport-independent conditional synchronization for offline grabber projects.

use std::path::PathBuf;

use thiserror::Error;

use crate::{
    domain::HttpUrl,
    grabber_offline::{OfflineMapper, OfflineRewriteError},
};

/// Validators sent with one conditional synchronization request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConditionalSyncRequest {
    pub url: HttpUrl,
    pub if_none_match: Option<String>,
    pub if_modified_since: Option<String>,
}

/// Durable manifest state for one synchronized remote object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncResource {
    pub url: HttpUrl,
    pub relative_path: PathBuf,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: u64,
    pub sha256: [u8; 32],
}

/// Identity and verified content metadata returned for a changed remote object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChangedRemoteObject {
    pub url: HttpUrl,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub content_length: u64,
    pub sha256: [u8; 32],
}

/// An opaque destination-local temporary download supplied by the transport layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StagedDownload {
    pub stage_id: String,
    pub content_length: u64,
    pub sha256: [u8; 32],
}

/// Result of one conditional remote request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalSyncOutcome {
    NotModified {
        url: HttpUrl,
    },
    Modified {
        remote: ChangedRemoteObject,
        staged: StagedDownload,
    },
    Deleted {
        url: HttpUrl,
    },
    Interrupted {
        url: HttpUrl,
    },
}

/// Local-deletion behavior for a resource missing remotely.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SyncPolicy {
    /// Opt-in only: the default preserves the local file and manifest entry.
    pub delete_local_on_remote_delete: bool,
}

/// Stable per-resource synchronization result.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyncDisposition {
    Unchanged,
    Replaced,
    PreservedRemoteDeletion,
    Deleted,
    Interrupted,
}

/// Durable local boundary used by [`ConditionalSynchronizer`].
///
/// `commit_verified_replacement` must re-verify the named destination-local stage and commit the
/// live file plus manifest entry as one recoverable transaction. On every error it must retain
/// the previous live file and manifest. `commit_deletion` has the same all-or-nothing requirement.
pub trait ConditionalSyncStore {
    /// Reads the current durable entry.
    ///
    /// # Errors
    /// Returns a stable local-store diagnostic when the manifest cannot be read.
    fn manifest_entry(&self, url: &HttpUrl) -> Result<Option<SyncResource>, String>;

    /// Verifies and atomically installs a staged file and its next manifest entry.
    ///
    /// # Errors
    /// Must return an error without changing the prior file or manifest when verification or the
    /// recoverable transaction fails.
    fn commit_verified_replacement(
        &mut self,
        staged: &StagedDownload,
        next: &SyncResource,
    ) -> Result<(), String>;

    /// Atomically removes the local file and manifest entry.
    ///
    /// # Errors
    /// Must return an error with both prior values preserved when deletion cannot commit.
    fn commit_deletion(&mut self, current: &SyncResource) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum ConditionalSyncError {
    #[error("local synchronization store failed: {0}")]
    Store(String),
    #[error("server reported not-modified for an object absent from the local manifest")]
    UnexpectedNotModified,
    #[error("staged content does not match the changed remote object")]
    StagedContentMismatch,
    #[error("offline path mapping failed: {0}")]
    Mapping(OfflineRewriteError),
}

/// Applies conditional remote outcomes through an atomic local-store boundary.
#[derive(Debug)]
pub struct ConditionalSynchronizer<S> {
    mapper: OfflineMapper,
    policy: SyncPolicy,
    store: S,
}

impl<S: ConditionalSyncStore> ConditionalSynchronizer<S> {
    #[must_use]
    pub const fn new(mapper: OfflineMapper, policy: SyncPolicy, store: S) -> Self {
        Self {
            mapper,
            policy,
            store,
        }
    }

    /// Builds a changed-only request from the durable validators, if present.
    ///
    /// # Errors
    /// Returns a local-store read failure.
    pub fn request_for(
        &self,
        url: &HttpUrl,
    ) -> Result<ConditionalSyncRequest, ConditionalSyncError> {
        let entry = self
            .store
            .manifest_entry(url)
            .map_err(ConditionalSyncError::Store)?;
        Ok(ConditionalSyncRequest {
            url: url.clone(),
            if_none_match: entry.as_ref().and_then(|value| value.etag.clone()),
            if_modified_since: entry.and_then(|value| value.last_modified),
        })
    }

    /// Applies one completed, deleted, or interrupted conditional request.
    ///
    /// A changed object is mapped through the S140 safe local mapper. The supplied stage must
    /// match the response length and SHA-256 before it can reach the store's verified atomic
    /// replacement boundary. Interrupted requests never mutate local state.
    ///
    /// # Errors
    /// Returns typed validator, staging, mapping, or transactional-store failures.
    pub fn apply(
        &mut self,
        outcome: ConditionalSyncOutcome,
    ) -> Result<SyncDisposition, ConditionalSyncError> {
        match outcome {
            ConditionalSyncOutcome::NotModified { url } => {
                self.store
                    .manifest_entry(&url)
                    .map_err(ConditionalSyncError::Store)?
                    .ok_or(ConditionalSyncError::UnexpectedNotModified)?;
                Ok(SyncDisposition::Unchanged)
            }
            ConditionalSyncOutcome::Modified { remote, staged } => {
                if staged.content_length != remote.content_length || staged.sha256 != remote.sha256
                {
                    return Err(ConditionalSyncError::StagedContentMismatch);
                }
                if self
                    .store
                    .manifest_entry(&remote.url)
                    .map_err(ConditionalSyncError::Store)?
                    .is_some_and(|current| same_remote_identity(&current, &remote))
                {
                    return Ok(SyncDisposition::Unchanged);
                }
                let mapped = self
                    .mapper
                    .map_url(&remote.url)
                    .map_err(ConditionalSyncError::Mapping)?;
                let next = SyncResource {
                    url: remote.url,
                    relative_path: mapped.relative,
                    etag: remote.etag,
                    last_modified: remote.last_modified,
                    content_length: remote.content_length,
                    sha256: remote.sha256,
                };
                self.store
                    .commit_verified_replacement(&staged, &next)
                    .map_err(ConditionalSyncError::Store)?;
                Ok(SyncDisposition::Replaced)
            }
            ConditionalSyncOutcome::Deleted { url } => {
                let Some(current) = self
                    .store
                    .manifest_entry(&url)
                    .map_err(ConditionalSyncError::Store)?
                else {
                    return Ok(SyncDisposition::Unchanged);
                };
                if !self.policy.delete_local_on_remote_delete {
                    return Ok(SyncDisposition::PreservedRemoteDeletion);
                }
                self.store
                    .commit_deletion(&current)
                    .map_err(ConditionalSyncError::Store)?;
                Ok(SyncDisposition::Deleted)
            }
            ConditionalSyncOutcome::Interrupted { .. } => Ok(SyncDisposition::Interrupted),
        }
    }

    #[must_use]
    pub fn store(&self) -> &S {
        &self.store
    }

    #[must_use]
    pub fn into_store(self) -> S {
        self.store
    }
}

fn same_remote_identity(current: &SyncResource, remote: &ChangedRemoteObject) -> bool {
    current.etag == remote.etag
        && current.last_modified == remote.last_modified
        && current.content_length == remote.content_length
        && current.sha256 == remote.sha256
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use sha2::{Digest, Sha256};
    use tempfile::tempdir;

    use super::{
        ChangedRemoteObject, ConditionalSyncError, ConditionalSyncOutcome, ConditionalSyncStore,
        ConditionalSynchronizer, StagedDownload, SyncDisposition, SyncPolicy, SyncResource,
    };
    use crate::{domain::HttpUrl, grabber_offline::OfflineMapper};

    #[derive(Debug, Clone)]
    struct FakeStore {
        manifest: HashMap<String, SyncResource>,
        files: HashMap<PathBuf, Vec<u8>>,
        stages: HashMap<String, Vec<u8>>,
        fail_commit: bool,
        commit_count: usize,
    }

    impl ConditionalSyncStore for FakeStore {
        fn manifest_entry(&self, url: &HttpUrl) -> Result<Option<SyncResource>, String> {
            Ok(self.manifest.get(url.as_str()).cloned())
        }

        fn commit_verified_replacement(
            &mut self,
            staged: &StagedDownload,
            next: &SyncResource,
        ) -> Result<(), String> {
            let bytes = self.stages.get(&staged.stage_id).ok_or("missing stage")?;
            if bytes.len() as u64 != staged.content_length || hash(bytes) != staged.sha256 {
                return Err("stage verification failed".to_owned());
            }
            if self.fail_commit {
                return Err("injected interruption before atomic commit".to_owned());
            }
            self.files.insert(next.relative_path.clone(), bytes.clone());
            self.manifest
                .insert(next.url.as_str().to_owned(), next.clone());
            self.stages.remove(&staged.stage_id);
            self.commit_count += 1;
            Ok(())
        }

        fn commit_deletion(&mut self, current: &SyncResource) -> Result<(), String> {
            if self.fail_commit {
                return Err("injected interruption before atomic delete".to_owned());
            }
            self.files.remove(&current.relative_path);
            self.manifest.remove(current.url.as_str());
            self.commit_count += 1;
            Ok(())
        }
    }

    use std::path::PathBuf;

    #[test]
    fn unchanged_fixture_sends_validators_and_never_commits() {
        let (mut synchronizer, fixture_url, old) = fixture(false);
        let request = synchronizer.request_for(&fixture_url).expect("request");
        assert_eq!(request.if_none_match.as_deref(), Some("\"old\""));
        assert_eq!(request.if_modified_since.as_deref(), Some("yesterday"));

        let result = synchronizer
            .apply(ConditionalSyncOutcome::NotModified { url: fixture_url })
            .expect("unchanged");
        assert_eq!(result, SyncDisposition::Unchanged);
        assert_eq!(synchronizer.store().commit_count, 0);
        assert_eq!(synchronizer.store().files[&old.relative_path], b"old");
    }

    #[test]
    fn changed_fixture_replaces_verified_stage_and_manifest_together() {
        let (mut synchronizer, fixture_url, old) = fixture(false);
        let new_bytes = b"changed".to_vec();
        let staged = stage("new-stage", &new_bytes);
        synchronizer
            .store
            .stages
            .insert(staged.stage_id.clone(), new_bytes.clone());
        let result = synchronizer
            .apply(changed(fixture_url.clone(), staged, &new_bytes))
            .expect("changed resource");

        assert_eq!(result, SyncDisposition::Replaced);
        let store = synchronizer.store();
        let next = &store.manifest[fixture_url.as_str()];
        assert_eq!(next.etag.as_deref(), Some("\"new\""));
        assert_eq!(store.files[&next.relative_path], new_bytes);
        assert_eq!(next.relative_path, old.relative_path);
        assert_eq!(store.commit_count, 1);
    }

    #[test]
    fn deleted_fixture_preserves_by_default_and_deletes_only_when_opted_in() {
        let (mut preserving, fixture_url, old) = fixture(false);
        assert_eq!(
            preserving
                .apply(ConditionalSyncOutcome::Deleted {
                    url: fixture_url.clone(),
                })
                .expect("preserve"),
            SyncDisposition::PreservedRemoteDeletion
        );
        assert!(preserving.store().files.contains_key(&old.relative_path));
        assert!(
            preserving
                .store()
                .manifest
                .contains_key(fixture_url.as_str())
        );

        let (mut deleting, fixture_url, old) = fixture(true);
        assert_eq!(
            deleting
                .apply(ConditionalSyncOutcome::Deleted {
                    url: fixture_url.clone(),
                })
                .expect("delete"),
            SyncDisposition::Deleted
        );
        assert!(!deleting.store().files.contains_key(&old.relative_path));
        assert!(!deleting.store().manifest.contains_key(fixture_url.as_str()));
    }

    #[test]
    fn interrupted_fixture_and_failed_commit_retain_old_file_and_manifest() {
        let (mut synchronizer, fixture_url, old) = fixture(false);
        assert_eq!(
            synchronizer
                .apply(ConditionalSyncOutcome::Interrupted {
                    url: fixture_url.clone(),
                })
                .expect("interrupted report"),
            SyncDisposition::Interrupted
        );
        synchronizer.store.fail_commit = true;
        let new_bytes = b"uncommitted".to_vec();
        let staged = stage("interrupted-stage", &new_bytes);
        synchronizer
            .store
            .stages
            .insert(staged.stage_id.clone(), new_bytes.clone());
        assert!(matches!(
            synchronizer.apply(changed(fixture_url.clone(), staged, &new_bytes)),
            Err(ConditionalSyncError::Store(_))
        ));

        let store = synchronizer.store();
        assert_eq!(store.files[&old.relative_path], b"old");
        assert_eq!(store.manifest[fixture_url.as_str()], old);
        assert_eq!(store.commit_count, 0);
    }

    fn fixture(delete_local: bool) -> (ConditionalSynchronizer<FakeStore>, HttpUrl, SyncResource) {
        let directory = tempdir().expect("destination").keep();
        let fixture_url = url("https://example.test/site/index.html");
        let mapper = OfflineMapper::new(&directory, &fixture_url).expect("mapper");
        let relative_path = mapper.map_url(&fixture_url).expect("mapping").relative;
        let old = SyncResource {
            url: fixture_url.clone(),
            relative_path: relative_path.clone(),
            etag: Some("\"old\"".to_owned()),
            last_modified: Some("yesterday".to_owned()),
            content_length: 3,
            sha256: hash(b"old"),
        };
        let store = FakeStore {
            manifest: HashMap::from([(fixture_url.as_str().to_owned(), old.clone())]),
            files: HashMap::from([(relative_path, b"old".to_vec())]),
            stages: HashMap::new(),
            fail_commit: false,
            commit_count: 0,
        };
        (
            ConditionalSynchronizer::new(
                mapper,
                SyncPolicy {
                    delete_local_on_remote_delete: delete_local,
                },
                store,
            ),
            fixture_url,
            old,
        )
    }

    fn changed(url: HttpUrl, staged: StagedDownload, bytes: &[u8]) -> ConditionalSyncOutcome {
        ConditionalSyncOutcome::Modified {
            remote: ChangedRemoteObject {
                url,
                etag: Some("\"new\"".to_owned()),
                last_modified: Some("today".to_owned()),
                content_length: bytes.len() as u64,
                sha256: hash(bytes),
            },
            staged,
        }
    }

    fn stage(id: &str, bytes: &[u8]) -> StagedDownload {
        StagedDownload {
            stage_id: id.to_owned(),
            content_length: bytes.len() as u64,
            sha256: hash(bytes),
        }
    }

    fn hash(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn url(value: &str) -> HttpUrl {
        HttpUrl::parse(value).expect("fixture URL")
    }
}
