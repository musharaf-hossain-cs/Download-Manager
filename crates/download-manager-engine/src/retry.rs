//! Bounded retry classification, exponential backoff, jitter, and `Retry-After` handling.

use std::time::{Duration, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetryPolicy {
    pub maximum_attempts: u32,
    pub base_delay: Duration,
    pub maximum_delay: Duration,
    /// Symmetric jitter in thousandths; `200` means ±20%.
    pub jitter_per_mille: u16,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            maximum_attempts: 5,
            base_delay: Duration::from_secs(1),
            maximum_delay: Duration::from_secs(60),
            jitter_per_mille: 200,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetryDecision {
    RetryAfter(Duration),
    Stop,
}

impl RetryPolicy {
    /// Returns deterministic exponential backoff for a one-based retry attempt.
    #[must_use]
    pub fn backoff(self, attempt: u32, seed: u64) -> Duration {
        let exponent = attempt.saturating_sub(1).min(31);
        let multiplier = 1_u32 << exponent;
        let unjittered = self
            .base_delay
            .saturating_mul(multiplier)
            .min(self.maximum_delay);
        apply_jitter(
            unjittered,
            self.jitter_per_mille,
            mix(seed ^ u64::from(attempt)),
        )
        .min(self.maximum_delay)
    }

    /// Classifies an HTTP status and applies the retry budget and optional `Retry-After` value.
    #[must_use]
    pub fn decide_http(
        self,
        status: u16,
        retry_after: Option<&str>,
        attempt: u32,
        now: SystemTime,
        seed: u64,
    ) -> RetryDecision {
        if attempt == 0 || attempt > self.maximum_attempts || !is_retryable_status(status) {
            return RetryDecision::Stop;
        }
        let delay = retry_after
            .and_then(|value| parse_retry_after(value, now))
            .unwrap_or_else(|| self.backoff(attempt, seed))
            .min(self.maximum_delay);
        RetryDecision::RetryAfter(delay)
    }
}

#[must_use]
pub const fn is_retryable_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500..=599)
}

/// Parses either delta-seconds or an IMF-fixdate relative to the supplied clock.
#[must_use]
pub fn parse_retry_after(value: &str, now: SystemTime) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let target = httpdate::parse_http_date(value).ok()?;
    Some(target.duration_since(now).unwrap_or(Duration::ZERO))
}

fn apply_jitter(delay: Duration, per_mille: u16, random: u64) -> Duration {
    if per_mille == 0 || delay.is_zero() {
        return delay;
    }
    let span = u64::from(per_mille) * 2 + 1;
    let signed = i128::from(random % span) - i128::from(per_mille);
    let millis = i128::try_from(delay.as_millis()).unwrap_or(i128::MAX);
    let adjusted = millis.saturating_add(millis.saturating_mul(signed) / 1_000);
    Duration::from_millis(u64::try_from(adjusted.max(0)).unwrap_or(u64::MAX))
}

const fn mix(mut value: u64) -> u64 {
    value ^= value >> 12;
    value ^= value << 25;
    value ^= value >> 27;
    value.wrapping_mul(2_685_821_657_736_338_717)
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use super::{RetryDecision, RetryPolicy, is_retryable_status, parse_retry_after};

    #[test]
    fn deterministic_backoff_has_bounded_jitter_and_budget() {
        let policy = RetryPolicy::default();
        for attempt in 1..=policy.maximum_attempts {
            let first = policy.backoff(attempt, 42);
            let repeated = policy.backoff(attempt, 42);
            assert_eq!(first, repeated);
            let base = Duration::from_secs(1_u64 << (attempt - 1));
            assert!(first >= base.mul_f64(0.8));
            assert!(first <= base.mul_f64(1.2));
            assert!(first <= policy.maximum_delay);
        }
        assert_eq!(
            policy.decide_http(503, None, 6, SystemTime::UNIX_EPOCH, 42),
            RetryDecision::Stop
        );
        assert_eq!(
            policy.decide_http(404, None, 1, SystemTime::UNIX_EPOCH, 42),
            RetryDecision::Stop
        );
    }

    #[test]
    fn retry_after_seconds_date_malformed_and_status_mapping() {
        let now = SystemTime::UNIX_EPOCH + Duration::from_secs(1_000_000);
        assert_eq!(parse_retry_after("12", now), Some(Duration::from_secs(12)));
        let date = httpdate::fmt_http_date(now + Duration::from_secs(30));
        assert_eq!(parse_retry_after(&date, now), Some(Duration::from_secs(30)));
        assert_eq!(parse_retry_after("not-a-date", now), None);

        for status in [408, 425, 429, 500, 503, 599] {
            assert!(is_retryable_status(status));
            assert_eq!(
                RetryPolicy::default().decide_http(status, Some("7"), 1, now, 1),
                RetryDecision::RetryAfter(Duration::from_secs(7))
            );
        }
        for status in [200, 400, 401, 404, 600] {
            assert!(!is_retryable_status(status));
        }
        let fallback = RetryPolicy::default().decide_http(429, Some("bad"), 1, now, 9);
        assert!(matches!(fallback, RetryDecision::RetryAfter(delay) if delay > Duration::ZERO));
    }
}
