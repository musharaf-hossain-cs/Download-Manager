//! Durable real-download backend for the private-use PU04 path.

use std::{
    collections::HashMap,
    env, fs, io,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Sender},
    },
    thread::{self, JoinHandle},
    time::{SystemTime, UNIX_EPOCH},
};

use download_manager_engine::{
    domain::{
        DownloadKind, HttpUrl, JobId, JobMetadata, Priority, RangeCapability, RemoteIdentity,
        Segment, SegmentId, UnixMillis,
    },
    filename::{safe_destination, sanitize_windows_filename, select_filename},
    finalize::{ConflictPolicy, finalize_cross_volume},
    ipc::{
        DestructiveAction, DuplicateAction, Envelope, JobAction, UI_PROTOCOL_VERSION, UiMessage,
        UiRequest, UiResponse,
    },
    probe::HttpProbe,
    recovery::{ReconciliationAction, reconcile_partial},
    resume::resume_single_stream_with_control,
    state::{DownloadState, is_legal_transition},
    storage::{Database, JobRecord, StorageError},
    transfer::{
        SingleStreamDownloader, TransferControl, TransferOutcome, TransferProgress, TransferStatus,
    },
};
use serde::Deserialize;

use crate::{AddRequest, BackendError, BrowserDownloadRequest, CliBackend, JobView, QueueStart};

const MAX_FAILURE_REASON_CHARS: usize = 4_096;
const DATABASE_FILE: &str = "downloads.sqlite3";
const PREFERENCES_FILE: &str = "preferences.json";
const BROWSER_SOURCE: &str = "browser_extension";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BrowserPreferences {
    last_download_directory: Option<PathBuf>,
}

/// SQLite-backed current-user authority with one FIFO HTTP(S) transfer worker.
pub struct DownloadBackend {
    database: Arc<Mutex<Database>>,
    work: Option<Sender<DownloadWork>>,
    worker: Option<JoinHandle<()>>,
    active: Arc<Mutex<HashMap<JobId, TransferControl>>>,
    progress: Arc<Mutex<HashMap<JobId, TransferProgress>>>,
    shutting_down: Arc<AtomicBool>,
    partial_directory: PathBuf,
    browser_download_directory: Result<PathBuf, String>,
}

impl DownloadBackend {
    /// Opens durable current-user state below `%LOCALAPPDATA%\DownloadManager\State`.
    ///
    /// # Errors
    /// Returns an I/O error when the current-user state root is unavailable or cannot be opened.
    pub fn for_current_user() -> Result<Self, io::Error> {
        let local_app_data = env::var_os("LOCALAPPDATA").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LOCALAPPDATA is required for private download state",
            )
        })?;
        let root = PathBuf::from(local_app_data).join("DownloadManager");
        let state = root.join("State");
        let mut backend =
            Self::new_with_database(state.join(DATABASE_FILE), state.join("partials"))?;
        backend.browser_download_directory = current_user_browser_download_directory(&root);
        Ok(backend)
    }

    /// Opens an isolated backend, storing its database beside the partial directory.
    ///
    /// # Errors
    /// Returns an I/O error when the database, partial directory, or worker cannot be initialized.
    pub fn new(partial_directory: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let partial_directory = partial_directory.into();
        let database_path = partial_directory
            .parent()
            .unwrap_or(&partial_directory)
            .join(DATABASE_FILE);
        Self::new_with_database(database_path, partial_directory)
    }

    /// Opens an isolated backend with explicit database and partial-file locations.
    ///
    /// # Errors
    /// Returns an I/O error when durable state cannot be opened, recovered, or served.
    pub fn new_with_database(
        database_path: impl AsRef<Path>,
        partial_directory: impl Into<PathBuf>,
    ) -> Result<Self, io::Error> {
        let partial_directory = partial_directory.into();
        fs::create_dir_all(&partial_directory)?;
        let mut database = Database::open(database_path).map_err(storage_io)?;
        recover_interrupted_jobs(&mut database, &partial_directory)?;

        let database = Arc::new(Mutex::new(database));
        let active = Arc::new(Mutex::new(HashMap::new()));
        let progress = Arc::new(Mutex::new(HashMap::new()));
        let shutting_down = Arc::new(AtomicBool::new(false));
        let (work, receiver) = mpsc::channel::<DownloadWork>();
        let worker_database = Arc::clone(&database);
        let worker_active = Arc::clone(&active);
        let worker_progress = Arc::clone(&progress);
        let worker_shutting_down = Arc::clone(&shutting_down);
        let worker_partials = partial_directory.clone();
        let worker = thread::Builder::new()
            .name("download-manager-pu04-worker".to_owned())
            .spawn(move || {
                while let Ok(download) = receiver.recv() {
                    if let Err(reason) = run_download(
                        &worker_database,
                        &worker_active,
                        &worker_progress,
                        &worker_shutting_down,
                        &worker_partials,
                        download,
                    ) {
                        fail_job(&worker_database, download.job_id, &reason);
                    }
                }
            })?;

        Ok(Self {
            database,
            work: Some(work),
            worker: Some(worker),
            active,
            progress,
            shutting_down,
            partial_directory,
            browser_download_directory: Err(
                "browser download destination is not configured".to_owned()
            ),
        })
    }

    /// Configures the destination used by browser-originated downloads.
    pub fn set_browser_download_directory(&mut self, directory: PathBuf) {
        self.browser_download_directory = Ok(directory);
    }

    fn add_with_identity(
        &mut self,
        request: AddRequest,
        job_id: JobId,
        source_browser: Option<String>,
        accept_durable_queue_failure: bool,
    ) -> Result<JobView, BackendError> {
        if !request.start_now {
            return Err(rejected(
                "unsupported_in_pu04",
                "adding a download for later is outside the private PU04 path",
            ));
        }
        if request.queue.is_some() {
            return Err(rejected(
                "unsupported_in_pu04",
                "download queues are outside the private-use milestone",
            ));
        }
        validate_output_directory(&request.output_directory)?;

        let probe = HttpProbe::new()
            .and_then(|client| client.probe(&request.source_url))
            .map_err(|error| rejected("probe_failed", error.to_string()))?;
        if !(200..300).contains(&probe.status) || probe.authentication_challenge.is_some() {
            return Err(rejected(
                "probe_failed",
                format!("HTTP probe returned status {}", probe.status),
            ));
        }
        let selected_filename = request.filename.unwrap_or_else(|| {
            select_filename(
                probe.content_disposition.as_deref(),
                &probe.final_url,
                probe.identity.mime_type.as_deref(),
            )
        });
        let filename = sanitize_windows_filename(&selected_filename);
        let created_at = now();
        let metadata = JobMetadata {
            id: job_id,
            display_url: probe.final_url.as_str().to_owned(),
            source_url: request.source_url,
            final_url: Some(probe.final_url),
            filename,
            destination: request.output_directory,
            state: DownloadState::Queued,
            kind: DownloadKind::File,
            total_bytes: probe.identity.content_length,
            downloaded_bytes: 0,
            category: None,
            queue: None,
            priority: Priority::Normal,
            source_browser,
            source_profile: None,
            retry_count: 0,
            error: None,
            created_at,
            updated_at: created_at,
            completed_at: None,
        };
        let segment = single_segment(job_id, &probe.identity, created_at);
        let partial = partial_path(&self.partial_directory, job_id);
        fs::write(&partial, []).map_err(|error| {
            BackendError::Transport(format!("could not create private partial: {error}"))
        })?;
        let segments = segment.as_slice();
        if let Err(error) = with_database(&self.database, |database| {
            database.create_job_with_segments(&metadata, Some(&probe.identity), segments)
        }) {
            let _ = fs::remove_file(&partial);
            return Err(BackendError::Transport(error));
        }

        let job = job_view(&metadata);
        if let Err(error) = send_work(
            self.work.as_ref(),
            DownloadWork {
                job_id,
                mode: TransferMode::Initial,
            },
        ) {
            pause_after_queue_failure(&self.database, job_id, &error);
            if accept_durable_queue_failure {
                return self.status(job_id);
            }
            return Err(BackendError::Transport(error));
        }
        Ok(job)
    }

    fn pause(&self, job_id: JobId) -> Result<(), BackendError> {
        let record = load_job(&self.database, job_id)?;
        match record.metadata.state {
            DownloadState::Paused | DownloadState::Pausing => return Ok(()),
            DownloadState::Queued => {
                transition(&self.database, job_id, DownloadState::Paused, None)?;
            }
            DownloadState::Downloading => {
                transition(&self.database, job_id, DownloadState::Pausing, None)?;
                if let Some(control) = lock(&self.active).get(&job_id) {
                    control.request_pause();
                }
            }
            state => {
                return Err(rejected(
                    "invalid_state",
                    format!("job {job_id} cannot be paused from {state}"),
                ));
            }
        }
        Ok(())
    }

    fn resume(&self, job_id: JobId) -> Result<(), BackendError> {
        let record = load_job(&self.database, job_id)?;
        if !matches!(
            record.metadata.state,
            DownloadState::Paused | DownloadState::Failed
        ) {
            return Err(rejected(
                "invalid_state",
                format!(
                    "job {job_id} cannot be resumed from {}",
                    record.metadata.state
                ),
            ));
        }
        let identity = record.remote_identity.as_ref().ok_or_else(|| {
            rejected(
                "resume_not_safe",
                "the durable remote identity is unavailable",
            )
        })?;
        if identity.range_capability != RangeCapability::Supported
            || identity.content_length.is_none()
        {
            return Err(rejected(
                "resume_not_safe",
                "the remote object does not advertise a supported bounded byte range",
            ));
        }
        let mut segments = with_database(&self.database, |database| database.segments(job_id))
            .map_err(BackendError::Transport)?;
        if segments.len() != 1 {
            return Err(rejected(
                "resume_not_safe",
                "the durable single-stream checkpoint is unavailable",
            ));
        }
        let partial = partial_path(&self.partial_directory, job_id);
        let reconciliation = reconcile_partial(&partial, segments[0].durable_offset)
            .map_err(|error| BackendError::Transport(error.to_string()))?;
        let Some(offset) = reconciliation.durable_offset else {
            return Err(rejected(
                "resume_not_safe",
                "the private partial file is missing",
            ));
        };
        if reconciliation.action == ReconciliationAction::DatabaseMustRewind {
            segments[0].durable_offset = offset;
            segments[0].checkpoint_at = now();
            with_database(&self.database, |database| {
                database.checkpoint_segments(job_id, &segments, now())
            })
            .map_err(BackendError::Transport)?;
        }
        with_database(&self.database, |database| {
            let mut metadata = database
                .get_job(job_id)?
                .ok_or_else(|| missing_storage_job(job_id))?
                .metadata;
            metadata.error = None;
            metadata.updated_at = now();
            database.update_job(&metadata)?;
            database.transition_job(job_id, DownloadState::Queued, now(), None)?;
            Ok(())
        })
        .map_err(BackendError::Transport)?;
        if let Err(error) = send_work(
            self.work.as_ref(),
            DownloadWork {
                job_id,
                mode: TransferMode::Resume,
            },
        ) {
            pause_after_queue_failure(&self.database, job_id, &error);
            return Err(BackendError::Transport(error));
        }
        Ok(())
    }

    fn cancel(&self, job_id: JobId) -> Result<(), BackendError> {
        let record = load_job(&self.database, job_id)?;
        match record.metadata.state {
            DownloadState::Cancelled => return Ok(()),
            DownloadState::Completed | DownloadState::Verifying | DownloadState::Assembling => {
                return Err(rejected(
                    "invalid_state",
                    "the completed-file publication boundary has already started; cancel does not delete completed output",
                ));
            }
            state if is_legal_transition(state, DownloadState::Cancelled) => {
                transition(&self.database, job_id, DownloadState::Cancelled, None)?;
                if let Some(control) = lock(&self.active).get(&job_id) {
                    control.request_pause();
                }
            }
            state => {
                return Err(rejected(
                    "invalid_state",
                    format!("job {job_id} cannot be cancelled from {state}"),
                ));
            }
        }
        Ok(())
    }
}

impl CliBackend for DownloadBackend {
    fn add(&mut self, request: AddRequest) -> Result<JobView, BackendError> {
        self.add_with_identity(request, JobId::new(), None, false)
    }

    fn add_browser(&mut self, request: BrowserDownloadRequest) -> Result<JobView, BackendError> {
        let job_id = JobId::from_uuid(request.request_id.as_uuid());
        match load_job(&self.database, job_id) {
            Ok(record) => {
                if record.metadata.source_browser.as_deref() == Some(BROWSER_SOURCE)
                    && record.metadata.source_url == request.source_url
                {
                    return Ok(job_view(&record.metadata));
                }
                return Err(rejected(
                    "duplicate_request",
                    "the browser request ID is already bound to another durable job",
                ));
            }
            Err(BackendError::NotFound(_)) => {}
            Err(error) => return Err(error),
        }

        let output_directory = self
            .browser_download_directory
            .as_ref()
            .map_err(|message| rejected("destination_unavailable", message.clone()))?
            .clone();
        self.add_with_identity(
            AddRequest {
                source_url: request.source_url,
                output_directory,
                filename: request.suggested_filename,
                start_now: true,
                queue: None,
            },
            job_id,
            Some(BROWSER_SOURCE.to_owned()),
            true,
        )
    }

    fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
        let mut jobs = with_database(&self.database, |database| database.list_jobs())
            .map_err(BackendError::Transport)?
            .into_iter()
            .map(|record| job_view(&record.metadata))
            .collect::<Vec<_>>();
        let progress = lock(&self.progress);
        for job in &mut jobs {
            apply_live_progress(job, progress.get(&job.id));
        }
        Ok(jobs)
    }

    fn status(&mut self, job_id: JobId) -> Result<JobView, BackendError> {
        let record = load_job(&self.database, job_id)?;
        let mut job = job_view(&record.metadata);
        apply_live_progress(&mut job, lock(&self.progress).get(&job_id));
        Ok(job)
    }

    fn control(&mut self, job_id: JobId, action: JobAction) -> Result<JobView, BackendError> {
        match action {
            JobAction::Pause => self.pause(job_id)?,
            JobAction::Resume | JobAction::Start => self.resume(job_id)?,
            JobAction::Cancel | JobAction::Stop => self.cancel(job_id)?,
        }
        self.status(job_id)
    }

    fn start_queue(&mut self, _name: &str) -> Result<QueueStart, BackendError> {
        Err(rejected(
            "unsupported_in_pu04",
            "download queues are outside the private-use milestone",
        ))
    }

    fn desktop_request(
        &mut self,
        request: Envelope<UiRequest>,
    ) -> Result<Envelope<UiMessage>, BackendError> {
        if request.version != UI_PROTOCOL_VERSION {
            return Err(rejected(
                "unsupported_protocol_version",
                format!(
                    "desktop protocol version {} is unsupported; expected {UI_PROTOCOL_VERSION}",
                    request.version
                ),
            ));
        }
        let response = match request.payload {
            UiRequest::Health => UiResponse::Health {
                engine_version: download_manager_engine::VERSION.to_owned(),
            },
            UiRequest::ListJobs => UiResponse::Jobs {
                jobs: self.desktop_jobs()?,
            },
            UiRequest::ProbeUrl {
                source_url,
                destination,
            } => probe_for_desktop(&source_url, &destination),
            UiRequest::AddJob {
                source_url,
                filename,
                destination,
                start_now,
                duplicate_action,
                ..
            } => {
                if duplicate_action == DuplicateAction::AddDuplicate {
                    if let Err(error) = verify_writable_directory(&destination) {
                        ui_rejected(
                            "destination_unavailable",
                            format!(
                                "destination {} is not writable: {error}",
                                destination.display()
                            ),
                        )
                    } else {
                        match self.add(AddRequest {
                            source_url,
                            output_directory: destination,
                            filename: Some(filename),
                            start_now,
                            queue: None,
                        }) {
                            Ok(job) => UiResponse::Accepted { job_id: job.id },
                            Err(error) => backend_error_response(error),
                        }
                    }
                } else {
                    ui_rejected(
                        "unsupported_duplicate_action",
                        "the private-use desktop currently adds a separate download",
                    )
                }
            }
            UiRequest::ControlJob { job_id, action } => match self.control(job_id, action) {
                Ok(_) => {
                    let metadata = load_job(&self.database, job_id)?.metadata;
                    UiResponse::JobState {
                        job_id,
                        state: metadata.state,
                        updated_at: metadata.updated_at,
                    }
                }
                Err(error) => backend_error_response(error),
            },
            UiRequest::DestructiveJob {
                job_id,
                action,
                confirmed_path,
            } => self.remove_desktop_job(job_id, action, confirmed_path.as_deref()),
        };
        Ok(Envelope::new(
            UI_PROTOCOL_VERSION,
            request.request_id,
            UiMessage::Response { response },
        ))
    }
}

impl DownloadBackend {
    fn remove_desktop_job(
        &self,
        job_id: JobId,
        action: DestructiveAction,
        confirmed_path: Option<&Path>,
    ) -> UiResponse {
        let record = match load_job(&self.database, job_id) {
            Ok(record) => record,
            Err(error) => return backend_error_response(error),
        };
        if !matches!(
            record.metadata.state,
            DownloadState::Created
                | DownloadState::Paused
                | DownloadState::Completed
                | DownloadState::NeedsAuthentication
                | DownloadState::NeedsRefresh
                | DownloadState::Blocked
                | DownloadState::Failed
                | DownloadState::Cancelled
                | DownloadState::MissingLocalFile
        ) {
            return ui_rejected(
                "job_active",
                "cancel or pause the download before removing it",
            );
        }

        let delete_file = action == DestructiveAction::DeleteFile;
        if delete_file {
            if record.metadata.state != DownloadState::Completed {
                return ui_rejected(
                    "file_delete_unavailable",
                    "only a completed download can delete its published file",
                );
            }
            let expected =
                safe_destination(&record.metadata.destination, &record.metadata.filename);
            if confirmed_path != Some(expected.as_path()) {
                return ui_rejected(
                    "confirmation_mismatch",
                    format!("confirmation must target exactly {}", expected.display()),
                );
            }
            if let Err(error) = fs::remove_file(&expected) {
                return ui_rejected(
                    "file_delete_failed",
                    format!("could not delete {}: {error}", expected.display()),
                );
            }
        } else if confirmed_path.is_some() {
            return ui_rejected(
                "confirmation_mismatch",
                "record-only removal must not include a file path",
            );
        }

        if record.metadata.state != DownloadState::Completed {
            let partial = partial_path(&self.partial_directory, job_id);
            if let Err(error) = fs::remove_file(&partial)
                && error.kind() != io::ErrorKind::NotFound
            {
                return ui_rejected(
                    "partial_delete_failed",
                    format!("could not remove private partial data: {error}"),
                );
            }
        }
        match with_database(&self.database, |database| database.delete_job(job_id)) {
            Ok(true) => UiResponse::JobRemoved {
                job_id,
                file_deleted: delete_file,
            },
            Ok(false) => ui_rejected("job_not_found", format!("job {job_id} does not exist")),
            Err(error) => backend_error_response(BackendError::Transport(error)),
        }
    }

    fn desktop_jobs(&self) -> Result<Vec<JobMetadata>, BackendError> {
        let mut jobs = with_database(&self.database, |database| database.list_jobs())
            .map_err(BackendError::Transport)?
            .into_iter()
            .map(|record| record.metadata)
            .collect::<Vec<_>>();
        let progress = lock(&self.progress);
        for job in &mut jobs {
            if let Some(live) = progress.get(&job.id) {
                job.downloaded_bytes = live.downloaded_bytes;
                job.total_bytes = live.total_bytes.or(job.total_bytes);
            }
        }
        Ok(jobs)
    }
}

impl Drop for DownloadBackend {
    fn drop(&mut self) {
        self.shutting_down.store(true, Ordering::Release);
        for control in lock(&self.active).values() {
            control.request_pause();
        }
        self.work.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[derive(Clone, Copy)]
struct DownloadWork {
    job_id: JobId,
    mode: TransferMode,
}

#[derive(Clone, Copy)]
enum TransferMode {
    Initial,
    Resume,
}

fn run_download(
    database: &Arc<Mutex<Database>>,
    active: &Arc<Mutex<HashMap<JobId, TransferControl>>>,
    progress: &Arc<Mutex<HashMap<JobId, TransferProgress>>>,
    shutting_down: &AtomicBool,
    partial_directory: &Path,
    work: DownloadWork,
) -> Result<(), String> {
    let control = TransferControl::default();
    lock(active).insert(work.job_id, control.clone());
    let Some(record) = claim_queued_job(database, work.job_id, shutting_down)? else {
        lock(active).remove(&work.job_id);
        return Ok(());
    };
    if let Err(error) = transition_internal(database, work.job_id, DownloadState::Downloading, None)
    {
        lock(active).remove(&work.job_id);
        return Err(error);
    }

    let url = record
        .metadata
        .final_url
        .as_ref()
        .unwrap_or(&record.metadata.source_url);
    let partial = partial_path(partial_directory, work.job_id);
    let transfer = (|| -> Result<TransferStatus, String> {
        match work.mode {
            TransferMode::Initial => SingleStreamDownloader::new()
                .map_err(|error| error.to_string())?
                .download_with_control(url, &partial, &control, |event| {
                    lock(progress).insert(work.job_id, event);
                })
                .map_err(|error| error.to_string()),
            TransferMode::Resume => {
                let identity = record
                    .remote_identity
                    .as_ref()
                    .ok_or_else(|| "the durable remote identity is unavailable".to_owned())?;
                let segments = with_database(database, |database| database.segments(work.job_id))?;
                if segments.len() != 1 {
                    return Err("the durable single-stream checkpoint is unavailable".to_owned());
                }
                resume_single_stream_with_control(
                    url,
                    &partial,
                    segments[0].durable_offset,
                    identity,
                    &control,
                    |event| {
                        lock(progress).insert(work.job_id, event);
                    },
                )
                .map_err(|error| error.to_string())
            }
        }
    })();
    lock(active).remove(&work.job_id);
    lock(progress).remove(&work.job_id);
    let transfer = transfer?;

    match transfer {
        TransferStatus::Paused {
            downloaded_bytes, ..
        } => acknowledge_stopped_transfer(database, work.job_id, downloaded_bytes),
        TransferStatus::Completed(outcome) => {
            match claim_completed_transfer(database, work.job_id, &outcome)? {
                CompletedTransferClaim::Cancelled | CompletedTransferClaim::Paused => return Ok(()),
                CompletedTransferClaim::Verifying => {}
            }
            if current_state(database, work.job_id)? == DownloadState::Cancelled {
                return Ok(());
            }
            transition_internal(database, work.job_id, DownloadState::Assembling, None)?;
            let latest = load_job_internal(database, work.job_id)?;
            let final_path =
                safe_destination(&latest.metadata.destination, &latest.metadata.filename);
            finalize_cross_volume(&partial, &final_path, ConflictPolicy::Ask)
                .map_err(|error| error.to_string())?;
            with_database(database, |database| {
                database.replace_segments(work.job_id, &[])?;
                Ok(())
            })?;
            if current_state(database, work.job_id)? != DownloadState::Cancelled {
                transition_internal(database, work.job_id, DownloadState::Completed, None)?;
            }
            Ok(())
        }
    }
}

fn acknowledge_stopped_transfer(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    durable_offset: u64,
) -> Result<(), String> {
    match current_state(database, job_id)? {
        DownloadState::Cancelled => Ok(()),
        DownloadState::Downloading => {
            transition_internal(database, job_id, DownloadState::Pausing, None)?;
            checkpoint_pause(database, job_id, durable_offset)
        }
        DownloadState::Pausing => checkpoint_pause(database, job_id, durable_offset),
        state => Err(format!(
            "stopped transfer reached unexpected durable state {state}"
        )),
    }
}

fn checkpoint_pause(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    durable_offset: u64,
) -> Result<(), String> {
    with_database(database, |database| {
        database.checkpoint_single_stream_pause(job_id, durable_offset, now())
    })
}

enum CompletedTransferClaim {
    Cancelled,
    Paused,
    Verifying,
}

fn claim_completed_transfer(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    outcome: &TransferOutcome,
) -> Result<CompletedTransferClaim, String> {
    with_database(database, |database| {
        let record = database
            .get_job(job_id)?
            .ok_or_else(|| missing_storage_job(job_id))?;
        if record.metadata.state == DownloadState::Cancelled {
            return Ok(CompletedTransferClaim::Cancelled);
        }
        let mut metadata = record.metadata;
        metadata.downloaded_bytes = outcome.downloaded_bytes;
        metadata.total_bytes = outcome.total_bytes;
        metadata.error = None;
        metadata.updated_at = now();
        database.update_job(&metadata)?;
        if metadata.state == DownloadState::Pausing {
            database.checkpoint_single_stream_pause(job_id, outcome.downloaded_bytes, now())?;
            Ok(CompletedTransferClaim::Paused)
        } else {
            database.transition_job(job_id, DownloadState::Verifying, now(), None)?;
            Ok(CompletedTransferClaim::Verifying)
        }
    })
}

fn recover_interrupted_jobs(database: &mut Database, partial_directory: &Path) -> io::Result<()> {
    for record in database.list_jobs().map_err(storage_io)? {
        match record.metadata.state {
            DownloadState::Queued => {
                database
                    .transition_job(
                        record.metadata.id,
                        DownloadState::Paused,
                        now(),
                        Some("backend restarted before transfer began".to_owned()),
                    )
                    .map_err(storage_io)?;
            }
            DownloadState::Downloading | DownloadState::Pausing => {
                recover_active_job(database, partial_directory, &record)?;
            }
            DownloadState::Connecting
            | DownloadState::Probing
            | DownloadState::Verifying
            | DownloadState::Assembling => {
                fail_recovered_job(database, &record, "backend restart interrupted this stage")?;
            }
            _ => {}
        }
    }
    Ok(())
}

fn recover_active_job(
    database: &mut Database,
    partial_directory: &Path,
    record: &JobRecord,
) -> io::Result<()> {
    let job_id = record.metadata.id;
    let segments = database.segments(job_id).map_err(storage_io)?;
    if segments.len() != 1 {
        return fail_recovered_job(
            database,
            record,
            "restart found no safe single-stream checkpoint",
        );
    }
    let partial = partial_path(partial_directory, job_id);
    if !partial.exists() && segments[0].durable_offset == 0 {
        fs::write(&partial, [])?;
    }
    let reconciliation = reconcile_partial(&partial, segments[0].durable_offset)
        .map_err(|error| io::Error::other(error.to_string()))?;
    let Some(offset) = reconciliation.durable_offset else {
        return fail_recovered_job(
            database,
            record,
            "restart found the private partial missing",
        );
    };
    if record.metadata.state == DownloadState::Downloading {
        database
            .transition_job(
                job_id,
                DownloadState::Pausing,
                now(),
                Some("backend restart recovered a safe checkpoint".to_owned()),
            )
            .map_err(storage_io)?;
    }
    database
        .checkpoint_single_stream_pause(job_id, offset, now())
        .map_err(storage_io)
}

fn fail_recovered_job(database: &mut Database, record: &JobRecord, reason: &str) -> io::Result<()> {
    let mut metadata = record.metadata.clone();
    metadata.error = Some(bounded_failure_reason(reason));
    metadata.updated_at = now();
    database.update_job(&metadata).map_err(storage_io)?;
    database
        .transition_job(
            metadata.id,
            DownloadState::Failed,
            now(),
            Some(reason.to_owned()),
        )
        .map_err(storage_io)?;
    Ok(())
}

fn fail_job(database: &Arc<Mutex<Database>>, job_id: JobId, reason: &str) {
    let _ = with_database(database, |database| {
        let Some(record) = database.get_job(job_id)? else {
            return Ok(());
        };
        if record.metadata.state == DownloadState::Cancelled {
            return Ok(());
        }
        let mut metadata = record.metadata;
        metadata.error = Some(bounded_failure_reason(reason));
        metadata.updated_at = now();
        database.update_job(&metadata)?;
        if is_legal_transition(metadata.state, DownloadState::Failed) {
            database.transition_job(
                job_id,
                DownloadState::Failed,
                now(),
                Some(bounded_failure_reason(reason)),
            )?;
        }
        Ok(())
    });
}

fn pause_after_queue_failure(database: &Arc<Mutex<Database>>, job_id: JobId, reason: &str) {
    let _ = with_database(database, |database| {
        let mut metadata = database
            .get_job(job_id)?
            .ok_or_else(|| missing_storage_job(job_id))?
            .metadata;
        metadata.error = Some(bounded_failure_reason(reason));
        metadata.updated_at = now();
        database.update_job(&metadata)?;
        database.transition_job(
            job_id,
            DownloadState::Paused,
            now(),
            Some(reason.to_owned()),
        )?;
        Ok(())
    });
}

fn single_segment(job_id: JobId, identity: &RemoteIdentity, at: UnixMillis) -> Option<Segment> {
    let end = identity.content_length?.checked_sub(1)?;
    Some(Segment {
        id: SegmentId::new(),
        job_id,
        start: 0,
        end,
        durable_offset: 0,
        worker_state: "queued".to_owned(),
        checksum: None,
        checkpoint_at: at,
    })
}

fn load_job(database: &Arc<Mutex<Database>>, job_id: JobId) -> Result<JobRecord, BackendError> {
    with_database(database, |database| database.get_job(job_id))
        .map_err(BackendError::Transport)?
        .ok_or_else(|| BackendError::NotFound(format!("job {job_id} was not found")))
}

fn load_job_internal(database: &Arc<Mutex<Database>>, job_id: JobId) -> Result<JobRecord, String> {
    with_database(database, |database| database.get_job(job_id))?
        .ok_or_else(|| format!("job {job_id} was not found"))
}

fn claim_queued_job(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    shutting_down: &AtomicBool,
) -> Result<Option<JobRecord>, String> {
    with_database(database, |database| {
        let record = database
            .get_job(job_id)?
            .ok_or_else(|| missing_storage_job(job_id))?;
        if record.metadata.state != DownloadState::Queued {
            return Ok(None);
        }
        if shutting_down.load(Ordering::Acquire) {
            database.transition_job(
                job_id,
                DownloadState::Paused,
                now(),
                Some("backend shutdown preserved queued work".to_owned()),
            )?;
            return Ok(None);
        }
        database.transition_job(job_id, DownloadState::Connecting, now(), None)?;
        Ok(Some(record))
    })
}

fn current_state(database: &Arc<Mutex<Database>>, job_id: JobId) -> Result<DownloadState, String> {
    Ok(load_job_internal(database, job_id)?.metadata.state)
}

fn transition(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    next: DownloadState,
    reason: Option<String>,
) -> Result<(), BackendError> {
    transition_internal(database, job_id, next, reason).map_err(BackendError::Transport)
}

fn transition_internal(
    database: &Arc<Mutex<Database>>,
    job_id: JobId,
    next: DownloadState,
    reason: Option<String>,
) -> Result<(), String> {
    with_database(database, |database| {
        database.transition_job(job_id, next, now(), reason)
    })?;
    Ok(())
}

fn job_view(metadata: &JobMetadata) -> JobView {
    JobView {
        id: metadata.id,
        state: metadata.state,
        filename: metadata.filename.clone(),
        downloaded_bytes: metadata.downloaded_bytes,
        total_bytes: metadata.total_bytes,
        failure_reason: metadata.error.clone(),
    }
}

fn apply_live_progress(job: &mut JobView, progress: Option<&TransferProgress>) {
    if let Some(progress) = progress {
        job.downloaded_bytes = progress.downloaded_bytes;
        job.total_bytes = progress.total_bytes.or(job.total_bytes);
    }
}

fn partial_path(directory: &Path, job_id: JobId) -> PathBuf {
    directory.join(format!("{job_id}.part"))
}

fn current_user_browser_download_directory(root: &Path) -> Result<PathBuf, String> {
    let preferences_path = root.join(PREFERENCES_FILE);
    match fs::read(&preferences_path) {
        Ok(bytes) => {
            if bytes.len() > 16_384 {
                return Err("desktop preferences exceed the browser policy limit".to_owned());
            }
            let preferences: BrowserPreferences = serde_json::from_slice(&bytes)
                .map_err(|error| format!("desktop preferences are invalid: {error}"))?;
            if let Some(directory) = preferences.last_download_directory {
                if directory.is_absolute() {
                    return Ok(directory);
                }
                return Err("saved browser download destination is not absolute".to_owned());
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(format!("desktop preferences could not be read: {error}"));
        }
    }

    let user_profile = env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .ok_or_else(|| {
            "USERPROFILE is unavailable for the browser download destination".to_owned()
        })?;
    Ok(user_profile.join("Downloads"))
}

fn validate_output_directory(directory: &Path) -> Result<(), BackendError> {
    let metadata = fs::metadata(directory).map_err(|error| {
        rejected(
            "destination_unavailable",
            format!(
                "destination {} is unavailable: {error}",
                directory.display()
            ),
        )
    })?;
    if metadata.is_dir() {
        Ok(())
    } else {
        Err(rejected(
            "destination_unavailable",
            format!("destination {} is not a directory", directory.display()),
        ))
    }
}

fn probe_for_desktop(source_url: &HttpUrl, destination: &Path) -> UiResponse {
    if let Err(error) = verify_writable_directory(destination) {
        return ui_rejected(
            "destination_unavailable",
            format!(
                "destination {} is not writable: {error}",
                destination.display()
            ),
        );
    }
    match HttpProbe::new().and_then(|client| client.probe(source_url)) {
        Ok(probe)
            if (200..300).contains(&probe.status) && probe.authentication_challenge.is_none() =>
        {
            let filename = select_filename(
                probe.content_disposition.as_deref(),
                &probe.final_url,
                probe.identity.mime_type.as_deref(),
            );
            UiResponse::UrlProbe {
                destination: safe_destination(destination, &filename),
                final_url: probe.final_url,
                filename,
                total_bytes: probe.identity.content_length,
                mime_type: probe.identity.mime_type,
            }
        }
        Ok(probe) => ui_rejected(
            "probe_failed",
            format!("HTTP probe returned status {}", probe.status),
        ),
        Err(error) => ui_rejected("probe_failed", error.to_string()),
    }
}

fn verify_writable_directory(destination: &Path) -> io::Result<()> {
    if !destination.metadata()?.is_dir() {
        return Err(io::Error::new(
            io::ErrorKind::NotADirectory,
            "path is not a directory",
        ));
    }
    let marker = destination.join(format!(".download-manager-write-test-{}", JobId::new()));
    let file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&marker)?;
    drop(file);
    fs::remove_file(marker)
}

fn ui_rejected(code: &str, message: impl Into<String>) -> UiResponse {
    UiResponse::Rejected {
        code: code.to_owned(),
        message: message.into(),
    }
}

fn backend_error_response(error: BackendError) -> UiResponse {
    match error {
        BackendError::NotFound(message) => ui_rejected("job_not_found", message),
        BackendError::Rejected { code, message } => ui_rejected(&code, message),
        BackendError::Transport(message) => ui_rejected("backend_transport", message),
    }
}

fn send_work(sender: Option<&Sender<DownloadWork>>, work: DownloadWork) -> Result<(), String> {
    sender
        .ok_or_else(|| "download worker is unavailable".to_owned())?
        .send(work)
        .map_err(|error| format!("download worker is unavailable: {error}"))
}

fn with_database<T>(
    database: &Arc<Mutex<Database>>,
    operation: impl FnOnce(&mut Database) -> Result<T, StorageError>,
) -> Result<T, String> {
    operation(&mut lock(database)).map_err(|error| error.to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn now() -> UnixMillis {
    let milliseconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    UnixMillis::new(i64::try_from(milliseconds).unwrap_or(i64::MAX))
        .expect("system time is non-negative")
}

fn bounded_failure_reason(reason: &str) -> String {
    reason.chars().take(MAX_FAILURE_REASON_CHARS).collect()
}

fn missing_storage_job(job_id: JobId) -> StorageError {
    StorageError::Corrupt(format!("job {job_id} does not exist"))
}

fn storage_io(error: StorageError) -> io::Error {
    io::Error::other(error)
}

fn rejected(code: &str, message: impl Into<String>) -> BackendError {
    BackendError::Rejected {
        code: code.to_owned(),
        message: message.into(),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io::{Read, Write},
        net::{SocketAddr, TcpListener, TcpStream},
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        thread,
        time::{Duration, Instant},
    };

    use download_manager_engine::{
        domain::{HttpUrl, JobId, JobMetadata, RequestId},
        ipc::{
            DestructiveAction, DuplicateAction, Envelope, UI_PROTOCOL_VERSION, UiMessage,
            UiRequest, UiResponse,
        },
        state::DownloadState,
    };

    use crate::{BrowserDownloadRequest, CliBackend};

    use super::DownloadBackend;

    struct HttpFixture {
        address: SocketAddr,
        requests: Arc<AtomicUsize>,
        stop: Arc<AtomicBool>,
        worker: Option<thread::JoinHandle<()>>,
    }

    impl HttpFixture {
        fn start(body: Vec<u8>) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture");
            listener.set_nonblocking(true).expect("nonblocking fixture");
            let address = listener.local_addr().expect("fixture address");
            let requests = Arc::new(AtomicUsize::new(0));
            let request_count = Arc::clone(&requests);
            let stop = Arc::new(AtomicBool::new(false));
            let should_stop = Arc::clone(&stop);
            let worker = thread::spawn(move || {
                while !should_stop.load(Ordering::Acquire) {
                    match listener.accept() {
                        Ok((mut stream, _)) => {
                            stream
                                .set_nonblocking(false)
                                .expect("make fixture connection blocking");
                            if let Some(request) = read_request(&mut stream) {
                                request_count.fetch_add(1, Ordering::Relaxed);
                                write_fixture_response(&mut stream, &request, &body);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(1));
                        }
                        Err(error) => panic!("fixture accept failed: {error}"),
                    }
                }
            });
            Self {
                address,
                requests,
                stop,
                worker: Some(worker),
            }
        }

        fn url(&self) -> String {
            format!("http://{}/fixture", self.address)
        }
    }

    impl Drop for HttpFixture {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            let _ = TcpStream::connect(self.address);
            if let Some(worker) = self.worker.take() {
                worker.join().expect("fixture worker");
            }
        }
    }

    fn read_request(stream: &mut TcpStream) -> Option<String> {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .expect("fixture timeout");
        let mut bytes = Vec::new();
        let mut buffer = [0_u8; 512];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let count = stream.read(&mut buffer).ok()?;
            if count == 0 {
                return None;
            }
            bytes.extend_from_slice(&buffer[..count]);
            if bytes.len() > 64 * 1024 {
                return None;
            }
        }
        String::from_utf8(bytes).ok()
    }

    fn write_fixture_response(stream: &mut TcpStream, request: &str, body: &[u8]) {
        let is_head = request.starts_with("HEAD ");
        let range_start = request.lines().find_map(|line| {
            line.strip_prefix("Range: bytes=")
                .or_else(|| line.strip_prefix("range: bytes="))
                .and_then(|value| value.split('-').next())
                .and_then(|value| value.parse::<usize>().ok())
        });
        let (status, selected, content_range) = range_start.map_or_else(
            || ("200 OK", body, None),
            |start| {
                let selected = body.get(start..).unwrap_or_default();
                (
                    "206 Partial Content",
                    selected,
                    Some(format!(
                        "Content-Range: bytes {start}-{}/{}\r\n",
                        body.len().saturating_sub(1),
                        body.len()
                    )),
                )
            },
        );
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/octet-stream\r\nContent-Disposition: attachment; filename=fixture.bin\r\nAccept-Ranges: bytes\r\nETag: \"fixture-v1\"\r\n{}Connection: close\r\n\r\n",
            selected.len(),
            content_range.unwrap_or_default()
        );
        if stream.write_all(headers.as_bytes()).is_ok() && !is_head {
            for chunk in selected.chunks(8 * 1024) {
                if stream.write_all(chunk).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
            }
        }
    }

    fn temp_directory() -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("download-manager-pu06-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&path).expect("test directory");
        path
    }

    fn desktop(backend: &mut DownloadBackend, payload: UiRequest) -> UiResponse {
        let response = backend
            .desktop_request(Envelope::new(
                UI_PROTOCOL_VERSION,
                RequestId::new(),
                payload,
            ))
            .expect("desktop request");
        match response.payload {
            UiMessage::Response { response } => response,
            UiMessage::Event { .. } => panic!("expected desktop response"),
        }
    }

    fn assert_record_only_removal_keeps_file(
        backend: &mut DownloadBackend,
        job_id: JobId,
        completed_path: &std::path::Path,
        expected: &[u8],
    ) {
        assert!(matches!(
            desktop(
                backend,
                UiRequest::DestructiveJob {
                    job_id,
                    action: DestructiveAction::RemoveRecord,
                    confirmed_path: None,
                },
            ),
            UiResponse::JobRemoved {
                job_id: removed_id,
                file_deleted: false,
            } if removed_id == job_id
        ));
        assert!(matches!(
            desktop(backend, UiRequest::ListJobs),
            UiResponse::Jobs { jobs } if jobs.is_empty()
        ));
        assert_eq!(
            std::fs::read(completed_path).expect("record-only removal keeps completed file"),
            expected
        );
    }

    fn wait_for_desktop_completion(backend: &mut DownloadBackend, job_id: JobId) -> JobMetadata {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let UiResponse::Jobs { jobs } = desktop(backend, UiRequest::ListJobs) else {
                panic!("expected job list");
            };
            let job = jobs
                .into_iter()
                .find(|job| job.id == job_id)
                .expect("created job");
            if job.state == DownloadState::Completed {
                return job;
            }
            assert!(Instant::now() < deadline, "download did not complete");
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn browser_request_id_creates_one_real_durable_job_across_retries() {
        let body = b"PU10 browser handoff fixture".repeat(1_024);
        let server = HttpFixture::start(body.clone());
        let root = temp_directory();
        let destination = root.join("downloads");
        std::fs::create_dir(&destination).expect("download directory");
        let mut backend = DownloadBackend::new(root.join("partials")).expect("backend");
        backend.set_browser_download_directory(destination.clone());
        let request_id = RequestId::new();
        let request = BrowserDownloadRequest {
            request_id,
            source_url: HttpUrl::parse(server.url()).expect("fixture URL"),
            referrer: Some(HttpUrl::parse("https://example.test/page").expect("referrer")),
            suggested_filename: Some("captured.bin".to_owned()),
        };

        let first = backend
            .add_browser(request.clone())
            .expect("first browser request");
        let duplicate = backend
            .add_browser(request.clone())
            .expect("idempotent retry");
        assert_eq!(first.id, JobId::from_uuid(request_id.as_uuid()));
        assert_eq!(duplicate.id, first.id);
        assert_eq!(backend.list().expect("durable jobs").len(), 1);

        let completed = wait_for_desktop_completion(&mut backend, first.id);
        assert_eq!(
            completed.source_browser.as_deref(),
            Some("browser_extension")
        );
        assert_eq!(completed.destination, destination);
        assert_eq!(
            std::fs::read(destination.join("captured.bin")).expect("completed browser file"),
            body
        );
        assert!(server.requests.load(Ordering::Relaxed) >= 2);
        drop(backend);

        let mut reopened = DownloadBackend::new(root.join("partials")).expect("reopen backend");
        reopened.set_browser_download_directory(destination.clone());
        let after_restart = reopened
            .add_browser(request)
            .expect("idempotent retry after backend restart");
        assert_eq!(after_restart.id, first.id);
        assert_eq!(reopened.list().expect("reopened durable jobs").len(), 1);
        drop(reopened);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn browser_destination_rejection_creates_no_job() {
        let server = HttpFixture::start(b"unused".to_vec());
        let root = temp_directory();
        let mut backend = DownloadBackend::new(root.join("partials")).expect("backend");
        backend.set_browser_download_directory(root.join("missing"));

        let error = backend
            .add_browser(BrowserDownloadRequest {
                request_id: RequestId::new(),
                source_url: HttpUrl::parse(server.url()).expect("fixture URL"),
                referrer: None,
                suggested_filename: None,
            })
            .expect_err("missing destination must be rejected");
        assert!(matches!(
            error,
            crate::BackendError::Rejected { code, .. } if code == "destination_unavailable"
        ));
        assert_eq!(backend.list().expect("durable jobs"), Vec::new());
        assert_eq!(server.requests.load(Ordering::Relaxed), 0);
        drop(backend);
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn desktop_probe_add_and_list_use_the_real_download_worker() {
        let body = vec![0x5a; 128 * 1024];
        let server = HttpFixture::start(body.clone());
        let root = temp_directory();
        let destination = root.join("downloads");
        std::fs::create_dir(&destination).expect("download destination");
        let mut backend = DownloadBackend::new(root.join("partials")).expect("backend");
        let source_url = HttpUrl::parse(server.url()).expect("fixture URL");

        let missing = desktop(
            &mut backend,
            UiRequest::ProbeUrl {
                source_url: source_url.clone(),
                destination: root.join("missing"),
            },
        );
        assert!(matches!(
            missing,
            UiResponse::Rejected { code, .. } if code == "destination_unavailable"
        ));
        assert_eq!(server.requests.load(Ordering::Relaxed), 0);

        let invalid_add = desktop(
            &mut backend,
            UiRequest::AddJob {
                source_url: source_url.clone(),
                final_url: source_url.clone(),
                filename: "fixture.bin".to_owned(),
                destination: root.join("missing"),
                total_bytes: Some(body.len() as u64),
                start_now: true,
                duplicate_action: DuplicateAction::AddDuplicate,
            },
        );
        assert!(matches!(
            invalid_add,
            UiResponse::Rejected { code, .. } if code == "destination_unavailable"
        ));
        assert_eq!(server.requests.load(Ordering::Relaxed), 0);

        let preview = desktop(
            &mut backend,
            UiRequest::ProbeUrl {
                source_url: source_url.clone(),
                destination: destination.clone(),
            },
        );
        let (final_url, filename, total_bytes) = match preview {
            UiResponse::UrlProbe {
                final_url,
                filename,
                destination: preview_destination,
                total_bytes,
                ..
            } => {
                assert_eq!(preview_destination, destination.join("fixture.bin"));
                (final_url, filename, total_bytes)
            }
            other => panic!("expected URL preview, got {other:?}"),
        };
        assert_eq!(filename, "fixture.bin");
        assert_eq!(total_bytes, Some(body.len() as u64));

        let accepted = desktop(
            &mut backend,
            UiRequest::AddJob {
                source_url,
                final_url,
                filename: filename.clone(),
                destination: destination.clone(),
                total_bytes,
                start_now: true,
                duplicate_action: DuplicateAction::AddDuplicate,
            },
        );
        let job_id = match accepted {
            UiResponse::Accepted { job_id } => job_id,
            other => panic!("expected accepted job, got {other:?}"),
        };

        let completed = wait_for_desktop_completion(&mut backend, job_id);

        assert_eq!(completed.filename, filename);
        assert_eq!(completed.destination, destination);
        assert_eq!(completed.downloaded_bytes, body.len() as u64);
        assert_eq!(
            std::fs::read(destination.join("fixture.bin")).expect("completed file"),
            body
        );
        assert!(server.requests.load(Ordering::Relaxed) >= 3);

        assert_record_only_removal_keeps_file(
            &mut backend,
            job_id,
            &destination.join("fixture.bin"),
            &body,
        );

        drop(backend);
        std::fs::remove_dir_all(root).expect("cleanup");
    }
}
