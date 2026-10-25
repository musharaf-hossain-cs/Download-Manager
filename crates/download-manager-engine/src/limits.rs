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

#[cfg(test)]
mod tests {
    use super::{BandwidthLimiter, ConnectionCapSnapshot, ConnectionLimiter};
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
}
