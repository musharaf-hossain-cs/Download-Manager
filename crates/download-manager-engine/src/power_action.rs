//! Cancellable confirmation countdowns and the operating-system power-action boundary.

use std::io;
#[cfg(windows)]
use std::process::Command;
use std::time::Duration;

use chrono::{DateTime, Utc};
use thiserror::Error;

/// Power actions that a completed queue may request after user confirmation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerAction {
    Sleep,
    Hibernate,
    SignOut,
    Shutdown,
}

/// Failure reported by a power-action adapter.
#[derive(Debug, Error)]
pub enum PowerActionError {
    #[error("power actions are unsupported on this platform")]
    UnsupportedPlatform,
    #[error("failed to launch the operating-system power action")]
    Launch(#[source] io::Error),
    #[error("the operating-system power action was rejected (exit code {code:?})")]
    Rejected { code: Option<i32> },
}

/// Adapter boundary that keeps destructive operating-system calls mockable.
pub trait PowerActionAdapter {
    /// Requests one action. Implementations must not return success before the
    /// operating system has accepted the request.
    ///
    /// # Errors
    /// Returns an adapter-specific error when the request cannot be accepted.
    fn perform(&mut self, action: PowerAction) -> Result<(), PowerActionError>;
}

/// Production adapter for Windows power actions.
///
/// Callers must only invoke this through a completed [`PowerActionCountdown`].
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemPowerActionAdapter;

impl PowerActionAdapter for SystemPowerActionAdapter {
    fn perform(&mut self, action: PowerAction) -> Result<(), PowerActionError> {
        #[cfg(windows)]
        {
            let (program, arguments) = windows_command(action);
            let status = Command::new(program)
                .args(arguments)
                .status()
                .map_err(PowerActionError::Launch)?;
            if status.success() {
                Ok(())
            } else {
                Err(PowerActionError::Rejected {
                    code: status.code(),
                })
            }
        }

        #[cfg(not(windows))]
        {
            let _ = action;
            Err(PowerActionError::UnsupportedPlatform)
        }
    }
}

#[cfg(windows)]
fn windows_command(action: PowerAction) -> (&'static str, &'static [&'static str]) {
    match action {
        PowerAction::Sleep => ("rundll32.exe", &["powrprof.dll,SetSuspendState", "0,1,0"]),
        PowerAction::Hibernate => ("shutdown.exe", &["/h"]),
        PowerAction::SignOut => ("shutdown.exe", &["/l"]),
        PowerAction::Shutdown => ("shutdown.exe", &["/s", "/t", "0"]),
    }
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum PowerActionCountdownError {
    #[error("power-action confirmation countdown must be greater than zero")]
    ZeroDuration,
    #[error("power-action confirmation countdown is too long")]
    DurationOutOfRange,
    #[error("power-action confirmation deadline is outside the supported time range")]
    DeadlineOutOfRange,
}

/// Stable terminal and in-progress states for a confirmed action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerActionCountdownStatus {
    Pending,
    Cancelled,
    Executed,
    Failed,
}

/// Result of polling a confirmed power-action countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PowerActionCountdownPoll {
    Pending { remaining: Duration },
    Cancelled,
    Executed,
    Failed,
}

/// A once-only power action gated by an explicit confirmation countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerActionCountdown {
    action: PowerAction,
    deadline: DateTime<Utc>,
    status: PowerActionCountdownStatus,
}

impl PowerActionCountdown {
    /// Starts the countdown after the user has explicitly confirmed the action.
    ///
    /// # Errors
    /// Rejects zero, unrepresentable durations, and timestamp overflow.
    pub fn after_confirmation(
        action: PowerAction,
        confirmed_at: DateTime<Utc>,
        countdown: Duration,
    ) -> Result<Self, PowerActionCountdownError> {
        if countdown.is_zero() {
            return Err(PowerActionCountdownError::ZeroDuration);
        }
        let countdown = chrono::Duration::from_std(countdown)
            .map_err(|_| PowerActionCountdownError::DurationOutOfRange)?;
        let deadline = confirmed_at
            .checked_add_signed(countdown)
            .ok_or(PowerActionCountdownError::DeadlineOutOfRange)?;
        Ok(Self {
            action,
            deadline,
            status: PowerActionCountdownStatus::Pending,
        })
    }

    #[must_use]
    pub const fn action(&self) -> PowerAction {
        self.action
    }

    #[must_use]
    pub const fn deadline(&self) -> DateTime<Utc> {
        self.deadline
    }

    #[must_use]
    pub const fn status(&self) -> PowerActionCountdownStatus {
        self.status
    }

    /// Cancels a pending action. Terminal actions cannot be changed.
    pub fn cancel(&mut self) -> bool {
        if self.status != PowerActionCountdownStatus::Pending {
            return false;
        }
        self.status = PowerActionCountdownStatus::Cancelled;
        true
    }

    /// Advances the countdown using the supplied clock snapshot.
    ///
    /// The adapter is called at most once, exactly when the deadline is first
    /// observed. The state becomes terminal before the external call so a
    /// rejected request cannot be retried accidentally.
    ///
    /// # Errors
    /// Returns the adapter error from the first execution attempt.
    pub fn poll(
        &mut self,
        now: DateTime<Utc>,
        adapter: &mut impl PowerActionAdapter,
    ) -> Result<PowerActionCountdownPoll, PowerActionError> {
        match self.status {
            PowerActionCountdownStatus::Cancelled => {
                return Ok(PowerActionCountdownPoll::Cancelled);
            }
            PowerActionCountdownStatus::Executed => {
                return Ok(PowerActionCountdownPoll::Executed);
            }
            PowerActionCountdownStatus::Failed => return Ok(PowerActionCountdownPoll::Failed),
            PowerActionCountdownStatus::Pending => {}
        }

        if now < self.deadline {
            let remaining = (self.deadline - now).to_std().unwrap_or_default();
            return Ok(PowerActionCountdownPoll::Pending { remaining });
        }

        self.status = PowerActionCountdownStatus::Executed;
        if let Err(error) = adapter.perform(self.action) {
            self.status = PowerActionCountdownStatus::Failed;
            return Err(error);
        }
        Ok(PowerActionCountdownPoll::Executed)
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;

    #[derive(Default)]
    struct MockPowerActionAdapter {
        actions: Vec<PowerAction>,
        reject: bool,
    }

    impl PowerActionAdapter for MockPowerActionAdapter {
        fn perform(&mut self, action: PowerAction) -> Result<(), PowerActionError> {
            self.actions.push(action);
            if self.reject {
                Err(PowerActionError::Rejected { code: Some(5) })
            } else {
                Ok(())
            }
        }
    }

    fn confirmed_at() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 28, 10, 0, 0)
            .single()
            .expect("valid fixture")
    }

    #[test]
    fn cancellation_prevents_the_confirmed_action() {
        let start = confirmed_at();
        let mut countdown = PowerActionCountdown::after_confirmation(
            PowerAction::Shutdown,
            start,
            Duration::from_secs(10),
        )
        .expect("valid countdown");
        let mut adapter = MockPowerActionAdapter::default();

        assert_eq!(
            countdown.poll(start, &mut adapter).expect("poll"),
            PowerActionCountdownPoll::Pending {
                remaining: Duration::from_secs(10)
            }
        );
        assert!(countdown.cancel());
        assert_eq!(
            countdown
                .poll(start + chrono::Duration::seconds(30), &mut adapter)
                .expect("cancelled poll"),
            PowerActionCountdownPoll::Cancelled
        );
        assert_eq!(adapter.actions, Vec::<PowerAction>::new());
        assert!(!countdown.cancel());
    }

    #[test]
    fn deadline_executes_exactly_once() {
        let start = confirmed_at();
        let mut countdown = PowerActionCountdown::after_confirmation(
            PowerAction::Hibernate,
            start,
            Duration::from_secs(5),
        )
        .expect("valid countdown");
        let mut adapter = MockPowerActionAdapter::default();

        assert_eq!(
            countdown
                .poll(start + chrono::Duration::seconds(4), &mut adapter)
                .expect("pending"),
            PowerActionCountdownPoll::Pending {
                remaining: Duration::from_secs(1)
            }
        );
        assert_eq!(
            countdown
                .poll(start + chrono::Duration::seconds(5), &mut adapter)
                .expect("execute"),
            PowerActionCountdownPoll::Executed
        );
        assert_eq!(
            countdown
                .poll(start + chrono::Duration::seconds(6), &mut adapter)
                .expect("terminal poll"),
            PowerActionCountdownPoll::Executed
        );
        assert_eq!(adapter.actions, vec![PowerAction::Hibernate]);
    }

    #[test]
    fn rejected_action_is_terminal_and_not_retried() {
        let start = confirmed_at();
        let mut countdown = PowerActionCountdown::after_confirmation(
            PowerAction::SignOut,
            start,
            Duration::from_secs(1),
        )
        .expect("valid countdown");
        let mut adapter = MockPowerActionAdapter {
            reject: true,
            ..MockPowerActionAdapter::default()
        };

        assert!(matches!(
            countdown.poll(start + chrono::Duration::seconds(1), &mut adapter),
            Err(PowerActionError::Rejected { code: Some(5) })
        ));
        assert_eq!(countdown.status(), PowerActionCountdownStatus::Failed);
        assert_eq!(
            countdown
                .poll(start + chrono::Duration::seconds(2), &mut adapter)
                .expect("terminal poll"),
            PowerActionCountdownPoll::Failed
        );
        assert_eq!(adapter.actions, vec![PowerAction::SignOut]);
    }

    #[test]
    fn zero_duration_is_rejected() {
        assert_eq!(
            PowerActionCountdown::after_confirmation(
                PowerAction::Sleep,
                confirmed_at(),
                Duration::ZERO,
            ),
            Err(PowerActionCountdownError::ZeroDuration)
        );
    }
}
