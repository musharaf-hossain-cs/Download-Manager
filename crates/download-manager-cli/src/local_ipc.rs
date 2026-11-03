//! Current-Windows-user authenticated loopback transport for CLI automation.

use std::{
    fs,
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    time::Duration,
};

use download_manager_engine::{
    credentials::{CredentialError, CredentialRef, WindowsCredentialStore},
    domain::{HttpUrl, JobId},
    ipc::JobAction,
};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

use crate::{AddRequest, BackendError, CliBackend, JobView, QueueStart};

const IPC_PROTOCOL_VERSION: u16 = 1;
const MAX_IPC_MESSAGE_BYTES: usize = 1_048_576;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const IO_TIMEOUT: Duration = Duration::from_secs(10);
const MANIFEST_FILE: &str = "cli-ipc-v1.json";
const CREDENTIAL_NAMESPACE: &str = "DownloadManager/CLI-IPC";
const CREDENTIAL_HOST: &str = "local-cli-ipc-v1";
const CREDENTIAL_ACCOUNT: &str = "current-windows-user";
const USER_SCOPE: &str = "windows_current_user";

#[derive(Debug, Error)]
pub enum LocalIpcError {
    #[error("current-user IPC is available only on Windows")]
    UnsupportedPlatform,
    #[error("LOCALAPPDATA is unavailable")]
    MissingLocalAppData,
    #[error("IPC filesystem operation failed: {0}")]
    Io(#[from] std::io::Error),
    #[error("IPC credential operation failed: {0}")]
    Credential(#[from] CredentialError),
    #[error("IPC manifest is malformed: {0}")]
    Manifest(#[from] serde_json::Error),
    #[error("IPC manifest violates the current-user loopback policy")]
    UnsafeManifest,
    #[error("IPC message exceeds the {MAX_IPC_MESSAGE_BYTES}-byte limit")]
    MessageTooLarge,
    #[error("IPC peer returned an unexpected response")]
    UnexpectedResponse,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EndpointManifest {
    version: u16,
    scope: String,
    instance_id: String,
    address: SocketAddr,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AuthenticatedRequest {
    version: u16,
    instance_id: String,
    bearer_token: String,
    payload: RequestPayload,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RequestPayload {
    Add {
        source_url: HttpUrl,
        output_directory: PathBuf,
        filename: Option<String>,
        start_now: bool,
        queue: Option<String>,
    },
    List,
    Status {
        job_id: JobId,
    },
    Control {
        job_id: JobId,
        action: JobAction,
    },
    StartQueue {
        name: String,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireResponse {
    version: u16,
    result: Result<ResponsePayload, WireError>,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ResponsePayload {
    Job { job: JobView },
    Jobs { jobs: Vec<JobView> },
    Queue { queue: QueueStart },
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum WireError {
    NotFound { message: String },
    Rejected { code: String, message: String },
    Transport { message: String },
}

impl From<BackendError> for WireError {
    fn from(value: BackendError) -> Self {
        match value {
            BackendError::NotFound(message) => Self::NotFound { message },
            BackendError::Rejected { code, message } => Self::Rejected { code, message },
            BackendError::Transport(message) => Self::Transport { message },
        }
    }
}

impl From<WireError> for BackendError {
    fn from(value: WireError) -> Self {
        match value {
            WireError::NotFound { message } => Self::NotFound(message),
            WireError::Rejected { code, message } => Self::Rejected { code, message },
            WireError::Transport { message } => Self::Transport(message),
        }
    }
}

/// CLI backend that authenticates every bounded request with a random secret held in the
/// current Windows user's Credential Manager vault. The discovery file contains only a loopback
/// address and instance ID, so another Windows account cannot recover the bearer secret even if
/// it can observe the endpoint.
pub struct AuthenticatedIpcBackend {
    manifest: EndpointManifest,
    bearer_token: String,
}

impl AuthenticatedIpcBackend {
    /// Discovers the running engine endpoint and loads its secret from the current user's vault.
    ///
    /// # Errors
    /// Returns an error for missing state, non-loopback/tampered manifests, or inaccessible
    /// current-user credentials.
    pub fn discover_current_user() -> Result<Self, LocalIpcError> {
        #[cfg(not(windows))]
        {
            Err(LocalIpcError::UnsupportedPlatform)
        }
        #[cfg(windows)]
        {
            let directory = current_user_ipc_directory()?;
            let manifest = read_manifest(&directory.join(MANIFEST_FILE))?;
            let (store, reference) = current_user_credential()?;
            let bearer_token = store.load(&reference)?;
            validate_token(&bearer_token)?;
            Ok(Self {
                manifest,
                bearer_token,
            })
        }
    }

    fn exchange(&self, payload: RequestPayload) -> Result<ResponsePayload, BackendError> {
        self.exchange_transport(payload)
            .map_err(|error| BackendError::Transport(error.to_string()))?
            .map_err(BackendError::from)
    }

    fn exchange_transport(
        &self,
        payload: RequestPayload,
    ) -> Result<Result<ResponsePayload, WireError>, LocalIpcError> {
        let mut stream = TcpStream::connect_timeout(&self.manifest.address, CONNECT_TIMEOUT)?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let request = AuthenticatedRequest {
            version: IPC_PROTOCOL_VERSION,
            instance_id: self.manifest.instance_id.clone(),
            bearer_token: self.bearer_token.clone(),
            payload,
        };
        write_message(&mut stream, &request)?;
        let response: WireResponse = read_message(&mut stream)?;
        if response.version != IPC_PROTOCOL_VERSION {
            return Err(LocalIpcError::UnexpectedResponse);
        }
        Ok(response.result)
    }

    #[cfg(test)]
    fn discover_with_token(directory: &Path, bearer_token: String) -> Result<Self, LocalIpcError> {
        validate_token(&bearer_token)?;
        Ok(Self {
            manifest: read_manifest(&directory.join(MANIFEST_FILE))?,
            bearer_token,
        })
    }
}

impl CliBackend for AuthenticatedIpcBackend {
    fn add(&mut self, request: AddRequest) -> Result<JobView, BackendError> {
        match self.exchange(RequestPayload::Add {
            source_url: request.source_url,
            output_directory: request.output_directory,
            filename: request.filename,
            start_now: request.start_now,
            queue: request.queue,
        })? {
            ResponsePayload::Job { job } => Ok(job),
            _ => Err(unexpected_response()),
        }
    }

    fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
        match self.exchange(RequestPayload::List)? {
            ResponsePayload::Jobs { jobs } => Ok(jobs),
            _ => Err(unexpected_response()),
        }
    }

    fn status(&mut self, job_id: JobId) -> Result<JobView, BackendError> {
        match self.exchange(RequestPayload::Status { job_id })? {
            ResponsePayload::Job { job } => Ok(job),
            _ => Err(unexpected_response()),
        }
    }

    fn control(&mut self, job_id: JobId, action: JobAction) -> Result<JobView, BackendError> {
        match self.exchange(RequestPayload::Control { job_id, action })? {
            ResponsePayload::Job { job } => Ok(job),
            _ => Err(unexpected_response()),
        }
    }

    fn start_queue(&mut self, name: &str) -> Result<QueueStart, BackendError> {
        match self.exchange(RequestPayload::StartQueue {
            name: name.to_owned(),
        })? {
            ResponsePayload::Queue { queue } => Ok(queue),
            _ => Err(unexpected_response()),
        }
    }
}

/// Engine-side half of the authenticated CLI transport. Binding rotates the random vault secret
/// and publishes only a safe loopback discovery manifest. Each accepted request is authenticated
/// before any [`CliBackend`] authority is invoked.
pub struct LocalIpcServer {
    listener: TcpListener,
    manifest_path: PathBuf,
    instance_id: String,
    bearer_token: String,
    credential: Option<(WindowsCredentialStore, CredentialRef)>,
}

impl LocalIpcServer {
    /// Binds the current-user endpoint and publishes its vault-backed discovery state.
    ///
    /// # Errors
    /// Returns an error if loopback binding, current-user vault storage, or manifest publication
    /// fails.
    pub fn bind_current_user() -> Result<Self, LocalIpcError> {
        #[cfg(not(windows))]
        {
            Err(LocalIpcError::UnsupportedPlatform)
        }
        #[cfg(windows)]
        {
            let directory = current_user_ipc_directory()?;
            fs::create_dir_all(&directory)?;
            let (store, reference) = current_user_credential()?;
            let bearer_token = random_token();
            store.store(&reference, &bearer_token)?;
            match Self::bind(
                &directory,
                bearer_token,
                Some((store.clone(), reference.clone())),
            ) {
                Ok(server) => Ok(server),
                Err(error) => {
                    let _ = store.delete(&reference);
                    Err(error)
                }
            }
        }
    }

    fn bind(
        directory: &Path,
        bearer_token: String,
        credential: Option<(WindowsCredentialStore, CredentialRef)>,
    ) -> Result<Self, LocalIpcError> {
        validate_token(&bearer_token)?;
        fs::create_dir_all(directory)?;
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
        let instance_id = Uuid::new_v4().to_string();
        let manifest_path = directory.join(MANIFEST_FILE);
        let manifest = EndpointManifest {
            version: IPC_PROTOCOL_VERSION,
            scope: USER_SCOPE.to_owned(),
            instance_id: instance_id.clone(),
            address: listener.local_addr()?,
        };
        publish_manifest(&manifest_path, &manifest)?;
        Ok(Self {
            listener,
            manifest_path,
            instance_id,
            bearer_token,
            credential,
        })
    }

    /// Accepts, authenticates, and dispatches exactly one request.
    ///
    /// # Errors
    /// Returns an error for transport/framing failures. Authentication failure is returned to the
    /// peer without invoking `backend`.
    pub fn serve_once<B: CliBackend + ?Sized>(&self, backend: &mut B) -> Result<(), LocalIpcError> {
        let (mut stream, _) = self.listener.accept()?;
        stream.set_read_timeout(Some(IO_TIMEOUT))?;
        stream.set_write_timeout(Some(IO_TIMEOUT))?;
        let request: AuthenticatedRequest = read_message(&mut stream)?;
        let result = if request.version != IPC_PROTOCOL_VERSION
            || request.instance_id != self.instance_id
            || !constant_time_eq(
                request.bearer_token.as_bytes(),
                self.bearer_token.as_bytes(),
            ) {
            Err(WireError::Transport {
                message: "current-user IPC authentication rejected".to_owned(),
            })
        } else {
            dispatch(request.payload, backend)
        };
        write_message(
            &mut stream,
            &WireResponse {
                version: IPC_PROTOCOL_VERSION,
                result,
            },
        )
    }

    #[cfg(test)]
    fn bind_with_token(directory: &Path, bearer_token: String) -> Result<Self, LocalIpcError> {
        Self::bind(directory, bearer_token, None)
    }
}

impl Drop for LocalIpcServer {
    fn drop(&mut self) {
        let owns_manifest = read_manifest(&self.manifest_path)
            .is_ok_and(|manifest| manifest.instance_id == self.instance_id);
        if owns_manifest {
            let _ = fs::remove_file(&self.manifest_path);
            if let Some((store, reference)) = &self.credential {
                let _ = store.delete(reference);
            }
        }
    }
}

fn dispatch<B: CliBackend + ?Sized>(
    payload: RequestPayload,
    backend: &mut B,
) -> Result<ResponsePayload, WireError> {
    match payload {
        RequestPayload::Add {
            source_url,
            output_directory,
            filename,
            start_now,
            queue,
        } => backend
            .add(AddRequest {
                source_url,
                output_directory,
                filename,
                start_now,
                queue,
            })
            .map(|job| ResponsePayload::Job { job }),
        RequestPayload::List => backend.list().map(|jobs| ResponsePayload::Jobs { jobs }),
        RequestPayload::Status { job_id } => backend
            .status(job_id)
            .map(|job| ResponsePayload::Job { job }),
        RequestPayload::Control { job_id, action } => backend
            .control(job_id, action)
            .map(|job| ResponsePayload::Job { job }),
        RequestPayload::StartQueue { name } => backend
            .start_queue(&name)
            .map(|queue| ResponsePayload::Queue { queue }),
    }
    .map_err(WireError::from)
}

fn current_user_ipc_directory() -> Result<PathBuf, LocalIpcError> {
    std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .map(|path| path.join("DownloadManager").join("ipc"))
        .ok_or(LocalIpcError::MissingLocalAppData)
}

fn current_user_credential() -> Result<(WindowsCredentialStore, CredentialRef), LocalIpcError> {
    let store = WindowsCredentialStore::new(CREDENTIAL_NAMESPACE)?;
    let reference = store.reference(CREDENTIAL_HOST, CREDENTIAL_ACCOUNT)?;
    Ok((store, reference))
}

fn random_token() -> String {
    format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple())
}

fn validate_token(token: &str) -> Result<(), LocalIpcError> {
    if token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err(LocalIpcError::UnsafeManifest)
    }
}

fn read_manifest(path: &Path) -> Result<EndpointManifest, LocalIpcError> {
    let metadata = fs::metadata(path)?;
    if metadata.len() > 16_384 {
        return Err(LocalIpcError::UnsafeManifest);
    }
    let manifest: EndpointManifest = serde_json::from_slice(&fs::read(path)?)?;
    if manifest.version != IPC_PROTOCOL_VERSION
        || manifest.scope != USER_SCOPE
        || manifest.instance_id.parse::<Uuid>().is_err()
        || !manifest.address.ip().is_loopback()
        || manifest.address.port() == 0
    {
        return Err(LocalIpcError::UnsafeManifest);
    }
    Ok(manifest)
}

fn publish_manifest(path: &Path, manifest: &EndpointManifest) -> Result<(), LocalIpcError> {
    let parent = path.parent().ok_or(LocalIpcError::UnsafeManifest)?;
    let temporary = parent.join(format!(".{MANIFEST_FILE}.{}.tmp", manifest.instance_id));
    let bytes = serde_json::to_vec(manifest)?;
    fs::write(&temporary, bytes)?;
    if path.exists() {
        fs::remove_file(path)?;
    }
    fs::rename(temporary, path)?;
    Ok(())
}

fn write_message<T: Serialize>(writer: &mut impl Write, value: &T) -> Result<(), LocalIpcError> {
    let bytes = serde_json::to_vec(value)?;
    if bytes.len() > MAX_IPC_MESSAGE_BYTES {
        return Err(LocalIpcError::MessageTooLarge);
    }
    writer.write_all(&bytes)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

fn read_message<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> Result<T, LocalIpcError> {
    let mut bytes = Vec::new();
    BufReader::new(reader)
        .take((MAX_IPC_MESSAGE_BYTES + 2) as u64)
        .read_until(b'\n', &mut bytes)?;
    if bytes.len() > MAX_IPC_MESSAGE_BYTES + 1 || bytes.last() != Some(&b'\n') {
        return Err(LocalIpcError::MessageTooLarge);
    }
    bytes.pop();
    Ok(serde_json::from_slice(&bytes)?)
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    left.iter()
        .zip(right)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

fn unexpected_response() -> BackendError {
    BackendError::Transport(LocalIpcError::UnexpectedResponse.to_string())
}

#[cfg(test)]
mod tests {
    use std::{fs, net::SocketAddr, thread};

    use download_manager_engine::{domain::JobId, state::DownloadState};

    use super::{
        AuthenticatedIpcBackend, EndpointManifest, IPC_PROTOCOL_VERSION, LocalIpcError,
        LocalIpcServer, MANIFEST_FILE, USER_SCOPE,
    };
    use crate::{AddRequest, BackendError, CliBackend, JobView, QueueStart};

    struct FixtureBackend {
        jobs: Vec<JobView>,
        calls: usize,
    }

    impl CliBackend for FixtureBackend {
        fn add(&mut self, _request: AddRequest) -> Result<JobView, BackendError> {
            self.calls += 1;
            Ok(self.jobs[0].clone())
        }

        fn list(&mut self) -> Result<Vec<JobView>, BackendError> {
            self.calls += 1;
            Ok(self.jobs.clone())
        }

        fn status(&mut self, _job_id: JobId) -> Result<JobView, BackendError> {
            self.calls += 1;
            Ok(self.jobs[0].clone())
        }

        fn control(
            &mut self,
            _job_id: JobId,
            _action: download_manager_engine::ipc::JobAction,
        ) -> Result<JobView, BackendError> {
            self.calls += 1;
            Ok(self.jobs[0].clone())
        }

        fn start_queue(&mut self, name: &str) -> Result<QueueStart, BackendError> {
            self.calls += 1;
            Ok(QueueStart {
                name: name.to_owned(),
                started_jobs: 1,
            })
        }
    }

    fn directory() -> std::path::PathBuf {
        let path =
            std::env::temp_dir().join(format!("download-manager-ipc-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&path).expect("test directory");
        path
    }

    fn job() -> JobView {
        JobView {
            id: JobId::new(),
            state: DownloadState::Downloading,
            filename: "archive.zip".to_owned(),
            downloaded_bytes: 512,
            total_bytes: Some(1_024),
        }
    }

    #[test]
    fn authenticated_loopback_round_trip_dispatches_to_engine_authority() {
        let directory = directory();
        let token = "a".repeat(64);
        let server = LocalIpcServer::bind_with_token(&directory, token.clone()).expect("bind");
        let mut client =
            AuthenticatedIpcBackend::discover_with_token(&directory, token).expect("discover");
        let expected = job();
        let server_expected = expected.clone();
        let server_thread = thread::spawn(move || {
            let mut backend = FixtureBackend {
                jobs: vec![server_expected],
                calls: 0,
            };
            server.serve_once(&mut backend).expect("serve request");
            backend.calls
        });

        assert_eq!(client.list().expect("list jobs"), vec![expected]);
        assert_eq!(server_thread.join().expect("server thread"), 1);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn wrong_user_secret_is_rejected_before_backend_dispatch() {
        let directory = directory();
        let server = LocalIpcServer::bind_with_token(&directory, "b".repeat(64)).expect("bind");
        let mut client = AuthenticatedIpcBackend::discover_with_token(&directory, "c".repeat(64))
            .expect("discover");
        let server_thread = thread::spawn(move || {
            let mut backend = FixtureBackend {
                jobs: vec![job()],
                calls: 0,
            };
            server.serve_once(&mut backend).expect("serve request");
            backend.calls
        });

        assert!(
            matches!(client.list(), Err(BackendError::Transport(message)) if message.contains("authentication rejected"))
        );
        assert_eq!(server_thread.join().expect("server thread"), 0);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn discovery_rejects_non_loopback_unknown_scope_and_unknown_fields() {
        let directory = directory();
        let path = directory.join(MANIFEST_FILE);
        let write_manifest = |scope: &str, address: &str, extra: &str| {
            fs::write(
                &path,
                format!(
                    r#"{{"version":{IPC_PROTOCOL_VERSION},"scope":"{scope}","instance_id":"{}","address":"{address}"{extra}}}"#,
                    uuid::Uuid::new_v4()
                ),
            )
            .expect("write manifest");
        };

        write_manifest(USER_SCOPE, "192.0.2.10:1234", "");
        assert!(matches!(
            AuthenticatedIpcBackend::discover_with_token(&directory, "d".repeat(64)),
            Err(LocalIpcError::UnsafeManifest)
        ));
        write_manifest("machine", "127.0.0.1:1234", "");
        assert!(matches!(
            AuthenticatedIpcBackend::discover_with_token(&directory, "d".repeat(64)),
            Err(LocalIpcError::UnsafeManifest)
        ));
        write_manifest(USER_SCOPE, "127.0.0.1:1234", r#","extra":true"#);
        assert!(matches!(
            AuthenticatedIpcBackend::discover_with_token(&directory, "d".repeat(64)),
            Err(LocalIpcError::Manifest(_))
        ));
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn manifest_contract_never_persists_the_bearer_secret() {
        let directory = directory();
        let token = "e".repeat(64);
        let server = LocalIpcServer::bind_with_token(&directory, token.clone()).expect("bind");
        let persisted = fs::read_to_string(directory.join(MANIFEST_FILE)).expect("manifest");
        assert!(!persisted.contains(&token));
        let parsed: EndpointManifest = serde_json::from_str(&persisted).expect("strict manifest");
        assert_eq!(parsed.scope, USER_SCOPE);
        drop(server);
        fs::remove_dir_all(directory).expect("cleanup");
    }

    #[test]
    fn discovery_rejects_invalid_token_without_connecting() {
        let directory = directory();
        let manifest = EndpointManifest {
            version: IPC_PROTOCOL_VERSION,
            scope: USER_SCOPE.to_owned(),
            instance_id: uuid::Uuid::new_v4().to_string(),
            address: "127.0.0.1:1234".parse::<SocketAddr>().expect("address"),
        };
        fs::write(
            directory.join(MANIFEST_FILE),
            serde_json::to_vec(&manifest).expect("serialize"),
        )
        .expect("write");
        assert!(matches!(
            AuthenticatedIpcBackend::discover_with_token(&directory, "short".to_owned()),
            Err(LocalIpcError::UnsafeManifest)
        ));
        fs::remove_dir_all(directory).expect("cleanup");
    }
}
