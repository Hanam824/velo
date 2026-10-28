//! A log file that starts a new one when it grows too big, keeping a few old ones.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// A file written to by many threads, rotated by size: `velo.log` becomes `velo.log.1`, which
/// becomes `velo.log.2`, and so on; the oldest is deleted.
///
/// It never fails a write: when the file can't be opened or written, log lines are dropped,
/// because logging must not stop the program.
#[derive(Debug)]
pub struct Rotating {
    path: PathBuf,
    max_bytes: u64,
    keep: u32,
    state: Mutex<State>,
}

#[derive(Debug)]
struct State {
    file: Option<File>,
    size: u64,
}

impl Rotating {
    /// Opens `path` for appending, first rotating it if it is already over `max_bytes`; keeps
    /// `keep` old files.
    pub fn open(path: &Path, max_bytes: u64, keep: u32) -> Self {
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let this = Self {
            path: path.to_path_buf(),
            max_bytes,
            keep,
            state: Mutex::new(State {
                file: None,
                size: 0,
            }),
        };
        {
            let mut state = this.state.lock().unwrap();
            let size = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
            if size > max_bytes {
                this.rotate(&mut state);
            } else {
                state.file = append(path);
                state.size = size;
            }
        }
        this
    }

    /// Writes `buf`, rotating first if it wouldn't fit.
    pub fn write_line(&self, buf: &[u8]) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if state.size > 0 && state.size + buf.len() as u64 > self.max_bytes {
            self.rotate(&mut state);
        }
        if let Some(file) = state.file.as_mut()
            && file.write_all(buf).is_ok()
        {
            state.size += buf.len() as u64;
        }
    }

    /// Flushes buffered bytes to the file.
    pub fn flush(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(file) = state.file.as_mut() {
            let _ = file.flush();
        }
    }

    fn rotate(&self, state: &mut State) {
        state.file = None;
        let name = |n: u32| {
            let mut name = self.path.clone().into_os_string();
            name.push(format!(".{n}"));
            PathBuf::from(name)
        };
        if self.keep == 0 {
            let _ = fs::remove_file(&self.path);
        } else {
            let _ = fs::remove_file(name(self.keep));
            for n in (1..self.keep).rev() {
                let _ = fs::rename(name(n), name(n + 1));
            }
            let _ = fs::rename(&self.path, name(1));
        }
        state.file = append(&self.path);
        state.size = 0;
    }
}

fn append(path: &Path) -> Option<File> {
    OpenOptions::new().create(true).append(true).open(path).ok()
}

/// A handle that writes to a shared [`Rotating`] file, for `tracing_subscriber`.
#[derive(Debug, Clone)]
pub struct Handle(pub std::sync::Arc<Rotating>);

impl Write for Handle {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.write_line(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        self.0.flush();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(path: &Path) -> String {
        fs::read_to_string(path).unwrap_or_default()
    }

    #[test]
    fn a_full_file_moves_aside_and_the_oldest_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        let log = Rotating::open(&path, 10, 2);
        for line in ["1111111\n", "2222222\n", "3333333\n", "4444444\n"] {
            log.write_line(line.as_bytes());
        }
        assert_eq!(read(&path), "4444444\n");
        assert_eq!(read(&dir.path().join("a.log.1")), "3333333\n");
        assert_eq!(read(&dir.path().join("a.log.2")), "2222222\n");
        assert!(!dir.path().join("a.log.3").exists());
    }

    #[test]
    fn an_oversized_file_is_rotated_when_opened_and_a_small_one_appended_to() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.log");
        fs::write(&path, "0123456789ABC").unwrap();
        let log = Rotating::open(&path, 10, 1);
        log.write_line(b"new\n");
        assert_eq!(read(&path), "new\n");
        assert_eq!(read(&dir.path().join("a.log.1")), "0123456789ABC");

        let log = Rotating::open(&path, 10, 1);
        log.write_line(b"more\n");
        assert_eq!(read(&path), "new\nmore\n");
    }

    #[test]
    fn an_unwritable_place_drops_lines_without_failing() {
        let dir = tempfile::tempdir().unwrap();
        // A file where the folder should be.
        let blocker = dir.path().join("blocker");
        fs::write(&blocker, "").unwrap();
        let log = Rotating::open(&blocker.join("a.log"), 10, 1);
        log.write_line(b"lost\n");
        log.flush();
    }
}
