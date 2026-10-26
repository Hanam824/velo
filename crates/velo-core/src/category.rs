//! Categories: named groups of file extensions, each with an optional save folder.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// A named group of file extensions, such as "Video", used to file and filter downloads.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Category {
    /// Name shown in the categories tree.
    pub name: String,
    /// Extensions in this category, lowercase and without the dot.
    pub extensions: Vec<String>,
    /// Folder that downloads in this category are saved to; the default download folder if unset.
    pub save_dir: Option<PathBuf>,
}

impl Category {
    /// Creates a category with no save folder of its own.
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|e| e.to_string()).collect(),
            save_dir: None,
        }
    }

    /// The built-in categories: Compressed, Documents, Music, Programs and Video.
    pub fn defaults() -> Vec<Self> {
        vec![
            Self::new(
                "Compressed",
                &[
                    "7z", "ace", "arj", "bz2", "cab", "gz", "lz", "lzma", "rar", "tar", "tgz",
                    "xz", "zip", "zst",
                ],
            ),
            Self::new(
                "Documents",
                &[
                    "csv", "doc", "docx", "epub", "md", "odp", "ods", "odt", "pdf", "pps", "ppt",
                    "pptx", "rtf", "txt", "xls", "xlsx",
                ],
            ),
            Self::new(
                "Music",
                &[
                    "aac", "aif", "aiff", "flac", "m4a", "mp3", "mpa", "ogg", "opus", "ra", "wav",
                    "wma",
                ],
            ),
            Self::new(
                "Programs",
                &[
                    "apk", "appimage", "bin", "deb", "dmg", "exe", "msi", "pkg", "rpm",
                ],
            ),
            Self::new(
                "Video",
                &[
                    "3gp", "asf", "avi", "flv", "m4v", "mkv", "mov", "mp4", "mpe", "mpeg", "mpg",
                    "ogv", "qt", "rm", "ts", "webm", "wmv",
                ],
            ),
        ]
    }

    /// Whether `file_name`'s extension belongs to this category, ignoring case.
    pub fn matches(&self, file_name: &str) -> bool {
        let Some((_, ext)) = file_name.rsplit_once('.') else {
            return false;
        };
        self.extensions.iter().any(|e| e.eq_ignore_ascii_case(ext))
    }

    /// The first category in `categories` that `file_name` belongs to.
    pub fn for_file_name<'a>(categories: &'a [Self], file_name: &str) -> Option<&'a Self> {
        categories.iter().find(|c| c.matches(file_name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_matched_by_extension_ignoring_case() {
        let categories = Category::defaults();
        let name = |file| Category::for_file_name(&categories, file).map(|c| c.name.as_str());
        assert_eq!(name("movie.MKV"), Some("Video"));
        assert_eq!(name("archive.tar.gz"), Some("Compressed"));
        assert_eq!(name("setup.exe"), Some("Programs"));
        assert_eq!(name("README"), None);
        assert_eq!(name("page.html"), None);
    }
}
