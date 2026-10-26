//! Desktop-facing request dispatcher. The desktop never opens engine storage directly.

use std::{fs, fs::OpenOptions, path::Path};

use thiserror::Error;

use crate::{
    VERSION,
    domain::{HttpUrl, JobId},
    filename::{safe_destination, select_filename},
    ipc::{
        Envelope, ProtocolError, UI_PROTOCOL_VERSION, UiMessage, UiRequest, UiResponse,
        decode_ui_request,
    },
    probe::HttpProbe,
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
            UiRequest::ProbeUrl {
                source_url,
                destination,
            } => probe_url(&source_url, &destination),
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

fn probe_url(source_url: &HttpUrl, destination: &Path) -> UiResponse {
    if let Err(error) = verify_writable_directory(destination) {
        return UiResponse::Rejected {
            code: "destination_unavailable".to_owned(),
            message: format!(
                "destination {} is not writable: {error}",
                destination.display()
            ),
        };
    }
    match HttpProbe::new().and_then(|probe| probe.probe(source_url)) {
        Ok(probe) => {
            let filename = select_filename(
                probe.content_disposition.as_deref(),
                &probe.final_url,
                probe.identity.mime_type.as_deref(),
            );
            UiResponse::UrlProbe {
                final_url: probe.final_url,
                destination: safe_destination(destination, &filename),
                filename,
                total_bytes: probe.identity.content_length,
                mime_type: probe.identity.mime_type,
            }
        }
        Err(error) => UiResponse::Rejected {
            code: "probe_failed".to_owned(),
            message: error.to_string(),
        },
    }
}

fn verify_writable_directory(destination: &Path) -> std::io::Result<()> {
    if !destination.metadata()?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotADirectory,
            "path is not a directory",
        ));
    }
    let marker = destination.join(format!(".download-manager-write-test-{}", JobId::new()));
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    fs::remove_file(marker)
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::{
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, RequestId, UnixMillis},
        ipc::{Envelope, UI_PROTOCOL_VERSION, UiMessage, UiRequest, UiResponse},
        state::DownloadState,
        storage::Database,
        test_support::{Fixture, FixtureServer},
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

    #[test]
    fn url_probe_validates_destination_and_preserves_unicode_filename() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .header("Content-Length", "42")
                .header("Content-Type", "application/pdf")
                .header(
                    "Content-Disposition",
                    "attachment; filename*=UTF-8''%E0%A6%AC%E0%A6%BE%E0%A6%82%E0%A6%B2%E0%A6%BE.pdf",
                ),
        ]);
        let directory = tempfile::tempdir().expect("temp directory");
        let unicode_directory = directory.path().join("ডাউনলোড");
        fs::create_dir(&unicode_directory).expect("unicode directory");
        let service = DesktopService::new(Database::open_in_memory().expect("database"));

        let response = service
            .handle(&request(UiRequest::ProbeUrl {
                source_url: HttpUrl::parse(server.url("/document")).expect("URL"),
                destination: unicode_directory.clone(),
            }))
            .expect("probe response");
        assert!(matches!(
            response.payload,
            UiMessage::Response {
                response: UiResponse::UrlProbe {
                    filename,
                    destination,
                    total_bytes: Some(42),
                    ..
                }
            } if filename == "বাংলা.pdf"
                && destination == unicode_directory.join("বাংলা.pdf")
        ));

        let inaccessible = service
            .handle(&request(UiRequest::ProbeUrl {
                source_url: HttpUrl::parse(server.url("/unused")).expect("URL"),
                destination: directory.path().join("missing"),
            }))
            .expect("rejection response");
        assert!(matches!(
            inaccessible.payload,
            UiMessage::Response {
                response: UiResponse::Rejected { code, .. }
            } if code == "destination_unavailable"
        ));
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn url_probe_rejects_invalid_url_before_dispatch() {
        let service = DesktopService::new(Database::open_in_memory().expect("database"));
        let encoded = br#"{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"probe_url","source_url":"file:///secret","destination":"C:\\Downloads"}}"#;
        assert!(service.handle(encoded).is_err());
    }
}
