//! Desktop-facing request dispatcher. The desktop never opens engine storage directly.

use thiserror::Error;

use crate::{
    VERSION,
    ipc::{
        Envelope, ProtocolError, UI_PROTOCOL_VERSION, UiMessage, UiRequest, UiResponse,
        decode_ui_request,
    },
    storage::{Database, StorageError},
};

#[derive(Debug, Error)]
pub enum DesktopServiceError {
    #[error("invalid UI request: {0}")]
    Protocol(#[from] ProtocolError),
    #[error("desktop storage request failed: {0}")]
    Storage(#[from] StorageError),
}

/// Synchronous core of the local IPC service hosted by the engine process.
pub struct DesktopService {
    database: Database,
}

impl DesktopService {
    #[must_use]
    pub const fn new(database: Database) -> Self {
        Self { database }
    }

    /// Strictly decodes and dispatches one desktop request.
    ///
    /// # Errors
    /// Returns a protocol error for malformed/unversioned input and a storage error when
    /// persisted jobs cannot be loaded.
    pub fn handle(
        &self,
        encoded_request: &[u8],
    ) -> Result<Envelope<UiMessage>, DesktopServiceError> {
        let request = decode_ui_request(encoded_request)?;
        let response = match request.payload {
            UiRequest::Health => UiResponse::Health {
                engine_version: VERSION.to_owned(),
            },
            UiRequest::ListJobs => UiResponse::Jobs {
                jobs: self
                    .database
                    .list_jobs()?
                    .into_iter()
                    .map(|record| record.metadata)
                    .collect(),
            },
            UiRequest::AddJob { .. } | UiRequest::TransitionJob { .. } => UiResponse::Rejected {
                code: "not_implemented".to_owned(),
                message: "this desktop command is not available yet".to_owned(),
            },
        };
        Ok(Envelope::new(
            UI_PROTOCOL_VERSION,
            request.request_id,
            UiMessage::Response { response },
        ))
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::{
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, RequestId, UnixMillis},
        ipc::{Envelope, UI_PROTOCOL_VERSION, UiMessage, UiRequest, UiResponse},
        state::DownloadState,
        storage::Database,
    };

    use super::DesktopService;

    fn persisted_job() -> JobMetadata {
        JobMetadata {
            id: JobId::new(),
            source_url: HttpUrl::parse("https://example.test/persisted.zip").expect("URL"),
            final_url: None,
            display_url: "https://example.test/persisted.zip".to_owned(),
            filename: "persisted.zip".to_owned(),
            destination: PathBuf::from(r"C:\Downloads"),
            state: DownloadState::Queued,
            kind: DownloadKind::File,
            total_bytes: Some(1_024),
            downloaded_bytes: 128,
            category: Some("Compressed".to_owned()),
            queue: None,
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: UnixMillis::new(1_700_000_000_000).expect("time"),
            updated_at: UnixMillis::new(1_700_000_000_100).expect("time"),
            completed_at: None,
        }
    }

    fn request(payload: UiRequest) -> Vec<u8> {
        serde_json::to_vec(&Envelope::new(
            UI_PROTOCOL_VERSION,
            RequestId::new(),
            payload,
        ))
        .expect("serialize request")
    }

    #[test]
    fn health_and_persisted_jobs_are_dispatched_over_typed_ipc() {
        let mut database = Database::open_in_memory().expect("database");
        let job = persisted_job();
        database.create_job(&job, None).expect("persist job");
        let service = DesktopService::new(database);

        let health = service
            .handle(&request(UiRequest::Health))
            .expect("health response");
        assert!(matches!(
            health.payload,
            UiMessage::Response {
                response: UiResponse::Health { .. }
            }
        ));

        let listed = service
            .handle(&request(UiRequest::ListJobs))
            .expect("list response");
        assert!(matches!(
            listed.payload,
            UiMessage::Response {
                response: UiResponse::Jobs { jobs }
            } if jobs == vec![job]
        ));
    }

    #[test]
    fn malformed_desktop_input_is_rejected_before_dispatch() {
        let service = DesktopService::new(Database::open_in_memory().expect("database"));
        assert!(
            service
                .handle(br#"{"version":1,"unexpected":true}"#)
                .is_err()
        );
    }
}
