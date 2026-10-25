//! Byte- and time-bounded durable checkpoint scheduling.

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointPolicy {
    pub maximum_interval: Duration,
    pub maximum_uncheckpointed_bytes: u64,
}

impl Default for CheckpointPolicy {
    fn default() -> Self {
        Self {
            maximum_interval: Duration::from_secs(5),
            maximum_uncheckpointed_bytes: 8 * 1024 * 1024,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CheckpointTracker {
    policy: CheckpointPolicy,
    checkpointed_bytes: u64,
    checkpointed_at: Duration,
}

impl CheckpointTracker {
    #[must_use]
    pub const fn new(
        policy: CheckpointPolicy,
        checkpointed_bytes: u64,
        checkpointed_at: Duration,
    ) -> Self {
        Self {
            policy,
            checkpointed_bytes,
            checkpointed_at,
        }
    }

    #[must_use]
    pub fn is_due(self, current_bytes: u64, now: Duration) -> bool {
        current_bytes.saturating_sub(self.checkpointed_bytes)
            >= self.policy.maximum_uncheckpointed_bytes
            || now.saturating_sub(self.checkpointed_at) >= self.policy.maximum_interval
    }

    pub fn record(&mut self, current_bytes: u64, now: Duration) {
        self.checkpointed_bytes = current_bytes;
        self.checkpointed_at = now;
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CheckpointPolicy, CheckpointTracker};

    #[test]
    fn fake_clock_and_byte_threshold_bound_checkpoint_writes() {
        let policy = CheckpointPolicy::default();
        let mut tracker = CheckpointTracker::new(policy, 0, Duration::ZERO);
        let mib = 1024 * 1024;
        let mut checkpoints = Vec::new();
        for second in 1..=20 {
            let bytes = second * mib;
            let now = Duration::from_secs(second);
            if tracker.is_due(bytes, now) {
                checkpoints.push((bytes, now));
                tracker.record(bytes, now);
                assert!(!tracker.is_due(bytes, now));
            }
        }
        assert_eq!(checkpoints.len(), 4);
        assert!(
            checkpoints
                .windows(2)
                .all(|window| window[1].1.saturating_sub(window[0].1) == Duration::from_secs(5))
        );

        let mut byte_limited = CheckpointTracker::new(policy, 0, Duration::ZERO);
        assert!(!byte_limited.is_due(8 * mib - 1, Duration::from_secs(1)));
        assert!(byte_limited.is_due(8 * mib, Duration::from_secs(1)));
        byte_limited.record(8 * mib, Duration::from_secs(1));
        assert!(!byte_limited.is_due(8 * mib, Duration::from_secs(1)));
    }
}
