//! A segment: one byte range of a download, fetched by one connection into its own part file.

use serde::{Deserialize, Serialize};

/// Identifies a segment within its download; also names its part file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct SegmentId(pub u32);

/// Where a segment is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum SegmentState {
    /// Has bytes left and no worker is fetching it.
    Pending,
    /// A worker is fetching it.
    Active,
    /// Every byte of its range is in its part file.
    Done,
}

/// One byte range `[start, end)` of a download.
///
/// The segments of a download are kept sorted by `start` and tile `[0, total size)` without gaps
/// or overlaps. Bytes `[start, start + downloaded)` are in the part file; `end` can shrink while
/// the segment is active, when the planner gives its upper half to another worker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Segment {
    /// The segment's id, unique within its download.
    pub id: SegmentId,
    /// Offset of the first byte of the range.
    pub start: u64,
    /// Offset one past the last byte of the range.
    pub end: u64,
    /// Bytes already written to the part file, counted from `start`.
    pub downloaded: u64,
    /// Lifecycle state.
    pub state: SegmentState,
}

impl Segment {
    /// Creates a pending segment covering `[start, end)` with nothing downloaded.
    pub fn new(id: SegmentId, start: u64, end: u64) -> Self {
        Self {
            id,
            start,
            end,
            downloaded: 0,
            state: SegmentState::Pending,
        }
    }

    /// Length of the range in bytes.
    pub fn len(&self) -> u64 {
        self.end - self.start
    }

    /// Whether the range is empty.
    pub fn is_empty(&self) -> bool {
        self.start == self.end
    }

    /// Offset of the next byte to fetch.
    pub fn position(&self) -> u64 {
        self.start + self.downloaded
    }

    /// Bytes still to fetch.
    pub fn remaining(&self) -> u64 {
        self.end.saturating_sub(self.position())
    }

    /// File name of the segment's part file, `<id>.part`, inside the download's temp directory.
    pub fn part_file_name(&self) -> String {
        format!("{}.part", self.id.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remaining_counts_from_the_downloaded_position() {
        let mut segment = Segment::new(SegmentId(3), 100, 300);
        segment.downloaded = 50;
        assert_eq!(segment.len(), 200);
        assert_eq!(segment.position(), 150);
        assert_eq!(segment.remaining(), 150);
        assert_eq!(segment.part_file_name(), "3.part");
    }
}
