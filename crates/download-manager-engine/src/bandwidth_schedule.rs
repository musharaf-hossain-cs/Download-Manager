//! Time-zone-aware global and per-queue bandwidth windows and quiet hours.

use std::num::NonZeroU64;

use chrono::{DateTime, Datelike, NaiveTime, Utc, Weekday};
use chrono_tz::Tz;
use thiserror::Error;

const MAX_QUEUE_NAME_LENGTH: usize = 256;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum BandwidthScheduleError {
    #[error("unknown IANA time zone: {0}")]
    InvalidTimeZone(String),
    #[error("a scheduled bandwidth rule must select at least one weekday")]
    EmptyWeekdays,
    #[error("bandwidth window start and end must differ")]
    EmptyWindow,
    #[error("queue name must be trimmed, non-empty, and at most 256 characters")]
    InvalidQueueName,
    #[error("a scheduled rule must set a bandwidth limit, quiet hours, or both")]
    NoEffect,
}

/// The downloads affected by one scheduled rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BandwidthScope {
    Global,
    Queue(String),
}

/// One recurring local-time bandwidth/quiet-hours window.
///
/// `start` is inclusive and `end` is exclusive. When `end` is earlier than `start`, the window
/// crosses midnight and its after-midnight portion belongs to the selected start weekday.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledBandwidthRule {
    pub scope: BandwidthScope,
    pub weekdays: Vec<Weekday>,
    pub start: NaiveTime,
    pub end: NaiveTime,
    pub bytes_per_second: Option<NonZeroU64>,
    pub quiet: bool,
}

/// Effective policy at one instant. `None` means unlimited bandwidth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScheduledBandwidthDecision {
    pub bytes_per_second: Option<u64>,
    pub quiet: bool,
}

/// A validated weekly schedule evaluated in one IANA time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduledBandwidthPolicy {
    time_zone: Tz,
    rules: Vec<ScheduledBandwidthRule>,
}

impl ScheduledBandwidthPolicy {
    /// Creates a policy, normalizing duplicate weekdays in each rule.
    ///
    /// # Errors
    /// Returns an error for an unknown time zone or a malformed/no-op rule.
    pub fn new(
        time_zone: &str,
        mut rules: Vec<ScheduledBandwidthRule>,
    ) -> Result<Self, BandwidthScheduleError> {
        let time_zone = time_zone
            .parse::<Tz>()
            .map_err(|_| BandwidthScheduleError::InvalidTimeZone(time_zone.to_owned()))?;
        for rule in &mut rules {
            rule.weekdays
                .sort_unstable_by_key(Weekday::num_days_from_monday);
            rule.weekdays.dedup();
            if rule.weekdays.is_empty() {
                return Err(BandwidthScheduleError::EmptyWeekdays);
            }
            if rule.start == rule.end {
                return Err(BandwidthScheduleError::EmptyWindow);
            }
            if rule.bytes_per_second.is_none() && !rule.quiet {
                return Err(BandwidthScheduleError::NoEffect);
            }
            if let BandwidthScope::Queue(queue_name) = &rule.scope
                && !valid_queue_name(queue_name)
            {
                return Err(BandwidthScheduleError::InvalidQueueName);
            }
        }
        Ok(Self { time_zone, rules })
    }

    /// Resolves the active global rules plus rules for the named queue.
    ///
    /// Overlapping limits use the lowest rate. Quiet mode is active if any applicable rule is
    /// quiet. Passing `None` evaluates only global rules.
    #[must_use]
    pub fn decision_at(
        &self,
        now: DateTime<Utc>,
        queue_name: Option<&str>,
    ) -> ScheduledBandwidthDecision {
        let local = now.with_timezone(&self.time_zone);
        let mut decision = ScheduledBandwidthDecision {
            bytes_per_second: None,
            quiet: false,
        };
        for rule in &self.rules {
            if !scope_applies(&rule.scope, queue_name)
                || !window_contains(rule, local.weekday(), local.time())
            {
                continue;
            }
            decision.quiet |= rule.quiet;
            if let Some(limit) = rule.bytes_per_second.map(NonZeroU64::get) {
                decision.bytes_per_second = Some(
                    decision
                        .bytes_per_second
                        .map_or(limit, |active| active.min(limit)),
                );
            }
        }
        decision
    }
}

fn valid_queue_name(queue_name: &str) -> bool {
    !queue_name.is_empty()
        && queue_name.trim() == queue_name
        && queue_name.chars().count() <= MAX_QUEUE_NAME_LENGTH
        && !queue_name.chars().any(char::is_control)
}

fn scope_applies(scope: &BandwidthScope, queue_name: Option<&str>) -> bool {
    match scope {
        BandwidthScope::Global => true,
        BandwidthScope::Queue(rule_queue) => queue_name == Some(rule_queue.as_str()),
    }
}

fn window_contains(rule: &ScheduledBandwidthRule, weekday: Weekday, time: NaiveTime) -> bool {
    if rule.start < rule.end {
        rule.weekdays.contains(&weekday) && rule.start <= time && time < rule.end
    } else if time >= rule.start {
        rule.weekdays.contains(&weekday)
    } else {
        time < rule.end && rule.weekdays.contains(&weekday.pred())
    }
}

#[cfg(test)]
mod tests {
    use std::num::NonZeroU64;

    use chrono::{DateTime, NaiveTime, Utc, Weekday};

    use super::{
        BandwidthScheduleError, BandwidthScope, ScheduledBandwidthDecision,
        ScheduledBandwidthPolicy, ScheduledBandwidthRule,
    };

    #[test]
    fn daytime_window_is_start_inclusive_and_end_exclusive() {
        let policy = policy(vec![rule(
            BandwidthScope::Global,
            &[Weekday::Mon],
            "09:00",
            "17:00",
            Some(1_000),
            true,
        )]);

        assert_eq!(
            policy.decision_at(utc("2026-10-26T08:59:59Z"), Some("Nightly")),
            unrestricted()
        );
        assert_eq!(
            policy.decision_at(utc("2026-10-26T09:00:00Z"), Some("Nightly")),
            ScheduledBandwidthDecision {
                bytes_per_second: Some(1_000),
                quiet: true,
            }
        );
        assert_eq!(
            policy.decision_at(utc("2026-10-26T16:59:59Z"), None),
            ScheduledBandwidthDecision {
                bytes_per_second: Some(1_000),
                quiet: true,
            }
        );
        assert_eq!(
            policy.decision_at(utc("2026-10-26T17:00:00Z"), None),
            unrestricted()
        );
    }

    #[test]
    fn overnight_window_uses_its_start_weekday_on_both_sides_of_midnight() {
        let policy = policy(vec![rule(
            BandwidthScope::Global,
            &[Weekday::Mon],
            "22:00",
            "06:00",
            None,
            true,
        )]);

        assert!(!policy.decision_at(utc("2026-10-26T21:59:59Z"), None).quiet);
        assert!(policy.decision_at(utc("2026-10-26T22:00:00Z"), None).quiet);
        assert!(policy.decision_at(utc("2026-10-27T05:59:59Z"), None).quiet);
        assert!(!policy.decision_at(utc("2026-10-27T06:00:00Z"), None).quiet);
        assert!(!policy.decision_at(utc("2026-10-28T05:00:00Z"), None).quiet);
    }

    #[test]
    fn exact_queue_rule_combines_with_global_rule_using_the_lower_limit() {
        let policy = policy(vec![
            rule(
                BandwidthScope::Global,
                &[Weekday::Mon],
                "08:00",
                "18:00",
                Some(4_000),
                false,
            ),
            rule(
                BandwidthScope::Queue("Nightly".to_owned()),
                &[Weekday::Mon],
                "10:00",
                "12:00",
                Some(2_000),
                true,
            ),
        ]);
        let now = utc("2026-10-26T10:30:00Z");

        assert_eq!(
            policy.decision_at(now, Some("Nightly")),
            ScheduledBandwidthDecision {
                bytes_per_second: Some(2_000),
                quiet: true,
            }
        );
        assert_eq!(
            policy.decision_at(now, Some("Interactive")),
            ScheduledBandwidthDecision {
                bytes_per_second: Some(4_000),
                quiet: false,
            }
        );
        assert_eq!(
            policy.decision_at(now, None),
            ScheduledBandwidthDecision {
                bytes_per_second: Some(4_000),
                quiet: false,
            }
        );
    }

    #[test]
    fn policy_uses_local_time_and_rejects_ambiguous_configuration() {
        let dhaka = ScheduledBandwidthPolicy::new(
            "Asia/Dhaka",
            vec![rule(
                BandwidthScope::Global,
                &[Weekday::Mon],
                "08:00",
                "09:00",
                Some(1),
                false,
            )],
        )
        .expect("Dhaka policy");
        assert_eq!(
            dhaka
                .decision_at(utc("2026-10-26T02:00:00Z"), None)
                .bytes_per_second,
            Some(1)
        );

        assert_eq!(
            ScheduledBandwidthPolicy::new("Not/AZone", vec![]),
            Err(BandwidthScheduleError::InvalidTimeZone(
                "Not/AZone".to_owned()
            ))
        );
        assert_eq!(
            ScheduledBandwidthPolicy::new(
                "UTC",
                vec![rule(
                    BandwidthScope::Queue(" Nightly".to_owned()),
                    &[Weekday::Mon],
                    "09:00",
                    "10:00",
                    Some(1),
                    false,
                )],
            ),
            Err(BandwidthScheduleError::InvalidQueueName)
        );
        assert_eq!(
            ScheduledBandwidthPolicy::new(
                "UTC",
                vec![rule(
                    BandwidthScope::Global,
                    &[],
                    "09:00",
                    "10:00",
                    Some(1),
                    false,
                )],
            ),
            Err(BandwidthScheduleError::EmptyWeekdays)
        );
        assert_eq!(
            ScheduledBandwidthPolicy::new(
                "UTC",
                vec![rule(
                    BandwidthScope::Global,
                    &[Weekday::Mon],
                    "09:00",
                    "09:00",
                    Some(1),
                    false,
                )],
            ),
            Err(BandwidthScheduleError::EmptyWindow)
        );
        assert_eq!(
            ScheduledBandwidthPolicy::new(
                "UTC",
                vec![rule(
                    BandwidthScope::Global,
                    &[Weekday::Mon],
                    "09:00",
                    "10:00",
                    None,
                    false,
                )],
            ),
            Err(BandwidthScheduleError::NoEffect)
        );
    }

    fn policy(rules: Vec<ScheduledBandwidthRule>) -> ScheduledBandwidthPolicy {
        ScheduledBandwidthPolicy::new("UTC", rules).expect("valid policy")
    }

    fn rule(
        scope: BandwidthScope,
        weekdays: &[Weekday],
        start: &str,
        end: &str,
        bytes_per_second: Option<u64>,
        quiet: bool,
    ) -> ScheduledBandwidthRule {
        ScheduledBandwidthRule {
            scope,
            weekdays: weekdays.to_vec(),
            start: time(start),
            end: time(end),
            bytes_per_second: bytes_per_second
                .map(|value| NonZeroU64::new(value).expect("nonzero")),
            quiet,
        }
    }

    const fn unrestricted() -> ScheduledBandwidthDecision {
        ScheduledBandwidthDecision {
            bytes_per_second: None,
            quiet: false,
        }
    }

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().expect("valid UTC fixture")
    }

    fn time(value: &str) -> NaiveTime {
        NaiveTime::parse_from_str(value, "%H:%M").expect("valid time fixture")
    }
}
