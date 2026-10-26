//! Joins a finished download's part files into the file at its save path.

use std::path::{Path, PathBuf};

use tokio::fs::{self, File};
use tokio::io::{AsyncWriteExt, BufWriter};
use velo_core::{Segment, SegmentState};

use crate::{Error, Result};

/// Joins the part files in `dir` into `save_path`, then deletes `dir`.
///
/// The parts are copied in `start` order into `<save_path>.velo-tmp`, which is synced to disk,
/// checked against `expected_size` (when given) and only then renamed to `save_path`. So the
/// real name never holds a partial file, and on any error the part files are left as they were.
/// Fails with [`Error::NotFinished`] unless every segment is done and they tile the file, with
/// [`Error::PartSize`] if a part file isn't exactly its segment's length, and with
/// [`Error::Exists`] if something is already at `save_path`.
pub async fn merge(
    dir: &Path,
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

    let temp = temp_path(save_path);
    if let Err(error) = join(dir, segments, &temp, position).await {
        let _ = fs::remove_file(&temp).await;
        return Err(error);
    }
    fs::rename(&temp, save_path).await?;
    fs::remove_dir_all(dir).await?;
    Ok(())
}

/// Path the parts are joined into before the rename: `<save_path>.velo-tmp`.
fn temp_path(save_path: &Path) -> PathBuf {
    let mut name = save_path.file_name().unwrap_or_default().to_os_string();
    name.push(".velo-tmp");
    save_path.with_file_name(name)
}

/// Copies the part files into `out`, syncs it and checks it holds `size` bytes.
async fn join(dir: &Path, segments: &[Segment], out: &Path, size: u64) -> Result<()> {
    let mut writer = BufWriter::new(File::create(out).await?);
    for segment in segments {
        let path = dir.join(segment.part_file_name());
        let mut part = File::open(&path).await?;
        let actual = part.metadata().await?.len();
        if actual != segment.len() {
            return Err(Error::PartSize {
                path,
                expected: segment.len(),
                actual,
            });
        }
        tokio::io::copy(&mut part, &mut writer).await?;
    }
    writer.flush().await?;
    let file = writer.into_inner();
    file.sync_all().await?;
    let written = file.metadata().await?.len();
    if written != size {
        return Err(Error::SizeMismatch {
            expected: size,
            actual: written,
        });
    }
    Ok(())
}
