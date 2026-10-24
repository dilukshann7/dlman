//! Range ownership and the private coordinator/worker control protocol.

use crate::error::DlmanError;
use dlman_types::Segment;
use tokio::sync::oneshot;

pub(super) const MIN_SPLIT_SIZE: u64 = 1024 * 1024;

#[derive(Clone, Debug)]
pub(super) struct WorkerSnapshot {
    pub segment: Segment,
    /// Set only after the worker has validated a bounded HTTP 206 response.
    pub ready: bool,
}

pub(super) struct SplitCommand {
    pub child_index: u32,
    pub reply: oneshot::Sender<Result<Option<(Segment, Segment)>, DlmanError>>,
}

/// Divide only the unreceived tail, leaving the donor's saved prefix in place.
pub(super) fn split_tail(segment: &Segment, child_index: u32) -> Option<(Segment, Segment)> {
    if segment.complete
        || segment.is_unknown_size()
        || segment.end < segment.start
        || segment.index == child_index
    {
        return None;
    }
    let remaining = segment.size().checked_sub(segment.downloaded)?;
    if remaining < 2 * MIN_SPLIT_SIZE {
        return None;
    }
    let next = segment.start.checked_add(segment.downloaded)?;
    let midpoint = next.checked_add(remaining / 2)?;
    let mut donor = segment.clone();
    donor.end = midpoint - 1;
    Some((donor, Segment::new(child_index, midpoint, segment.end)))
}

pub(super) fn largest_candidate<'a>(
    snapshots: impl Iterator<Item = &'a WorkerSnapshot>,
) -> Option<u32> {
    snapshots
        .filter(|snapshot| snapshot.ready)
        .filter_map(|snapshot| {
            let segment = &snapshot.segment;
            if segment.complete || segment.is_unknown_size() || segment.end < segment.start {
                return None;
            }
            let remaining = segment.size().checked_sub(segment.downloaded)?;
            (remaining >= 2 * MIN_SPLIT_SIZE)
                .then_some((remaining, std::cmp::Reverse(segment.index)))
        })
        .max()
        .map(|(_, index)| index.0)
}

/// Check ownership independently of segment IDs or persisted row order.
pub(super) fn validate_map(
    segments: &[Segment],
    total: Option<u64>,
    complete: bool,
) -> Result<(), DlmanError> {
    use super::refresh::invalid;
    if segments.is_empty() {
        return Err(invalid("Saved segment map is empty."));
    }
    if segments.len() == 1 && segments[0].start == 0 && segments[0].is_unknown_size() {
        if complete && !segments[0].complete {
            return Err(invalid("Cannot assemble an unfinished segment."));
        }
        return Ok(());
    }
    let mut ordered: Vec<_> = segments.iter().collect();
    ordered.sort_by_key(|segment| segment.start);
    let mut next = 0;
    let mut ids = std::collections::HashSet::new();
    for segment in ordered {
        if !ids.insert(segment.index)
            || segment.is_unknown_size()
            || segment.start != next
            || segment.end < segment.start
            || segment.downloaded > segment.size()
            || (complete && (!segment.complete || segment.downloaded != segment.size()))
        {
            return Err(invalid(
                "Saved segment boundaries or progress are inconsistent.",
            ));
        }
        next = segment
            .end
            .checked_add(1)
            .ok_or_else(|| invalid("Invalid segment end."))?;
    }
    if total.is_some_and(|total| total != next) {
        return Err(invalid(
            "Saved segment map does not cover the complete file.",
        ));
    }
    Ok(())
}

#[cfg(test)]
#[path = "segmentation_tests.rs"]
mod tests;
