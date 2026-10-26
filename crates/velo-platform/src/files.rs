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
