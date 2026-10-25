//! User-bound Windows Credential Manager adapter.

use serde::{Deserialize, Serialize};
use thiserror::Error;

#[cfg(windows)]
use keyring_core::api::CredentialStoreApi;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CredentialRef {
    pub service: String,
    pub account: String,
}

#[derive(Debug, Error)]
pub enum CredentialError {
    #[error("credential reference fields must be non-empty and bounded")]
    InvalidReference,
    #[cfg(windows)]
    #[error("Windows credential store operation failed: {0}")]
    Platform(#[from] keyring_core::Error),
    #[cfg(not(windows))]
    #[error("Windows Credential Manager is unavailable on this platform")]
    UnsupportedPlatform,
}

/// Stores secrets in the current Windows user's Credential Manager vault.
#[derive(Debug, Clone)]
pub struct WindowsCredentialStore {
    namespace: String,
    #[cfg(windows)]
    backend: std::sync::Arc<windows_native_keyring_store::Store>,
}

impl WindowsCredentialStore {
    /// Creates a namespaced adapter so tests/profiles cannot collide.
    ///
    /// # Errors
    /// Returns an error for an empty or oversized namespace.
    pub fn new(namespace: impl Into<String>) -> Result<Self, CredentialError> {
        let namespace = namespace.into();
        validate(&namespace, 256)?;
        #[cfg(windows)]
        let backend = windows_native_keyring_store::Store::new()?;
        Ok(Self {
            namespace,
            #[cfg(windows)]
            backend,
        })
    }

    /// Produces a non-secret reference safe to store in SQLite or logs.
    ///
    /// # Errors
    /// Returns an error for empty or oversized host/account values.
    pub fn reference(&self, host: &str, account: &str) -> Result<CredentialRef, CredentialError> {
        validate(host, 512)?;
        validate(account, 512)?;
        Ok(CredentialRef {
            service: format!("{}/{}", self.namespace, host.to_ascii_lowercase()),
            account: account.to_owned(),
        })
    }

    /// Saves or replaces a password in the current user's encrypted OS vault.
    ///
    /// # Errors
    /// Returns an error if the reference is invalid or the OS store rejects the operation.
    pub fn store(&self, reference: &CredentialRef, password: &str) -> Result<(), CredentialError> {
        validate_reference(reference)?;
        #[cfg(windows)]
        {
            self.backend
                .build(&reference.service, &reference.account, None)?
                .set_password(password)?;
            Ok(())
        }
        #[cfg(not(windows))]
        {
            let _ = password;
            Err(CredentialError::UnsupportedPlatform)
        }
    }

    /// Loads a password from the current user's encrypted OS vault.
    ///
    /// # Errors
    /// Returns an error if the reference is invalid, missing, or inaccessible.
    pub fn load(&self, reference: &CredentialRef) -> Result<String, CredentialError> {
        validate_reference(reference)?;
        #[cfg(windows)]
        {
            Ok(self
                .backend
                .build(&reference.service, &reference.account, None)?
                .get_password()?)
        }
        #[cfg(not(windows))]
        {
            Err(CredentialError::UnsupportedPlatform)
        }
    }

    /// Removes a password without touching any persisted non-secret reference.
    ///
    /// # Errors
    /// Returns an error if the reference is invalid or the OS store rejects the operation.
    pub fn delete(&self, reference: &CredentialRef) -> Result<(), CredentialError> {
        validate_reference(reference)?;
        #[cfg(windows)]
        {
            self.backend
                .build(&reference.service, &reference.account, None)?
                .delete_credential()?;
            Ok(())
        }
        #[cfg(not(windows))]
        {
            Err(CredentialError::UnsupportedPlatform)
        }
    }
}

fn validate_reference(reference: &CredentialRef) -> Result<(), CredentialError> {
    validate(&reference.service, 1_024)?;
    validate(&reference.account, 512)
}

fn validate(value: &str, maximum: usize) -> Result<(), CredentialError> {
    if value.trim().is_empty() || value.len() > maximum {
        Err(CredentialError::InvalidReference)
    } else {
        Ok(())
    }
}

#[cfg(all(test, windows))]
mod tests {
    use std::fs;

    use rusqlite::Connection;
    use tempfile::tempdir;
    use uuid::Uuid;

    use super::WindowsCredentialStore;

    #[test]
    fn windows_store_round_trips_without_plaintext_database_or_log_value() {
        let unique = Uuid::new_v4().to_string();
        let store = WindowsCredentialStore::new(format!("DownloadManager/Test/{unique}"))
            .expect("credential store");
        let reference = store
            .reference("example.test", "test-user")
            .expect("reference");
        let secret = format!("secret-{unique}");
        store.store(&reference, &secret).expect("store password");
        assert_eq!(store.load(&reference).expect("load password"), secret);

        let directory = tempdir().expect("temp directory");
        let database_path = directory.path().join("references.sqlite");
        let database = Connection::open(&database_path).expect("database");
        database
            .execute_batch(
                "CREATE TABLE credential_refs(service TEXT NOT NULL, account TEXT NOT NULL);",
            )
            .expect("schema");
        database
            .execute(
                "INSERT INTO credential_refs VALUES (?1, ?2)",
                (&reference.service, &reference.account),
            )
            .expect("reference row");
        drop(database);
        let database_bytes = fs::read(database_path).expect("database bytes");
        assert!(
            !database_bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes())
        );
        assert!(!format!("{reference:?}").contains(&secret));

        store.delete(&reference).expect("delete password");
        assert!(store.load(&reference).is_err());
    }
}
