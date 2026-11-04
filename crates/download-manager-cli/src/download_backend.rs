//! Minimal real-download backend for the private-use PU03 path.

use std::{
    env, fs, io,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        mpsc::{self, Sender},
    },
    thread,
};

use download_manager_engine::{
    domain::{HttpUrl, JobId},
    filename::{safe_destination, sanitize_windows_filename, select_filename},
    finalize::{ConflictPolicy, finalize_cross_volume},
    ipc::JobAction,
    probe::HttpProbe,
    state::DownloadState,
    transfer::{SingleStreamDownloader, TransferProgress},
};

use crate::{AddRequest, BackendError, CliBackend, JobView, QueueStart};

const MAX_FAILURE_REASON_CHARS: usize = 4_096;

/// In-memory PU03 authority backed by one FIFO HTTP(S) transfer worker.
///
/// Durable job recovery and download controls deliberately remain PU04 work. Completed files are
/// still published safely: each transfer first writes a private partial and only then finalizes it
/// into the requested output directory.
pub struct DownloadBackend {
    jobs: Arc<Mutex<Vec<JobView>>>,
    work: Sender<DownloadWork>,
    partial_directory: PathBuf,
}

impl DownloadBackend {
    /// Creates the current-user backend with private partials below `%LOCALAPPDATA%`.
    ///
    /// # Errors
    /// Returns an I/O error when `LOCALAPPDATA` is unavailable, the partial directory cannot be
    /// created, or the worker thread cannot be started.
    pub fn for_current_user() -> Result<Self, io::Error> {
        let local_app_data = env::var_os("LOCALAPPDATA").ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::NotFound,
                "LOCALAPPDATA is required for private download state",
            )
        })?;
        Self::new(
            PathBuf::from(local_app_data)
                .join("DownloadManager")
                .join("State")
                .join("partials"),
        )
    }

    /// Creates a backend using an explicit private-partial directory.
    ///
    /// This constructor is also the isolation boundary used by local fixture tests.
    ///
    /// # Errors
    /// Returns an I/O error when the directory cannot be created or the worker cannot be started.
    pub fn new(partial_directory: impl Into<PathBuf>) -> Result<Self, io::Error> {
        let partial_directory = partial_directory.into();
        fs::create_dir_all(&partial_directory)?;

        let jobs = Arc::new(Mutex::new(Vec::new()));
        let worker_jobs = Arc::clone(&jobs);
        let (work, receiver) = mpsc::channel::<DownloadWork>();
        thread::Builder::new()
            .name("download-manager-pu03-worker".to_owned())
            .spawn(move || {
                while let Ok(download) = receiver.recv() {
                    if let Err(reason) = run_download(&worker_jobs, &download) {
                        fail_job(&worker_jobs, download.job_id, &reason);
                    }
                }
            })?;

        Ok(Self {
            jobs,
            work,
            partial_directory,
        })
    }
}

impl CliBackend for DownloadBackend {
    fn add(&mut self, request: AddRequest) -> Result<JobView, BackendError> {
        if !request.start_now {
            return Err(rejected(
                "unsupported_in_pu03",
                "adding a download for later is not available until PU04",
            ));
        }
        if request.queue.is_some() {
            return Err(rejected(
                "unsupported_in_pu03",
                "download queues are outside the private PU03 path",
            ));
        }
        validate_output_directory(&request.output_directory)?;

        let provisional_filename = request.filename.as_deref().map_or_else(
            || sanitize_windows_filename(&select_filename(None, &request.source_url, None)),
            sanitize_windows_filename,
        );
        let job_id = JobId::new();
        let job = JobView {
            id: job_id,
            state: DownloadState::Probing,
            filename: provisional_filename,
            downloaded_bytes: 0,
            total_bytes: None,
            failure_reason: None,
        };
        let download = DownloadWork {
            job_id,
            source_url: request.source_url,
            output_directory: request.output_directory,
            requested_filename: request.filename,
            partial_path: self.partial_directory.join(format!("{job_id}.part")),
        };

        with_jobs(&self.jobs, |jobs| jobs.push(job.clone()));
        if let Err(error) = self.work.send(download) {
            let reason = format!("download worker is unavailable: {error}");
            fail_job(&self.jobs, job_id, &reason);
            return Err(BackendError::Transport(reason));
        }
        Ok(job)
    }

    fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
        Ok(with_jobs(&self.jobs, |jobs| jobs.clone()))
    }

    fn status(&mut self, job_id: JobId) -> Result<JobView, BackendError> {
        with_jobs(&self.jobs, |jobs| {
            jobs.iter().find(|job| job.id == job_id).cloned()
        })
        .ok_or_else(|| BackendError::NotFound(format!("job {job_id} was not found")))
    }

    fn control(&mut self, _job_id: JobId, _action: JobAction) -> Result<JobView, BackendError> {
        Err(rejected(
            "unsupported_in_pu03",
            "pause, resume, and cancel are not available until PU04",
        ))
    }

    fn start_queue(&mut self, _name: &str) -> Result<QueueStart, BackendError> {
        Err(rejected(
            "unsupported_in_pu03",
            "download queues are outside the private-use milestone",
        ))
    }
}

struct DownloadWork {
    job_id: JobId,
    source_url: HttpUrl,
    output_directory: PathBuf,
    requested_filename: Option<String>,
    partial_path: PathBuf,
}

fn run_download(jobs: &Arc<Mutex<Vec<JobView>>>, work: &DownloadWork) -> Result<(), String> {
    let probe = HttpProbe::new()
        .and_then(|client| client.probe(&work.source_url))
        .map_err(|error| error.to_string())?;
    if !(200..300).contains(&probe.status) || probe.authentication_challenge.is_some() {
        return Err(format!("HTTP probe returned status {}", probe.status));
    }
    let selected_filename = work.requested_filename.clone().unwrap_or_else(|| {
        select_filename(
            probe.content_disposition.as_deref(),
            &probe.final_url,
            probe.identity.mime_type.as_deref(),
        )
    });
    let filename = sanitize_windows_filename(&selected_filename);
    let final_path = safe_destination(&work.output_directory, &filename);
    update_job(jobs, work.job_id, |job| {
        job.state = DownloadState::Queued;
        job.filename = filename;
        job.total_bytes = probe.identity.content_length;
    });
    set_state(jobs, work.job_id, DownloadState::Connecting);
    let downloader = SingleStreamDownloader::new().map_err(|error| error.to_string())?;
    set_state(jobs, work.job_id, DownloadState::Downloading);
    let outcome = downloader
        .download(&probe.final_url, &work.partial_path, |progress| {
            set_progress(jobs, work.job_id, progress);
        })
        .map_err(|error| error.to_string())?;

    update_job(jobs, work.job_id, |job| {
        job.state = DownloadState::Verifying;
        job.downloaded_bytes = outcome.downloaded_bytes;
        job.total_bytes = outcome.total_bytes;
    });
    set_state(jobs, work.job_id, DownloadState::Assembling);
    finalize_cross_volume(&work.partial_path, &final_path, ConflictPolicy::Ask)
        .map_err(|error| error.to_string())?;
    update_job(jobs, work.job_id, |job| {
        job.state = DownloadState::Completed;
        job.failure_reason = None;
    });
    Ok(())
}

fn validate_output_directory(directory: &std::path::Path) -> Result<(), BackendError> {
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

fn set_state(jobs: &Arc<Mutex<Vec<JobView>>>, job_id: JobId, state: DownloadState) {
    update_job(jobs, job_id, |job| job.state = state);
}

fn set_progress(jobs: &Arc<Mutex<Vec<JobView>>>, job_id: JobId, progress: TransferProgress) {
    update_job(jobs, job_id, |job| {
        job.downloaded_bytes = progress.downloaded_bytes;
        job.total_bytes = progress.total_bytes;
    });
}

fn fail_job(jobs: &Arc<Mutex<Vec<JobView>>>, job_id: JobId, reason: &str) {
    update_job(jobs, job_id, |job| {
        job.state = DownloadState::Failed;
        job.failure_reason = Some(bounded_failure_reason(reason));
    });
}

fn update_job(jobs: &Arc<Mutex<Vec<JobView>>>, job_id: JobId, update: impl FnOnce(&mut JobView)) {
    with_jobs(jobs, |jobs| {
        if let Some(job) = jobs.iter_mut().find(|job| job.id == job_id) {
            update(job);
        }
    });
}

fn with_jobs<T>(
    jobs: &Arc<Mutex<Vec<JobView>>>,
    operation: impl FnOnce(&mut Vec<JobView>) -> T,
) -> T {
    let mut guard = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    operation(&mut guard)
}

fn bounded_failure_reason(reason: &str) -> String {
    reason.chars().take(MAX_FAILURE_REASON_CHARS).collect()
}

fn rejected(code: &str, message: impl Into<String>) -> BackendError {
    BackendError::Rejected {
        code: code.to_owned(),
        message: message.into(),
    }
}
