//! Progressive transfer quotas over rolling or anchor-aligned time windows.

use std::collections::VecDeque;

use thiserror::Error;

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum TransferQuotaError {
    #[error("quota byte limit and window duration must be greater than zero")]
    InvalidConfiguration,
    #[error("quota clock moved backwards")]
    ClockMovedBackwards,
    #[error("configured quota clock is before its anchor")]
    ClockBeforeAnchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferQuotaWindow {
    /// Usage expires independently after this many milliseconds.
    Rolling { duration_millis: u64 },
    /// All usage resets together on boundaries measured from `anchor_millis`.
    Configured {
        duration_millis: u64,
        anchor_millis: u64,
    },
}

impl TransferQuotaWindow {
    fn duration_millis(self) -> u64 {
        match self {
            Self::Rolling { duration_millis }
            | Self::Configured {
                duration_millis, ..
            } => duration_millis,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferQuotaState {
    Available,
    Paused { resumes_at_millis: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferQuotaSnapshot {
    pub limit_bytes: u64,
    pub used_bytes: u64,
    pub remaining_bytes: u64,
    pub state: TransferQuotaState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferQuotaGrant {
    pub granted_bytes: u64,
    pub snapshot: TransferQuotaSnapshot,
}

#[derive(Debug, Clone, Copy)]
struct Usage {
    at_millis: u64,
    bytes: u64,
}

/// Tracks quota usage and changes a paused quota back to available as virtual time advances.
#[derive(Debug, Clone)]
pub struct TransferQuota {
    limit_bytes: u64,
    window: TransferQuotaWindow,
    usage: VecDeque<Usage>,
    used_bytes: u64,
    configured_window_start: Option<u64>,
    last_millis: Option<u64>,
}

impl TransferQuota {
    /// Creates a quota with a non-zero byte limit and time window.
    ///
    /// # Errors
    /// Returns an error when either value is zero.
    pub fn new(limit_bytes: u64, window: TransferQuotaWindow) -> Result<Self, TransferQuotaError> {
        if limit_bytes == 0 || window.duration_millis() == 0 {
            return Err(TransferQuotaError::InvalidConfiguration);
        }
        Ok(Self {
            limit_bytes,
            window,
            usage: VecDeque::new(),
            used_bytes: 0,
            configured_window_start: None,
            last_millis: None,
        })
    }

    /// Grants up to the requested bytes and returns the resulting pause/resume state.
    ///
    /// A request larger than the remaining allowance receives a partial grant. Later calls at or
    /// beyond the reported resume time automatically observe refreshed allowance.
    ///
    /// # Errors
    /// Returns an error when virtual time regresses or precedes a configured anchor.
    pub fn grant(
        &mut self,
        now_millis: u64,
        requested_bytes: u64,
    ) -> Result<TransferQuotaGrant, TransferQuotaError> {
        self.advance(now_millis)?;
        let granted_bytes = requested_bytes.min(self.limit_bytes - self.used_bytes);
        if granted_bytes > 0 {
            self.used_bytes += granted_bytes;
            if matches!(self.window, TransferQuotaWindow::Rolling { .. }) {
                self.usage.push_back(Usage {
                    at_millis: now_millis,
                    bytes: granted_bytes,
                });
            }
        }
        Ok(TransferQuotaGrant {
            granted_bytes,
            snapshot: self.current_snapshot(),
        })
    }

    /// Returns current usage after applying any reset implied by virtual time.
    ///
    /// # Errors
    /// Returns an error when virtual time regresses or precedes a configured anchor.
    pub fn snapshot(
        &mut self,
        now_millis: u64,
    ) -> Result<TransferQuotaSnapshot, TransferQuotaError> {
        self.advance(now_millis)?;
        Ok(self.current_snapshot())
    }

    fn advance(&mut self, now_millis: u64) -> Result<(), TransferQuotaError> {
        if self.last_millis.is_some_and(|last| now_millis < last) {
            return Err(TransferQuotaError::ClockMovedBackwards);
        }
        match self.window {
            TransferQuotaWindow::Rolling { duration_millis } => {
                while self.usage.front().is_some_and(|usage| {
                    usage.at_millis.saturating_add(duration_millis) <= now_millis
                }) {
                    let expired = self.usage.pop_front().expect("front was present");
                    self.used_bytes -= expired.bytes;
                }
            }
            TransferQuotaWindow::Configured {
                duration_millis,
                anchor_millis,
            } => {
                let elapsed = now_millis
                    .checked_sub(anchor_millis)
                    .ok_or(TransferQuotaError::ClockBeforeAnchor)?;
                let start = anchor_millis
                    .saturating_add((elapsed / duration_millis).saturating_mul(duration_millis));
                if self.configured_window_start != Some(start) {
                    self.configured_window_start = Some(start);
                    self.used_bytes = 0;
                }
            }
        }
        self.last_millis = Some(now_millis);
        Ok(())
    }

    fn current_snapshot(&self) -> TransferQuotaSnapshot {
        let remaining_bytes = self.limit_bytes - self.used_bytes;
        let state = if remaining_bytes > 0 {
            TransferQuotaState::Available
        } else {
            TransferQuotaState::Paused {
                resumes_at_millis: self.resume_time(),
            }
        };
        TransferQuotaSnapshot {
            limit_bytes: self.limit_bytes,
            used_bytes: self.used_bytes,
            remaining_bytes,
            state,
        }
    }

    fn resume_time(&self) -> u64 {
        match self.window {
            TransferQuotaWindow::Rolling { duration_millis } => self
                .usage
                .front()
                .expect("an exhausted rolling quota has usage")
                .at_millis
                .saturating_add(duration_millis),
            TransferQuotaWindow::Configured {
                duration_millis, ..
            } => self
                .configured_window_start
                .expect("an exhausted configured quota has a window")
                .saturating_add(duration_millis),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{TransferQuota, TransferQuotaError, TransferQuotaState, TransferQuotaWindow};

    #[test]
    fn rolling_window_partially_grants_then_auto_resumes_as_usage_expires() {
        let mut quota = TransferQuota::new(
            100,
            TransferQuotaWindow::Rolling {
                duration_millis: 1_000,
            },
        )
        .expect("quota");

        assert_eq!(quota.grant(100, 60).expect("first grant").granted_bytes, 60);
        let exhausted = quota.grant(400, 80).expect("partial grant");
        assert_eq!(exhausted.granted_bytes, 40);
        assert_eq!(
            exhausted.snapshot.state,
            TransferQuotaState::Paused {
                resumes_at_millis: 1_100
            }
        );
        assert_eq!(
            quota.grant(1_099, 1).expect("still paused").granted_bytes,
            0
        );

        let resumed = quota.grant(1_100, 80).expect("automatic resume");
        assert_eq!(resumed.granted_bytes, 60);
        assert_eq!(resumed.snapshot.used_bytes, 100);
        assert_eq!(
            resumed.snapshot.state,
            TransferQuotaState::Paused {
                resumes_at_millis: 1_400
            }
        );
    }

    #[test]
    fn configured_window_resets_on_anchor_aligned_boundary() {
        let mut quota = TransferQuota::new(
            1_024,
            TransferQuotaWindow::Configured {
                duration_millis: 10_000,
                anchor_millis: 5_000,
            },
        )
        .expect("quota");

        let exhausted = quota.grant(14_999, 2_000).expect("partial grant");
        assert_eq!(exhausted.granted_bytes, 1_024);
        assert_eq!(
            exhausted.snapshot.state,
            TransferQuotaState::Paused {
                resumes_at_millis: 15_000
            }
        );
        let resumed = quota.snapshot(15_000).expect("boundary reset");
        assert_eq!(resumed.used_bytes, 0);
        assert_eq!(resumed.remaining_bytes, 1_024);
        assert_eq!(resumed.state, TransferQuotaState::Available);
    }

    #[test]
    fn invalid_configuration_and_clock_inputs_fail_closed() {
        assert_eq!(
            TransferQuota::new(0, TransferQuotaWindow::Rolling { duration_millis: 1 })
                .expect_err("zero limit"),
            TransferQuotaError::InvalidConfiguration
        );
        let mut configured = TransferQuota::new(
            1,
            TransferQuotaWindow::Configured {
                duration_millis: 10,
                anchor_millis: 100,
            },
        )
        .expect("quota");
        assert_eq!(
            configured.snapshot(99),
            Err(TransferQuotaError::ClockBeforeAnchor)
        );

        let mut rolling = TransferQuota::new(
            1,
            TransferQuotaWindow::Rolling {
                duration_millis: 10,
            },
        )
        .expect("quota");
        rolling.snapshot(20).expect("initial time");
        assert_eq!(
            rolling.snapshot(19),
            Err(TransferQuotaError::ClockMovedBackwards)
        );
    }
}
