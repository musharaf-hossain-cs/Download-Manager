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
        DownloadKind, JobId, JobMetadata, Priority, RangeCapability, RemoteIdentity, Segment,
        SegmentId, UnixMillis,
    },
    filename::{safe_destination, sanitize_windows_filename, select_filename},
    finalize::{ConflictPolicy, finalize_cross_volume},
    ipc::JobAction,
    probe::HttpProbe,
    recovery::{ReconciliationAction, reconcile_partial},
    resume::resume_single_stream_with_control,
    state::{DownloadState, is_legal_transition},
    storage::{Database, JobRecord, StorageError},
    transfer::{
        SingleStreamDownloader, TransferControl, TransferOutcome, TransferProgress, TransferStatus,
    },
};

use crate::{AddRequest, BackendError, CliBackend, JobView, QueueStart};

const MAX_FAILURE_REASON_CHARS: usize = 4_096;
const DATABASE_FILE: &str = "downloads.sqlite3";

/// SQLite-backed current-user authority with one FIFO HTTP(S) transfer worker.
pub struct DownloadBackend {
    database: Arc<Mutex<Database>>,
    work: Option<Sender<DownloadWork>>,
    worker: Option<JoinHandle<()>>,
    active: Arc<Mutex<HashMap<JobId, TransferControl>>>,
    progress: Arc<Mutex<HashMap<JobId, TransferProgress>>>,
    shutting_down: Arc<AtomicBool>,
    partial_directory: PathBuf,
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
        let state = PathBuf::from(local_app_data)
            .join("DownloadManager")
            .join("State");
        Self::new_with_database(state.join(DATABASE_FILE), state.join("partials"))
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
        })
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
        let job_id = JobId::new();
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
            source_browser: None,
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
            return Err(BackendError::Transport(error));
        }
        Ok(job)
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
