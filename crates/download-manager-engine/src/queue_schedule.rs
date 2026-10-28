//! Queue recurrence expressed as local wall-clock time in an IANA time zone.

use chrono::{DateTime, Datelike, LocalResult, NaiveDateTime, NaiveTime, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use thiserror::Error;

const MAX_QUEUE_NAME_LENGTH: usize = 256;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum QueueScheduleError {
    #[error("queue name must be trimmed, non-empty, and at most 256 characters")]
    InvalidQueueName,
    #[error("unknown IANA time zone: {0}")]
    InvalidTimeZone(String),
    #[error("a weekly recurrence must select at least one weekday")]
    EmptyWeekdays,
    #[error("{local} does not exist in {time_zone}")]
    NonexistentLocalTime { local: NaiveDateTime, time_zone: Tz },
    #[error("{local} is ambiguous in {time_zone}")]
    AmbiguousLocalTime { local: NaiveDateTime, time_zone: Tz },
    #[error("scheduler watermark {last_observed_at} is after wake time {woke_at}")]
    InvalidWakeWindow {
        last_observed_at: DateTime<Utc>,
        woke_at: DateTime<Utc>,
    },
}

/// What to do when scheduled occurrences elapsed while the scheduler was unavailable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MissedRunPolicy {
    /// Discard elapsed occurrences and wait for the next recurrence.
    Skip,
    /// Coalesce all elapsed occurrences into one immediate queue start.
    RunOnWake,
}

/// The supported bounded recurrence forms for one named queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueRecurrence {
    /// Runs once at the supplied local date and time.
    OneTime { local_at: NaiveDateTime },
    /// Runs on the selected local weekdays at the supplied local time.
    Weekly {
        weekdays: Vec<Weekday>,
        local_time: NaiveTime,
    },
}

/// A validated queue recurrence anchored to an IANA time zone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueueSchedule {
    queue_name: String,
    time_zone: Tz,
    recurrence: QueueRecurrence,
}

impl QueueSchedule {
    /// Creates a recurrence for an existing named queue.
    ///
    /// Weekdays are normalized to calendar order and duplicates are removed.
    ///
    /// # Errors
    /// Returns an error for an invalid queue name, unknown IANA zone, or empty weekly selection.
    pub fn new(
        queue_name: impl Into<String>,
        time_zone: &str,
        mut recurrence: QueueRecurrence,
    ) -> Result<Self, QueueScheduleError> {
        let queue_name = queue_name.into();
        if queue_name.is_empty()
            || queue_name.trim() != queue_name
            || queue_name.chars().count() > MAX_QUEUE_NAME_LENGTH
            || queue_name.chars().any(char::is_control)
        {
            return Err(QueueScheduleError::InvalidQueueName);
        }
        let time_zone = time_zone
            .parse::<Tz>()
            .map_err(|_| QueueScheduleError::InvalidTimeZone(time_zone.to_owned()))?;
        if let QueueRecurrence::Weekly { weekdays, .. } = &mut recurrence {
            weekdays.sort_unstable_by_key(Weekday::num_days_from_monday);
            weekdays.dedup();
            if weekdays.is_empty() {
                return Err(QueueScheduleError::EmptyWeekdays);
            }
        }
        Ok(Self {
            queue_name,
            time_zone,
            recurrence,
        })
    }

    #[must_use]
    pub fn queue_name(&self) -> &str {
        &self.queue_name
    }

    #[must_use]
    pub const fn time_zone(&self) -> Tz {
        self.time_zone
    }

    #[must_use]
    pub const fn recurrence(&self) -> &QueueRecurrence {
        &self.recurrence
    }

    /// Returns the first scheduled instant strictly after `now`.
    ///
    /// DST gaps and folds deliberately return explicit errors. S126 supplies the configurable
    /// skip/run policy rather than letting this base recurrence silently choose an instant.
    ///
    /// # Errors
    /// Returns an error when the next local wall-clock candidate is nonexistent or ambiguous.
    pub fn next_run_after(
        &self,
        now: DateTime<Utc>,
    ) -> Result<Option<DateTime<Utc>>, QueueScheduleError> {
        match &self.recurrence {
            QueueRecurrence::OneTime { local_at } => {
                let candidate = self.resolve_local(*local_at)?;
                Ok((candidate > now).then_some(candidate))
            }
            QueueRecurrence::Weekly {
                weekdays,
                local_time,
            } => {
                let local_today = now.with_timezone(&self.time_zone).date_naive();
                for days_ahead in 0..=7 {
                    let date = local_today + chrono::Days::new(days_ahead);
                    if !weekdays.contains(&date.weekday()) {
                        continue;
                    }
                    let candidate = self.resolve_local(date.and_time(*local_time))?;
                    if candidate > now {
                        return Ok(Some(candidate));
                    }
                }
                unreachable!("a non-empty weekly recurrence has a candidate within seven days")
            }
        }
    }

    /// Decides whether this queue should start once at `woke_at` after an offline or restart gap.
    ///
    /// `last_observed_at` is the caller's durable scheduler watermark. The interval is open at that
    /// watermark and closed at `woke_at`, so persisting the new watermark after handling a decision
    /// prevents a restart from starting the same occurrence again. A nonexistent DST wall time is
    /// a missed occurrence. An ambiguous wall time occurs once, at its earlier UTC instant.
    ///
    /// # Errors
    /// Returns an error when the watermark is later than the wake time.
    pub fn should_run_on_wake(
        &self,
        last_observed_at: DateTime<Utc>,
        woke_at: DateTime<Utc>,
        policy: MissedRunPolicy,
    ) -> Result<bool, QueueScheduleError> {
        if last_observed_at > woke_at {
            return Err(QueueScheduleError::InvalidWakeWindow {
                last_observed_at,
                woke_at,
            });
        }
        if policy == MissedRunPolicy::Skip || last_observed_at == woke_at {
            return Ok(false);
        }

        Ok(self.has_occurrence_in(last_observed_at, woke_at))
    }

    fn has_occurrence_in(&self, last_observed_at: DateTime<Utc>, woke_at: DateTime<Utc>) -> bool {
        match &self.recurrence {
            QueueRecurrence::OneTime { local_at } => {
                self.local_occurs_in(*local_at, last_observed_at, woke_at)
            }
            QueueRecurrence::Weekly {
                weekdays,
                local_time,
            } => {
                let last_local_date = last_observed_at.with_timezone(&self.time_zone).date_naive();
                let woke_local_date = woke_at.with_timezone(&self.time_zone).date_naive();
                // Every selected weekday occurs in an eight-date suffix. Older dates cannot alter
                // this boolean decision and need not make a long-offline scan unbounded.
                let first_date = (woke_local_date - chrono::Days::new(8)).max(last_local_date);
                let mut date = first_date;
                while date <= woke_local_date {
                    if weekdays.contains(&date.weekday())
                        && self.local_occurs_in(
                            date.and_time(*local_time),
                            last_observed_at,
                            woke_at,
                        )
                    {
                        return true;
                    }
                    date = date
                        .succ_opt()
                        .expect("bounded scheduler dates remain valid");
                }
                false
            }
        }
    }

    fn local_occurs_in(
        &self,
        local: NaiveDateTime,
        last_observed_at: DateTime<Utc>,
        woke_at: DateTime<Utc>,
    ) -> bool {
        match self.time_zone.from_local_datetime(&local) {
            LocalResult::Single(value) => {
                let instant = value.with_timezone(&Utc);
                instant > last_observed_at && instant <= woke_at
            }
            LocalResult::Ambiguous(first, second) => {
                let instant = first.min(second).with_timezone(&Utc);
                instant > last_observed_at && instant <= woke_at
            }
            LocalResult::None => {
                let last_local = last_observed_at
                    .with_timezone(&self.time_zone)
                    .naive_local();
                let woke_local = woke_at.with_timezone(&self.time_zone).naive_local();
                local > last_local && local <= woke_local
            }
        }
    }

    fn resolve_local(&self, local: NaiveDateTime) -> Result<DateTime<Utc>, QueueScheduleError> {
        match self.time_zone.from_local_datetime(&local) {
            LocalResult::Single(value) => Ok(value.with_timezone(&Utc)),
            LocalResult::None => Err(QueueScheduleError::NonexistentLocalTime {
                local,
                time_zone: self.time_zone,
            }),
            LocalResult::Ambiguous(_, _) => Err(QueueScheduleError::AmbiguousLocalTime {
                local,
                time_zone: self.time_zone,
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, NaiveDate, NaiveTime, Utc, Weekday};

    use super::{MissedRunPolicy, QueueRecurrence, QueueSchedule, QueueScheduleError};

    struct NextRunCase {
        name: &'static str,
        zone: &'static str,
        recurrence: QueueRecurrence,
        now: &'static str,
        expected: Option<&'static str>,
    }

    #[test]
    fn fake_clock_next_run_table_covers_one_time_weekly_and_time_zones() {
        let cases = [
            NextRunCase {
                name: "one time Dhaka future",
                zone: "Asia/Dhaka",
                recurrence: QueueRecurrence::OneTime {
                    local_at: local(2026, 10, 29, 9, 30),
                },
                now: "2026-10-28T12:00:00Z",
                expected: Some("2026-10-29T03:30:00Z"),
            },
            NextRunCase {
                name: "one time elapsed",
                zone: "UTC",
                recurrence: QueueRecurrence::OneTime {
                    local_at: local(2026, 10, 28, 9, 30),
                },
                now: "2026-10-28T12:00:00Z",
                expected: None,
            },
            NextRunCase {
                name: "weekly same local day future",
                zone: "Asia/Dhaka",
                recurrence: weekly(&[Weekday::Wed], 19, 0),
                now: "2026-10-28T10:00:00Z",
                expected: Some("2026-10-28T13:00:00Z"),
            },
            NextRunCase {
                name: "weekly elapsed rolls seven days",
                zone: "Asia/Dhaka",
                recurrence: weekly(&[Weekday::Wed], 15, 0),
                now: "2026-10-28T10:00:00Z",
                expected: Some("2026-11-04T09:00:00Z"),
            },
            NextRunCase {
                name: "earliest selected weekday wins",
                zone: "UTC",
                recurrence: weekly(&[Weekday::Mon, Weekday::Fri], 8, 0),
                now: "2026-10-28T12:00:00Z",
                expected: Some("2026-10-30T08:00:00Z"),
            },
            NextRunCase {
                name: "New York standard offset",
                zone: "America/New_York",
                recurrence: weekly(&[Weekday::Fri], 9, 0),
                now: "2026-01-01T12:00:00Z",
                expected: Some("2026-01-02T14:00:00Z"),
            },
            NextRunCase {
                name: "Berlin daylight offset",
                zone: "Europe/Berlin",
                recurrence: weekly(&[Weekday::Fri], 9, 0),
                now: "2026-07-02T12:00:00Z",
                expected: Some("2026-07-03T07:00:00Z"),
            },
        ];

        for case in cases {
            let schedule = QueueSchedule::new("Night", case.zone, case.recurrence)
                .unwrap_or_else(|error| panic!("{}: {error}", case.name));
            let actual = schedule
                .next_run_after(utc(case.now))
                .unwrap_or_else(|error| panic!("{}: {error}", case.name));
            assert_eq!(
                actual,
                case.expected.map(utc),
                "next run mismatch for {}",
                case.name
            );
        }
    }

    #[test]
    fn constructor_normalizes_weekdays_and_rejects_invalid_inputs() {
        let schedule = QueueSchedule::new(
            "Night",
            "Asia/Dhaka",
            weekly(&[Weekday::Fri, Weekday::Mon, Weekday::Fri], 2, 0),
        )
        .expect("valid schedule");
        assert_eq!(schedule.queue_name(), "Night");
        assert_eq!(schedule.time_zone().name(), "Asia/Dhaka");
        assert!(matches!(
            schedule.recurrence(),
            QueueRecurrence::Weekly { weekdays, .. }
                if weekdays == &[Weekday::Mon, Weekday::Fri]
        ));

        assert!(matches!(
            QueueSchedule::new("Night", "Mars/Olympus", weekly(&[Weekday::Mon], 1, 0)),
            Err(QueueScheduleError::InvalidTimeZone(_))
        ));
        assert_eq!(
            QueueSchedule::new("Night", "UTC", weekly(&[], 1, 0)),
            Err(QueueScheduleError::EmptyWeekdays)
        );
        assert_eq!(
            QueueSchedule::new(" Night", "UTC", weekly(&[Weekday::Mon], 1, 0)),
            Err(QueueScheduleError::InvalidQueueName)
        );
    }

    #[test]
    fn dst_gap_and_fold_require_the_next_slice_policy() {
        let gap = QueueSchedule::new(
            "Night",
            "America/New_York",
            QueueRecurrence::OneTime {
                local_at: local(2026, 3, 8, 2, 30),
            },
        )
        .expect("gap schedule");
        assert!(matches!(
            gap.next_run_after(utc("2026-03-07T00:00:00Z")),
            Err(QueueScheduleError::NonexistentLocalTime { .. })
        ));

        let fold = QueueSchedule::new(
            "Night",
            "America/New_York",
            QueueRecurrence::OneTime {
                local_at: local(2026, 11, 1, 1, 30),
            },
        )
        .expect("fold schedule");
        assert!(matches!(
            fold.next_run_after(utc("2026-10-31T00:00:00Z")),
            Err(QueueScheduleError::AmbiguousLocalTime { .. })
        ));
    }

    #[test]
    fn dst_gap_and_fold_have_explicit_once_only_wake_behavior() {
        let gap = QueueSchedule::new("Night", "America/New_York", weekly(&[Weekday::Sun], 2, 30))
            .expect("gap schedule");
        assert!(
            gap.should_run_on_wake(
                utc("2026-03-08T06:55:00Z"),
                utc("2026-03-08T07:05:00Z"),
                MissedRunPolicy::RunOnWake,
            )
            .expect("gap decision")
        );
        assert!(
            !gap.should_run_on_wake(
                utc("2026-03-08T06:55:00Z"),
                utc("2026-03-08T07:05:00Z"),
                MissedRunPolicy::Skip,
            )
            .expect("gap skip")
        );

        let fold = QueueSchedule::new(
            "Night",
            "America/New_York",
            QueueRecurrence::OneTime {
                local_at: local(2026, 11, 1, 1, 30),
            },
        )
        .expect("fold schedule");
        assert!(
            fold.should_run_on_wake(
                utc("2026-11-01T05:20:00Z"),
                utc("2026-11-01T05:40:00Z"),
                MissedRunPolicy::RunOnWake,
            )
            .expect("first fold decision")
        );
        assert!(
            !fold
                .should_run_on_wake(
                    utc("2026-11-01T05:40:00Z"),
                    utc("2026-11-01T06:40:00Z"),
                    MissedRunPolicy::RunOnWake,
                )
                .expect("second fold decision")
        );
    }

    #[test]
    fn offline_and_restart_windows_coalesce_or_skip_missed_runs() {
        let schedule =
            QueueSchedule::new("Night", "UTC", weekly(&[Weekday::Mon, Weekday::Fri], 8, 0))
                .expect("weekly schedule");
        let durable_watermark = utc("2026-10-23T07:00:00Z");
        let woke_at = utc("2026-11-02T09:00:00Z");

        assert!(
            schedule
                .should_run_on_wake(durable_watermark, woke_at, MissedRunPolicy::RunOnWake,)
                .expect("offline decision")
        );
        assert!(
            !schedule
                .should_run_on_wake(durable_watermark, woke_at, MissedRunPolicy::Skip)
                .expect("offline skip")
        );
        assert!(
            !schedule
                .should_run_on_wake(
                    woke_at,
                    utc("2026-11-02T09:01:00Z"),
                    MissedRunPolicy::RunOnWake,
                )
                .expect("restart after persisted watermark")
        );
        assert!(matches!(
            schedule.should_run_on_wake(woke_at, durable_watermark, MissedRunPolicy::RunOnWake),
            Err(QueueScheduleError::InvalidWakeWindow { .. })
        ));
    }

    fn weekly(days: &[Weekday], hour: u32, minute: u32) -> QueueRecurrence {
        QueueRecurrence::Weekly {
            weekdays: days.to_vec(),
            local_time: NaiveTime::from_hms_opt(hour, minute, 0).expect("test time"),
        }
    }

    fn local(year: i32, month: u32, day: u32, hour: u32, minute: u32) -> chrono::NaiveDateTime {
        NaiveDate::from_ymd_opt(year, month, day)
            .expect("test date")
            .and_hms_opt(hour, minute, 0)
            .expect("test time")
    }

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().expect("test UTC timestamp")
    }
}
