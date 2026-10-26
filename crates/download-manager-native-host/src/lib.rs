//! Bounded browser native-messaging transport for the browser bridge.

use std::{
    fs,
    io::{self, ErrorKind, Read, Write},
    path::{Path, PathBuf},
};

use download_manager_engine::ipc::{
    EXTENSION_PROTOCOL_VERSION, ExtensionEnvelope, ExtensionResponse, MAX_EXTENSION_MESSAGE_BYTES,
    RequestIdTracker, decode_extension_request,
};
use serde::{Deserialize, Serialize};

const NATIVE_HOST_NAME: &str = "io.github.musharaf_hossain_cs.download_manager";

#[derive(Deserialize)]
struct FirefoxNativeHostManifest {
    name: String,
    path: PathBuf,
    r#type: String,
    allowed_extensions: Vec<String>,
}

#[derive(Serialize)]
struct ResponseEnvelope {
    version: u16,
    request_id: download_manager_engine::domain::RequestId,
    payload: ExtensionResponse,
}

/// Authorizes a decoded request against the browser-authenticated launch
/// origin, a packaged extension-ID allowlist, and an opaque current-user
/// identity supplied by the platform boundary.
///
/// The request's self-declared extension ID is never sufficient by itself: it
/// must exactly match the ID authenticated by the launch origin.
///
/// # Errors
/// Returns [`ErrorKind::PermissionDenied`] for an invalid/unlisted origin, an
/// envelope-ID mismatch, or a cross-user connection.
pub fn authorize_native_request(
    launch_origin: &str,
    request: &ExtensionEnvelope,
    packaged_extension_ids: &[&str],
    current_user_identity: &str,
    browser_user_identity: &str,
) -> io::Result<()> {
    if current_user_identity.is_empty() || current_user_identity != browser_user_identity {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "native messaging peer is not the current user",
        ));
    }

    let Some(extension_id) = launch_origin
        .strip_prefix("chrome-extension://")
        .and_then(|origin| origin.strip_suffix('/'))
        .filter(|id| !id.is_empty() && !id.contains(['/', '?', '#']))
    else {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "invalid browser extension origin",
        ));
    };
    if !packaged_extension_ids.contains(&extension_id) {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "browser extension is not allowlisted",
        ));
    }
    if request.extension_id != extension_id {
        return Err(io::Error::new(
            ErrorKind::PermissionDenied,
            "request extension ID does not match launch origin",
        ));
    }
    Ok(())
}

/// Authorizes Firefox's Windows native-messaging launch contract.
///
/// Firefox supplies the full native-host manifest path as its first argument
/// and the calling add-on ID as its second. The adapter binds both arguments
/// to a registered-style manifest, the running executable, the packaged
/// add-on allowlist, and the request envelope before any payload is handled.
///
/// # Errors
/// Returns [`ErrorKind::PermissionDenied`] when the manifest is missing or
/// invalid, its executable does not match, the add-on ID is not allowlisted,
/// the envelope ID differs, or the peer is not the current user.
pub fn authorize_firefox_native_request(
    launch_manifest_path: &Path,
    launch_extension_id: &str,
    expected_executable: &Path,
    request: &ExtensionEnvelope,
    packaged_extension_ids: &[&str],
    current_user_identity: &str,
    browser_user_identity: &str,
) -> io::Result<()> {
    if current_user_identity.is_empty() || current_user_identity != browser_user_identity {
        return Err(permission_denied(
            "native messaging peer is not the current user",
        ));
    }
    if launch_extension_id.is_empty()
        || !packaged_extension_ids.contains(&launch_extension_id)
        || request.extension_id != launch_extension_id
    {
        return Err(permission_denied(
            "Firefox add-on ID is not allowlisted or does not match the request",
        ));
    }

    let manifest_bytes = fs::read(launch_manifest_path)
        .map_err(|_| permission_denied("Firefox native-host manifest is unavailable"))?;
    let manifest: FirefoxNativeHostManifest = serde_json::from_slice(&manifest_bytes)
        .map_err(|_| permission_denied("Firefox native-host manifest is invalid"))?;
    let manifest_executable = manifest
        .path
        .canonicalize()
        .map_err(|_| permission_denied("Firefox native-host executable is unavailable"))?;
    let expected_executable = expected_executable
        .canonicalize()
        .map_err(|_| permission_denied("running native-host executable is unavailable"))?;
    if manifest.name != NATIVE_HOST_NAME
        || manifest.r#type != "stdio"
        || manifest_executable != expected_executable
        || !manifest
            .allowed_extensions
            .iter()
            .any(|id| id == launch_extension_id)
    {
        return Err(permission_denied(
            "Firefox native-host manifest does not match this launch",
        ));
    }
    Ok(())
}

fn permission_denied(message: &'static str) -> io::Error {
    io::Error::new(ErrorKind::PermissionDenied, message)
}

/// Runs one native-messaging stream until the browser closes its input.
///
/// Each request is a four-byte little-endian length followed by one strict,
/// size-bounded JSON envelope. A clean EOF between frames ends the session;
/// truncated frames, invalid schemas, oversized messages, and output failures
/// are returned as I/O errors.
///
/// # Errors
/// Returns an error when a frame or request is invalid, or when the response
/// cannot be written to the browser.
pub fn run_native_messaging_session<R, W, F>(
    mut reader: R,
    mut writer: W,
    mut handle: F,
) -> io::Result<()>
where
    R: Read,
    W: Write,
    F: FnMut(&ExtensionEnvelope) -> ExtensionResponse,
{
    let mut tracker = RequestIdTracker::default();
    while let Some(frame) = read_frame(&mut reader)? {
        let request = decode_extension_request(&frame, &mut tracker)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        let response = ResponseEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id: request.request_id,
            payload: handle(&request),
        };
        let encoded = serde_json::to_vec(&response)
            .map_err(|error| io::Error::new(ErrorKind::InvalidData, error))?;
        write_frame(&mut writer, &encoded)?;
    }
    Ok(())
}

fn read_frame(reader: &mut impl Read) -> io::Result<Option<Vec<u8>>> {
    let mut header = [0_u8; 4];
    let mut header_read = 0;
    while header_read < header.len() {
        match reader.read(&mut header[header_read..]) {
            Ok(0) if header_read == 0 => return Ok(None),
            Ok(0) => {
                return Err(io::Error::new(
                    ErrorKind::UnexpectedEof,
                    "native-message length header was truncated",
                ));
            }
            Ok(count) => header_read += count,
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }

    let length = u32::from_le_bytes(header) as usize;
    if length > MAX_EXTENSION_MESSAGE_BYTES {
        return Err(io::Error::new(
            ErrorKind::InvalidData,
            format!("native message size {length} exceeds limit {MAX_EXTENSION_MESSAGE_BYTES}"),
        ));
    }

    let mut body = vec![0_u8; length];
    reader.read_exact(&mut body).map_err(|error| {
        if error.kind() == ErrorKind::UnexpectedEof {
            io::Error::new(
                ErrorKind::UnexpectedEof,
                "native-message body was truncated",
            )
        } else {
            error
        }
    })?;
    Ok(Some(body))
}

fn write_frame(writer: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let length = u32::try_from(body.len())
        .map_err(|_| io::Error::new(ErrorKind::InvalidData, "response is too large"))?;
    writer.write_all(&length.to_le_bytes())?;
    writer.write_all(body)?;
    writer.flush()
}

#[cfg(test)]
mod tests {
    use std::{
        cell::Cell,
        fs,
        io::{self, Cursor, ErrorKind, Read, Write},
        path::Path,
        rc::Rc,
    };

    use download_manager_engine::{
        domain::RequestId,
        ipc::{
            EXTENSION_PROTOCOL_VERSION, ExtensionEnvelope, ExtensionRequest, ExtensionResponse,
            MAX_EXTENSION_MESSAGE_BYTES,
        },
    };

    use super::{
        authorize_firefox_native_request, authorize_native_request, run_native_messaging_session,
    };

    const PACKAGED_ID: &str = "abcdefghijklmnopabcdefghijklmnop";
    const FIREFOX_ID: &str = "download-manager@musharaf-hossain-cs.github.io";

    struct ChunkedReader<R> {
        inner: R,
        maximum_chunk: usize,
    }

    impl<R: Read> Read for ChunkedReader<R> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let limit = buffer.len().min(self.maximum_chunk);
            self.inner.read(&mut buffer[..limit])
        }
    }

    struct BrokenPipeWriter;

    impl Write for BrokenPipeWriter {
        fn write(&mut self, _buffer: &[u8]) -> io::Result<usize> {
            Err(io::Error::new(
                ErrorKind::BrokenPipe,
                "browser disconnected",
            ))
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn framed_request(request_id: RequestId) -> Vec<u8> {
        let body = serde_json::to_vec(&ExtensionEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id,
            nonce: "abcdefghijklmnop".to_owned(),
            extension_id: "packaged-extension-id".to_owned(),
            payload: ExtensionRequest::Health,
        })
        .expect("serialize fixture");
        let mut framed = Vec::with_capacity(4 + body.len());
        framed.extend_from_slice(
            &u32::try_from(body.len())
                .expect("fixture length")
                .to_le_bytes(),
        );
        framed.extend_from_slice(&body);
        framed
    }

    #[test]
    fn partial_reads_produce_one_framed_validated_response() {
        let request_id = RequestId::new();
        let reader = ChunkedReader {
            inner: Cursor::new(framed_request(request_id)),
            maximum_chunk: 1,
        };
        let mut output = Vec::new();

        run_native_messaging_session(reader, &mut output, |_| ExtensionResponse::Healthy {
            host_version: "0.1.0".to_owned(),
        })
        .expect("session");

        let length = u32::from_le_bytes(output[..4].try_into().expect("header")) as usize;
        assert_eq!(length, output.len() - 4);
        let value: serde_json::Value = serde_json::from_slice(&output[4..]).expect("response JSON");
        assert_eq!(value["version"], EXTENSION_PROTOCOL_VERSION);
        assert_eq!(value["request_id"], request_id.to_string());
        assert_eq!(value["payload"]["type"], "healthy");
    }

    #[test]
    fn oversize_and_invalid_frames_never_reach_the_handler() {
        let calls = Rc::new(Cell::new(0));
        let oversized = u32::try_from(MAX_EXTENSION_MESSAGE_BYTES + 1)
            .expect("configured limit")
            .to_le_bytes()
            .to_vec();
        let calls_for_oversize = Rc::clone(&calls);
        let error = run_native_messaging_session(Cursor::new(oversized), Vec::new(), move |_| {
            calls_for_oversize.set(calls_for_oversize.get() + 1);
            ExtensionResponse::Accepted { job_ids: vec![] }
        })
        .expect_err("oversize must fail");
        assert_eq!(error.kind(), ErrorKind::InvalidData);

        let invalid_body = br#"{"version":1,"unexpected":true}"#;
        let mut invalid = u32::try_from(invalid_body.len())
            .expect("fixture length")
            .to_le_bytes()
            .to_vec();
        invalid.extend_from_slice(invalid_body);
        let calls_for_invalid = Rc::clone(&calls);
        let error = run_native_messaging_session(Cursor::new(invalid), Vec::new(), move |_| {
            calls_for_invalid.set(calls_for_invalid.get() + 1);
            ExtensionResponse::Accepted { job_ids: vec![] }
        })
        .expect_err("invalid schema must fail");
        assert_eq!(error.kind(), ErrorKind::InvalidData);
        assert_eq!(calls.get(), 0);
    }

    #[test]
    fn truncated_input_and_broken_output_are_reported() {
        let truncated = vec![10, 0, 0, 0, b'{'];
        let error = run_native_messaging_session(Cursor::new(truncated), Vec::new(), |_| {
            ExtensionResponse::Accepted { job_ids: vec![] }
        })
        .expect_err("truncated body must fail");
        assert_eq!(error.kind(), ErrorKind::UnexpectedEof);

        let error = run_native_messaging_session(
            Cursor::new(framed_request(RequestId::new())),
            BrokenPipeWriter,
            |_| ExtensionResponse::Accepted { job_ids: vec![] },
        )
        .expect_err("broken output must fail");
        assert_eq!(error.kind(), ErrorKind::BrokenPipe);
    }

    #[test]
    fn packaged_origin_and_matching_current_user_are_authorized() {
        let request = ExtensionEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id: RequestId::new(),
            nonce: "abcdefghijklmnop".to_owned(),
            extension_id: PACKAGED_ID.to_owned(),
            payload: ExtensionRequest::Health,
        };

        authorize_native_request(
            &format!("chrome-extension://{PACKAGED_ID}/"),
            &request,
            &[PACKAGED_ID],
            "S-1-5-21-current",
            "S-1-5-21-current",
        )
        .expect("authorized request");
    }

    #[test]
    fn wrong_origin_payload_id_and_user_are_rejected() {
        let request = ExtensionEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id: RequestId::new(),
            nonce: "abcdefghijklmnop".to_owned(),
            extension_id: PACKAGED_ID.to_owned(),
            payload: ExtensionRequest::Health,
        };
        for (origin, user, peer) in [
            ("https://example.test/", "current", "current"),
            (
                "chrome-extension://ponmlkjihgfedcbaponmlkjihgfedcba/",
                "current",
                "current",
            ),
            (
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/path",
                "current",
                "current",
            ),
            (
                "chrome-extension://abcdefghijklmnopabcdefghijklmnop/",
                "current",
                "other",
            ),
        ] {
            let error = authorize_native_request(origin, &request, &[PACKAGED_ID], user, peer)
                .expect_err("request must be rejected");
            assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        }

        let mut forged_request = request;
        forged_request.extension_id = "ponmlkjihgfedcbaponmlkjihgfedcba".to_owned();
        let error = authorize_native_request(
            &format!("chrome-extension://{PACKAGED_ID}/"),
            &forged_request,
            &[PACKAGED_ID],
            "current",
            "current",
        )
        .expect_err("payload ID cannot override origin");
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);
    }

    #[test]
    fn firefox_manifest_and_explicit_add_on_id_are_bound_to_the_request() {
        let root = std::env::temp_dir().join(format!(
            "download-manager-firefox-auth-{}",
            RequestId::new()
        ));
        fs::create_dir_all(&root).expect("fixture directory");
        let executable = root.join("download-manager-native-host.exe");
        fs::write(&executable, b"fixture executable").expect("fixture executable");
        let manifest = root.join("native-host.json");
        write_firefox_manifest(&manifest, &executable, FIREFOX_ID);
        let request = ExtensionEnvelope {
            version: EXTENSION_PROTOCOL_VERSION,
            request_id: RequestId::new(),
            nonce: "abcdefghijklmnop".to_owned(),
            extension_id: FIREFOX_ID.to_owned(),
            payload: ExtensionRequest::Health,
        };

        authorize_firefox_native_request(
            &manifest,
            FIREFOX_ID,
            &executable,
            &request,
            &[FIREFOX_ID],
            "current-user",
            "current-user",
        )
        .expect("valid Firefox launch");

        let missing_manifest = root.join("missing.json");
        let wrong_executable = root.join("wrong.exe");
        for (manifest_path, extension_id, expected_executable) in [
            (
                manifest.as_path(),
                "wrong@example.test",
                executable.as_path(),
            ),
            (missing_manifest.as_path(), FIREFOX_ID, executable.as_path()),
            (manifest.as_path(), FIREFOX_ID, wrong_executable.as_path()),
        ] {
            let error = authorize_firefox_native_request(
                manifest_path,
                extension_id,
                expected_executable,
                &request,
                &[FIREFOX_ID],
                "current-user",
                "current-user",
            )
            .expect_err("forged Firefox launch must be rejected");
            assert_eq!(error.kind(), ErrorKind::PermissionDenied);
        }

        write_firefox_manifest(&manifest, &executable, "other@example.test");
        let error = authorize_firefox_native_request(
            &manifest,
            FIREFOX_ID,
            &executable,
            &request,
            &[FIREFOX_ID],
            "current-user",
            "current-user",
        )
        .expect_err("manifest allowlist must be authoritative");
        assert_eq!(error.kind(), ErrorKind::PermissionDenied);

        fs::remove_dir_all(root).expect("remove fixture directory");
    }

    fn write_firefox_manifest(path: &Path, executable: &Path, extension_id: &str) {
        let manifest = serde_json::json!({
            "name": "io.github.musharaf_hossain_cs.download_manager",
            "description": "Download Manager native messaging host",
            "path": executable,
            "type": "stdio",
            "allowed_extensions": [extension_id],
        });
        fs::write(
            path,
            serde_json::to_vec(&manifest).expect("serialize manifest"),
        )
        .expect("write manifest");
    }
}
