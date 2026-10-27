//! User-initiated refresh validation for expired media request URLs.

use thiserror::Error;
use url::Url;

use crate::{
    dash::{DashSegmentPlan, DashSegmentResource},
    domain::{HttpUrl, JobId, JobMetadata},
    media::{HlsInitSegment, HlsMediaPlaylist, HlsMediaSegment},
    state::DownloadState,
};

const MAX_BROWSER_LENGTH: usize = 128;
const MAX_PROFILE_LENGTH: usize = 256;
const MAX_SESSION_LENGTH: usize = 256;

/// The browser context that is allowed to refresh one expired media job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshScope {
    browser: String,
    profile: String,
    session: String,
}

impl RefreshScope {
    /// Creates an opaque, bounded browser/profile/session scope.
    ///
    /// # Errors
    /// Returns [`RefreshError::InvalidScope`] when a component is empty or exceeds its bound.
    pub fn new(
        browser: impl Into<String>,
        profile: impl Into<String>,
        session: impl Into<String>,
    ) -> Result<Self, RefreshError> {
        let scope = Self {
            browser: browser.into(),
            profile: profile.into(),
            session: session.into(),
        };
        validate_scope_component("browser", &scope.browser, MAX_BROWSER_LENGTH)?;
        validate_scope_component("profile", &scope.profile, MAX_PROFILE_LENGTH)?;
        validate_scope_component("session", &scope.session, MAX_SESSION_LENGTH)?;
        Ok(scope)
    }

    #[must_use]
    pub fn browser(&self) -> &str {
        &self.browser
    }

    #[must_use]
    pub fn profile(&self) -> &str {
        &self.profile
    }

    #[must_use]
    pub fn session(&self) -> &str {
        &self.session
    }
}

/// One explicit browser request to replace an expired media URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshRequest {
    pub job_id: JobId,
    pub scope: RefreshScope,
    pub user_initiated: bool,
}

/// Unforgeable proof that the original browser context authorized one job refresh.
#[derive(Debug, PartialEq, Eq)]
pub struct ValidatedRefresh {
    job_id: JobId,
}

impl ValidatedRefresh {
    pub(crate) fn permits(self, job: &JobMetadata) -> Result<(), RefreshError> {
        if self.job_id != job.id {
            return Err(RefreshError::JobMismatch);
        }
        if job.state != DownloadState::NeedsRefresh {
            return Err(RefreshError::InvalidState);
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) const fn for_test(job_id: JobId) -> Self {
        Self { job_id }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RefreshError {
    #[error("invalid refresh scope component {field}")]
    InvalidScope { field: &'static str },
    #[error("refresh request targets a different job")]
    JobMismatch,
    #[error("job is not waiting for refreshed request context")]
    InvalidState,
    #[error("media refresh must be explicitly initiated by the user")]
    UserActionRequired,
    #[error("job browser/profile does not match its refresh scope")]
    JobScopeMismatch,
    #[error("refresh request came from a different browser/profile/session")]
    RequestScopeMismatch,
    #[error("HLS segment duration must be finite")]
    InvalidHlsDuration,
    #[error("refreshed media object differs in {0}")]
    ObjectMismatch(&'static str),
    #[error("media resource URL is invalid")]
    InvalidResourceUrl,
}

/// Verifies that an explicit refresh came from the context that created the expired job.
///
/// This gate performs no state change. Callers may replace request context and reconnect only after
/// it succeeds.
///
/// # Errors
/// Returns a precise refusal for a different job, state, implicit retry, or browser context.
pub fn validate_refresh_request(
    job: &JobMetadata,
    expected_scope: &RefreshScope,
    request: &RefreshRequest,
) -> Result<ValidatedRefresh, RefreshError> {
    if request.job_id != job.id {
        return Err(RefreshError::JobMismatch);
    }
    if job.state != DownloadState::NeedsRefresh {
        return Err(RefreshError::InvalidState);
    }
    if !request.user_initiated {
        return Err(RefreshError::UserActionRequired);
    }
    if job.source_browser.as_deref() != Some(expected_scope.browser())
        || job.source_profile.as_deref() != Some(expected_scope.profile())
    {
        return Err(RefreshError::JobScopeMismatch);
    }
    if &request.scope != expected_scope {
        return Err(RefreshError::RequestScopeMismatch);
    }
    Ok(ValidatedRefresh { job_id: job.id })
}

/// Accepts an HLS refresh only when token material changed without changing stream topology.
///
/// URL query strings and fragments are deliberately ignored. Scheme, host, effective port, path,
/// ordering, timing, byte ranges, initialization objects, and discontinuities must still match.
///
/// # Errors
/// Returns a mismatch before a downloader can touch durable output.
pub fn validate_hls_refresh(
    original: &HlsMediaPlaylist,
    refreshed: &HlsMediaPlaylist,
) -> Result<(), RefreshError> {
    if original.media_sequence != refreshed.media_sequence
        || original.target_duration != refreshed.target_duration
        || original.has_end_list != refreshed.has_end_list
        || original.segments.len() != refreshed.segments.len()
    {
        return Err(RefreshError::ObjectMismatch("HLS playlist topology"));
    }
    for index in 0..original.segments.len() {
        let original_segment = &original.segments[index];
        let refreshed_segment = &refreshed.segments[index];
        let original_init_transition = index == 0
            || original_segment.init_segment != original.segments[index - 1].init_segment;
        let refreshed_init_transition = index == 0
            || refreshed_segment.init_segment != refreshed.segments[index - 1].init_segment;
        if original_init_transition != refreshed_init_transition {
            return Err(RefreshError::ObjectMismatch(
                "HLS initialization transition topology",
            ));
        }
        validate_hls_segment(original_segment, refreshed_segment)?;
    }
    Ok(())
}

/// Accepts a DASH refresh only when token material changed without changing segment topology.
///
/// # Errors
/// Returns a mismatch before a downloader can touch durable output.
pub fn validate_dash_refresh(
    original: &DashSegmentPlan,
    refreshed: &DashSegmentPlan,
) -> Result<(), RefreshError> {
    match (&original.initialization, &refreshed.initialization) {
        (Some(original), Some(refreshed)) => validate_dash_resource(original, refreshed)?,
        (None, None) => {}
        _ => return Err(RefreshError::ObjectMismatch("DASH initialization resource")),
    }
    if original.media.len() != refreshed.media.len() {
        return Err(RefreshError::ObjectMismatch("DASH segment count"));
    }
    for (original, refreshed) in original.media.iter().zip(&refreshed.media) {
        if original.number != refreshed.number
            || original.time != refreshed.time
            || original.duration != refreshed.duration
        {
            return Err(RefreshError::ObjectMismatch("DASH segment timeline"));
        }
        validate_dash_resource(&original.resource, &refreshed.resource)?;
    }
    Ok(())
}

fn validate_scope_component(
    field: &'static str,
    value: &str,
    maximum: usize,
) -> Result<(), RefreshError> {
    if value.trim().is_empty() || value.len() > maximum {
        Err(RefreshError::InvalidScope { field })
    } else {
        Ok(())
    }
}

fn validate_hls_segment(
    original: &HlsMediaSegment,
    refreshed: &HlsMediaSegment,
) -> Result<(), RefreshError> {
    if !original.duration_seconds.is_finite() || !refreshed.duration_seconds.is_finite() {
        return Err(RefreshError::InvalidHlsDuration);
    }
    if original.sequence != refreshed.sequence
        || original.duration_seconds.to_bits() != refreshed.duration_seconds.to_bits()
        || original.byte_range != refreshed.byte_range
        || original.discontinuity != refreshed.discontinuity
    {
        return Err(RefreshError::ObjectMismatch("HLS segment topology"));
    }
    validate_http_locator(&original.uri, &refreshed.uri)?;
    match (&original.init_segment, &refreshed.init_segment) {
        (Some(original), Some(refreshed)) => validate_hls_init(original, refreshed),
        (None, None) => Ok(()),
        _ => Err(RefreshError::ObjectMismatch("HLS initialization object")),
    }
}

fn validate_hls_init(
    original: &HlsInitSegment,
    refreshed: &HlsInitSegment,
) -> Result<(), RefreshError> {
    if original.byte_range != refreshed.byte_range {
        return Err(RefreshError::ObjectMismatch(
            "HLS initialization byte range",
        ));
    }
    validate_http_locator(&original.uri, &refreshed.uri)
}

fn validate_http_locator(original: &HttpUrl, refreshed: &HttpUrl) -> Result<(), RefreshError> {
    let original = Url::parse(original.as_str()).map_err(|_| RefreshError::InvalidResourceUrl)?;
    let refreshed = Url::parse(refreshed.as_str()).map_err(|_| RefreshError::InvalidResourceUrl)?;
    validate_url_locator(&original, &refreshed)
}

fn validate_dash_resource(
    original: &DashSegmentResource,
    refreshed: &DashSegmentResource,
) -> Result<(), RefreshError> {
    if original.byte_range != refreshed.byte_range {
        return Err(RefreshError::ObjectMismatch("DASH resource byte range"));
    }
    validate_url_locator(&original.url, &refreshed.url)
}

fn validate_url_locator(original: &Url, refreshed: &Url) -> Result<(), RefreshError> {
    if original.scheme() != refreshed.scheme()
        || original.username() != refreshed.username()
        || original.password() != refreshed.password()
        || original.host_str() != refreshed.host_str()
        || original.port_or_known_default() != refreshed.port_or_known_default()
        || original.path() != refreshed.path()
        || stable_query(original) != stable_query(refreshed)
    {
        return Err(RefreshError::ObjectMismatch("resource URL locator"));
    }
    Ok(())
}

fn stable_query(url: &Url) -> Vec<(String, String)> {
    let mut pairs = url
        .query_pairs()
        .filter(|(name, _)| !is_signature_parameter(name))
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect::<Vec<_>>();
    pairs.sort_unstable();
    pairs
}

fn is_signature_parameter(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.starts_with("x-amz-")
        || name.starts_with("x-goog-")
        || matches!(
            name.as_str(),
            "token"
                | "auth"
                | "authorization"
                | "sig"
                | "signature"
                | "expires"
                | "expiry"
                | "exp"
                | "policy"
                | "key-pair-id"
                | "hdnts"
        )
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use url::Url;

    use super::{
        RefreshError, RefreshRequest, RefreshScope, validate_dash_refresh, validate_hls_refresh,
        validate_refresh_request,
    };
    use crate::{
        dash::{DashByteRange, DashMediaSegment, DashSegmentPlan, DashSegmentResource},
        domain::{DownloadKind, HttpUrl, JobId, JobMetadata, Priority, UnixMillis},
        media::{HlsByteRange, HlsInitSegment, HlsMediaPlaylist, HlsMediaSegment},
        state::DownloadState,
    };

    fn scope(session: &str) -> RefreshScope {
        RefreshScope::new("firefox", "profile-abcdefghijkl", session).expect("scope")
    }

    fn job(id: JobId) -> JobMetadata {
        let at = UnixMillis::new(1).expect("timestamp");
        JobMetadata {
            id,
            source_url: HttpUrl::parse("https://media.example/video.m3u8?token=old").expect("URL"),
            final_url: None,
            display_url: "https://media.example/video.m3u8".to_owned(),
            filename: "video.mp4".to_owned(),
            destination: PathBuf::from("video.mp4"),
            state: DownloadState::NeedsRefresh,
            kind: DownloadKind::Media,
            total_bytes: None,
            downloaded_bytes: 10,
            category: None,
            queue: None,
            priority: Priority::Normal,
            source_browser: Some("firefox".to_owned()),
            source_profile: Some("profile-abcdefghijkl".to_owned()),
            retry_count: 0,
            error: None,
            created_at: at,
            updated_at: at,
            completed_at: None,
        }
    }

    #[test]
    fn refresh_request_requires_original_scope_state_and_user_action() {
        let id = JobId::new();
        let expected = scope("tab-session-one");
        let request = RefreshRequest {
            job_id: id,
            scope: expected.clone(),
            user_initiated: true,
        };
        let mut stored_job = job(id);
        validate_refresh_request(&stored_job, &expected, &request).expect("valid refresh");

        assert_eq!(
            validate_refresh_request(
                &stored_job,
                &expected,
                &RefreshRequest {
                    user_initiated: false,
                    ..request.clone()
                }
            ),
            Err(RefreshError::UserActionRequired)
        );
        assert_eq!(
            validate_refresh_request(
                &stored_job,
                &expected,
                &RefreshRequest {
                    scope: scope("different-tab"),
                    ..request.clone()
                }
            ),
            Err(RefreshError::RequestScopeMismatch)
        );
        stored_job.state = DownloadState::Paused;
        assert_eq!(
            validate_refresh_request(&stored_job, &expected, &request),
            Err(RefreshError::InvalidState)
        );
    }

    #[test]
    fn scope_and_job_binding_reject_invalid_or_cross_profile_values() {
        assert_eq!(
            RefreshScope::new(" ", "profile", "session"),
            Err(RefreshError::InvalidScope { field: "browser" })
        );
        assert_eq!(
            RefreshScope::new("browser", "x".repeat(257), "session"),
            Err(RefreshError::InvalidScope { field: "profile" })
        );

        let id = JobId::new();
        let mut cross_profile_job = job(id);
        let expected = scope("tab-session-one");
        let request = RefreshRequest {
            job_id: id,
            scope: expected.clone(),
            user_initiated: true,
        };
        cross_profile_job.source_profile = Some("another-profile".to_owned());
        assert_eq!(
            validate_refresh_request(&cross_profile_job, &expected, &request),
            Err(RefreshError::JobScopeMismatch)
        );
        let other_job = RefreshRequest {
            job_id: JobId::new(),
            ..request
        };
        assert_eq!(
            validate_refresh_request(&job(id), &expected, &other_job),
            Err(RefreshError::JobMismatch)
        );
    }

    fn hls(token: &str, segment_path: &str) -> HlsMediaPlaylist {
        HlsMediaPlaylist {
            media_sequence: 41,
            target_duration: 6,
            has_end_list: true,
            segments: vec![HlsMediaSegment {
                sequence: 41,
                uri: HttpUrl::parse(format!(
                    "https://cdn.example{segment_path}?token={token}#ignored"
                ))
                .expect("segment URL"),
                duration_seconds: 5.5,
                byte_range: Some(HlsByteRange {
                    offset: 0,
                    length: 512,
                }),
                init_segment: Some(HlsInitSegment {
                    uri: HttpUrl::parse(format!("https://cdn.example/init.mp4?token={token}"))
                        .expect("init URL"),
                    byte_range: None,
                }),
                discontinuity: false,
            }],
        }
    }

    #[test]
    fn hls_refresh_accepts_rotated_query_but_rejects_path_or_topology_change() {
        let original = hls("expired", "/part-41.m4s");
        validate_hls_refresh(&original, &hls("fresh", "/part-41.m4s"))
            .expect("query-only token refresh");
        assert!(matches!(
            validate_hls_refresh(&original, &hls("fresh", "/part-42.m4s")),
            Err(RefreshError::ObjectMismatch("resource URL locator"))
        ));
        let mut changed_query_identity = hls("fresh", "/part-41.m4s");
        changed_query_identity.segments[0].uri =
            HttpUrl::parse("https://cdn.example/part-41.m4s?asset=other&token=fresh")
                .expect("segment URL");
        assert!(matches!(
            validate_hls_refresh(&original, &changed_query_identity),
            Err(RefreshError::ObjectMismatch("resource URL locator"))
        ));
        let mut changed = hls("fresh", "/part-41.m4s");
        changed.segments[0].discontinuity = true;
        assert!(matches!(
            validate_hls_refresh(&original, &changed),
            Err(RefreshError::ObjectMismatch("HLS segment topology"))
        ));

        let mut original_transitions = hls("old-a", "/part-41.m4s");
        let mut second = original_transitions.segments[0].clone();
        second.sequence += 1;
        second.uri =
            HttpUrl::parse("https://cdn.example/part-42.m4s?token=old-b").expect("segment URL");
        second.init_segment = Some(HlsInitSegment {
            uri: HttpUrl::parse("https://cdn.example/init.mp4?token=old-b").expect("init URL"),
            byte_range: None,
        });
        original_transitions.segments.push(second);
        let mut collapsed = original_transitions.clone();
        collapsed.segments[0].init_segment = Some(HlsInitSegment {
            uri: HttpUrl::parse("https://cdn.example/init.mp4?token=fresh").expect("init URL"),
            byte_range: None,
        });
        collapsed.segments[1].init_segment = collapsed.segments[0].init_segment.clone();
        assert!(matches!(
            validate_hls_refresh(&original_transitions, &collapsed),
            Err(RefreshError::ObjectMismatch(
                "HLS initialization transition topology"
            ))
        ));
    }

    fn dash(token: &str, path: &str) -> DashSegmentPlan {
        let resource = |path: &str| DashSegmentResource {
            url: Url::parse(&format!("https://cdn.example{path}?token={token}#ignored"))
                .expect("DASH URL"),
            byte_range: Some(DashByteRange { start: 0, end: 99 }),
        };
        DashSegmentPlan {
            initialization: Some(resource("/init.mp4")),
            media: vec![DashMediaSegment {
                number: 7,
                time: 12_000,
                duration: 2_000,
                resource: resource(path),
            }],
        }
    }

    #[test]
    fn dash_refresh_accepts_rotated_query_but_rejects_path_or_timeline_change() {
        let original = dash("expired", "/segment-7.m4s");
        validate_dash_refresh(&original, &dash("fresh", "/segment-7.m4s"))
            .expect("query-only token refresh");
        assert!(matches!(
            validate_dash_refresh(&original, &dash("fresh", "/segment-8.m4s")),
            Err(RefreshError::ObjectMismatch("resource URL locator"))
        ));
        let mut changed = dash("fresh", "/segment-7.m4s");
        changed.media[0].time += 1;
        assert!(matches!(
            validate_dash_refresh(&original, &changed),
            Err(RefreshError::ObjectMismatch("DASH segment timeline"))
        ));
    }
}
