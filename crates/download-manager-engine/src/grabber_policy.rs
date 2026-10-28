//! Conservative robots, request-spacing, and sensitive-navigation policy for the site grabber.

use std::{collections::HashMap, num::NonZeroU64};

use percent_encoding::percent_decode_str;
use thiserror::Error;
use url::Url;

const DEFAULT_REQUEST_INTERVAL_MILLIS: u64 = 1_000;
const MAX_ROBOTS_BYTES: usize = 512 * 1_024;
const MAX_ROBOTS_LINE_BYTES: usize = 4 * 1_024;
const MAX_ROBOTS_RULES: usize = 4_096;
const MAX_ROBOTS_PATTERN_BYTES: usize = 2 * 1_024;
const MAX_MATCH_TARGET_BYTES: usize = 16 * 1_024;
const MAX_CRAWL_DELAY_MILLIS: u64 = 24 * 60 * 60 * 1_000;

/// How an HTML or CSS URL was discovered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateProvenance {
    Anchor,
    Asset,
    FormAction,
}

/// A safety rule that refused to enqueue a discovered URL.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlBlockReason {
    Robots,
    FormAction,
    SensitiveAction,
}

/// Result of applying the non-rate crawl safety rules to one candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlSafetyDecision {
    Allowed,
    Blocked(CrawlBlockReason),
}

/// Default-on safety configuration for one grabber project.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrawlSafetyConfig {
    pub respect_robots: bool,
    /// `None` is an explicit request to disable request spacing.
    pub minimum_request_interval_millis: Option<NonZeroU64>,
}

impl Default for CrawlSafetyConfig {
    fn default() -> Self {
        Self {
            respect_robots: true,
            minimum_request_interval_millis: NonZeroU64::new(DEFAULT_REQUEST_INTERVAL_MILLIS),
        }
    }
}

/// A deterministic request start reserved by the per-origin rate gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateReservation {
    pub start_at_millis: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum RobotsParseError {
    #[error("robots.txt exceeds the 512 KiB input bound")]
    DocumentTooLarge,
    #[error("robots.txt must be UTF-8")]
    InvalidUtf8,
    #[error("robots.txt contains an oversized line or pattern")]
    LineTooLong,
    #[error("robots.txt exceeds the rule-count bound")]
    TooManyRules,
    #[error("robots.txt product token must be trimmed, non-empty ASCII without whitespace")]
    InvalidProductToken,
    #[error("robots.txt contains an invalid crawl-delay")]
    InvalidCrawlDelay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    Allow,
    Disallow,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RobotsRule {
    kind: RuleKind,
    pattern: String,
    specificity: usize,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct RobotsGroup {
    agents: Vec<String>,
    rules: Vec<RobotsRule>,
    crawl_delay_millis: Option<u64>,
    has_directives: bool,
}

/// Bounded robots rules selected for one crawler product token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RobotsRules {
    rules: Vec<RobotsRule>,
    crawl_delay_millis: Option<u64>,
    deny_everything: bool,
}

impl RobotsRules {
    /// Creates rules that permit every URL.
    #[must_use]
    pub const fn allow_all() -> Self {
        Self {
            rules: Vec::new(),
            crawl_delay_millis: None,
            deny_everything: false,
        }
    }

    /// Creates fail-closed rules, for example after a forbidden or malformed robots response.
    #[must_use]
    pub const fn deny_all() -> Self {
        Self {
            rules: Vec::new(),
            crawl_delay_millis: None,
            deny_everything: true,
        }
    }

    /// Parses a bounded robots document and selects the most-specific matching user-agent groups.
    ///
    /// Directive names and user-agent tokens are ASCII-case-insensitive. URL patterns remain
    /// case-sensitive. Matching supports `*` and terminal `$`; the longest rule wins and `Allow`
    /// wins an equal-specificity tie.
    ///
    /// # Errors
    /// Rejects oversized/non-UTF-8 documents, excessive rules, invalid product tokens, and
    /// malformed `Crawl-delay` values. Callers should fail closed on an error.
    pub fn parse_bounded(document: &[u8], product_token: &str) -> Result<Self, RobotsParseError> {
        validate_product_token(product_token)?;
        if document.len() > MAX_ROBOTS_BYTES {
            return Err(RobotsParseError::DocumentTooLarge);
        }
        let text = std::str::from_utf8(document).map_err(|_| RobotsParseError::InvalidUtf8)?;
        let mut groups = Vec::new();
        let mut current = RobotsGroup::default();
        let mut rule_count = 0_usize;

        for raw_line in text.lines() {
            if raw_line.len() > MAX_ROBOTS_LINE_BYTES {
                return Err(RobotsParseError::LineTooLong);
            }
            let line = raw_line.split('#').next().unwrap_or_default().trim();
            if line.is_empty() {
                continue;
            }
            let Some((name, value)) = line.split_once(':') else {
                continue;
            };
            let name = name.trim();
            let value = value.trim();
            if name.eq_ignore_ascii_case("user-agent") {
                if current.has_directives && !current.agents.is_empty() {
                    groups.push(std::mem::take(&mut current));
                }
                if !value.is_empty() {
                    current.agents.push(value.to_ascii_lowercase());
                }
                continue;
            }
            if current.agents.is_empty() {
                continue;
            }
            if name.eq_ignore_ascii_case("allow") || name.eq_ignore_ascii_case("disallow") {
                current.has_directives = true;
                if value.is_empty() {
                    continue;
                }
                if value.len() > MAX_ROBOTS_PATTERN_BYTES {
                    return Err(RobotsParseError::LineTooLong);
                }
                rule_count += 1;
                if rule_count > MAX_ROBOTS_RULES {
                    return Err(RobotsParseError::TooManyRules);
                }
                current.rules.push(RobotsRule {
                    kind: if name.eq_ignore_ascii_case("allow") {
                        RuleKind::Allow
                    } else {
                        RuleKind::Disallow
                    },
                    specificity: rule_specificity(value),
                    pattern: value.to_owned(),
                });
            } else if name.eq_ignore_ascii_case("crawl-delay") {
                current.has_directives = true;
                let delay = parse_crawl_delay(value)?;
                current.crawl_delay_millis = Some(
                    current
                        .crawl_delay_millis
                        .map_or(delay, |existing| existing.max(delay)),
                );
            }
        }
        if !current.agents.is_empty() {
            groups.push(current);
        }

        Ok(select_groups(&groups, product_token))
    }

    #[must_use]
    pub const fn crawl_delay_millis(&self) -> Option<u64> {
        self.crawl_delay_millis
    }

    /// Applies the selected longest-match rule to an HTTP(S) URL.
    #[must_use]
    pub fn allows(&self, url: &Url) -> bool {
        if self.deny_everything {
            return false;
        }
        let mut target = url.path().to_owned();
        if let Some(query) = url.query() {
            target.push('?');
            target.push_str(query);
        }
        if target.len() > MAX_MATCH_TARGET_BYTES {
            return false;
        }
        let winner = self
            .rules
            .iter()
            .filter(|rule| robots_pattern_matches(&rule.pattern, &target))
            .max_by(|left, right| {
                left.specificity.cmp(&right.specificity).then_with(|| {
                    matches!(left.kind, RuleKind::Allow).cmp(&matches!(right.kind, RuleKind::Allow))
                })
            });
        winner.is_none_or(|rule| matches!(rule.kind, RuleKind::Allow))
    }
}

/// Stateful safety policy for candidate admission and conservative request scheduling.
#[derive(Debug, Clone)]
pub struct CrawlSafetyPolicy {
    config: CrawlSafetyConfig,
    robots: RobotsRules,
    next_request_by_origin: HashMap<String, u64>,
}

impl CrawlSafetyPolicy {
    #[must_use]
    pub fn new(config: CrawlSafetyConfig, robots: RobotsRules) -> Self {
        Self {
            config,
            robots,
            next_request_by_origin: HashMap::new(),
        }
    }

    #[must_use]
    pub const fn config(&self) -> CrawlSafetyConfig {
        self.config
    }

    /// Rejects forms, credential/destructive endpoints, and robots-disallowed URLs in that order.
    #[must_use]
    pub fn evaluate_candidate(
        &self,
        url: &Url,
        provenance: CandidateProvenance,
    ) -> CrawlSafetyDecision {
        if provenance == CandidateProvenance::FormAction {
            return CrawlSafetyDecision::Blocked(CrawlBlockReason::FormAction);
        }
        if is_sensitive_action_url(url) {
            return CrawlSafetyDecision::Blocked(CrawlBlockReason::SensitiveAction);
        }
        if self.config.respect_robots && !self.robots.allows(url) {
            return CrawlSafetyDecision::Blocked(CrawlBlockReason::Robots);
        }
        CrawlSafetyDecision::Allowed
    }

    /// Reserves a deterministic request start for the URL's normalized origin.
    ///
    /// The configured interval and robots `Crawl-delay` are combined using the stricter value.
    /// Callers wait until `start_at_millis` before starting I/O. Saturation fails safe by reserving
    /// `u64::MAX` rather than wrapping time backwards.
    #[must_use]
    pub fn reserve_request(&mut self, url: &Url, now_millis: u64) -> RateReservation {
        let configured = self
            .config
            .minimum_request_interval_millis
            .map(NonZeroU64::get);
        let interval = match (configured, self.robots.crawl_delay_millis()) {
            (Some(configured), Some(robots)) => Some(configured.max(robots)),
            (Some(configured), None) => Some(configured),
            (None, Some(robots)) if self.config.respect_robots => Some(robots),
            (None, Some(_) | None) => None,
        };
        let Some(interval) = interval else {
            return RateReservation {
                start_at_millis: now_millis,
            };
        };
        let origin = url.origin().ascii_serialization();
        let next = self
            .next_request_by_origin
            .entry(origin)
            .or_insert(now_millis);
        let start_at_millis = now_millis.max(*next);
        *next = start_at_millis.saturating_add(interval);
        RateReservation { start_at_millis }
    }
}

fn validate_product_token(product_token: &str) -> Result<(), RobotsParseError> {
    if product_token.is_empty()
        || product_token.trim() != product_token
        || !product_token.is_ascii()
        || product_token.bytes().any(|byte| byte.is_ascii_whitespace())
    {
        Err(RobotsParseError::InvalidProductToken)
    } else {
        Ok(())
    }
}

fn parse_crawl_delay(value: &str) -> Result<u64, RobotsParseError> {
    let (whole, fraction) = value
        .split_once('.')
        .map_or((value, None), |(whole, fraction)| (whole, Some(fraction)));
    if whole.is_empty()
        || !whole.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.is_some_and(|digits| {
            digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(RobotsParseError::InvalidCrawlDelay);
    }
    let seconds = whole
        .parse::<u64>()
        .map_err(|_| RobotsParseError::InvalidCrawlDelay)?;
    let mut fractional_millis = 0_u64;
    if let Some(digits) = fraction {
        for byte in digits.bytes().take(3) {
            fractional_millis = fractional_millis * 10 + u64::from(byte - b'0');
        }
        for _ in digits.len().min(3)..3 {
            fractional_millis *= 10;
        }
        if digits.bytes().skip(3).any(|byte| byte != b'0') {
            fractional_millis += 1;
        }
    }
    let millis = seconds
        .checked_mul(1_000)
        .and_then(|value| value.checked_add(fractional_millis))
        .ok_or(RobotsParseError::InvalidCrawlDelay)?;
    if millis > MAX_CRAWL_DELAY_MILLIS {
        return Err(RobotsParseError::InvalidCrawlDelay);
    }
    Ok(millis)
}

fn select_groups(groups: &[RobotsGroup], product_token: &str) -> RobotsRules {
    let product = product_token.to_ascii_lowercase();
    let best_specificity = groups
        .iter()
        .filter_map(|group| group_specificity(group, &product))
        .max();
    let Some(best_specificity) = best_specificity else {
        return RobotsRules::allow_all();
    };
    let mut rules = Vec::new();
    let mut crawl_delay_millis = None;
    for group in groups
        .iter()
        .filter(|group| group_specificity(group, &product) == Some(best_specificity))
    {
        rules.extend(group.rules.iter().cloned());
        if let Some(delay) = group.crawl_delay_millis {
            crawl_delay_millis =
                Some(crawl_delay_millis.map_or(delay, |existing: u64| existing.max(delay)));
        }
    }
    RobotsRules {
        rules,
        crawl_delay_millis,
        deny_everything: false,
    }
}

fn group_specificity(group: &RobotsGroup, product: &str) -> Option<usize> {
    group
        .agents
        .iter()
        .filter_map(|agent| {
            if agent == "*" {
                Some(0)
            } else if product.starts_with(agent) {
                Some(agent.len())
            } else {
                None
            }
        })
        .max()
}

fn rule_specificity(pattern: &str) -> usize {
    pattern
        .as_bytes()
        .iter()
        .filter(|byte| !matches!(byte, b'*' | b'$'))
        .count()
}

fn robots_pattern_matches(pattern: &str, target: &str) -> bool {
    let anchored_end = pattern.ends_with('$');
    let pattern = pattern.strip_suffix('$').unwrap_or(pattern).as_bytes();
    let target = target.as_bytes();
    let mut pattern_index = 0_usize;
    let mut target_index = 0_usize;
    let mut last_star = None;
    let mut star_target_index = 0_usize;

    loop {
        if pattern_index == pattern.len() {
            return !anchored_end || target_index == target.len();
        }
        if pattern[pattern_index] == b'*' {
            last_star = Some(pattern_index);
            pattern_index += 1;
            star_target_index = target_index;
            continue;
        }
        if target_index < target.len() && pattern[pattern_index] == target[target_index] {
            pattern_index += 1;
            target_index += 1;
            continue;
        }
        if let Some(star_index) = last_star
            && star_target_index < target.len()
        {
            star_target_index += 1;
            target_index = star_target_index;
            pattern_index = star_index + 1;
            continue;
        }
        return false;
    }
}

fn is_sensitive_action_url(url: &Url) -> bool {
    if url.path_segments().is_some_and(|segments| {
        segments.into_iter().any(|segment| {
            let decoded = percent_decode_str(segment).decode_utf8_lossy();
            is_sensitive_token(decoded.as_ref())
        })
    }) {
        return true;
    }
    url.query_pairs().any(|(key, value)| {
        is_sensitive_token(key.as_ref())
            || (is_action_query_key(key.as_ref()) && is_sensitive_token(value.as_ref()))
    })
}

fn is_sensitive_token(value: &str) -> bool {
    let lower = value.to_ascii_lowercase();
    let base = lower
        .strip_suffix(".php")
        .or_else(|| lower.strip_suffix(".asp"))
        .or_else(|| lower.strip_suffix(".aspx"))
        .unwrap_or(&lower);
    matches!(
        base,
        "login"
            | "log-in"
            | "signin"
            | "sign-in"
            | "logout"
            | "log-out"
            | "signout"
            | "sign-out"
            | "delete"
            | "remove"
            | "destroy"
    )
}

fn is_action_query_key(value: &str) -> bool {
    matches!(
        value.to_ascii_lowercase().as_str(),
        "action" | "do" | "cmd" | "operation"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(value: &str) -> Url {
        Url::parse(value).expect("fixture URL")
    }

    #[test]
    fn robots_uses_exact_agent_longest_rule_allow_tie_and_wildcards() {
        let rules = RobotsRules::parse_bounded(
            br"
                User-agent: *
                Disallow: /

                User-agent: Download-Manager
                Disallow: /private
                Allow: /private/public
                Disallow: /private/public
                Disallow: /*.zip$
                Crawl-delay: 1.5
            ",
            "download-manager",
        )
        .expect("robots rules");

        assert!(!rules.allows(&url("https://example.test/private/item")));
        assert!(rules.allows(&url("https://example.test/private/public")));
        assert!(rules.allows(&url("https://example.test/private/public/more")));
        assert!(!rules.allows(&url("https://example.test/files/archive.zip")));
        assert!(rules.allows(&url("https://example.test/files/archive.zip?mirror=1")));
        assert!(rules.allows(&url("https://example.test/safe")));
        assert_eq!(rules.crawl_delay_millis(), Some(1_500));
    }

    #[test]
    fn policy_blocks_forms_sensitive_actions_and_robots_with_stable_reasons() {
        let robots =
            RobotsRules::parse_bounded(b"User-agent: *\nDisallow: /private\n", "download-manager")
                .expect("robots rules");
        let policy = CrawlSafetyPolicy::new(CrawlSafetyConfig::default(), robots);

        let cases = [
            (
                "https://example.test/search",
                CandidateProvenance::FormAction,
                CrawlSafetyDecision::Blocked(CrawlBlockReason::FormAction),
            ),
            (
                "https://example.test/account/login.php",
                CandidateProvenance::Anchor,
                CrawlSafetyDecision::Blocked(CrawlBlockReason::SensitiveAction),
            ),
            (
                "https://example.test/account/%6Cogout",
                CandidateProvenance::Anchor,
                CrawlSafetyDecision::Blocked(CrawlBlockReason::SensitiveAction),
            ),
            (
                "https://example.test/account?ACTION=delete",
                CandidateProvenance::Anchor,
                CrawlSafetyDecision::Blocked(CrawlBlockReason::SensitiveAction),
            ),
            (
                "https://example.test/private/report",
                CandidateProvenance::Asset,
                CrawlSafetyDecision::Blocked(CrawlBlockReason::Robots),
            ),
            (
                "https://example.test/articles/deletion-policy",
                CandidateProvenance::Anchor,
                CrawlSafetyDecision::Allowed,
            ),
            (
                "https://example.test/assets/remove-background.png",
                CandidateProvenance::Asset,
                CrawlSafetyDecision::Allowed,
            ),
        ];
        for (candidate, provenance, expected) in cases {
            assert_eq!(
                policy.evaluate_candidate(&url(candidate), provenance),
                expected,
                "candidate {candidate}"
            );
        }
    }

    #[test]
    fn conservative_default_and_virtual_rate_reservations_are_deterministic() {
        let config = CrawlSafetyConfig::default();
        assert!(config.respect_robots);
        assert_eq!(
            config.minimum_request_interval_millis.map(NonZeroU64::get),
            Some(1_000)
        );
        let robots =
            RobotsRules::parse_bounded(b"User-agent: *\nCrawl-delay: 1.5\n", "download-manager")
                .expect("robots rules");
        let mut policy = CrawlSafetyPolicy::new(config, robots);
        let first = policy.reserve_request(&url("https://EXAMPLE.test/a"), 0);
        let second = policy.reserve_request(&url("https://example.test/b"), 0);
        let third = policy.reserve_request(&url("https://example.test/c"), 200);
        let other_origin = policy.reserve_request(&url("https://cdn.example.test/a"), 200);

        assert_eq!(first.start_at_millis, 0);
        assert_eq!(second.start_at_millis, 1_500);
        assert_eq!(third.start_at_millis, 3_000);
        assert_eq!(other_origin.start_at_millis, 200);
    }

    #[test]
    fn robots_bounds_and_malformed_delay_fail_closed_without_panicking() {
        assert_eq!(parse_crawl_delay("0.0001"), Ok(1));
        assert_eq!(parse_crawl_delay("1.5000"), Ok(1_500));
        assert_eq!(
            RobotsRules::parse_bounded(&vec![b'a'; MAX_ROBOTS_BYTES + 1], "download-manager"),
            Err(RobotsParseError::DocumentTooLarge)
        );
        assert_eq!(
            RobotsRules::parse_bounded(
                b"User-agent: *\nCrawl-delay: forever\n",
                "download-manager"
            ),
            Err(RobotsParseError::InvalidCrawlDelay)
        );
        assert_eq!(
            parse_crawl_delay("1.2.3"),
            Err(RobotsParseError::InvalidCrawlDelay)
        );
        assert!(!RobotsRules::deny_all().allows(&url("https://example.test/")));
    }

    #[test]
    fn robots_can_be_explicitly_ignored_and_rate_spacing_explicitly_disabled() {
        let config = CrawlSafetyConfig {
            respect_robots: false,
            minimum_request_interval_millis: None,
        };
        let mut policy = CrawlSafetyPolicy::new(config, RobotsRules::deny_all());
        let candidate = url("https://example.test/private");
        assert_eq!(
            policy.evaluate_candidate(&candidate, CandidateProvenance::Anchor),
            CrawlSafetyDecision::Allowed
        );
        assert_eq!(
            policy.reserve_request(&candidate, 42),
            RateReservation {
                start_at_millis: 42
            }
        );
        assert_eq!(
            policy.reserve_request(&candidate, 42),
            RateReservation {
                start_at_millis: 42
            }
        );
    }
}
