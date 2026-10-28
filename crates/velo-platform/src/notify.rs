//! Desktop notifications.

use crate::{Error, Result};

/// Shows a desktop notification with `title` and `body`, from Velo.
///
/// Returns at once: the system is asked on a background thread, because on macOS the call
/// blocks and runs the app's event loop, which crashes the app when made from inside one of
/// its own event handlers. Whether and how the notification shows depends on the user's
/// notification settings; a failure to deliver it is logged, not returned.
pub fn notify(title: &str, body: &str) -> Result<()> {
    let (title, body) = (title.to_string(), body.to_string());
    in_background(move || {
        let shown = notify_rust::Notification::new()
            .appname("Velo")
            .summary(&title)
            .body(&body)
            .show();
        if let Err(error) = shown {
            tracing::warn!(%error, title, "couldn't show a notification");
        }
    })
}

/// Runs `work` on a new thread named `velo-notify`, without waiting for it.
fn in_background(work: impl FnOnce() + Send + 'static) -> Result<()> {
    std::thread::Builder::new()
        .name("velo-notify".into())
        .spawn(work)
        .map(|_| ())
        .map_err(|e| Error::Notify(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn the_work_runs_on_another_thread_without_being_waited_for() {
        let (tx, rx) = mpsc::channel();
        let caller = std::thread::current().id();
        let started = Instant::now();
        in_background(move || {
            std::thread::sleep(Duration::from_millis(300));
            let _ = tx.send((
                std::thread::current().id(),
                std::thread::current().name().map(String::from),
            ));
        })
        .unwrap();
        assert!(started.elapsed() < Duration::from_millis(200), "it waited");
        let (thread, name) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_ne!(thread, caller);
        assert_eq!(name.as_deref(), Some("velo-notify"));
    }
}
