//! Global/per-job bandwidth and global/per-host connection limits.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use thiserror::Error;

use crate::domain::JobId;

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LimitError {
    #[error("rate and bucket capacity must be greater than zero")]
    InvalidRate,
    #[error("job has no configured bandwidth limit")]
    UnknownJob,
    #[error("connection caps must be greater than zero")]
    InvalidConnectionCap,
    #[error("queue name must contain 1 to 256 characters without surrounding whitespace")]
    InvalidQueueName,
    #[error("queue concurrent-download cap must be greater than zero")]
    InvalidQueueCap,
    #[error("queue has no configured concurrent-download cap")]
    UnknownQueue,
}

#[derive(Debug, Clone)]
struct TokenBucket {
    rate_per_second: u64,
    capacity: u64,
    tokens: u64,
    remainder_millis: u64,
    last_millis: u64,
}

impl TokenBucket {
    fn new(rate_per_second: u64, capacity: u64) -> Result<Self, LimitError> {
        if rate_per_second == 0 || capacity == 0 {
            return Err(LimitError::InvalidRate);
        }
        Ok(Self {
            rate_per_second,
            capacity,
            tokens: capacity,
            remainder_millis: 0,
            last_millis: 0,
        })
    }

    fn refill(&mut self, now_millis: u64) {
        let elapsed = now_millis.saturating_sub(self.last_millis);
        let produced_millis = u128::from(self.rate_per_second)
            .saturating_mul(u128::from(elapsed))
            .saturating_add(u128::from(self.remainder_millis));
        let produced = produced_millis / 1_000;
        self.remainder_millis = u64::try_from(produced_millis % 1_000).expect("remainder fits");
        self.tokens = u64::try_from(u128::from(self.tokens).saturating_add(produced))
            .unwrap_or(u64::MAX)
            .min(self.capacity);
        self.last_millis = now_millis.max(self.last_millis);
    }

    fn consume(&mut self, bytes: u64) {
        self.tokens -= bytes;
    }
}

/// Applies both an aggregate global rate and an optional stricter rate for every job.
#[derive(Debug, Clone)]
pub struct BandwidthLimiter {
    global: TokenBucket,
    jobs: HashMap<JobId, TokenBucket>,
}

impl BandwidthLimiter {
    /// Creates the global bucket.
    ///
    /// # Errors
    /// Returns an error for a zero rate or capacity.
    pub fn new(global_rate: u64, global_capacity: u64) -> Result<Self, LimitError> {
        Ok(Self {
            global: TokenBucket::new(global_rate, global_capacity)?,
            jobs: HashMap::new(),
        })
    }

    /// Adds or replaces one job's bucket.
    ///
    /// # Errors
    /// Returns an error for a zero rate or capacity.
    pub fn configure_job(
        &mut self,
        job_id: JobId,
        rate: u64,
        capacity: u64,
    ) -> Result<(), LimitError> {
        self.jobs.insert(job_id, TokenBucket::new(rate, capacity)?);
        Ok(())
    }

    /// Grants bytes available from both buckets at a virtual monotonic timestamp.
    ///
    /// # Errors
    /// Returns an error when the job has not been configured.
    pub fn grant(
        &mut self,
        job_id: JobId,
        now_millis: u64,
        requested: u64,
    ) -> Result<u64, LimitError> {
        self.global.refill(now_millis);
        let global_available = self.global.tokens;
        let granted = {
            let job = self.jobs.get_mut(&job_id).ok_or(LimitError::UnknownJob)?;
            job.refill(now_millis);
            let granted = requested.min(global_available).min(job.tokens);
            job.consume(granted);
            granted
        };
        self.global.consume(granted);
        Ok(granted)
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConnectionCapSnapshot {
    pub active_global: u32,
    pub peak_global: u32,
}

#[derive(Debug)]
struct ConnectionState {
    global_limit: u32,
    per_host_limit: u32,
    active_global: u32,
    peak_global: u32,
    hosts: HashMap<String, u32>,
}

/// Cloneable permit source enforcing aggregate and normalized-host caps.
#[derive(Debug, Clone)]
pub struct ConnectionLimiter {
    state: Arc<Mutex<ConnectionState>>,
}

impl ConnectionLimiter {
    /// Creates a limiter with non-zero hard caps.
    ///
    /// # Errors
    /// Returns an error if either cap is zero.
    pub fn new(global_limit: u32, per_host_limit: u32) -> Result<Self, LimitError> {
        if global_limit == 0 || per_host_limit == 0 {
            return Err(LimitError::InvalidConnectionCap);
        }
        Ok(Self {
            state: Arc::new(Mutex::new(ConnectionState {
                global_limit,
                per_host_limit,
                active_global: 0,
                peak_global: 0,
                hosts: HashMap::new(),
            })),
        })
    }

    /// Attempts to reserve one connection until the returned permit is dropped.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal limiter lock.
    #[must_use]
    pub fn try_acquire(&self, host: &str) -> Option<ConnectionPermit> {
        let normalized = host.to_ascii_lowercase();
        let mut state = self.state.lock().expect("connection limiter lock");
        let host_active = state.hosts.get(&normalized).copied().unwrap_or(0);
        if state.active_global >= state.global_limit || host_active >= state.per_host_limit {
            return None;
        }
        state.active_global += 1;
        state.peak_global = state.peak_global.max(state.active_global);
        *state.hosts.entry(normalized.clone()).or_default() += 1;
        drop(state);
        Some(ConnectionPermit {
            state: Arc::clone(&self.state),
            host: normalized,
        })
    }

    /// Returns aggregate active/peak counts.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal limiter lock.
    #[must_use]
    pub fn snapshot(&self) -> ConnectionCapSnapshot {
        let state = self.state.lock().expect("connection limiter lock");
        ConnectionCapSnapshot {
            active_global: state.active_global,
            peak_global: state.peak_global,
        }
    }
}

#[derive(Debug)]
pub struct ConnectionPermit {
    state: Arc<Mutex<ConnectionState>>,
    host: String,
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        let mut state = self.state.lock().expect("connection limiter lock");
        state.active_global -= 1;
        if let Some(active) = state.hosts.get_mut(&self.host) {
            *active -= 1;
            if *active == 0 {
                state.hosts.remove(&self.host);
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QueueConcurrencySnapshot {
    pub limit: u32,
    pub active: u32,
    pub peak: u32,
}

#[derive(Debug)]
struct QueueConcurrencyEntry {
    limit: u32,
    active: u32,
    peak: u32,
}

/// Cloneable permit source enforcing an independent concurrent-download cap per named queue.
#[derive(Debug, Clone, Default)]
pub struct QueueConcurrencyLimiter {
    queues: Arc<Mutex<HashMap<String, QueueConcurrencyEntry>>>,
}

impl QueueConcurrencyLimiter {
    /// Adds or replaces one named queue's non-zero concurrent-download cap.
    ///
    /// Reconfiguring an active queue keeps its current count. If the new cap is below that count,
    /// no additional permits are granted until enough active permits have been released.
    ///
    /// # Errors
    /// Returns an error for an invalid queue name or a zero cap.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal limiter lock.
    pub fn configure_queue(&self, queue_name: &str, limit: u32) -> Result<(), LimitError> {
        validate_queue_name(queue_name)?;
        if limit == 0 {
            return Err(LimitError::InvalidQueueCap);
        }
        let mut queues = self.queues.lock().expect("queue limiter lock");
        let entry = queues
            .entry(queue_name.to_owned())
            .or_insert(QueueConcurrencyEntry {
                limit,
                active: 0,
                peak: 0,
            });
        entry.limit = limit;
        Ok(())
    }

    /// Attempts to reserve one active download slot until the returned permit is dropped.
    ///
    /// # Errors
    /// Returns an error when the queue has not been configured.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal limiter lock.
    pub fn try_acquire(
        &self,
        queue_name: &str,
    ) -> Result<Option<QueueConcurrencyPermit>, LimitError> {
        let mut queues = self.queues.lock().expect("queue limiter lock");
        let entry = queues.get_mut(queue_name).ok_or(LimitError::UnknownQueue)?;
        if entry.active >= entry.limit {
            return Ok(None);
        }
        entry.active += 1;
        entry.peak = entry.peak.max(entry.active);
        Ok(Some(QueueConcurrencyPermit {
            queues: Arc::clone(&self.queues),
            queue_name: queue_name.to_owned(),
        }))
    }

    /// Returns the current, peak, and configured counts for one named queue.
    ///
    /// # Errors
    /// Returns an error when the queue has not been configured.
    ///
    /// # Panics
    /// Panics if another thread poisoned the internal limiter lock.
    pub fn snapshot(&self, queue_name: &str) -> Result<QueueConcurrencySnapshot, LimitError> {
        let queues = self.queues.lock().expect("queue limiter lock");
        let entry = queues.get(queue_name).ok_or(LimitError::UnknownQueue)?;
        Ok(QueueConcurrencySnapshot {
            limit: entry.limit,
            active: entry.active,
            peak: entry.peak,
        })
    }
}

#[derive(Debug)]
pub struct QueueConcurrencyPermit {
    queues: Arc<Mutex<HashMap<String, QueueConcurrencyEntry>>>,
    queue_name: String,
}

impl Drop for QueueConcurrencyPermit {
    fn drop(&mut self) {
        let mut queues = self.queues.lock().expect("queue limiter lock");
        let entry = queues
            .get_mut(&self.queue_name)
            .expect("configured queue remains while permits exist");
        entry.active -= 1;
    }
}

fn validate_queue_name(queue_name: &str) -> Result<(), LimitError> {
    let length = queue_name.chars().count();
    if queue_name.trim() != queue_name || !(1..=256).contains(&length) {
        return Err(LimitError::InvalidQueueName);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        BandwidthLimiter, ConnectionCapSnapshot, ConnectionLimiter, LimitError,
        QueueConcurrencyLimiter, QueueConcurrencySnapshot,
    };
    use crate::domain::JobId;

    #[test]
    fn virtual_time_global_and_job_rates_stay_within_five_percent() {
        let first = JobId::new();
        let second = JobId::new();
        let mut limiter = BandwidthLimiter::new(100_000, 10_000).expect("global limit");
        limiter
            .configure_job(first, 60_000, 6_000)
            .expect("first job");
        limiter
            .configure_job(second, 80_000, 8_000)
            .expect("second job");
        let mut first_bytes = 0_u64;
        let mut total_bytes = 0_u64;
        for now in (0..=10_000).step_by(100) {
            let first_grant = limiter.grant(first, now, 100_000).expect("grant");
            let second_grant = limiter.grant(second, now, 100_000).expect("grant");
            first_bytes += first_grant;
            total_bytes += first_grant + second_grant;
        }
        assert!((600_000..=630_000).contains(&first_bytes));
        assert!((1_000_000..=1_050_000).contains(&total_bytes));
    }

    #[test]
    fn global_and_per_host_caps_never_exceed_under_many_jobs() {
        let limiter = ConnectionLimiter::new(3, 2).expect("limiter");
        let mut permits = Vec::new();
        for host in [
            "example.test",
            "EXAMPLE.TEST",
            "example.test",
            "other.test",
            "third.test",
        ] {
            if let Some(permit) = limiter.try_acquire(host) {
                permits.push(permit);
            }
        }
        assert_eq!(permits.len(), 3);
        assert_eq!(
            limiter.snapshot(),
            ConnectionCapSnapshot {
                active_global: 3,
                peak_global: 3
            }
        );
        assert!(limiter.try_acquire("fourth.test").is_none());
        permits.clear();
        assert_eq!(limiter.snapshot().active_global, 0);
        assert!(limiter.try_acquire("example.test").is_some());
    }

    #[test]
    fn per_queue_caps_never_exceed_under_mixed_jobs() {
        let limiter = QueueConcurrencyLimiter::default();
        limiter.configure_queue("Night", 2).expect("night cap");
        limiter.configure_queue("Work", 1).expect("work cap");

        let night_one = limiter
            .try_acquire("Night")
            .expect("known queue")
            .expect("first night job");
        let night_two = limiter
            .try_acquire("Night")
            .expect("known queue")
            .expect("second night job");
        assert!(limiter.try_acquire("Night").expect("known queue").is_none());
        let work = limiter
            .try_acquire("Work")
            .expect("known queue")
            .expect("first work job");
        assert!(limiter.try_acquire("Work").expect("known queue").is_none());
        assert_eq!(
            limiter.snapshot("Night").expect("night snapshot"),
            QueueConcurrencySnapshot {
                limit: 2,
                active: 2,
                peak: 2,
            }
        );
        assert_eq!(
            limiter.snapshot("Work").expect("work snapshot"),
            QueueConcurrencySnapshot {
                limit: 1,
                active: 1,
                peak: 1,
            }
        );

        drop(night_one);
        let replacement = limiter
            .try_acquire("Night")
            .expect("known queue")
            .expect("released night slot");
        drop((night_two, replacement, work));
        assert_eq!(limiter.snapshot("Night").expect("night snapshot").active, 0);
        assert_eq!(limiter.snapshot("Work").expect("work snapshot").active, 0);
    }

    #[test]
    fn queue_caps_reject_invalid_configuration_and_unknown_queues() {
        let limiter = QueueConcurrencyLimiter::default();
        assert_eq!(
            limiter.configure_queue(" Night", 1),
            Err(LimitError::InvalidQueueName)
        );
        assert_eq!(
            limiter.configure_queue("Night", 0),
            Err(LimitError::InvalidQueueCap)
        );
        assert!(matches!(
            limiter.try_acquire("Missing"),
            Err(LimitError::UnknownQueue)
        ));
    }
}
