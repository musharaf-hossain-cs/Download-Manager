//! Dynamic segment scheduling and work stealing.

use thiserror::Error;

use crate::segmented::ByteRange;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WorkAssignment {
    pub segment_id: u64,
    pub range: ByteRange,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ScheduledSegment {
    id: u64,
    start: u64,
    end: u64,
    next_offset: u64,
    active: bool,
}

impl ScheduledSegment {
    const fn remaining(&self) -> u64 {
        if self.next_offset > self.end {
            0
        } else {
            self.end - self.next_offset + 1
        }
    }
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SchedulerError {
    #[error("initial ranges must be non-empty, ordered, and adjacent")]
    InvalidRangeMap,
    #[error("unknown segment {0}")]
    UnknownSegment(u64),
    #[error("offset {offset} is outside segment {segment_id}")]
    InvalidOffset { segment_id: u64, offset: u64 },
}

/// Tracks active byte ranges and splits unfinished work for newly idle workers.
#[derive(Debug, Clone)]
pub struct DynamicScheduler {
    segments: Vec<ScheduledSegment>,
    next_id: u64,
}

impl DynamicScheduler {
    /// Creates a scheduler over an ordered, gap-free range map.
    ///
    /// # Errors
    /// Returns [`SchedulerError::InvalidRangeMap`] for empty, overlapping, or gapped input.
    pub fn new(ranges: &[ByteRange]) -> Result<Self, SchedulerError> {
        if ranges.is_empty()
            || ranges.iter().any(|range| range.is_empty())
            || ranges
                .windows(2)
                .any(|pair| pair[0].end.checked_add(1) != Some(pair[1].start))
        {
            return Err(SchedulerError::InvalidRangeMap);
        }
        let mut next_id = 0_u64;
        let segments = ranges
            .iter()
            .map(|range| {
                let segment = ScheduledSegment {
                    id: next_id,
                    start: range.start,
                    end: range.end,
                    next_offset: range.start,
                    active: true,
                };
                next_id += 1;
                segment
            })
            .collect();
        Ok(Self { segments, next_id })
    }

    /// Records the next byte not yet completed by a worker.
    ///
    /// # Errors
    /// Returns an error for an unknown segment or an offset outside its current bounds.
    pub fn advance(&mut self, segment_id: u64, next_offset: u64) -> Result<(), SchedulerError> {
        let segment = self
            .segments
            .iter_mut()
            .find(|segment| segment.id == segment_id)
            .ok_or(SchedulerError::UnknownSegment(segment_id))?;
        let after_end = segment.end.saturating_add(1);
        if next_offset < segment.next_offset || next_offset > after_end {
            return Err(SchedulerError::InvalidOffset {
                segment_id,
                offset: next_offset,
            });
        }
        segment.next_offset = next_offset;
        segment.active = next_offset <= segment.end;
        Ok(())
    }

    /// Splits the largest active remainder and assigns its tail to an idle worker.
    #[must_use]
    pub fn steal_largest_remaining(&mut self) -> Option<WorkAssignment> {
        let donor_index = self
            .segments
            .iter()
            .enumerate()
            .filter(|(_, segment)| segment.active && segment.remaining() >= 2)
            .max_by_key(|(_, segment)| (segment.remaining(), std::cmp::Reverse(segment.start)))
            .map(|(index, _)| index)?;
        let donor = &mut self.segments[donor_index];
        let old_end = donor.end;
        let stolen_start = donor.next_offset + donor.remaining() / 2;
        donor.end = stolen_start - 1;
        let assignment = WorkAssignment {
            segment_id: self.next_id,
            range: ByteRange {
                start: stolen_start,
                end: old_end,
            },
        };
        self.next_id += 1;
        self.segments.push(ScheduledSegment {
            id: assignment.segment_id,
            start: assignment.range.start,
            end: assignment.range.end,
            next_offset: assignment.range.start,
            active: true,
        });
        Some(assignment)
    }

    #[must_use]
    pub fn unfinished_ranges(&self) -> Vec<WorkAssignment> {
        let mut ranges: Vec<_> = self
            .segments
            .iter()
            .filter(|segment| segment.remaining() > 0)
            .map(|segment| WorkAssignment {
                segment_id: segment.id,
                range: ByteRange {
                    start: segment.next_offset,
                    end: segment.end,
                },
            })
            .collect();
        ranges.sort_by_key(|assignment| assignment.range.start);
        ranges
    }
}

#[cfg(test)]
mod tests {
    use super::DynamicScheduler;
    use crate::segmented::{ByteRange, plan_fixed_ranges};

    #[test]
    fn slow_worker_fixture_steals_from_largest_remaining_segment() {
        let initial = plan_fixed_ranges(1_000, 2).expect("plan");
        let mut scheduler = DynamicScheduler::new(&initial).expect("scheduler");

        scheduler.advance(0, 50).expect("slow worker progress");
        scheduler.advance(1, 1_000).expect("fast worker completes");
        let stolen = scheduler
            .steal_largest_remaining()
            .expect("idle worker steals");

        assert_eq!(stolen.segment_id, 2);
        assert_eq!(
            stolen.range,
            ByteRange {
                start: 275,
                end: 499
            }
        );
        assert_eq!(
            scheduler.unfinished_ranges(),
            vec![
                super::WorkAssignment {
                    segment_id: 0,
                    range: ByteRange {
                        start: 50,
                        end: 274
                    }
                },
                stolen
            ]
        );
        let unfinished = scheduler.unfinished_ranges();
        assert_eq!(unfinished[0].range.end + 1, unfinished[1].range.start);
        assert_eq!(
            unfinished.iter().map(|work| work.range.len()).sum::<u64>(),
            450
        );
    }
}
