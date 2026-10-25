//! HTTP metadata probing without downloading the represented object.

use std::{io::Read, time::Duration};

use reqwest::{
    StatusCode,
    blocking::{Client, Response},
    header::{
        ACCEPT_RANGES, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, ETAG,
        HeaderMap, LAST_MODIFIED, RANGE, WWW_AUTHENTICATE,
    },
    redirect::Policy,
};
use thiserror::Error;

use crate::domain::{HttpUrl, RangeCapability, RemoteIdentity, ValidationError};

const MAX_REDIRECTS: usize = 10;
const PROBE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeResult {
    pub final_url: HttpUrl,
    pub status: u16,
    pub content_disposition: Option<String>,
    pub authentication_challenge: Option<String>,
    pub identity: RemoteIdentity,
}

#[derive(Debug, Error)]
pub enum ProbeError {
    #[error("HTTP probe failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("HTTP probe returned status {0}")]
    HttpStatus(u16),
    #[error("probe response URL is invalid: {0}")]
    InvalidFinalUrl(#[from] ValidationError),
    #[error("invalid Content-Range header: {0}")]
    InvalidContentRange(String),
    #[error("probe body could not be sampled: {0}")]
    Read(#[from] std::io::Error),
}

#[derive(Debug, Clone)]
pub struct HttpProbe {
    client: Client,
}

impl HttpProbe {
    /// Builds a probe client with bounded redirects and request timeouts.
    ///
    /// # Errors
    /// Returns an error if the TLS or HTTP client cannot be initialized.
    pub fn new() -> Result<Self, ProbeError> {
        let client = Client::builder()
            .redirect(Policy::limited(MAX_REDIRECTS))
            .timeout(PROBE_TIMEOUT)
            .build()?;
        Ok(Self { client })
    }

    /// Resolves redirects and discovers identity metadata using HEAD, with a one-byte GET fallback
    /// when the origin rejects HEAD.
    ///
    /// # Errors
    /// Returns an error for transport failures, non-success responses, or malformed range metadata.
    pub fn probe(&self, url: &HttpUrl) -> Result<ProbeResult, ProbeError> {
        let head = self.client.head(url.as_str()).send()?;
        if matches!(
            head.status(),
            StatusCode::METHOD_NOT_ALLOWED | StatusCode::NOT_IMPLEMENTED
        ) {
            return self.probe_with_small_get(url);
        }
        result_from_response(&head, false)
    }

    fn probe_with_small_get(&self, url: &HttpUrl) -> Result<ProbeResult, ProbeError> {
        let mut response = self
            .client
            .get(url.as_str())
            .header(RANGE, "bytes=0-0")
            .send()?;
        let mut sample = [0_u8; 1];
        let _sampled = response.read(&mut sample)?;
        result_from_response(&response, true)
    }
}

impl Default for HttpProbe {
    fn default() -> Self {
        Self::new().expect("default HTTP probe client")
    }
}

fn result_from_response(
    response: &Response,
    used_range_get: bool,
) -> Result<ProbeResult, ProbeError> {
    let status = response.status();
    if !status.is_success() {
        return Err(ProbeError::HttpStatus(status.as_u16()));
    }
    let headers = response.headers();
    let content_range = header(headers, CONTENT_RANGE);
    let content_length = if used_range_get && status == StatusCode::PARTIAL_CONTENT {
        content_range
            .as_deref()
            .map(content_range_length)
            .transpose()?
    } else {
        parse_u64_header(headers, CONTENT_LENGTH)
    };
    let range_capability = if status == StatusCode::PARTIAL_CONTENT
        || header(headers, ACCEPT_RANGES).is_some_and(|value| value.eq_ignore_ascii_case("bytes"))
    {
        RangeCapability::Supported
    } else if used_range_get {
        RangeCapability::Unsupported
    } else {
        RangeCapability::Unknown
    };
    Ok(ProbeResult {
        final_url: HttpUrl::parse(response.url().as_str())?,
        status: status.as_u16(),
        content_disposition: header(headers, CONTENT_DISPOSITION),
        authentication_challenge: header(headers, WWW_AUTHENTICATE),
        identity: RemoteIdentity {
            etag: header(headers, ETAG),
            last_modified: header(headers, LAST_MODIFIED),
            content_length,
            mime_type: header(headers, CONTENT_TYPE).map(|value| {
                value
                    .split_once(';')
                    .map_or(value.as_str(), |(mime, _)| mime)
                    .trim()
                    .to_owned()
            }),
            content_fingerprint: None,
            range_capability,
        },
    })
}

fn header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<String> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

fn parse_u64_header(headers: &HeaderMap, name: reqwest::header::HeaderName) -> Option<u64> {
    headers
        .get(name)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
}

fn content_range_length(value: &str) -> Result<u64, ProbeError> {
    value
        .split_once('/')
        .and_then(|(_, length)| (length != "*").then_some(length))
        .and_then(|length| length.parse().ok())
        .ok_or_else(|| ProbeError::InvalidContentRange(value.to_owned()))
}

#[cfg(test)]
mod tests {
    use super::{HttpProbe, ProbeError};
    use crate::{
        domain::{HttpUrl, RangeCapability},
        test_support::{Fixture, FixtureServer},
    };

    #[test]
    fn head_denied_falls_back_to_one_byte_get() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new()).status(405),
            Fixture::ok(b"x".to_vec())
                .status(206)
                .header("Content-Range", "bytes 0-0/123")
                .header("ETag", "\"v1\""),
        ]);
        let result = HttpProbe::new()
            .expect("client")
            .probe(&HttpUrl::parse(server.url("/file")).expect("URL"))
            .expect("probe");
        assert_eq!(result.identity.content_length, Some(123));
        assert_eq!(result.identity.etag.as_deref(), Some("\"v1\""));
        assert_eq!(result.identity.range_capability, RangeCapability::Supported);
        let requests = server.requests();
        assert_eq!(requests[0].method, "HEAD");
        assert_eq!(requests[1].method, "GET");
        assert_eq!(requests[1].header("range"), Some("bytes=0-0"));
    }

    #[test]
    fn follows_redirect_and_reports_final_url() {
        let server = FixtureServer::start(vec![
            Fixture::ok(Vec::new())
                .status(302)
                .header("Location", "/final"),
            Fixture::ok(Vec::new())
                .header("Content-Length", "42")
                .header("Accept-Ranges", "bytes"),
        ]);
        let result = HttpProbe::new()
            .expect("client")
            .probe(&HttpUrl::parse(server.url("/redirect")).expect("URL"))
            .expect("probe");
        assert!(result.final_url.as_str().ends_with("/final"));
        assert_eq!(result.identity.content_length, Some(42));
    }

    #[test]
    fn preserves_unknown_length_without_fetching_body() {
        let server = FixtureServer::start(vec![Fixture::ok(Vec::new()).without_content_length()]);
        let result = HttpProbe::new()
            .expect("client")
            .probe(&HttpUrl::parse(server.url("/stream")).expect("URL"))
            .expect("probe");
        assert_eq!(result.identity.content_length, None);
        assert_eq!(server.requests().len(), 1);
    }

    #[test]
    fn rejects_non_success_status() {
        let server = FixtureServer::start(vec![Fixture::ok(Vec::new()).status(404)]);
        let error = HttpProbe::new()
            .expect("client")
            .probe(&HttpUrl::parse(server.url("/missing")).expect("URL"))
            .expect_err("status error");
        assert!(matches!(error, ProbeError::HttpStatus(404)));
    }
}
