//! Ordered URL filters and resource-type presets for grabber discoveries.

use thiserror::Error;
use url::Url;

/// Maximum number of explicit rules in one filter table.
pub const MAX_GRABBER_FILTER_RULES: usize = 128;
/// Maximum UTF-8 byte length of one URL glob.
pub const MAX_GRABBER_FILTER_PATTERN_BYTES: usize = 1_024;

/// Built-in resource selections used when no explicit rule matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabberPreset {
    AllSite,
    Images,
    Documents,
    Media,
}

/// Result applied by a matching explicit rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabberFilterAction {
    Include,
    Exclude,
}

/// Validation errors for a bounded filter table.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum GrabberFilterError {
    #[error("grabber filter pattern must not be empty")]
    EmptyPattern,
    #[error("grabber filter pattern exceeds {MAX_GRABBER_FILTER_PATTERN_BYTES} bytes")]
    PatternTooLong,
    #[error("grabber filter table exceeds {MAX_GRABBER_FILTER_RULES} rules")]
    TooManyRules,
}

/// One `*`/`?` URL glob and its decision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabberFilterRule {
    action: GrabberFilterAction,
    pattern: String,
}

impl GrabberFilterRule {
    /// Creates a bounded rule. `*` matches zero or more bytes and `?` matches one byte.
    ///
    /// # Errors
    /// Returns an error when the trimmed pattern is empty or too long.
    pub fn new(
        action: GrabberFilterAction,
        pattern: impl Into<String>,
    ) -> Result<Self, GrabberFilterError> {
        let pattern = pattern.into().trim().to_owned();
        if pattern.is_empty() {
            return Err(GrabberFilterError::EmptyPattern);
        }
        if pattern.len() > MAX_GRABBER_FILTER_PATTERN_BYTES {
            return Err(GrabberFilterError::PatternTooLong);
        }
        Ok(Self { action, pattern })
    }

    #[must_use]
    pub const fn action(&self) -> GrabberFilterAction {
        self.action
    }

    #[must_use]
    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

/// Evidence explaining which part of the table produced a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrabberFilterSource {
    Rule(usize),
    Preset(GrabberPreset),
}

/// An explainable filter result suitable for a later preview UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrabberFilterResult {
    pub included: bool,
    pub source: GrabberFilterSource,
}

/// First-match-wins URL rules with a resource-type preset fallback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrabberFilter {
    preset: GrabberPreset,
    rules: Vec<GrabberFilterRule>,
}

impl GrabberFilter {
    /// Creates a bounded ordered table.
    ///
    /// # Errors
    /// Returns an error when the table contains too many rules.
    pub fn new(
        preset: GrabberPreset,
        rules: Vec<GrabberFilterRule>,
    ) -> Result<Self, GrabberFilterError> {
        if rules.len() > MAX_GRABBER_FILTER_RULES {
            return Err(GrabberFilterError::TooManyRules);
        }
        Ok(Self { preset, rules })
    }

    /// Evaluates rules in declaration order, then falls back to the selected type preset.
    ///
    /// Explicit URL globs are case-sensitive because URL paths can be case-sensitive. Preset MIME
    /// and extension matching is ASCII-case-insensitive. MIME parameters are ignored.
    #[must_use]
    pub fn evaluate(&self, url: &Url, mime_type: Option<&str>) -> GrabberFilterResult {
        if let Some((index, rule)) =
            self.rules.iter().enumerate().find(|(_, rule)| {
                wildcard_matches(rule.pattern.as_bytes(), url.as_str().as_bytes())
            })
        {
            return GrabberFilterResult {
                included: rule.action == GrabberFilterAction::Include,
                source: GrabberFilterSource::Rule(index),
            };
        }

        GrabberFilterResult {
            included: preset_matches(self.preset, url, mime_type),
            source: GrabberFilterSource::Preset(self.preset),
        }
    }
}

fn preset_matches(preset: GrabberPreset, url: &Url, mime_type: Option<&str>) -> bool {
    if preset == GrabberPreset::AllSite {
        return true;
    }
    let mime_type = mime_type
        .and_then(|value| value.split(';').next())
        .map(str::trim)
        .filter(|value| !value.is_empty());
    let extension = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .and_then(|name| name.rsplit_once('.').map(|(_, extension)| extension))
        .filter(|extension| !extension.is_empty());

    match preset {
        GrabberPreset::AllSite => true,
        GrabberPreset::Images => {
            mime_type.is_some_and(|value| value.to_ascii_lowercase().starts_with("image/"))
                || extension.is_some_and(|value| {
                    matches_ascii_case_insensitive(
                        value,
                        &[
                            "jpg", "jpeg", "png", "gif", "webp", "svg", "avif", "bmp", "ico",
                        ],
                    )
                })
        }
        GrabberPreset::Documents => {
            mime_type.is_some_and(is_document_mime)
                || extension.is_some_and(|value| {
                    matches_ascii_case_insensitive(
                        value,
                        &[
                            "pdf", "doc", "docx", "xls", "xlsx", "ppt", "pptx", "odt", "ods",
                            "odp", "rtf", "txt", "csv",
                        ],
                    )
                })
        }
        GrabberPreset::Media => {
            mime_type.is_some_and(is_media_mime)
                || extension.is_some_and(|value| {
                    matches_ascii_case_insensitive(
                        value,
                        &[
                            "mp4", "m4v", "webm", "mkv", "mov", "avi", "mp3", "m4a", "aac", "ogg",
                            "opus", "wav", "flac", "m3u8", "mpd",
                        ],
                    )
                })
        }
    }
}

fn is_document_mime(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "application/pdf"
            | "application/msword"
            | "application/rtf"
            | "application/vnd.ms-excel"
            | "application/vnd.ms-powerpoint"
            | "application/vnd.oasis.opendocument.presentation"
            | "application/vnd.oasis.opendocument.spreadsheet"
            | "application/vnd.oasis.opendocument.text"
            | "application/vnd.openxmlformats-officedocument.presentationml.presentation"
            | "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet"
            | "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
            | "text/csv"
            | "text/plain"
            | "text/rtf"
    )
}

fn is_media_mime(value: &str) -> bool {
    let value = value.to_ascii_lowercase();
    value.starts_with("audio/")
        || value.starts_with("video/")
        || matches!(
            value.as_str(),
            "application/dash+xml" | "application/vnd.apple.mpegurl" | "application/x-mpegurl"
        )
}

fn matches_ascii_case_insensitive(candidate: &str, expected: &[&str]) -> bool {
    expected
        .iter()
        .any(|value| candidate.eq_ignore_ascii_case(value))
}

fn wildcard_matches(pattern: &[u8], value: &[u8]) -> bool {
    let (mut pattern_index, mut value_index) = (0, 0);
    let (mut star_index, mut retry_value_index) = (None, 0);
    while value_index < value.len() {
        if pattern_index < pattern.len()
            && (pattern[pattern_index] == b'?' || pattern[pattern_index] == value[value_index])
        {
            pattern_index += 1;
            value_index += 1;
        } else if pattern_index < pattern.len() && pattern[pattern_index] == b'*' {
            star_index = Some(pattern_index);
            pattern_index += 1;
            retry_value_index = value_index;
        } else if let Some(star) = star_index {
            pattern_index = star + 1;
            retry_value_index += 1;
            value_index = retry_value_index;
        } else {
            return false;
        }
    }
    pattern[pattern_index..].iter().all(|byte| *byte == b'*')
}

#[cfg(test)]
mod tests {
    use super::{
        GrabberFilter, GrabberFilterAction, GrabberFilterError, GrabberFilterRule,
        GrabberFilterSource, GrabberPreset, MAX_GRABBER_FILTER_PATTERN_BYTES,
        MAX_GRABBER_FILTER_RULES,
    };
    use url::Url;

    fn rule(action: GrabberFilterAction, pattern: &str) -> GrabberFilterRule {
        GrabberFilterRule::new(action, pattern).expect("valid rule")
    }

    fn url(value: &str) -> Url {
        Url::parse(value).expect("valid URL")
    }

    #[test]
    fn ordered_rules_use_first_match_and_report_the_rule_index() {
        let private_first = GrabberFilter::new(
            GrabberPreset::AllSite,
            vec![
                rule(GrabberFilterAction::Exclude, "*/private/*"),
                rule(GrabberFilterAction::Include, "*/private/public/*"),
            ],
        )
        .expect("filter");
        let public_first = GrabberFilter::new(
            GrabberPreset::AllSite,
            vec![
                rule(GrabberFilterAction::Include, "*/private/public/*"),
                rule(GrabberFilterAction::Exclude, "*/private/*"),
            ],
        )
        .expect("filter");
        let candidate = url("https://example.test/private/public/report.pdf");

        assert!(!private_first.evaluate(&candidate, None).included);
        assert_eq!(
            private_first.evaluate(&candidate, None).source,
            GrabberFilterSource::Rule(0)
        );
        assert!(public_first.evaluate(&candidate, None).included);
    }

    #[test]
    fn explicit_include_can_override_a_type_preset() {
        let filter = GrabberFilter::new(
            GrabberPreset::Documents,
            vec![rule(GrabberFilterAction::Include, "*.webp")],
        )
        .expect("filter");
        let result = filter.evaluate(&url("https://example.test/banner.webp"), None);
        assert!(result.included);
        assert_eq!(result.source, GrabberFilterSource::Rule(0));
    }

    #[test]
    fn preset_table_normalizes_mime_extension_and_query_only() {
        let cases = [
            (
                GrabberPreset::Images,
                "https://e.test/P.JPG?x=.pdf",
                None,
                true,
            ),
            (
                GrabberPreset::Images,
                "https://e.test/blob",
                Some("IMAGE/AVIF; charset=binary"),
                true,
            ),
            (
                GrabberPreset::Documents,
                "https://e.test/a.DOCX?download=1",
                None,
                true,
            ),
            (
                GrabberPreset::Documents,
                "https://e.test/blob",
                Some("application/pdf; version=1.7"),
                true,
            ),
            (
                GrabberPreset::Media,
                "https://e.test/live.M3U8#part",
                None,
                true,
            ),
            (
                GrabberPreset::Media,
                "https://e.test/manifest",
                Some("application/dash+xml"),
                true,
            ),
            (
                GrabberPreset::Media,
                "https://e.test/archive.bin",
                None,
                false,
            ),
            (
                GrabberPreset::AllSite,
                "https://e.test/archive.bin",
                None,
                true,
            ),
        ];
        for (preset, candidate, mime_type, expected) in cases {
            let filter = GrabberFilter::new(preset, Vec::new()).expect("filter");
            let result = filter.evaluate(&url(candidate), mime_type);
            assert_eq!(result.included, expected, "{preset:?} {candidate}");
            assert_eq!(result.source, GrabberFilterSource::Preset(preset));
        }
    }

    #[test]
    fn url_globs_are_full_url_case_sensitive_and_support_question_mark() {
        let filter = GrabberFilter::new(
            GrabberPreset::Images,
            vec![rule(
                GrabberFilterAction::Include,
                "https://example.test/Files/report-?.PDF",
            )],
        )
        .expect("filter");
        assert!(
            filter
                .evaluate(&url("https://example.test/Files/report-1.PDF"), None)
                .included
        );
        assert!(
            !filter
                .evaluate(&url("https://example.test/files/report-1.PDF"), None)
                .included
        );
    }

    #[test]
    fn invalid_patterns_and_oversized_tables_are_rejected() {
        assert_eq!(
            GrabberFilterRule::new(GrabberFilterAction::Include, "  "),
            Err(GrabberFilterError::EmptyPattern)
        );
        assert_eq!(
            GrabberFilterRule::new(
                GrabberFilterAction::Include,
                "x".repeat(MAX_GRABBER_FILTER_PATTERN_BYTES + 1),
            ),
            Err(GrabberFilterError::PatternTooLong)
        );
        let rules = (0..=MAX_GRABBER_FILTER_RULES)
            .map(|_| rule(GrabberFilterAction::Include, "*"))
            .collect();
        assert_eq!(
            GrabberFilter::new(GrabberPreset::AllSite, rules),
            Err(GrabberFilterError::TooManyRules)
        );
    }
}
