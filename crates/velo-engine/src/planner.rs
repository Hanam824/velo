//! The segment planner: decides which byte range an idle worker fetches next.
//!
//! These are pure functions over a download's segment list, so the engine calls them under its
//! own lock and the tests can drive them without a network. Every function keeps the list
//! sorted by `start` and tiling `[0, total size)`.

use velo_core::{Segment, SegmentId, SegmentState};

/// Returns the segment list for a new download of `total` bytes: one pending segment covering
/// everything, or nothing if `total` is 0.
pub fn initial_segments(total: u64) -> Vec<Segment> {
    if total == 0 {
        return Vec::new();
    }
    vec![Segment::new(SegmentId(0), 0, total)]
}

/// Picks the segment an idle worker should fetch next and marks it active.
///
/// A pending segment with bytes left comes first (the one with the most bytes left), so work
/// left over from a stopped worker or a resume is picked up before anything is split. Otherwise
/// the active segment with the most bytes left is split with [`split_largest`]. Returns `None`
/// when there is nothing worth another connection.
pub fn next_work(segments: &mut Vec<Segment>, min_segment_size: u64) -> Option<SegmentId> {
    if let Some(index) = largest_remaining(segments, SegmentState::Pending) {
        let segment = &mut segments[index];
        segment.state = SegmentState::Active;
        return Some(segment.id);
    }
    split_largest(segments, min_segment_size)
}

/// Splits the active segment with the most bytes left in half and returns the new upper half,
/// which is marked active for the caller's worker.
///
/// The original segment's `end` shrinks to the midpoint of its remaining bytes, so its worker
/// must stop there. Nothing is split unless both halves would have at least `min_segment_size`
/// bytes left; then this returns `None`.
pub fn split_largest(segments: &mut Vec<Segment>, min_segment_size: u64) -> Option<SegmentId> {
    let index = largest_remaining(segments, SegmentState::Active)?;
    let half = segments[index].remaining() / 2;
    if half == 0 || half < min_segment_size {
        return None;
    }
    let id = next_id(segments);
    let original = &mut segments[index];
    let mid = original.position() + half;
    let mut upper = Segment::new(id, mid, original.end);
    upper.state = SegmentState::Active;
    original.end = mid;
    segments.insert(index + 1, upper);
    Some(id)
}

/// Index of the segment in `state` with the most bytes left, the lowest `start` on a tie;
/// `None` if no such segment has bytes left.
fn largest_remaining(segments: &[Segment], state: SegmentState) -> Option<usize> {
    let mut best: Option<usize> = None;
    for (index, segment) in segments.iter().enumerate() {
        if segment.state != state || segment.remaining() == 0 {
            continue;
        }
        if best.is_none_or(|b| segment.remaining() > segments[b].remaining()) {
            best = Some(index);
        }
    }
    best
}

/// An id not used by any segment in the list.
fn next_id(segments: &[Segment]) -> SegmentId {
    SegmentId(segments.iter().map(|s| s.id.0 + 1).max().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use proptest::prelude::*;

    use super::*;

    const KIB: u64 = 1024;

    /// Asserts the planner's invariants: sorted, tiling `[0, total)`, unique ids and no segment
    /// downloaded past its end.
    fn assert_tiles(segments: &[Segment], total: u64) {
        let mut position = 0;
        let mut ids = HashSet::new();
        for segment in segments {
            assert_eq!(segment.start, position, "gap or overlap in {segments:?}");
            assert!(segment.start < segment.end, "empty segment in {segments:?}");
            assert!(
                segment.downloaded <= segment.len(),
                "overrun in {segments:?}"
            );
            assert!(ids.insert(segment.id), "duplicate id in {segments:?}");
            position = segment.end;
        }
        assert_eq!(
            position, total,
            "segments don't reach the end in {segments:?}"
        );
    }

    #[test]
    fn a_new_download_is_one_segment() {
        assert!(initial_segments(0).is_empty());
        assert_eq!(
            initial_segments(10),
            vec![Segment::new(SegmentId(0), 0, 10)]
        );
    }

    #[test]
    fn idle_workers_split_the_largest_remaining_range_in_half() {
        let mut segments = initial_segments(1024 * KIB);
        assert_eq!(next_work(&mut segments, 256 * KIB), Some(SegmentId(0)));
        segments[0].downloaded = 24 * KIB;

        // 1000 KiB left: the new worker takes the upper 500 KiB.
        assert_eq!(next_work(&mut segments, 256 * KIB), Some(SegmentId(1)));
        assert_eq!((segments[0].start, segments[0].end), (0, 524 * KIB));
        assert_eq!(
            (segments[1].start, segments[1].end),
            (524 * KIB, 1024 * KIB)
        );
        assert_eq!(segments[1].state, SegmentState::Active);

        // 500 KiB left in each: halves of 250 KiB are below the minimum.
        assert_eq!(next_work(&mut segments, 256 * KIB), None);
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn pending_work_is_taken_before_anything_is_split() {
        let mut segments = vec![
            Segment::new(SegmentId(0), 0, 100),
            Segment::new(SegmentId(1), 100, 400),
        ];
        segments[0].state = SegmentState::Active;
        assert_eq!(next_work(&mut segments, 1), Some(SegmentId(1)));
        assert_eq!(segments.len(), 2);
    }

    #[test]
    fn ties_go_to_the_lowest_start() {
        let mut segments = vec![
            Segment::new(SegmentId(5), 0, 100),
            Segment::new(SegmentId(2), 100, 200),
        ];
        for segment in &mut segments {
            segment.state = SegmentState::Active;
        }
        assert_eq!(split_largest(&mut segments, 1), Some(SegmentId(6)));
        assert_eq!(
            segments.iter().map(|s| s.id.0).collect::<Vec<_>>(),
            [5, 6, 2]
        );
    }

    /// One step of a simulated download.
    #[derive(Debug, Clone)]
    enum Step {
        /// A worker becomes idle and asks for work.
        Idle,
        /// The worker on the n-th active segment receives a fraction (in 1/256ths) of its
        /// remaining bytes.
        Receive(usize, u8),
        /// The worker on the n-th active segment stops, leaving its segment pending.
        Stop(usize),
    }

    fn step() -> impl Strategy<Value = Step> {
        prop_oneof![
            2 => Just(Step::Idle),
            5 => (any::<usize>(), 1..=u8::MAX).prop_map(|(n, f)| Step::Receive(n, f)),
            1 => any::<usize>().prop_map(Step::Stop),
        ]
    }

    /// Index of the n-th active segment, wrapping around; `None` if none is active.
    fn nth_active(segments: &[Segment], n: usize) -> Option<usize> {
        let active: Vec<usize> = (0..segments.len())
            .filter(|&i| segments[i].state == SegmentState::Active)
            .collect();
        (!active.is_empty()).then(|| active[n % active.len()])
    }

    /// Applies `amount` received bytes to the segment at `index`, marking it done when full.
    fn receive(segments: &mut [Segment], index: usize, amount: u64) {
        let segment = &mut segments[index];
        segment.downloaded += amount.min(segment.remaining());
        if segment.remaining() == 0 {
            segment.state = SegmentState::Done;
        }
    }

    proptest! {
        #[test]
        fn segments_always_tile_the_file(
            total in 1..=64 * 1024 * KIB,
            min_segment_size in 1..=1024 * KIB,
            steps in prop::collection::vec(step(), 0..200),
        ) {
            let mut segments = initial_segments(total);
            for step in steps {
                match step {
                    Step::Idle => {
                        let before: Vec<Segment> = segments.clone();
                        if let Some(id) = next_work(&mut segments, min_segment_size) {
                            let taken = segments.iter().find(|s| s.id == id).unwrap();
                            prop_assert_eq!(taken.state, SegmentState::Active);
                            prop_assert!(taken.remaining() > 0);
                            if segments.len() > before.len() {
                                // A split leaves at least the minimum on both sides.
                                prop_assert!(taken.len() >= min_segment_size);
                                let lower = segments.iter().find(|s| s.end == taken.start).unwrap();
                                prop_assert!(lower.remaining() >= min_segment_size);
                            }
                        }
                    }
                    Step::Receive(n, fraction) => {
                        if let Some(i) = nth_active(&segments, n) {
                            let amount = (segments[i].remaining() * u64::from(fraction)).div_ceil(256);
                            receive(&mut segments, i, amount);
                        }
                    }
                    Step::Stop(n) => {
                        if let Some(i) = nth_active(&segments, n) {
                            segments[i].state = SegmentState::Pending;
                        }
                    }
                }
                assert_tiles(&segments, total);
            }
        }

        #[test]
        fn a_download_with_workers_always_finishes(
            total in 1..=64 * 1024 * KIB,
            min_segment_size in 1..=1024 * KIB,
            workers in 1..=32usize,
        ) {
            let mut segments = initial_segments(total);
            // Each worker fetches a quarter of its segment per round (at least one byte).
            for _ in 0..100_000 {
                let active = segments.iter().filter(|s| s.state == SegmentState::Active).count();
                for _ in active..workers {
                    if next_work(&mut segments, min_segment_size).is_none() {
                        break;
                    }
                }
                for i in 0..segments.len() {
                    if segments[i].state == SegmentState::Active {
                        let amount = segments[i].remaining().div_ceil(4);
                        receive(&mut segments, i, amount);
                    }
                }
                assert_tiles(&segments, total);
                if segments.iter().all(|s| s.state == SegmentState::Done) {
                    prop_assert_eq!(segments.iter().map(|s| s.downloaded).sum::<u64>(), total);
                    prop_assert!(segments.len() <= total.div_ceil(min_segment_size) as usize);
                    return Ok(());
                }
            }
            prop_assert!(false, "download never finished: {segments:?}");
        }
    }
}
