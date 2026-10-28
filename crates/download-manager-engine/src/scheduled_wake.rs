//! Optional OS wake scheduling behind an injectable platform adapter.

use chrono::{DateTime, Utc};
use thiserror::Error;

const MAX_QUEUE_NAME_LENGTH: usize = 256;

/// Whether the current OS adapter can request a wake timer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeSupport {
    Supported,
    Unsupported,
}

/// A validated request to wake for one named queue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRequest {
    pub queue_name: String,
    pub wake_at: DateTime<Utc>,
}

/// Result exposed to scheduler health/UI code after reconciling wake configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WakeOutcome {
    Disabled,
    Unsupported,
    Scheduled { wake_at: DateTime<Utc> },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ScheduledWakeError {
    #[error("queue name must be trimmed, non-empty, and at most 256 characters")]
    InvalidQueueName,
    #[error("scheduled wake must be strictly in the future")]
    NotInFuture,
    #[error("scheduled wake adapter failed: {0}")]
    Adapter(String),
}

/// Platform boundary for installing and removing a queue-specific OS wake timer.
pub trait ScheduledWakeAdapter {
    fn support(&self) -> WakeSupport;

    /// Installs or replaces the wake request for `request.queue_name`.
    ///
    /// # Errors
    /// Returns an adapter error if the operating system rejects the request.
    fn schedule(&mut self, request: &WakeRequest) -> Result<(), ScheduledWakeError>;

    /// Removes an existing wake request. Missing requests should be treated as success.
    ///
    /// # Errors
    /// Returns an adapter error if the operating system rejects the cancellation.
    fn cancel(&mut self, queue_name: &str) -> Result<(), ScheduledWakeError>;
}

/// Reconciles a queue's optional wake time with an OS adapter.
///
/// Unsupported systems report a stable non-error outcome so scheduling the queue itself remains
/// usable. Disabling wake is also a no-op on unsupported systems.
///
/// # Errors
/// Returns an error for an invalid queue name, a non-future wake time, or an adapter failure.
pub fn configure_scheduled_wake(
    adapter: &mut dyn ScheduledWakeAdapter,
    queue_name: &str,
    now: DateTime<Utc>,
    wake_at: Option<DateTime<Utc>>,
) -> Result<WakeOutcome, ScheduledWakeError> {
    validate_queue_name(queue_name)?;

    let Some(wake_at) = wake_at else {
        if adapter.support() == WakeSupport::Supported {
            adapter.cancel(queue_name)?;
        }
        return Ok(WakeOutcome::Disabled);
    };

    if wake_at <= now {
        return Err(ScheduledWakeError::NotInFuture);
    }
    if adapter.support() == WakeSupport::Unsupported {
        return Ok(WakeOutcome::Unsupported);
    }

    adapter.schedule(&WakeRequest {
        queue_name: queue_name.to_owned(),
        wake_at,
    })?;
    Ok(WakeOutcome::Scheduled { wake_at })
}

fn validate_queue_name(queue_name: &str) -> Result<(), ScheduledWakeError> {
    if queue_name.is_empty()
        || queue_name.trim() != queue_name
        || queue_name.chars().count() > MAX_QUEUE_NAME_LENGTH
        || queue_name.chars().any(char::is_control)
    {
        Err(ScheduledWakeError::InvalidQueueName)
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use chrono::{DateTime, Utc};

    use super::{
        ScheduledWakeAdapter, ScheduledWakeError, WakeOutcome, WakeRequest, WakeSupport,
        configure_scheduled_wake,
    };

    #[derive(Default)]
    struct MockAdapter {
        supported: bool,
        scheduled: Vec<WakeRequest>,
        cancelled: Vec<String>,
    }

    impl ScheduledWakeAdapter for MockAdapter {
        fn support(&self) -> WakeSupport {
            if self.supported {
                WakeSupport::Supported
            } else {
                WakeSupport::Unsupported
            }
        }

        fn schedule(&mut self, request: &WakeRequest) -> Result<(), ScheduledWakeError> {
            self.scheduled.push(request.clone());
            Ok(())
        }

        fn cancel(&mut self, queue_name: &str) -> Result<(), ScheduledWakeError> {
            self.cancelled.push(queue_name.to_owned());
            Ok(())
        }
    }

    #[test]
    fn supported_adapter_schedules_and_cancels_queue_wake() {
        let now = utc("2026-10-28T12:00:00Z");
        let wake_at = utc("2026-10-28T12:15:00Z");
        let mut adapter = MockAdapter {
            supported: true,
            ..MockAdapter::default()
        };

        assert_eq!(
            configure_scheduled_wake(&mut adapter, "Nightly", now, Some(wake_at)),
            Ok(WakeOutcome::Scheduled { wake_at })
        );
        assert_eq!(
            adapter.scheduled,
            [WakeRequest {
                queue_name: "Nightly".to_owned(),
                wake_at,
            }]
        );

        assert_eq!(
            configure_scheduled_wake(&mut adapter, "Nightly", now, None),
            Ok(WakeOutcome::Disabled)
        );
        assert_eq!(adapter.cancelled, ["Nightly"]);
    }

    #[test]
    fn unsupported_adapter_reports_capability_without_touching_os() {
        let now = utc("2026-10-28T12:00:00Z");
        let mut adapter = MockAdapter::default();

        assert_eq!(
            configure_scheduled_wake(
                &mut adapter,
                "Nightly",
                now,
                Some(utc("2026-10-28T12:15:00Z")),
            ),
            Ok(WakeOutcome::Unsupported)
        );
        assert_eq!(
            configure_scheduled_wake(&mut adapter, "Nightly", now, None),
            Ok(WakeOutcome::Disabled)
        );
        assert_eq!(adapter.scheduled, Vec::<WakeRequest>::new());
        assert_eq!(adapter.cancelled, Vec::<String>::new());
    }

    #[test]
    fn invalid_and_elapsed_requests_fail_before_adapter_use() {
        let now = utc("2026-10-28T12:00:00Z");
        let mut adapter = MockAdapter {
            supported: true,
            ..MockAdapter::default()
        };

        assert_eq!(
            configure_scheduled_wake(&mut adapter, " Nightly", now, Some(now)),
            Err(ScheduledWakeError::InvalidQueueName)
        );
        assert_eq!(
            configure_scheduled_wake(&mut adapter, "Nightly", now, Some(now)),
            Err(ScheduledWakeError::NotInFuture)
        );
        assert_eq!(adapter.scheduled, Vec::<WakeRequest>::new());
    }

    fn utc(value: &str) -> DateTime<Utc> {
        value.parse().expect("valid UTC fixture")
    }
}
