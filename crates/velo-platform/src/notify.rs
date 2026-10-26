//! Desktop notifications.

use crate::{Error, Result};

/// Shows a desktop notification with `title` and `body`, from Velo.
///
/// Returns once the system has taken it; whether and how it shows depends on the user's
/// notification settings.
pub fn notify(title: &str, body: &str) -> Result<()> {
    notify_rust::Notification::new()
        .appname("Velo")
        .summary(title)
        .body(body)
        .show()
        .map(|_| ())
        .map_err(|e| Error::Notify(e.to_string()))
}
