//! Adaptive per-job connection-count controller.

use thiserror::Error;

const MIN_PRODUCTIVE_BYTES_PER_SECOND: u64 = 64 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConcurrencySample {
    pub active_connections: u8,
    pub completed_bytes: u64,
    pub elapsed_millis: u64,
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
        if all_workers_busy && productive && retains_capacity && self.current < self.maximum {
            self.current = self.current.saturating_mul(2).min(self.maximum);
        }
        self.current
    }
}

#[cfg(test)]
mod tests {
    use super::{AutoConcurrencyController, ConcurrencySample};

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
}
