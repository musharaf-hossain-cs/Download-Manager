//! Deterministic resource caps for a site-grabber crawl.

use std::{collections::HashSet, time::Duration};

use thiserror::Error;

/// Resource boundary that refused additional crawl work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrawlCap {
    Depth,
    Pages,
    Bytes,
    Duration,
    Concurrency,
}

/// Validated limits for one crawl run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CrawlLimits {
    pub max_depth: u16,
    pub max_pages: usize,
    pub max_bytes: u64,
    pub max_duration: Duration,
    pub max_concurrency: usize,
}

impl CrawlLimits {
    /// Creates a cap set. Depth zero is valid and means that only the start page may run.
    ///
    /// # Errors
    /// Returns an error when a page, byte, duration, or concurrency limit is zero.
    pub const fn new(
        max_depth: u16,
        max_pages: usize,
        max_bytes: u64,
        max_duration: Duration,
        max_concurrency: usize,
    ) -> Result<Self, CrawlBudgetError> {
        if max_pages == 0 || max_bytes == 0 || max_duration.is_zero() || max_concurrency == 0 {
            return Err(CrawlBudgetError::InvalidLimits);
        }
        Ok(Self {
            max_depth,
            max_pages,
            max_bytes,
            max_duration,
            max_concurrency,
        })
    }
}

/// Opaque identity for one admitted page request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CrawlRequestId(u64);

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CrawlBudgetError {
    #[error("page, byte, duration, and concurrency limits must be non-zero")]
    InvalidLimits,
    #[error("crawl virtual time moved before its start")]
    ClockMovedBackwards,
    #[error("crawl request is unknown or already finished")]
    UnknownRequest,
    #[error("crawl stopped at its {0:?} cap")]
    CapReached(CrawlCap),
    #[error("crawl request identity space is exhausted")]
    RequestIdExhausted,
}

/// Stateful admission and byte accounting for one bounded crawl run.
///
/// Page count is charged when a request starts, including failed requests. Body bytes are charged
/// before callers retain a received chunk. This conservative accounting prevents retries or a
/// final oversized chunk from crossing a configured limit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CrawlBudget {
    limits: CrawlLimits,
    started_at: Duration,
    pages_started: usize,
    bytes_received: u64,
    next_request_id: u64,
    active_requests: HashSet<CrawlRequestId>,
}

impl CrawlBudget {
    #[must_use]
    pub fn new(limits: CrawlLimits, started_at: Duration) -> Self {
        Self {
            limits,
            started_at,
            pages_started: 0,
            bytes_received: 0,
            next_request_id: 0,
            active_requests: HashSet::new(),
        }
    }

    #[must_use]
    pub const fn pages_started(&self) -> usize {
        self.pages_started
    }

    #[must_use]
    pub const fn bytes_received(&self) -> u64 {
        self.bytes_received
    }

    #[must_use]
    pub fn active_requests(&self) -> usize {
        self.active_requests.len()
    }

    /// Admits one page at `depth`, charging the page cap atomically.
    ///
    /// Checks use a stable order: duration, depth, pages, then concurrency. Reaching the exact
    /// duration boundary refuses new work. A concurrency refusal is temporary; finishing an active
    /// request permits another page if the terminal caps still allow it.
    ///
    /// # Errors
    /// Returns the first reached cap, a regressed clock error, or identity exhaustion.
    pub fn start_page(
        &mut self,
        depth: u16,
        now: Duration,
    ) -> Result<CrawlRequestId, CrawlBudgetError> {
        self.check_duration(now)?;
        if depth > self.limits.max_depth {
            return Err(CrawlBudgetError::CapReached(CrawlCap::Depth));
        }
        if self.pages_started >= self.limits.max_pages {
            return Err(CrawlBudgetError::CapReached(CrawlCap::Pages));
        }
        if self.active_requests.len() >= self.limits.max_concurrency {
            return Err(CrawlBudgetError::CapReached(CrawlCap::Concurrency));
        }

        let request_id = CrawlRequestId(self.next_request_id);
        self.next_request_id = self
            .next_request_id
            .checked_add(1)
            .ok_or(CrawlBudgetError::RequestIdExhausted)?;
        self.pages_started += 1;
        if !self.active_requests.insert(request_id) {
            self.pages_started -= 1;
            return Err(CrawlBudgetError::RequestIdExhausted);
        }
        Ok(request_id)
    }

    /// Charges a received body chunk before it is retained.
    ///
    /// # Errors
    /// Refuses unknown requests, a duration boundary, arithmetic overflow, or a chunk that would
    /// cross the byte cap. Refused chunks do not change accounting.
    pub fn record_bytes(
        &mut self,
        request_id: CrawlRequestId,
        bytes: u64,
        now: Duration,
    ) -> Result<(), CrawlBudgetError> {
        if !self.active_requests.contains(&request_id) {
            return Err(CrawlBudgetError::UnknownRequest);
        }
        self.check_duration(now)?;
        let Some(total) = self.bytes_received.checked_add(bytes) else {
            return Err(CrawlBudgetError::CapReached(CrawlCap::Bytes));
        };
        if total > self.limits.max_bytes {
            return Err(CrawlBudgetError::CapReached(CrawlCap::Bytes));
        }
        self.bytes_received = total;
        Ok(())
    }

    /// Releases one active request slot.
    ///
    /// # Errors
    /// Returns an error when the request is unknown or already finished.
    pub fn finish_page(&mut self, request_id: CrawlRequestId) -> Result<(), CrawlBudgetError> {
        if self.active_requests.remove(&request_id) {
            Ok(())
        } else {
            Err(CrawlBudgetError::UnknownRequest)
        }
    }

    fn check_duration(&self, now: Duration) -> Result<(), CrawlBudgetError> {
        let elapsed = now
            .checked_sub(self.started_at)
            .ok_or(CrawlBudgetError::ClockMovedBackwards)?;
        if elapsed >= self.limits.max_duration {
            Err(CrawlBudgetError::CapReached(CrawlCap::Duration))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::{CrawlBudget, CrawlBudgetError, CrawlCap, CrawlLimits};

    fn limits() -> CrawlLimits {
        CrawlLimits::new(2, 3, 10, Duration::from_secs(5), 2).expect("valid limits")
    }

    #[test]
    fn depth_page_and_duration_caps_stop_at_exact_boundaries() {
        let mut budget = CrawlBudget::new(limits(), Duration::from_secs(10));
        assert_eq!(
            budget.start_page(3, Duration::from_secs(10)),
            Err(CrawlBudgetError::CapReached(CrawlCap::Depth))
        );
        let first = budget
            .start_page(0, Duration::from_secs(10))
            .expect("root page");
        budget.finish_page(first).expect("finish root");
        let second = budget
            .start_page(1, Duration::from_secs(11))
            .expect("second page");
        budget.finish_page(second).expect("finish second");
        let third = budget
            .start_page(2, Duration::from_secs(12))
            .expect("third page");
        budget.finish_page(third).expect("finish third");
        assert_eq!(
            budget.start_page(0, Duration::from_secs(12)),
            Err(CrawlBudgetError::CapReached(CrawlCap::Pages))
        );

        let mut duration = CrawlBudget::new(limits(), Duration::from_secs(10));
        assert_eq!(
            duration.start_page(0, Duration::from_secs(15)),
            Err(CrawlBudgetError::CapReached(CrawlCap::Duration))
        );
        assert_eq!(
            duration.start_page(0, Duration::from_secs(9)),
            Err(CrawlBudgetError::ClockMovedBackwards)
        );
    }

    #[test]
    fn byte_cap_accepts_the_exact_limit_and_refuses_crossing_chunks_atomically() {
        let mut budget = CrawlBudget::new(limits(), Duration::ZERO);
        let request = budget.start_page(0, Duration::ZERO).expect("page");
        budget
            .record_bytes(request, 6, Duration::from_secs(1))
            .expect("first chunk");
        assert_eq!(
            budget.record_bytes(request, 5, Duration::from_secs(2)),
            Err(CrawlBudgetError::CapReached(CrawlCap::Bytes))
        );
        assert_eq!(budget.bytes_received(), 6);
        budget
            .record_bytes(request, 4, Duration::from_secs(2))
            .expect("exact remaining bytes");
        assert_eq!(budget.bytes_received(), 10);
        assert_eq!(
            budget.record_bytes(request, 1, Duration::from_secs(2)),
            Err(CrawlBudgetError::CapReached(CrawlCap::Bytes))
        );
    }

    #[test]
    fn concurrency_refusal_is_released_only_by_the_matching_request() {
        let mut budget = CrawlBudget::new(limits(), Duration::ZERO);
        let first = budget.start_page(0, Duration::ZERO).expect("first");
        let second = budget.start_page(1, Duration::ZERO).expect("second");
        assert_eq!(budget.active_requests(), 2);
        assert_eq!(
            budget.start_page(2, Duration::ZERO),
            Err(CrawlBudgetError::CapReached(CrawlCap::Concurrency))
        );
        budget.finish_page(first).expect("release first");
        let third = budget.start_page(2, Duration::ZERO).expect("third");
        assert_eq!(budget.active_requests(), 2);
        assert_eq!(
            budget.finish_page(first),
            Err(CrawlBudgetError::UnknownRequest)
        );
        budget.finish_page(second).expect("release second");
        budget.finish_page(third).expect("release third");
    }

    #[test]
    fn invalid_limits_are_rejected() {
        assert_eq!(
            CrawlLimits::new(0, 0, 1, Duration::from_secs(1), 1),
            Err(CrawlBudgetError::InvalidLimits)
        );
        assert_eq!(
            CrawlLimits::new(0, 1, 0, Duration::from_secs(1), 1),
            Err(CrawlBudgetError::InvalidLimits)
        );
        assert_eq!(
            CrawlLimits::new(0, 1, 1, Duration::ZERO, 1),
            Err(CrawlBudgetError::InvalidLimits)
        );
        assert_eq!(
            CrawlLimits::new(0, 1, 1, Duration::from_secs(1), 0),
            Err(CrawlBudgetError::InvalidLimits)
        );
    }
}
