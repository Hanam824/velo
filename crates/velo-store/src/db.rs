//! The SQLite database of downloads and their segments.

use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension, Row, Transaction, params};
use velo_core::{
    Download, DownloadId, DownloadState, EngineEvent, Segment, SegmentId, SegmentState,
};

use crate::{Error, Result};

/// The schema, one entry per version: entry `n` moves a database from `user_version` `n` to
/// `n + 1`. Never edit an entry once released; add a new one.
const MIGRATIONS: &[&str] = &[
    // 1: downloads and their segments.
    "CREATE TABLE downloads (
        id INTEGER PRIMARY KEY,
        url TEXT NOT NULL,
        file_name TEXT NOT NULL,
        save_dir TEXT NOT NULL,
        total_size INTEGER,
        resumable INTEGER NOT NULL,
        etag TEXT,
        last_modified TEXT,
        category TEXT,
        description TEXT NOT NULL,
        state TEXT NOT NULL,
        error TEXT,
        added_at INTEGER,
        last_try_at INTEGER
    );
    CREATE TABLE segments (
        download_id INTEGER NOT NULL REFERENCES downloads(id) ON DELETE CASCADE,
        id INTEGER NOT NULL,
        start_offset INTEGER NOT NULL,
        end_offset INTEGER NOT NULL,
        downloaded INTEGER NOT NULL,
        state TEXT NOT NULL,
        PRIMARY KEY (download_id, id)
    );",
];

/// The download list on disk.
///
/// Every write is one transaction, so a crash leaves either the old or the new version of a
/// download, never a mix.
#[derive(Debug)]
pub struct Store {
    connection: Connection,
}

impl Store {
    /// Opens the database at `path`, creating it and its folder if needed, and brings its
    /// schema up to date.
    ///
    /// Fails with [`Error::NewerSchema`] if a newer Velo wrote it.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let connection = Connection::open(path)?;
        connection.pragma_update(None, "foreign_keys", true)?;
        let mut store = Self { connection };
        store.migrate()?;
        Ok(store)
    }

    /// Runs the migrations the database hasn't had yet.
    fn migrate(&mut self) -> Result<()> {
        let supported = MIGRATIONS.len() as i64;
        let found: i64 = self
            .connection
            .pragma_query_value(None, "user_version", |row| row.get(0))?;
        if found > supported {
            return Err(Error::NewerSchema { found, supported });
        }
        for (version, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
            let transaction = self.connection.transaction()?;
            transaction.execute_batch(sql)?;
            transaction.pragma_update(None, "user_version", version as i64 + 1)?;
            transaction.commit()?;
        }
        Ok(())
    }

    /// Loads every download with its segments, in the order they were added.
    pub fn load_downloads(&self) -> Result<Vec<Download>> {
        let mut statement = self.connection.prepare(
            "SELECT id, url, file_name, save_dir, total_size, resumable, etag, last_modified,
                    category, description, state, error, added_at, last_try_at
             FROM downloads ORDER BY id",
        )?;
        let mut downloads = statement
            .query_map([], download_from_row)?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .collect::<Result<Vec<_>>>()?;
        for download in &mut downloads {
            download.segments = self.load_segments(download.id)?;
        }
        Ok(downloads)
    }

    /// Loads the segments of download `id`, sorted by `start`.
    fn load_segments(&self, id: DownloadId) -> Result<Vec<Segment>> {
        let mut statement = self.connection.prepare(
            "SELECT id, start_offset, end_offset, downloaded, state
             FROM segments WHERE download_id = ?1 ORDER BY start_offset",
        )?;
        let rows = statement.query_map([id.0 as i64], |row| {
            Ok((
                Segment {
                    id: SegmentId(row.get(0)?),
                    start: row.get::<_, i64>(1)? as u64,
                    end: row.get::<_, i64>(2)? as u64,
                    downloaded: row.get::<_, i64>(3)? as u64,
                    state: SegmentState::Pending,
                },
                row.get::<_, String>(4)?,
            ))
        })?;
        rows.map(|row| {
            let (mut segment, state) = row?;
            segment.state = segment_state_from(&state)?;
            Ok(segment)
        })
        .collect()
    }

    /// Saves `download` and replaces its segments.
    pub fn save_download(&mut self, download: &Download) -> Result<()> {
        let transaction = self.connection.transaction()?;
        write_download(&transaction, download)?;
        transaction.commit()?;
        Ok(())
    }

    /// Deletes download `id` and its segments; a missing one is fine.
    pub fn delete_download(&mut self, id: DownloadId) -> Result<()> {
        self.connection
            .execute("DELETE FROM downloads WHERE id = ?1", [id.0 as i64])?;
        Ok(())
    }

    /// Records what an engine event changed: `Added` and `Changed` save the download,
    /// `Removed` deletes it, and `Progress` is ignored.
    pub fn apply(&mut self, event: &EngineEvent) -> Result<()> {
        match event {
            EngineEvent::Added(download) | EngineEvent::Changed(download) => {
                self.save_download(download)
            }
            EngineEvent::Removed(id) => self.delete_download(*id),
            EngineEvent::Progress(_) => Ok(()),
        }
    }

    /// Whether download `id` is stored.
    pub fn contains(&self, id: DownloadId) -> Result<bool> {
        Ok(self
            .connection
            .query_row(
                "SELECT 1 FROM downloads WHERE id = ?1",
                [id.0 as i64],
                |_| Ok(()),
            )
            .optional()?
            .is_some())
    }
}

/// Inserts or replaces `download` and its segments within `transaction`.
fn write_download(transaction: &Transaction<'_>, download: &Download) -> Result<()> {
    let save_dir = download
        .save_dir
        .to_str()
        .ok_or_else(|| Error::NonUtf8Path(download.save_dir.clone()))?;
    let (state, error) = state_to(&download.state);
    transaction.execute(
        "INSERT OR REPLACE INTO downloads
            (id, url, file_name, save_dir, total_size, resumable, etag, last_modified,
             category, description, state, error, added_at, last_try_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            download.id.0 as i64,
            download.url,
            download.file_name,
            save_dir,
            download.total_size.map(|s| s as i64),
            download.resumable,
            download.etag,
            download.last_modified,
            download.category,
            download.description,
            state,
            error,
            download.added_at,
            download.last_try_at,
        ],
    )?;
    // REPLACE on the parent row cascades to its segments, so these are all new.
    let mut insert = transaction.prepare(
        "INSERT INTO segments (download_id, id, start_offset, end_offset, downloaded, state)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
    )?;
    for segment in &download.segments {
        insert.execute(params![
            download.id.0 as i64,
            segment.id.0,
            segment.start as i64,
            segment.end as i64,
            segment.downloaded as i64,
            segment_state_to(segment.state),
        ])?;
    }
    Ok(())
}

/// Reads a download (without segments) from a `downloads` row.
fn download_from_row(row: &Row<'_>) -> rusqlite::Result<Result<Download>> {
    let state: String = row.get(10)?;
    let error: Option<String> = row.get(11)?;
    let state = match state_from(&state, error) {
        Ok(state) => state,
        Err(e) => return Ok(Err(e)),
    };
    Ok(Ok(Download {
        id: DownloadId(row.get::<_, i64>(0)? as u64),
        url: row.get(1)?,
        file_name: row.get(2)?,
        save_dir: PathBuf::from(row.get::<_, String>(3)?),
        total_size: row.get::<_, Option<i64>>(4)?.map(|s| s as u64),
        resumable: row.get(5)?,
        etag: row.get(6)?,
        last_modified: row.get(7)?,
        category: row.get(8)?,
        description: row.get(9)?,
        state,
        segments: Vec::new(),
        added_at: row.get(12)?,
        last_try_at: row.get(13)?,
    }))
}

/// The stored text of a download state, and its error message for `Failed`.
fn state_to(state: &DownloadState) -> (&'static str, Option<&str>) {
    match state {
        DownloadState::Queued => ("queued", None),
        DownloadState::Probing => ("probing", None),
        DownloadState::Downloading => ("downloading", None),
        DownloadState::Paused => ("paused", None),
        DownloadState::Merging => ("merging", None),
        DownloadState::Complete => ("complete", None),
        DownloadState::Failed(message) => ("failed", Some(message)),
    }
}

/// The download state stored as `text`, with `error` as the message of `Failed`.
fn state_from(text: &str, error: Option<String>) -> Result<DownloadState> {
    Ok(match text {
        "queued" => DownloadState::Queued,
        "probing" => DownloadState::Probing,
        "downloading" => DownloadState::Downloading,
        "paused" => DownloadState::Paused,
        "merging" => DownloadState::Merging,
        "complete" => DownloadState::Complete,
        "failed" => DownloadState::Failed(error.unwrap_or_default()),
        other => return Err(Error::UnknownValue(format!("download state {other:?}"))),
    })
}

/// The stored text of a segment state.
fn segment_state_to(state: SegmentState) -> &'static str {
    match state {
        SegmentState::Pending => "pending",
        SegmentState::Active => "active",
        SegmentState::Done => "done",
    }
}

/// The segment state stored as `text`.
fn segment_state_from(text: &str) -> Result<SegmentState> {
    Ok(match text {
        "pending" => SegmentState::Pending,
        "active" => SegmentState::Active,
        "done" => SegmentState::Done,
        other => return Err(Error::UnknownValue(format!("segment state {other:?}"))),
    })
}

#[cfg(test)]
mod tests {
    use velo_core::ProgressUpdate;

    use super::*;

    /// A paused download with two segments and every optional field set.
    fn sample(id: u64) -> Download {
        let mut download = Download::new(
            DownloadId(id),
            "http://example.test/a.iso",
            "a.iso",
            "/downloads/ünïcode",
        );
        download.total_size = Some(1000);
        download.resumable = true;
        download.etag = Some("\"v1\"".into());
        download.last_modified = Some("Mon, 26 Oct 2026 00:00:00 GMT".into());
        download.category = Some("Programs".into());
        download.description = "an image".into();
        download.state = DownloadState::Paused;
        download.added_at = Some(1_800_000_000);
        download.last_try_at = Some(1_800_000_100);
        let mut first = Segment::new(SegmentId(0), 0, 600);
        first.downloaded = 600;
        first.state = SegmentState::Done;
        let mut second = Segment::new(SegmentId(1), 600, 1000);
        second.downloaded = 150;
        download.segments = vec![first, second];
        download
    }

    #[test]
    fn downloads_round_trip_through_a_reopened_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("velo.db");
        let mut failed = sample(2);
        failed.state = DownloadState::Failed("server answered 404 Not Found".into());
        failed.segments.clear();
        {
            let mut store = Store::open(&path).unwrap();
            store.save_download(&sample(1)).unwrap();
            store.save_download(&failed).unwrap();
        }
        let store = Store::open(&path).unwrap();
        assert_eq!(store.load_downloads().unwrap(), vec![sample(1), failed]);
    }

    #[test]
    fn saving_again_replaces_the_segments() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("velo.db")).unwrap();
        let mut download = sample(1);
        store.save_download(&download).unwrap();
        download.segments.truncate(1);
        download.state = DownloadState::Complete;
        store.save_download(&download).unwrap();
        assert_eq!(store.load_downloads().unwrap(), vec![download]);
    }

    #[test]
    fn engine_events_are_applied() {
        let dir = tempfile::tempdir().unwrap();
        let mut store = Store::open(&dir.path().join("velo.db")).unwrap();
        let mut download = sample(7);
        store.apply(&EngineEvent::Added(download.clone())).unwrap();
        download.segments[1].downloaded = 400;
        store
            .apply(&EngineEvent::Changed(download.clone()))
            .unwrap();
        store
            .apply(&EngineEvent::Progress(ProgressUpdate {
                id: download.id,
                downloaded: 999,
                total: Some(1000),
                rate: 1,
                segments: Vec::new(),
            }))
            .unwrap();
        assert_eq!(store.load_downloads().unwrap(), vec![download.clone()]);

        store.apply(&EngineEvent::Removed(download.id)).unwrap();
        assert!(!store.contains(download.id).unwrap());
        let segments: i64 = store
            .connection
            .query_row("SELECT COUNT(*) FROM segments", [], |row| row.get(0))
            .unwrap();
        assert_eq!(segments, 0);
    }

    #[test]
    fn a_database_from_a_newer_velo_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("velo.db");
        drop(Store::open(&path).unwrap());
        Connection::open(&path)
            .unwrap()
            .pragma_update(None, "user_version", 99)
            .unwrap();
        assert!(matches!(
            Store::open(&path),
            Err(Error::NewerSchema { found: 99, .. })
        ));
    }
}
