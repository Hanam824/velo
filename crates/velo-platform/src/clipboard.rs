//! Watching the clipboard for copied text.

/// Tells what text was copied since it last looked.
///
/// Reading the clipboard is cheap enough to do about once a second from the UI thread.
pub struct ClipboardWatcher {
    /// The system clipboard; `None` until it can be opened (some Linux sessions have none).
    clipboard: Option<arboard::Clipboard>,
    /// The text seen last time.
    last: Option<String>,
}

impl ClipboardWatcher {
    /// Starts watching; what is on the clipboard now doesn't count as copied.
    pub fn new() -> Self {
        let mut watcher = Self {
            clipboard: None,
            last: None,
        };
        watcher.last = watcher.text();
        watcher
    }

    /// The clipboard's text if it changed since the last call, or since [`ClipboardWatcher::new`].
    pub fn copied(&mut self) -> Option<String> {
        let text = self.text();
        if text.is_some() && text != self.last {
            self.last.clone_from(&text);
            text
        } else {
            None
        }
    }

    /// The text on the clipboard now, if any.
    fn text(&mut self) -> Option<String> {
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        self.clipboard.as_mut()?.get_text().ok()
    }
}

impl Default for ClipboardWatcher {
    fn default() -> Self {
        Self::new()
    }
}
