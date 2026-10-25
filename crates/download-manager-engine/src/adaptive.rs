//! Adaptive per-job connection-count controller.

use thiserror::Error;

const MIN_PRODUCTIVE_BYTES_PER_SECOND: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencySample {
    pub active_connections: u8,
    pub completed_bytes: u64,
    pub elapsed_millis: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PressureSample {
    pub retry_count: u32,
    pub throttled: bool,
    pub network_loss: bool,
    pub pending_disk_writes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BackoffReason {
    Throttled,
    RetryOrLoss,
    DiskPressure,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BackoffDecision {
    pub target: u8,
    pub reason: Option<BackoffReason>,
}

impl ConcurrencySample {
    fn bytes_per_second(self) -> u64 {
        if self.elapsed_millis == 0 {
            return 0;
        }
        self.completed_bytes
            .saturating_mul(1_000)
            .saturating_div(self.elapsed_millis)
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum AdaptiveError {
    #[error("maximum concurrency must be between 1 and 32")]
    InvalidMaximum,
}

/// Ramps a job from one connection while useful parallel capacity remains.
#[derive(Debug, Clone)]
pub struct AutoConcurrencyController {
    current: u8,
    maximum: u8,
    peak_goodput: u64,
    cooldown_intervals: u8,
}

impl AutoConcurrencyController {
    /// Creates a controller with a hard upper bound in 1..=32.
    ///
    /// # Errors
    /// Returns [`AdaptiveError::InvalidMaximum`] outside the supported connection range.
    pub fn new(maximum: u8) -> Result<Self, AdaptiveError> {
        if !(1..=32).contains(&maximum) {
            return Err(AdaptiveError::InvalidMaximum);
        }
        Ok(Self {
            current: 1,
            maximum,
            peak_goodput: 0,
            cooldown_intervals: 0,
        })
    }

    #[must_use]
    pub const fn target(&self) -> u8 {
        self.current
    }

    /// Observes a stable transfer interval and returns the next connection target.
    pub fn observe_productive_interval(&mut self, sample: ConcurrencySample) -> u8 {
        let goodput = sample.bytes_per_second();
        let all_workers_busy = sample.active_connections >= self.current;
        let productive = goodput >= MIN_PRODUCTIVE_BYTES_PER_SECOND;
        let retains_capacity = self.peak_goodput == 0
            || goodput.saturating_mul(100) >= self.peak_goodput.saturating_mul(95);
        self.peak_goodput = self.peak_goodput.max(goodput);
        if self.cooldown_intervals > 0 {
            self.cooldown_intervals -= 1;
        } else if all_workers_busy && productive && retains_capacity && self.current < self.maximum
        {
            self.current = self.current.saturating_mul(2).min(self.maximum);
        }
        self.current
    }

    /// Applies immediate multiplicative decrease for server, network, or storage pressure.
    pub fn observe_pressure(&mut self, sample: PressureSample) -> BackoffDecision {
        let reason = if sample.throttled {
            Some(BackoffReason::Throttled)
        } else if sample.network_loss || sample.retry_count > 0 {
            Some(BackoffReason::RetryOrLoss)
        } else if sample.pending_disk_writes > u32::from(self.current).saturating_mul(2) {
            Some(BackoffReason::DiskPressure)
        } else {
            None
        };
        if reason.is_some() {
            self.current = (self.current / 2).max(1);
            self.cooldown_intervals = 2;
            self.peak_goodput = 0;
        }
        BackoffDecision {
            target: self.current,
            reason,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoConcurrencyController, BackoffReason, ConcurrencySample, PressureSample};

    struct ShapedFastServer {
        bytes_per_connection_per_interval: u64,
    }

    impl ShapedFastServer {
        fn sample(&self, connections: u8) -> ConcurrencySample {
            ConcurrencySample {
                active_connections: connections,
                completed_bytes: self
                    .bytes_per_connection_per_interval
                    .saturating_mul(u64::from(connections)),
                elapsed_millis: 1_000,
            }
        }
    }

    #[test]
    fn shaped_fast_server_ramps_from_one_to_bounded_thirty_two() {
        let server = ShapedFastServer {
            bytes_per_connection_per_interval: 8 * 1024 * 1024,
        };
        let mut controller = AutoConcurrencyController::new(32).expect("controller");
        let mut targets = Vec::new();
        for _ in 0..8 {
            targets
                .push(controller.observe_productive_interval(server.sample(controller.target())));
        }
        assert_eq!(&targets[..5], &[2, 4, 8, 16, 32]);
        assert!(targets[5..].iter().all(|target| *target == 32));

        let mut capped = AutoConcurrencyController::new(7).expect("capped controller");
        for _ in 0..8 {
            capped.observe_productive_interval(server.sample(capped.target()));
        }
        assert_eq!(capped.target(), 7);
    }

    #[test]
    fn throttle_loss_and_slow_disk_fixtures_reduce_load_and_hold_cooldown() {
        let server = ShapedFastServer {
            bytes_per_connection_per_interval: 8 * 1024 * 1024,
        };
        let mut baseline = AutoConcurrencyController::new(32).expect("controller");
        for _ in 0..4 {
            baseline.observe_productive_interval(server.sample(baseline.target()));
        }
        assert_eq!(baseline.target(), 16);

        for (pressure, reason) in [
            (
                PressureSample {
                    throttled: true,
                    ..PressureSample::default()
                },
                BackoffReason::Throttled,
            ),
            (
                PressureSample {
                    retry_count: 2,
                    network_loss: true,
                    ..PressureSample::default()
                },
                BackoffReason::RetryOrLoss,
            ),
            (
                PressureSample {
                    pending_disk_writes: 64,
                    ..PressureSample::default()
                },
                BackoffReason::DiskPressure,
            ),
        ] {
            let mut controller = baseline.clone();
            let decision = controller.observe_pressure(pressure);
            assert_eq!(decision.target, 8);
            assert_eq!(decision.reason, Some(reason));
            assert_eq!(
                controller.observe_productive_interval(server.sample(controller.target())),
                8
            );
            assert_eq!(
                controller.observe_productive_interval(server.sample(controller.target())),
                8
            );
            assert_eq!(
                controller.observe_productive_interval(server.sample(controller.target())),
                16
            );
        }
    }
}
