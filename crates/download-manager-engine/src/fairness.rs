//! Weighted fair selection across active download jobs.

use std::collections::HashSet;

use thiserror::Error;

use crate::domain::{JobId, Priority};

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum FairnessError {
    #[error("job is already registered")]
    DuplicateJob,
    #[error("job is not registered")]
    UnknownJob,
}

#[derive(Debug, Clone, Copy)]
struct FairJob {
    id: JobId,
    priority: Priority,
    active: bool,
    credits: u8,
}

impl FairJob {
    const fn weight(self) -> u8 {
        match self.priority {
            Priority::High => 4,
            Priority::Normal => 2,
            Priority::Low => 1,
        }
    }
}

/// Deficit-style weighted round robin with finite credit for every active job.
#[derive(Debug, Clone, Default)]
pub struct WeightedFairScheduler {
    jobs: Vec<FairJob>,
    known: HashSet<JobId>,
    cursor: usize,
}

impl WeightedFairScheduler {
    /// Registers an active job at the requested priority.
    ///
    /// # Errors
    /// Returns [`FairnessError::DuplicateJob`] if the ID already exists.
    pub fn register(&mut self, id: JobId, priority: Priority) -> Result<(), FairnessError> {
        if !self.known.insert(id) {
            return Err(FairnessError::DuplicateJob);
        }
        let job = FairJob {
            id,
            priority,
            active: true,
            credits: 0,
        };
        self.jobs.push(job);
        Ok(())
    }

    /// Activates or pauses an existing job without changing its stable queue position.
    ///
    /// # Errors
    /// Returns [`FairnessError::UnknownJob`] for an unregistered ID.
    pub fn set_active(&mut self, id: JobId, active: bool) -> Result<(), FairnessError> {
        let job = self
            .jobs
            .iter_mut()
            .find(|job| job.id == id)
            .ok_or(FairnessError::UnknownJob)?;
        job.active = active;
        if !active {
            job.credits = 0;
        }
        Ok(())
    }

    /// Selects one active job. Every active low-priority job is selected once per seven-credit
    /// cycle when competing with one normal and one high-priority job.
    #[must_use]
    pub fn next_job(&mut self) -> Option<JobId> {
        if !self.jobs.iter().any(|job| job.active) {
            return None;
        }
        if !self.jobs.iter().any(|job| job.active && job.credits > 0) {
            for job in &mut self.jobs {
                job.credits = if job.active { job.weight() } else { 0 };
            }
        }
        for _ in 0..self.jobs.len() {
            let index = self.cursor % self.jobs.len();
            self.cursor = (index + 1) % self.jobs.len();
            let job = &mut self.jobs[index];
            if job.active && job.credits > 0 {
                job.credits -= 1;
                return Some(job.id);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::WeightedFairScheduler;
    use crate::domain::{JobId, Priority};

    #[test]
    fn priority_has_weighted_advantage_without_starvation() {
        let high = JobId::new();
        let normal = JobId::new();
        let low = JobId::new();
        let mut scheduler = WeightedFairScheduler::default();
        scheduler.register(high, Priority::High).expect("high");
        scheduler
            .register(normal, Priority::Normal)
            .expect("normal");
        scheduler.register(low, Priority::Low).expect("low");

        let mut counts = HashMap::new();
        let mut longest_low_gap = 0_usize;
        let mut since_low = 0_usize;
        for _ in 0..700 {
            let selected = scheduler.next_job().expect("active job");
            *counts.entry(selected).or_insert(0_usize) += 1;
            if selected == low {
                longest_low_gap = longest_low_gap.max(since_low);
                since_low = 0;
            } else {
                since_low += 1;
            }
        }
        assert_eq!(counts[&high], 400);
        assert_eq!(counts[&normal], 200);
        assert_eq!(counts[&low], 100);
        assert!(longest_low_gap <= 6);

        scheduler.set_active(high, false).expect("pause high");
        assert!((0..3).any(|_| scheduler.next_job() == Some(low)));
    }
}
