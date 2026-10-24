use super::*;

#[test]
fn splits_only_the_unfinished_tail_and_keeps_segment_ids() {
    let mut donor = Segment::new(7, 100, 100 + 8 * MIN_SPLIT_SIZE - 1);
    donor.downloaded = 2 * MIN_SPLIT_SIZE;
    let (left, right) = split_tail(&donor, 19).unwrap();
    assert_eq!(left.index, 7);
    assert_eq!(left.start, 100);
    assert_eq!(left.downloaded, donor.downloaded);
    assert_eq!(right.index, 19);
    assert_eq!(right.start, 100 + 5 * MIN_SPLIT_SIZE);
    assert_eq!(left.end + 1, right.start);
    assert_eq!(right.end, donor.end);
    assert_eq!(right.downloaded, 0);
    assert!(!right.complete);
    assert_eq!(left.size() + right.size(), donor.size());
}

#[test]
fn minimum_size_odd_sizes_and_repeated_splits_cover_the_original_range() {
    assert!(split_tail(&Segment::new(0, 0, 2 * MIN_SPLIT_SIZE - 2), 1).is_none());
    let donor = Segment::new(0, 0, 4 * MIN_SPLIT_SIZE);
    let (left, right) = split_tail(&donor, 1).unwrap();
    let (middle, last) = split_tail(&right, 2).unwrap();
    assert_eq!(left.end + 1, middle.start);
    assert_eq!(middle.end + 1, last.start);
    assert_eq!(last.end, donor.end);
    assert!(middle.size() >= MIN_SPLIT_SIZE);
    assert!(last.size() >= MIN_SPLIT_SIZE);
    let exact = Segment::new(4, 23, 23 + 2 * MIN_SPLIT_SIZE - 1);
    let (left, right) = split_tail(&exact, 5).unwrap();
    assert_eq!(left.size(), MIN_SPLIT_SIZE);
    assert_eq!(right.size(), MIN_SPLIT_SIZE);
}

#[test]
fn complete_unknown_and_invalid_segments_are_not_split() {
    let mut complete = Segment::new(0, 0, 8 * MIN_SPLIT_SIZE);
    complete.complete = true;
    for segment in [
        complete,
        Segment::new(1, 0, u64::MAX),
        Segment::new(2, 5, 4),
    ] {
        assert!(split_tail(&segment, 3).is_none());
    }
    let mut invalid = Segment::new(0, 0, 8 * MIN_SPLIT_SIZE);
    invalid.downloaded = invalid.size() + 1;
    assert!(split_tail(&invalid, 1).is_none());
}

#[test]
fn largest_candidate_uses_remaining_bytes_and_skips_unready_workers() {
    let mut large = Segment::new(2, 0, 16 * MIN_SPLIT_SIZE - 1);
    large.downloaded = 15 * MIN_SPLIT_SIZE;
    let candidates = [
        WorkerSnapshot {
            segment: large,
            ready: true,
        },
        WorkerSnapshot {
            segment: Segment::new(8, 0, 8 * MIN_SPLIT_SIZE - 1),
            ready: false,
        },
        WorkerSnapshot {
            segment: Segment::new(5, 0, 4 * MIN_SPLIT_SIZE - 1),
            ready: true,
        },
    ];
    assert_eq!(largest_candidate(candidates.iter()).unwrap(), 5);
}

#[test]
fn ownership_validation_uses_offsets_and_rejects_holes_overlap_and_duplicate_ids() {
    let segments = vec![
        Segment::new(3, 20, 29),
        Segment::new(0, 0, 9),
        Segment::new(9, 10, 19),
    ];
    assert!(validate_map(&segments, Some(30), false).is_ok());
    assert!(validate_map(&segments, Some(31), false).is_err());
    for (position, replacement) in [
        (0, Segment::new(3, 21, 29)),
        (0, Segment::new(3, 19, 29)),
        (0, Segment::new(9, 20, 29)),
    ] {
        let mut bad = segments.clone();
        bad[position] = replacement;
        assert!(validate_map(&bad, Some(30), false).is_err());
    }
    assert!(validate_map(&segments, Some(30), true).is_err());
}
