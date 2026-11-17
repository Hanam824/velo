//! The download list's data: every download, its live progress, and the sort order, turned
//! into rows for the table.

use std::cmp::Ordering;
use std::collections::{HashMap, HashSet};

use velo_core::{
    Category, ConnectionProgress, Download, DownloadId, DownloadState, ProgressUpdate, Queue,
    QueueId,
};

use crate::{DownloadRow, FileKind, format};

/// A column of the download list, in display order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Column {
    Name,
    Queue,
    Size,
    Status,
    TimeLeft,
    Rate,
    LastTry,
    Description,
}

impl Column {
    /// The column at display position `index`, as the UI numbers them.
    pub fn from_index(index: i32) -> Option<Self> {
        use Column::*;
        [
            Name,
            Queue,
            Size,
            Status,
            TimeLeft,
            Rate,
            LastTry,
            Description,
        ]
        .get(usize::try_from(index).ok()?)
        .copied()
    }

    /// The display position of this column.
    pub fn index(self) -> i32 {
        self as i32
    }
}

/// Which downloads the list shows, from the row selected in the Categories tree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Filter {
    /// Only unfinished (`Some(false)`) or finished (`Some(true)`) downloads; both if `None`.
    pub finished: Option<bool>,
    /// Only downloads in this category.
    pub category: Option<String>,
    /// Only downloads in a queue (`Some(None)`) or in this queue (`Some(Some(id))`).
    pub queue: Option<Option<u32>>,
}

impl Filter {
    /// The filter for a Categories tree key such as `all`, `unfinished/Video`, `queues` or
    /// `queue/0`; unknown keys show everything.
    pub fn from_key(key: &str) -> Self {
        let (group, category) = match key.split_once('/') {
            Some((group, rest)) => (group, Some(rest.to_string())),
            None => (key, None),
        };
        match group {
            "unfinished" | "finished" => Self {
                finished: Some(group == "finished"),
                category,
                queue: None,
            },
            "queues" => Self {
                queue: Some(None),
                ..Self::default()
            },
            "queue" => Self {
                queue: Some(category.and_then(|id| id.parse().ok())),
                ..Self::default()
            },
            _ => Self {
                category,
                ..Self::default()
            },
        }
    }

    /// Whether `download`, which is in queue `queue` if any, passes the filter.
    pub fn accepts(&self, download: &Download, queue: Option<QueueId>) -> bool {
        self.finished.is_none_or(|f| download.is_finished() == f)
            && self
                .category
                .as_ref()
                .is_none_or(|c| download.category.as_ref() == Some(c))
            && match self.queue {
                None => true,
                Some(None) => queue.is_some(),
                Some(Some(id)) => queue == Some(QueueId(id)),
            }
    }
}

/// Every download the app knows, with live progress for the running ones.
#[derive(Debug)]
pub struct DownloadTable {
    downloads: Vec<Download>,
    progress: HashMap<DownloadId, ProgressUpdate>,
    sort: Column,
    ascending: bool,
    filter: Filter,
    /// Each queued download's queue and its place in it, from 0.
    queued: HashMap<DownloadId, (QueueId, usize)>,
    /// The selected downloads.
    selected: HashSet<DownloadId>,
    /// Where a Shift range starts: the row last clicked without Shift.
    anchor: Option<DownloadId>,
    /// The current row: the one last clicked, which Enter opens and the arrow keys move from.
    focus: Option<DownloadId>,
}

impl Default for DownloadTable {
    fn default() -> Self {
        Self {
            downloads: Vec::new(),
            progress: HashMap::new(),
            sort: Column::LastTry,
            ascending: false,
            filter: Filter::default(),
            queued: HashMap::new(),
            selected: HashSet::new(),
            anchor: None,
            focus: None,
        }
    }
}

impl DownloadTable {
    /// Adds `download`, or replaces the one with the same id.
    pub fn upsert(&mut self, download: Download) {
        if !download.is_running() {
            self.progress.remove(&download.id);
        }
        match self.downloads.iter_mut().find(|d| d.id == download.id) {
            Some(existing) => *existing = download,
            None => self.downloads.push(download),
        }
    }

    /// Removes download `id`.
    pub fn remove(&mut self, id: DownloadId) {
        self.downloads.retain(|d| d.id != id);
        self.progress.remove(&id);
        self.selected.remove(&id);
        self.anchor = self.anchor.filter(|&a| a != id);
        self.focus = self.focus.filter(|&f| f != id);
    }

    /// Records live progress for a running download.
    pub fn set_progress(&mut self, update: ProgressUpdate) {
        if self.get(update.id).is_some_and(Download::is_running) {
            self.progress.insert(update.id, update);
        }
    }

    /// The download with `id`.
    pub fn get(&self, id: DownloadId) -> Option<&Download> {
        self.downloads.iter().find(|d| d.id == id)
    }

    /// Every download, in the order they were added.
    pub fn downloads(&self) -> &[Download] {
        &self.downloads
    }

    /// Takes the queues' membership, for the Q column and the queue filters.
    pub fn set_queues<'a>(&mut self, queues: impl IntoIterator<Item = &'a Queue>) {
        self.queued = queues
            .into_iter()
            .flat_map(|q| {
                q.downloads
                    .iter()
                    .enumerate()
                    .map(move |(i, d)| (*d, (q.id, i)))
            })
            .collect();
    }

    /// The queue download `id` is in, if any.
    pub fn queue_of(&self, id: DownloadId) -> Option<QueueId> {
        self.queued.get(&id).map(|(queue, _)| *queue)
    }

    /// Whether `download` passes the filter.
    fn shown(&self, download: &Download) -> bool {
        self.filter.accepts(download, self.queue_of(download.id))
    }

    /// Shows only the downloads `filter` accepts.
    /// Shows only the downloads `filter` accepts; hidden downloads leave the selection.
    pub fn set_filter(&mut self, filter: Filter) {
        self.filter = filter;
        let visible: HashSet<DownloadId> = self.sorted().iter().map(|d| d.id).collect();
        self.selected.retain(|id| visible.contains(id));
        self.anchor = self.anchor.filter(|id| visible.contains(id));
        self.focus = self.focus.filter(|id| visible.contains(id));
    }

    /// Selects download `id` as a click does: alone, or flipping it in or out of the selection
    /// with `toggle` (Ctrl), or with `range` (Shift) every row from the current one to it.
    pub fn press(&mut self, id: DownloadId, toggle: bool, range: bool) {
        if !self.is_visible(id) {
            return;
        }
        let sorted: Vec<DownloadId> = self.sorted().iter().map(|d| d.id).collect();
        let anchor = self
            .anchor
            .and_then(|a| sorted.iter().position(|&d| d == a));
        self.focus = Some(id);
        match (range, anchor) {
            (true, Some(start)) => {
                let end = sorted.iter().position(|&d| d == id).unwrap_or(start);
                let (from, to) = (start.min(end), start.max(end));
                if !toggle {
                    self.selected.clear();
                }
                self.selected.extend(&sorted[from..=to]);
            }
            _ if toggle => {
                if !self.selected.remove(&id) {
                    self.selected.insert(id);
                }
                self.anchor = Some(id);
            }
            _ => {
                self.selected = HashSet::from([id]);
                self.anchor = Some(id);
            }
        }
    }

    /// Selects every row shown.
    pub fn select_all(&mut self) {
        let sorted: Vec<DownloadId> = self.sorted().iter().map(|d| d.id).collect();
        if self.current().is_none() {
            self.anchor = sorted.first().copied();
            self.focus = self.anchor;
        }
        self.selected = sorted.into_iter().collect();
    }

    /// Moves the current row `delta` rows, selecting it alone, or with `extend` (Shift) the
    /// rows from the range's start to it.
    pub fn move_selection(&mut self, delta: i32, extend: bool) {
        if let Some(next) = self.neighbour(self.current(), delta) {
            self.press(next, false, extend);
        }
    }

    /// The selected downloads shown, in display order.
    pub fn selection(&self) -> Vec<&Download> {
        self.sorted()
            .into_iter()
            .filter(|d| self.selected.contains(&d.id))
            .collect()
    }

    /// Whether download `id` is selected.
    pub fn is_selected(&self, id: DownloadId) -> bool {
        self.selected.contains(&id)
    }

    /// The current row, if shown.
    pub fn current(&self) -> Option<DownloadId> {
        self.focus.filter(|&id| self.is_visible(id))
    }

    /// Whether download `id` exists and passes the filter.
    pub fn is_visible(&self, id: DownloadId) -> bool {
        self.get(id).is_some_and(|d| self.shown(d))
    }

    /// How many downloads pass the filter.
    pub fn visible_count(&self) -> usize {
        self.downloads.iter().filter(|d| self.shown(d)).count()
    }

    /// The column the rows are sorted by, and whether ascending.
    pub fn sort_order(&self) -> (Column, bool) {
        (self.sort, self.ascending)
    }

    /// Sorts by `column`; sorting by the same column again flips the direction.
    pub fn sort_by(&mut self, column: Column) {
        if self.sort == column {
            self.ascending = !self.ascending;
        } else {
            self.sort = column;
            self.ascending = true;
        }
    }

    /// The downloads in display order.
    fn sorted(&self) -> Vec<&Download> {
        let mut sorted: Vec<&Download> = self.downloads.iter().filter(|d| self.shown(d)).collect();
        sorted.sort_by(|a, b| {
            let order = self.compare(a, b).then(a.id.cmp(&b.id));
            if self.ascending {
                order
            } else {
                order.reverse()
            }
        });
        sorted
    }

    /// Compares two downloads by the sort column.
    fn compare(&self, a: &Download, b: &Download) -> Ordering {
        match self.sort {
            Column::Name => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
            Column::Queue => {
                let place = |d: &Download| self.queued.get(&d.id).map(|(q, i)| (q.0, *i));
                place(a).cmp(&place(b))
            }
            Column::Size => a.total_size.cmp(&b.total_size),
            Column::Status => self.fraction(a).total_cmp(&self.fraction(b)),
            Column::TimeLeft => self.time_left(a).cmp(&self.time_left(b)),
            Column::Rate => self.rate(a).cmp(&self.rate(b)),
            Column::LastTry => a.last_try_at.cmp(&b.last_try_at),
            Column::Description => a.description.cmp(&b.description),
        }
    }

    /// The table rows, in display order.
    pub fn rows(&self) -> Vec<DownloadRow> {
        self.sorted().into_iter().map(|d| self.row(d)).collect()
    }

    /// The id `delta` rows away from `from` in display order, staying within the list; the
    /// first row if nothing is selected.
    pub fn neighbour(&self, from: Option<DownloadId>, delta: i32) -> Option<DownloadId> {
        let sorted = self.sorted();
        let last = sorted.len().checked_sub(1)?;
        let target = match from.and_then(|id| sorted.iter().position(|d| d.id == id)) {
            Some(index) => index.saturating_add_signed(delta as isize).min(last),
            None => 0,
        };
        Some(sorted[target].id)
    }

    /// Bytes downloaded so far, live if running.
    pub fn downloaded(&self, download: &Download) -> u64 {
        self.progress
            .get(&download.id)
            .map_or_else(|| download.downloaded(), |p| p.downloaded)
    }

    /// The current transfer rate of a running download.
    fn rate(&self, download: &Download) -> u64 {
        self.rate_of(download.id)
    }

    /// The connections of running download `id`, in order; empty if it isn't running or
    /// doesn't download in segments.
    pub fn connections(&self, id: DownloadId) -> &[ConnectionProgress] {
        self.progress.get(&id).map_or(&[], |p| &p.connections)
    }

    /// The segments of `download` as they are now: live while it runs, saved otherwise.
    pub fn segments<'a>(&'a self, download: &'a Download) -> &'a [velo_core::Segment] {
        match self.progress.get(&download.id) {
            Some(progress) if !progress.segments.is_empty() => &progress.segments,
            _ => &download.segments,
        }
    }

    /// The current transfer rate of download `id`; 0 unless it is running.
    pub fn rate_of(&self, id: DownloadId) -> u64 {
        self.progress.get(&id).map_or(0, |p| p.rate)
    }

    /// How far along the download is, from 0 to 1; complete ones count as 1.
    pub fn fraction(&self, download: &Download) -> f64 {
        match (
            download.state == DownloadState::Complete,
            download.total_size,
        ) {
            (true, _) => 1.0,
            (false, Some(total)) if total > 0 => self.downloaded(download) as f64 / total as f64,
            _ => 0.0,
        }
    }

    /// Seconds until a running download finishes at its current rate.
    pub fn time_left(&self, download: &Download) -> Option<u64> {
        let rate = self.rate(download);
        let total = download.total_size?;
        (rate > 0).then(|| {
            total
                .saturating_sub(self.downloaded(download))
                .div_ceil(rate)
        })
    }

    /// The table row for `download`.
    fn row(&self, download: &Download) -> DownloadRow {
        let downloaded = self.downloaded(download);
        let status = match (&download.state, download.total_size) {
            (DownloadState::Complete, _) => "Complete".into(),
            (DownloadState::Failed(_), _) => "Error".into(),
            (DownloadState::Probing, _) => "Connecting…".into(),
            (DownloadState::Queued, _) if downloaded == 0 => String::new(),
            (_, Some(total)) => format::percent(downloaded, total),
            (_, None) => format::size(downloaded),
        };
        let running = download.is_running();
        // Only while bytes arrive: there is no rate to show while connecting or finishing.
        let receiving = download.state == DownloadState::Downloading;
        DownloadRow {
            id: download.id.0 as i32,
            name: download.file_name.as_str().into(),
            kind: file_kind(&download.file_name),
            queue: self
                .queued
                .get(&download.id)
                .map(|(_, place)| (place + 1).to_string())
                .unwrap_or_default()
                .into(),
            size: download
                .total_size
                .map(format::size)
                .unwrap_or_default()
                .into(),
            status: status.into(),
            failed: matches!(download.state, DownloadState::Failed(_)),
            time_left: self
                .time_left(download)
                .filter(|_| receiving)
                .map(format::duration)
                .unwrap_or_default()
                .into(),
            rate: if receiving {
                format::rate(self.rate(download)).into()
            } else {
                Default::default()
            },
            last_try: download
                .last_try_at
                .map(format::date)
                .unwrap_or_default()
                .into(),
            description: download.description.as_str().into(),
            can_resume: crate::toolbar::can_resume(download),
            can_stop: running,
            finished: download.is_finished(),
            selected: self.selected.contains(&download.id),
        }
    }
}

/// The icon kind for `file_name`, from the built-in category its extension belongs to.
pub fn file_kind(file_name: &str) -> FileKind {
    let defaults = Category::defaults();
    match Category::for_file_name(&defaults, file_name).map(|c| c.name.as_str()) {
        Some("Compressed") => FileKind::Archive,
        Some("Documents") => FileKind::Document,
        Some("Music") => FileKind::Music,
        Some("Programs") => FileKind::Program,
        Some("Video") => FileKind::Video,
        _ => FileKind::Other,
    }
}

#[cfg(test)]
pub mod tests {
    use velo_core::{Segment, SegmentId};

    use super::*;

    /// A download of `name` in `state` with `downloaded` of `total` bytes.
    pub fn sample(
        id: u64,
        name: &str,
        state: DownloadState,
        downloaded: u64,
        total: u64,
    ) -> Download {
        let mut download = Download::new(
            DownloadId(id),
            format!("http://example.test/{name}"),
            name,
            "/downloads",
        );
        download.state = state;
        download.total_size = Some(total);
        let mut segment = Segment::new(SegmentId(0), 0, total);
        segment.downloaded = downloaded;
        download.segments = vec![segment];
        download.last_try_at = Some(1_792_998_180 + id as i64);
        download
    }

    fn names(table: &DownloadTable) -> Vec<String> {
        table.rows().iter().map(|r| r.name.to_string()).collect()
    }

    #[test]
    fn rows_show_status_size_and_kind() {
        let mut table = DownloadTable::default();
        table.upsert(sample(1, "movie.mkv", DownloadState::Paused, 4521, 10_000));
        table.upsert(sample(2, "a.zip", DownloadState::Complete, 10, 10));
        table.upsert(sample(3, "b.pdf", DownloadState::Failed("x".into()), 0, 10));
        table.upsert(sample(4, "c.dat", DownloadState::Queued, 0, 10));
        table.sort_by(Column::Name);
        let rows = table.rows();
        let row = |name: &str| rows.iter().find(|r| r.name == name).unwrap().clone();

        let paused = row("movie.mkv");
        assert_eq!(
            (paused.status.as_str(), paused.size.as_str()),
            ("45.21%", "9.77 KB")
        );
        assert_eq!(paused.kind, FileKind::Video);
        assert!(paused.can_resume && !paused.can_stop);
        assert_eq!(row("a.zip").status, "Complete");
        assert_eq!(row("a.zip").kind, FileKind::Archive);
        assert!(row("b.pdf").failed && row("b.pdf").status == "Error");
        assert_eq!(row("c.dat").status, "");
        assert_eq!(row("c.dat").kind, FileKind::Other);
    }

    #[test]
    fn live_progress_shows_rate_and_time_left_while_running() {
        let mut table = DownloadTable::default();
        table.upsert(sample(1, "a.iso", DownloadState::Downloading, 0, 10_000));
        table.set_progress(ProgressUpdate {
            id: DownloadId(1),
            downloaded: 4_000,
            total: Some(10_000),
            rate: 2_048,
            segments: Vec::new(),
            connections: Vec::new(),
        });
        let row = &table.rows()[0];
        assert_eq!(row.status, "40.00%");
        assert_eq!(row.rate, "2.00 KB/sec");
        assert_eq!(row.time_left, "3 sec");
        assert!(row.can_stop && !row.can_resume);

        // Once stopped, the live numbers go away.
        table.upsert(sample(1, "a.iso", DownloadState::Paused, 4_000, 10_000));
        let row = &table.rows()[0];
        assert_eq!((row.rate.as_str(), row.time_left.as_str()), ("", ""));
    }

    #[test]
    fn sorting_flips_on_the_same_column_and_breaks_ties_by_id() {
        let mut table = DownloadTable::default();
        table.upsert(sample(1, "b.zip", DownloadState::Complete, 10, 300));
        table.upsert(sample(2, "A.zip", DownloadState::Paused, 50, 100));
        table.upsert(sample(3, "c.zip", DownloadState::Paused, 0, 200));
        // Newest try first by default.
        assert_eq!(names(&table), ["c.zip", "A.zip", "b.zip"]);
        table.sort_by(Column::Name);
        assert_eq!(names(&table), ["A.zip", "b.zip", "c.zip"]);
        table.sort_by(Column::Name);
        assert_eq!(names(&table), ["c.zip", "b.zip", "A.zip"]);
        table.sort_by(Column::Size);
        assert_eq!(names(&table), ["A.zip", "c.zip", "b.zip"]);
        table.sort_by(Column::Status);
        assert_eq!(names(&table), ["c.zip", "A.zip", "b.zip"]);
        assert_eq!(table.sort_order(), (Column::Status, true));
    }

    #[test]
    fn neighbours_follow_the_display_order_and_stop_at_the_ends() {
        let mut table = DownloadTable::default();
        assert_eq!(table.neighbour(None, 1), None);
        for id in 1..=3 {
            table.upsert(sample(
                id,
                &format!("{id}.zip"),
                DownloadState::Paused,
                0,
                1,
            ));
        }
        table.sort_by(Column::Name);
        assert_eq!(table.neighbour(None, 1), Some(DownloadId(1)));
        assert_eq!(table.neighbour(Some(DownloadId(1)), 1), Some(DownloadId(2)));
        assert_eq!(table.neighbour(Some(DownloadId(3)), 1), Some(DownloadId(3)));
        assert_eq!(
            table.neighbour(Some(DownloadId(1)), -1),
            Some(DownloadId(1))
        );
        table.remove(DownloadId(2));
        assert_eq!(table.neighbour(Some(DownloadId(1)), 1), Some(DownloadId(3)));
    }

    #[test]
    fn clicks_with_ctrl_and_shift_build_the_selection_in_display_order() {
        let mut table = DownloadTable::default();
        for id in 1..=5 {
            table.upsert(sample(
                id,
                &format!("{id}.zip"),
                DownloadState::Paused,
                0,
                1,
            ));
        }
        table.sort_by(Column::Name);
        let selected = |table: &DownloadTable| -> Vec<u64> {
            table.selection().iter().map(|d| d.id.0).collect()
        };
        let id = DownloadId;

        table.press(id(2), false, false);
        assert_eq!((selected(&table), table.current()), (vec![2], Some(id(2))));
        // Ctrl adds and removes single rows.
        table.press(id(4), true, false);
        assert_eq!(selected(&table), [2, 4]);
        table.press(id(2), true, false);
        assert_eq!(selected(&table), [4]);
        // Shift selects from the last row clicked without it, in either direction.
        table.press(id(1), true, false);
        table.press(id(3), false, true);
        assert_eq!(selected(&table), [1, 2, 3]);
        table.press(id(5), false, true);
        assert_eq!(selected(&table), [1, 2, 3, 4, 5]);
        // The range follows the display order, not the ids.
        table.sort_by(Column::Name);
        table.press(id(4), false, false);
        table.press(id(2), false, true);
        assert_eq!(selected(&table), [4, 3, 2]);
        assert_eq!(table.rows().iter().filter(|r| r.selected).count(), 3);
        // A plain click selects one row again.
        table.press(id(1), false, false);
        assert_eq!(selected(&table), [1]);
    }

    #[test]
    fn select_all_takes_the_shown_rows_and_arrows_move_or_extend() {
        let mut table = DownloadTable::default();
        table.upsert(sample(1, "a.zip", DownloadState::Complete, 1, 1));
        table.upsert(sample(2, "b.zip", DownloadState::Paused, 0, 1));
        table.upsert(sample(3, "c.zip", DownloadState::Paused, 0, 1));
        table.sort_by(Column::Name);
        let selected = |table: &DownloadTable| -> Vec<u64> {
            table.selection().iter().map(|d| d.id.0).collect()
        };

        table.set_filter(Filter::from_key("unfinished"));
        table.select_all();
        assert_eq!(selected(&table), [2, 3]);
        assert_eq!(table.current(), Some(DownloadId(2)));
        // Hidden rows leave the selection.
        table.set_filter(Filter::default());
        table.select_all();
        table.set_filter(Filter::from_key("finished"));
        assert_eq!(selected(&table), [1]);
        table.set_filter(Filter::default());
        assert_eq!(selected(&table), [1]);

        // The current row was hidden, so an arrow starts at the first row; arrows move one
        // selected row, and with Shift they grow the range.
        table.move_selection(1, false);
        assert_eq!(selected(&table), [1]);
        table.move_selection(1, false);
        assert_eq!(selected(&table), [2]);
        table.move_selection(1, true);
        assert_eq!(selected(&table), [2, 3]);
        assert_eq!(table.current(), Some(DownloadId(3)));
        table.move_selection(-1, true);
        assert_eq!(selected(&table), [2]);
        // Removed downloads leave the selection.
        table.remove(DownloadId(2));
        assert!(selected(&table).is_empty() && table.current().is_none());
    }

    #[test]
    fn tree_keys_filter_by_group_category_and_queue() {
        let mut table = DownloadTable::default();
        let mut video = sample(1, "a.mkv", DownloadState::Complete, 1, 1);
        video.category = Some("Video".into());
        let mut music = sample(2, "b.mp3", DownloadState::Paused, 0, 1);
        music.category = Some("Music".into());
        let plain = sample(3, "c.dat", DownloadState::Downloading, 0, 1);
        for download in [video, music, plain] {
            table.upsert(download);
        }
        table.sort_by(Column::Name);
        let shown = |table: &mut DownloadTable, key: &str| {
            table.set_filter(Filter::from_key(key));
            names(table)
        };
        assert_eq!(shown(&mut table, "all"), ["a.mkv", "b.mp3", "c.dat"]);
        assert_eq!(shown(&mut table, "all/Music"), ["b.mp3"]);
        assert_eq!(shown(&mut table, "unfinished"), ["b.mp3", "c.dat"]);
        assert_eq!(shown(&mut table, "finished"), ["a.mkv"]);
        assert_eq!(shown(&mut table, "finished/Music"), Vec::<String>::new());
        assert_eq!(shown(&mut table, "queue/0"), Vec::<String>::new());
        let mut main = Queue::main();
        main.downloads = vec![DownloadId(3), DownloadId(1)];
        table.set_queues([&main]);
        assert_eq!(shown(&mut table, "queue/0"), ["a.mkv", "c.dat"]);
        assert_eq!(shown(&mut table, "queues"), ["a.mkv", "c.dat"]);
        assert_eq!(shown(&mut table, "queue/7"), Vec::<String>::new());
        table.set_filter(Filter::default());
        let rows = table.rows();
        let q = |name: &str| {
            rows.iter()
                .find(|r| r.name == name)
                .unwrap()
                .queue
                .to_string()
        };
        assert_eq!(
            (q("c.dat"), q("a.mkv"), q("b.mp3")),
            ("1".into(), "2".into(), String::new())
        );
        table.set_filter(Filter::from_key("unfinished/Music"));
        assert_eq!(table.visible_count(), 1);
        assert!(table.is_visible(DownloadId(2)) && !table.is_visible(DownloadId(1)));
        assert_eq!(table.neighbour(None, 1), Some(DownloadId(2)));
    }

    #[test]
    fn columns_round_trip_through_their_index() {
        for index in 0..8 {
            assert_eq!(Column::from_index(index).unwrap().index(), index);
        }
        assert_eq!(Column::from_index(8), None);
        assert_eq!(Column::from_index(-1), None);
    }
}
