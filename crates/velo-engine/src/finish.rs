//! Moves a finished download's data file to its save path.

use std::path::{Path, PathBuf};

use tokio::fs::{self, OpenOptions};
use velo_core::{Segment, SegmentState};

use crate::{Error, Result};

/// Checks the data file at `data` and renames it to `save_path`.
///
/// Fails with [`Error::NotFinished`] unless every segment is done, they tile the file and match
/// `expected_size` (when given), with [`Error::Exists`] if something is already at `save_path`,
/// and with [`Error::SizeMismatch`] if the file isn't as long as the segments. The file is synced
/// before the rename, so the real name never holds bytes that aren't on disk; on any error it is
/// left where it is.
pub async fn finish(
    data: &Path,
    segments: &[Segment],
    expected_size: Option<u64>,
    save_path: &Path,
) -> Result<()> {
    let mut position = 0;
    for segment in segments {
        if segment.state != SegmentState::Done || segment.start != position {
            return Err(Error::NotFinished);
        }
        position = segment.end;
    }
    if expected_size.is_some_and(|size| size != position) {
        return Err(Error::NotFinished);
    }
    if fs::try_exists(save_path).await? {
        return Err(Error::Exists(save_path.to_path_buf()));
    }
    let file = OpenOptions::new().write(true).open(data).await?;
    file.sync_all().await?;
    let actual = file.metadata().await?.len();
    drop(file);
    if actual != position {
        return Err(Error::SizeMismatch {
            expected: position,
            actual,
        });
    }
    fs::rename(data, save_path).await?;
    Ok(())
}

/// Path a download is written to until it is finished: `<save_path>.velo-tmp`.
pub fn temp_path(save_path: &Path) -> PathBuf {
    let mut name = save_path.file_name().unwrap_or_default().to_os_string();
    name.push(".velo-tmp");
    save_path.with_file_name(name)
}
