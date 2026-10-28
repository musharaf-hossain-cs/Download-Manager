//! Durable, non-secret metadata for short-lived browser request context.

use serde::Serialize;
use thiserror::Error;
use url::Url;

use crate::{
    credentials::CredentialRef,
    domain::{HttpUrl, JobId, UnixMillis, ValidationError},
};

const MAX_USER_AGENT_LENGTH: usize = 1_024;
const MAX_HEADER_PROFILE_LENGTH: usize = 256;

/// A sanitized request-context record whose secret values live only in the user vault.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RequestContext {
    job_id: JobId,
    referer: Option<HttpUrl>,
    origin: Option<HttpUrl>,
    user_agent: Option<String>,
    header_profile: Option<String>,
    credential_reference: Option<CredentialRef>,
    cookie_reference: Option<CredentialRef>,
    expires_at: UnixMillis,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RequestContextError {
    #[error("invalid request-context URL: {0}")]
    Url(#[from] ValidationError),
    #[error("{field} must be non-empty and no longer than {maximum} bytes")]
    InvalidText { field: &'static str, maximum: usize },
    #[error("header profile must be an opaque identifier, not header data")]
    InvalidHeaderProfile,
    #[error("credential reference fields must be non-empty and bounded")]
    InvalidCredentialReference,
}

impl RequestContext {
    /// Builds persistable metadata, removing userinfo, query strings, and fragments from URLs.
    ///
    /// Header and cookie values must be stored in the user vault and represented here only by
    /// opaque references.
    ///
    /// # Errors
    /// Returns a validation error for malformed or oversized metadata and non-opaque profile IDs.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        job_id: JobId,
        referer: Option<&HttpUrl>,
        origin: Option<&HttpUrl>,
        user_agent: Option<String>,
        header_profile: Option<String>,
        credential_reference: Option<CredentialRef>,
        cookie_reference: Option<CredentialRef>,
        expires_at: UnixMillis,
    ) -> Result<Self, RequestContextError> {
        validate_optional_text("user_agent", user_agent.as_deref(), MAX_USER_AGENT_LENGTH)?;
        validate_optional_text(
            "header_profile",
            header_profile.as_deref(),
            MAX_HEADER_PROFILE_LENGTH,
        )?;
        if header_profile
            .as_deref()
            .is_some_and(|value| !is_opaque_identifier(value))
        {
            return Err(RequestContextError::InvalidHeaderProfile);
        }
        for reference in [credential_reference.as_ref(), cookie_reference.as_ref()]
            .into_iter()
            .flatten()
        {
            validate_reference(reference)?;
        }
        Ok(Self {
            job_id,
            referer: referer.map(sanitize_referer).transpose()?,
            origin: origin.map(sanitize_origin).transpose()?,
            user_agent,
            header_profile,
            credential_reference,
            cookie_reference,
            expires_at,
        })
    }

    #[must_use]
    pub const fn job_id(&self) -> JobId {
        self.job_id
    }

    #[must_use]
    pub fn referer(&self) -> Option<&HttpUrl> {
        self.referer.as_ref()
    }

    #[must_use]
    pub fn origin(&self) -> Option<&HttpUrl> {
        self.origin.as_ref()
    }

    #[must_use]
    pub fn user_agent(&self) -> Option<&str> {
        self.user_agent.as_deref()
    }

    #[must_use]
    pub fn header_profile(&self) -> Option<&str> {
        self.header_profile.as_deref()
    }

    #[must_use]
    pub fn credential_reference(&self) -> Option<&CredentialRef> {
        self.credential_reference.as_ref()
    }

    #[must_use]
    pub fn cookie_reference(&self) -> Option<&CredentialRef> {
        self.cookie_reference.as_ref()
    }

    #[must_use]
    pub const fn expires_at(&self) -> UnixMillis {
        self.expires_at
    }
}

fn sanitize_referer(value: &HttpUrl) -> Result<HttpUrl, RequestContextError> {
    let mut url = Url::parse(value.as_str()).map_err(|error| {
        RequestContextError::Url(ValidationError::InvalidUrl(error.to_string()))
    })?;
    let _ = url.set_username("");
    let _ = url.set_password(None);
    url.set_query(None);
    url.set_fragment(None);
    Ok(HttpUrl::parse(url.to_string())?)
}

pub(crate) fn sanitize_durable_url(value: &HttpUrl) -> Result<HttpUrl, RequestContextError> {
    sanitize_referer(value)
}

fn sanitize_origin(value: &HttpUrl) -> Result<HttpUrl, RequestContextError> {
    let url = Url::parse(value.as_str()).map_err(|error| {
        RequestContextError::Url(ValidationError::InvalidUrl(error.to_string()))
    })?;
    Ok(HttpUrl::parse(format!(
        "{}/",
        url.origin().ascii_serialization()
    ))?)
}

fn validate_optional_text(
    field: &'static str,
    value: Option<&str>,
    maximum: usize,
) -> Result<(), RequestContextError> {
    if value.is_some_and(|value| value.trim().is_empty() || value.len() > maximum) {
        Err(RequestContextError::InvalidText { field, maximum })
    } else {
        Ok(())
    }
}

fn is_opaque_identifier(value: &str) -> bool {
    value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn validate_reference(reference: &CredentialRef) -> Result<(), RequestContextError> {
    if reference.service.trim().is_empty()
        || reference.service.len() > 1_024
        || reference.account.trim().is_empty()
        || reference.account.len() > 512
    {
        Err(RequestContextError::InvalidCredentialReference)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{RequestContext, RequestContextError};
    use crate::{
        credentials::CredentialRef,
        domain::{HttpUrl, JobId, UnixMillis},
    };

    #[test]
    fn constructor_redacts_urls_and_rejects_header_values_as_profile_ids() {
        let referer =
            HttpUrl::parse("https://user:password@example.test/watch?id=42&token=secret#player")
                .expect("referer");
        let origin = HttpUrl::parse("https://example.test/path?cookie=secret").expect("origin");
        let context = RequestContext::new(
            JobId::new(),
            Some(&referer),
            Some(&origin),
            Some("Browser/1.0".to_owned()),
            Some("media-default".to_owned()),
            None,
            Some(CredentialRef {
                service: "DownloadManager/cookie/example.test".to_owned(),
                account: "profile-one".to_owned(),
            }),
            UnixMillis::new(10).expect("expiry"),
        )
        .expect("context");

        assert_eq!(
            context.referer().map(HttpUrl::as_str),
            Some("https://example.test/watch")
        );
        assert_eq!(
            context.origin().map(HttpUrl::as_str),
            Some("https://example.test/")
        );
        assert!(!format!("{context:?}").contains("secret"));
        assert_eq!(
            RequestContext::new(
                JobId::new(),
                None,
                None,
                None,
                Some("Authorization: Bearer secret".to_owned()),
                None,
                None,
                UnixMillis::new(10).expect("expiry"),
            ),
            Err(RequestContextError::InvalidHeaderProfile)
        );
    }
}
