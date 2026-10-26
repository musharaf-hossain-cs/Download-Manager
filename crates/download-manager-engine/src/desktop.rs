//! Desktop-facing request dispatcher. The desktop never opens engine storage directly.

use std::{
    fs,
    fs::OpenOptions,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use thiserror::Error;

use crate::{
    VERSION,
    domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
    filename::{safe_destination, sanitize_windows_filename, select_filename},
    ipc::{
        DuplicateAction, Envelope, JobAction, ProtocolError, UI_PROTOCOL_VERSION, UiMessage,
        UiRequest, UiResponse, decode_ui_request,
    },
    probe::HttpProbe,
    state::DownloadState,
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
        &mut self,
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
            UiRequest::AddJob {
                source_url,
                final_url,
                filename,
                destination,
                total_bytes,
                start_now,
                duplicate_action,
            } => self.add_job(AddJobInput {
                source_url,
                final_url,
                filename,
                destination,
                total_bytes,
                start_now,
                duplicate_action,
            })?,
            UiRequest::ControlJob { job_id, action } => self.control_job(job_id, action)?,
        };
        Ok(Envelope::new(
            UI_PROTOCOL_VERSION,
            request.request_id,
            UiMessage::Response { response },
        ))
    }

    fn add_job(&mut self, input: AddJobInput) -> Result<UiResponse, StorageError> {
        let existing = self.database.list_jobs()?.into_iter().find(|job| {
            job.metadata.source_url == input.source_url
                || job.metadata.final_url.as_ref() == Some(&input.final_url)
        });
        if input.duplicate_action == DuplicateAction::FocusExisting
            && let Some(existing) = existing
        {
            return Ok(UiResponse::Accepted {
                job_id: existing.metadata.id,
            });
        }
        let now = current_time();
        let metadata = JobMetadata {
            id: JobId::new(),
            display_url: input.source_url.to_string(),
            source_url: input.source_url,
            final_url: Some(input.final_url),
            filename: sanitize_windows_filename(&input.filename),
            destination: input.destination,
            state: if input.start_now {
                DownloadState::Queued
            } else {
                DownloadState::Created
            },
            kind: DownloadKind::File,
            total_bytes: input.total_bytes,
            downloaded_bytes: 0,
            category: None,
            queue: None,
            priority: Priority::Normal,
            source_browser: None,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at: now,
            updated_at: now,
            completed_at: None,
        };
        if input.duplicate_action == DuplicateAction::ReplaceExisting
            && let Some(existing) = existing
        {
            self.database.replace_job(existing.metadata.id, &metadata)?;
        } else {
            self.database.create_job(&metadata, None)?;
        }
        Ok(UiResponse::Accepted {
            job_id: metadata.id,
        })
    }

    fn control_job(
        &mut self,
        job_id: JobId,
        action: JobAction,
    ) -> Result<UiResponse, StorageError> {
        let Some(job) = self.database.get_job(job_id)? else {
            return Ok(UiResponse::Rejected {
                code: "job_not_found".to_owned(),
                message: format!("job {job_id} does not exist"),
            });
        };
        let Some(next) = control_target(job.metadata.state, action) else {
            return Ok(UiResponse::Rejected {
                code: "invalid_job_action".to_owned(),
                message: format!("cannot {action:?} a {} job", job.metadata.state),
            });
        };
        let at = current_time();
        let state = self.database.transition_job(
            job_id,
            next,
            at,
            Some(format!("desktop {action:?} control").to_ascii_lowercase()),
        )?;
        Ok(UiResponse::JobState {
            job_id,
            state,
            updated_at: at,
        })
    }
}

const fn control_target(current: DownloadState, action: JobAction) -> Option<DownloadState> {
    match (current, action) {
        (DownloadState::Created, JobAction::Start) => Some(DownloadState::Probing),
        (DownloadState::Queued, JobAction::Start) => Some(DownloadState::Connecting),
        (DownloadState::Downloading, JobAction::Pause | JobAction::Stop) => {
            Some(DownloadState::Pausing)
        }
        (DownloadState::Queued, JobAction::Pause | JobAction::Stop) => Some(DownloadState::Paused),
        (DownloadState::Paused, JobAction::Resume) => Some(DownloadState::Queued),
        _ => None,
    }
}

struct AddJobInput {
    source_url: HttpUrl,
    final_url: HttpUrl,
    filename: String,
    destination: PathBuf,
    total_bytes: Option<u64>,
    start_now: bool,
    duplicate_action: DuplicateAction,
}

fn current_time() -> UnixMillis {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    UnixMillis::new(i64::try_from(milliseconds).unwrap_or(i64::MAX)).expect("bounded timestamp")
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
        ipc::{
            DuplicateAction, Envelope, JobAction, UI_PROTOCOL_VERSION, UiMessage, UiRequest,
            UiResponse,
        },
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

    fn add_request(source_url: &str, start_now: bool, action: DuplicateAction) -> UiRequest {
        UiRequest::AddJob {
            source_url: HttpUrl::parse(source_url).expect("source URL"),
            final_url: HttpUrl::parse(source_url).expect("final URL"),
            filename: if start_now {
                "started.bin".to_owned()
            } else {
                "later.bin".to_owned()
            },
            destination: PathBuf::from(r"C:\Downloads"),
            total_bytes: Some(42),
            start_now,
            duplicate_action: action,
        }
    }

    fn accepted_id(service: &mut DesktopService, payload: UiRequest) -> JobId {
        let response = service.handle(&request(payload)).expect("add response");
        match response.payload {
            UiMessage::Response {
                response: UiResponse::Accepted { job_id },
            } => job_id,
            other => panic!("expected accepted response, got {other:?}"),
        }
    }

    fn listed_jobs(service: &mut DesktopService) -> Vec<JobMetadata> {
        let response = service
            .handle(&request(UiRequest::ListJobs))
            .expect("list response");
        match response.payload {
            UiMessage::Response {
                response: UiResponse::Jobs { jobs },
            } => jobs,
            other => panic!("expected jobs response, got {other:?}"),
        }
    }

    #[test]
    fn health_and_persisted_jobs_are_dispatched_over_typed_ipc() {
        let mut database = Database::open_in_memory().expect("database");
        let job = persisted_job();
        database.create_job(&job, None).expect("persist job");
        let mut service = DesktopService::new(database);

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
        let mut service = DesktopService::new(Database::open_in_memory().expect("database"));
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
        let mut service = DesktopService::new(Database::open_in_memory().expect("database"));

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
        let mut service = DesktopService::new(Database::open_in_memory().expect("database"));
        let encoded = br#"{"version":1,"request_id":"018f47ce-8e51-7f31-a1ec-4a97f84a12cd","payload":{"type":"probe_url","source_url":"file:///secret","destination":"C:\\Downloads"}}"#;
        assert!(service.handle(encoded).is_err());
    }

    #[test]
    fn start_choices_persist_exactly_one_job_in_the_requested_state() {
        let mut service = DesktopService::new(Database::open_in_memory().expect("database"));
        accepted_id(
            &mut service,
            add_request(
                "https://example.test/now",
                true,
                DuplicateAction::AddDuplicate,
            ),
        );
        accepted_id(
            &mut service,
            add_request(
                "https://example.test/later",
                false,
                DuplicateAction::AddDuplicate,
            ),
        );

        let jobs = listed_jobs(&mut service);
        assert_eq!(jobs.len(), 2);
        assert_eq!(
            jobs.iter()
                .find(|job| job.source_url.as_str().ends_with("/now"))
                .expect("start-now job")
                .state,
            DownloadState::Queued
        );
        assert_eq!(
            jobs.iter()
                .find(|job| job.source_url.as_str().ends_with("/later"))
                .expect("start-later job")
                .state,
            DownloadState::Created
        );
    }

    #[test]
    fn duplicate_choices_focus_add_or_atomically_replace_one_durable_job() {
        let source = "https://example.test/duplicate";

        let mut focus = DesktopService::new(Database::open_in_memory().expect("database"));
        let original = accepted_id(
            &mut focus,
            add_request(source, false, DuplicateAction::AddDuplicate),
        );
        let focused = accepted_id(
            &mut focus,
            add_request(source, true, DuplicateAction::FocusExisting),
        );
        assert_eq!(focused, original);
        assert_eq!(listed_jobs(&mut focus).len(), 1);

        let mut add = DesktopService::new(Database::open_in_memory().expect("database"));
        accepted_id(
            &mut add,
            add_request(source, false, DuplicateAction::AddDuplicate),
        );
        accepted_id(
            &mut add,
            add_request(source, true, DuplicateAction::AddDuplicate),
        );
        assert_eq!(listed_jobs(&mut add).len(), 2);

        let mut replace = DesktopService::new(Database::open_in_memory().expect("database"));
        let replaced = accepted_id(
            &mut replace,
            add_request(source, false, DuplicateAction::AddDuplicate),
        );
        let replacement = accepted_id(
            &mut replace,
            add_request(source, true, DuplicateAction::ReplaceExisting),
        );
        let jobs = listed_jobs(&mut replace);
        assert_ne!(replacement, replaced);
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].id, replacement);
        assert_eq!(jobs[0].state, DownloadState::Queued);
        assert_eq!(jobs[0].filename, "started.bin");
    }

    #[test]
    fn start_pause_resume_and_stop_controls_are_atomic_and_journaled() {
        let cases = [
            (
                DownloadState::Created,
                JobAction::Start,
                DownloadState::Probing,
            ),
            (
                DownloadState::Downloading,
                JobAction::Pause,
                DownloadState::Pausing,
            ),
            (
                DownloadState::Paused,
                JobAction::Resume,
                DownloadState::Queued,
            ),
            (
                DownloadState::Queued,
                JobAction::Stop,
                DownloadState::Paused,
            ),
        ];

        for (initial, action, expected) in cases {
            let mut database = Database::open_in_memory().expect("database");
            let mut job = persisted_job();
            job.state = initial;
            database.create_job(&job, None).expect("persist job");
            let mut service = DesktopService::new(database);
            let response = service
                .handle(&request(UiRequest::ControlJob {
                    job_id: job.id,
                    action,
                }))
                .expect("control response");

            assert!(matches!(
                response.payload,
                UiMessage::Response {
                    response: UiResponse::JobState { job_id, state, .. }
                } if job_id == job.id && state == expected
            ));
            assert_eq!(listed_jobs(&mut service)[0].state, expected);
            assert_eq!(service.database.events(job.id).expect("events").len(), 1);
        }
    }

    #[test]
    fn invalid_job_control_is_rejected_without_durable_state_change() {
        let mut database = Database::open_in_memory().expect("database");
        let mut job = persisted_job();
        job.state = DownloadState::Completed;
        job.completed_at = Some(job.updated_at);
        database.create_job(&job, None).expect("persist job");
        let mut service = DesktopService::new(database);

        let response = service
            .handle(&request(UiRequest::ControlJob {
                job_id: job.id,
                action: JobAction::Resume,
            }))
            .expect("rejection response");

        assert!(matches!(
            response.payload,
            UiMessage::Response {
                response: UiResponse::Rejected { code, .. }
            } if code == "invalid_job_action"
        ));
        assert_eq!(listed_jobs(&mut service)[0].state, DownloadState::Completed);
        assert_eq!(service.database.events(job.id).expect("events").len(), 0);
    }
}
