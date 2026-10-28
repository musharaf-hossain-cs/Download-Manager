//! Deterministic stop conditions for one running queue.

use chrono::{DateTime, Utc};
use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum QueueStopPolicyError {
    #[error("completed-file stop count must be greater than zero")]
    ZeroCompletedFiles,
    #[error("transferred-byte stop count must be greater than zero")]
    ZeroTransferredBytes,
    #[error("failure stop count must be greater than zero")]
    ZeroFailures,
}

/// Cumulative counters for the current queue run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueRunMetrics {
    /// Files that completed successfully during this run.
    pub completed_files: u64,
    /// Bytes transferred during this run, as accounted by the transfer engine.
    pub transferred_bytes: u64,
    /// Files that reached a terminal failure during this run.
    pub failures: u64,
}

/// The first configured boundary that requires a running queue to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueueStopReason {
    TimeReached,
    CompletedFilesReached,
    TransferredBytesReached,
    FailuresReached,
}

/// Optional time and cumulative-metric boundaries for one queue run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct QueueStopPolicy {
    stop_at: Option<DateTime<Utc>>,
    completed_files: Option<u64>,
    transferred_bytes: Option<u64>,
    failures: Option<u64>,
}

impl QueueStopPolicy {
    /// Creates a policy. An absent boundary disables that stop condition.
    ///
    /// # Errors
    /// Returns an error for any configured numeric boundary of zero.
    pub fn new(
        stop_at: Option<DateTime<Utc>>,
        completed_files: Option<u64>,
        transferred_bytes: Option<u64>,
        failures: Option<u64>,
    ) -> Result<Self, QueueStopPolicyError> {
        if completed_files == Some(0) {
            return Err(QueueStopPolicyError::ZeroCompletedFiles);
        }
        if transferred_bytes == Some(0) {
            return Err(QueueStopPolicyError::ZeroTransferredBytes);
        }
        if failures == Some(0) {
            return Err(QueueStopPolicyError::ZeroFailures);
        }
        Ok(Self {
            stop_at,
            completed_files,
            transferred_bytes,
            failures,
        })
    }

    /// Returns the first reached boundary in stable time/files/bytes/failures precedence.
    #[must_use]
    pub fn evaluate(
        &self,
        now: DateTime<Utc>,
        metrics: QueueRunMetrics,
    ) -> Option<QueueStopReason> {
        if self.stop_at.is_some_and(|stop_at| now >= stop_at) {
            return Some(QueueStopReason::TimeReached);
        }
        if self
            .completed_files
            .is_some_and(|limit| metrics.completed_files >= limit)
        {
            return Some(QueueStopReason::CompletedFilesReached);
        }
        if self
            .transferred_bytes
            .is_some_and(|limit| metrics.transferred_bytes >= limit)
        {
            return Some(QueueStopReason::TransferredBytesReached);
        }
        if self.failures.is_some_and(|limit| metrics.failures >= limit) {
            return Some(QueueStopReason::FailuresReached);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{QueueRunMetrics, QueueStopPolicy, QueueStopPolicyError, QueueStopReason};

    #[test]
    fn fake_clock_stops_at_and_after_the_configured_time() {
        let policy = QueueStopPolicy::new(Some(utc("2026-10-28T18:00:00Z")), None, None, None)
            .expect("time policy");

        assert_eq!(
            policy.evaluate(utc("2026-10-28T17:59:59Z"), QueueRunMetrics::default()),
            None
        );
        assert_eq!(
            policy.evaluate(utc("2026-10-28T18:00:00Z"), QueueRunMetrics::default()),
            Some(QueueStopReason::TimeReached)
        );
        assert_eq!(
            policy.evaluate(utc("2026-10-28T18:00:01Z"), QueueRunMetrics::default()),
            Some(QueueStopReason::TimeReached)
        );
    }

    #[test]
    fn metric_table_stops_below_at_and_above_each_boundary() {
        struct Case {
            name: &'static str,
            policy: QueueStopPolicy,
            below: QueueRunMetrics,
            at: QueueRunMetrics,
            above: QueueRunMetrics,
            reason: QueueStopReason,
        }

        let cases = [
            Case {
                name: "completed files",
                policy: QueueStopPolicy::new(None, Some(3), None, None).expect("file policy"),
                below: metrics(2, 900, 4),
                at: metrics(3, 900, 4),
                above: metrics(4, 900, 4),
                reason: QueueStopReason::CompletedFilesReached,
            },
            Case {
                name: "transferred bytes",
                policy: QueueStopPolicy::new(None, None, Some(1_000), None).expect("byte policy"),
                below: metrics(8, 999, 4),
                at: metrics(8, 1_000, 4),
                above: metrics(8, 1_001, 4),
                reason: QueueStopReason::TransferredBytesReached,
            },
            Case {
                name: "failures",
                policy: QueueStopPolicy::new(None, None, None, Some(5)).expect("failure policy"),
                below: metrics(8, 900, 4),
                at: metrics(8, 900, 5),
                above: metrics(8, 900, 6),
                reason: QueueStopReason::FailuresReached,
            },
        ];

        for case in cases {
            assert_eq!(
                case.policy
                    .evaluate(utc("2026-10-28T12:00:00Z"), case.below),
                None,
                "{} below",
                case.name
            );
            assert_eq!(
                case.policy.evaluate(utc("2026-10-28T12:00:00Z"), case.at),
                Some(case.reason),
                "{} at",
                case.name
            );
            assert_eq!(
                case.policy
                    .evaluate(utc("2026-10-28T12:00:00Z"), case.above),
                Some(case.reason),
                "{} above",
                case.name
            );
        }
    }

    #[test]
    fn empty_policy_and_unrelated_metrics_continue() {
        let empty = QueueStopPolicy::default();
        assert_eq!(
            empty.evaluate(
                utc("2026-10-28T12:00:00Z"),
                metrics(u64::MAX, u64::MAX, u64::MAX),
            ),
            None
        );

        let files = QueueStopPolicy::new(None, Some(2), None, None).expect("file policy");
        assert_eq!(
            files.evaluate(utc("2026-10-28T12:00:00Z"), metrics(1, u64::MAX, u64::MAX),),
            None
        );
    }

    #[test]
    fn simultaneous_boundaries_use_stable_precedence() {
        let now = utc("2026-10-28T12:00:00Z");
        let policy =
            QueueStopPolicy::new(Some(now), Some(1), Some(1), Some(1)).expect("combined policy");
        assert_eq!(
            policy.evaluate(now, metrics(1, 1, 1)),
            Some(QueueStopReason::TimeReached)
        );

        let metrics_only =
            QueueStopPolicy::new(None, Some(1), Some(1), Some(1)).expect("combined metrics policy");
        assert_eq!(
            metrics_only.evaluate(now, metrics(1, 1, 1)),
            Some(QueueStopReason::CompletedFilesReached)
        );
    }

    #[test]
    fn zero_thresholds_are_rejected_and_max_values_do_not_overflow() {
        assert_eq!(
            QueueStopPolicy::new(None, Some(0), None, None),
            Err(QueueStopPolicyError::ZeroCompletedFiles)
        );
        assert_eq!(
            QueueStopPolicy::new(None, None, Some(0), None),
            Err(QueueStopPolicyError::ZeroTransferredBytes)
        );
        assert_eq!(
            QueueStopPolicy::new(None, None, None, Some(0)),
            Err(QueueStopPolicyError::ZeroFailures)
        );

        let max = QueueStopPolicy::new(None, Some(u64::MAX), Some(u64::MAX), Some(u64::MAX))
            .expect("maximum policy");
        assert_eq!(
            max.evaluate(
                utc("2026-10-28T12:00:00Z"),
                metrics(u64::MAX, u64::MAX, u64::MAX),
            ),
            Some(QueueStopReason::CompletedFilesReached)
        );
    }

    const fn metrics(
        completed_files: u64,
        transferred_bytes: u64,
        failures: u64,
    ) -> QueueRunMetrics {
        QueueRunMetrics {
            completed_files,
            transferred_bytes,
            failures,
        }
    }

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().expect("test UTC timestamp")
    }
}
