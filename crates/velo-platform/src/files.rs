//! Opening files and showing them in the file manager.

use std::path::Path;

use crate::{Error, Result};

/// Opens `path` with the app the system uses for its type, without waiting for that app.
pub fn open(path: &Path) -> Result<()> {
    opener::open(path).map_err(|e| Error::Open {
        path: path.to_path_buf(),
        message: e.to_string(),
    })
}

/// Shows `path` in the file manager, selected where the file manager supports it; otherwise
/// opens the folder that holds it.
pub fn reveal(path: &Path) -> Result<()> {
    opener::reveal(path).or_else(|_| match path.parent() {
        Some(folder) => open(folder),
        None => Err(Error::Open {
            path: path.to_path_buf(),
            message: "it has no folder".into(),
        }),
    })
}

/// Sets `file`'s length to `len` without writing the bytes, so writes anywhere in it are cheap.
///
/// Unwritten ranges read as zeros on every OS. On Windows the file is first marked sparse:
/// otherwise NTFS fills the gap before a write far into the file with zeros while the write
/// waits. Where the file system has no sparse files (FAT32, exFAT) the length is set anyway.
/// No disk space is reserved, so a full disk shows up as a failed write later.
pub fn preallocate(file: &std::fs::File, len: u64) -> Result<()> {
    #[cfg(windows)]
    if let Err(error) = set_sparse(file) {
        tracing::warn!(%error, "couldn't mark a download file sparse");
    }
    file.set_len(len)?;
    Ok(())
}

/// Marks `file` sparse with `FSCTL_SET_SPARSE`.
#[cfg(windows)]
#[allow(unsafe_code)]
fn set_sparse(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::System::IO::DeviceIoControl;
    use windows_sys::Win32::System::Ioctl::FSCTL_SET_SPARSE;

    let mut returned = 0u32;
    // SAFETY: the handle is open for as long as `file` is borrowed; no buffers are passed
    // besides `returned`, which outlives the synchronous call.
    let ok = unsafe {
        DeviceIoControl(
            file.as_raw_handle(),
            FSCTL_SET_SPARSE,
            std::ptr::null(),
            0,
            std::ptr::null_mut(),
            0,
            &mut returned,
            std::ptr::null_mut(),
        )
    };
    if ok == 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use super::*;

    #[test]
    fn a_preallocated_file_has_its_length_and_reads_zeros_where_unwritten() {
        let dir = tempfile::tempdir().unwrap();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(dir.path().join("big"))
            .unwrap();
        let len = 1 << 30;
        preallocate(&file, len).unwrap();
        assert_eq!(file.metadata().unwrap().len(), len);
        file.seek(SeekFrom::Start(len - 1)).unwrap();
        file.write_all(&[7]).unwrap();
        file.seek(SeekFrom::Start(0)).unwrap();
        let mut first = [1u8; 4];
        file.read_exact(&mut first).unwrap();
        assert_eq!(first, [0; 4]);
        assert_eq!(file.metadata().unwrap().len(), len);
    }
}
