//! Bounded Chromium native-messaging transport for the browser bridge.

use std::io::{self, ErrorKind, Read, Write};

use download_manager_engine::ipc::{
    EXTENSION_PROTOCOL_VERSION, ExtensionEnvelope, ExtensionResponse, MAX_EXTENSION_MESSAGE_BYTES,
    RequestIdTracker, decode_extension_request,
};
use serde::Serialize;

#[derive(Serialize)]
struct ResponseEnvelope {
    version: u16,
    request_id: download_manager_engine::domain::RequestId,
    payload: ExtensionResponse,
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
        io::{self, Cursor, ErrorKind, Read, Write},
        rc::Rc,
    };

    use download_manager_engine::{
        domain::RequestId,
        ipc::{
            EXTENSION_PROTOCOL_VERSION, ExtensionEnvelope, ExtensionRequest, ExtensionResponse,
            MAX_EXTENSION_MESSAGE_BYTES,
        },
    };

    use super::run_native_messaging_session;

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
}
