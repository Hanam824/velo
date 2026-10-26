//! The local-socket protocol between the Velo app, `velo-cli` and the browser native-messaging host.
//!
//! The running app listens on an [`Endpoint`]; other processes connect, send one
//! [`Message`] as a line of JSON and disconnect. Every line carries a protocol version, so an
//! older app can refuse what it doesn't understand instead of misreading it.
//!
//! The socket is a named pipe on Windows, an abstract socket on Linux and a socket file on
//! macOS, chosen at run time by what the system supports.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use interprocess::local_socket::{
    GenericFilePath, GenericNamespaced, ListenerOptions, Name, Stream, prelude::*,
};
use serde::{Deserialize, Serialize};

/// The protocol version this build speaks.
pub const VERSION: u32 = 1;

/// Errors from the local socket.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Nothing listens: the app isn't running.
    #[error("Velo isn't running")]
    NotRunning,
    /// The socket failed.
    #[error("local socket error: {0}")]
    Io(#[from] std::io::Error),
    /// A line wasn't a message this version understands.
    #[error("unreadable message: {0}")]
    Malformed(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A request to the running app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Show the main window and bring it to the front; sent by a second launch of the app.
    Activate,
}

/// A message as sent over the socket: the protocol version and the message.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    #[serde(flatten)]
    message: Message,
}

/// Where the running app listens: one place per user, so two users on one computer each get
/// their own app.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    /// The name on systems with a socket namespace (Windows, Linux).
    namespaced: String,
    /// The socket file elsewhere (macOS).
    file: PathBuf,
}

impl Endpoint {
    /// The endpoint for an app keeping its data in `data_dir`: a socket file there, or a name
    /// derived from that folder, which is per user.
    pub fn for_data_dir(data_dir: &std::path::Path) -> Self {
        // FNV-1a: stable across builds, unlike the standard library's hasher.
        let hash = data_dir
            .to_string_lossy()
            .bytes()
            .fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
                (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
            });
        Self {
            namespaced: format!("velo-{hash:016x}.sock"),
            file: data_dir.join("velo.sock"),
        }
    }

    /// The socket name this system uses.
    fn name(&self) -> std::io::Result<Name<'_>> {
        if GenericNamespaced::is_supported() {
            self.namespaced.as_str().to_ns_name::<GenericNamespaced>()
        } else {
            self.file.as_path().to_fs_name::<GenericFilePath>()
        }
    }
}

/// Sends `message` to the app listening at `endpoint`.
///
/// Fails with [`Error::NotRunning`] if nothing listens there.
pub fn send(endpoint: &Endpoint, message: &Message) -> Result<()> {
    let mut stream = match Stream::connect(endpoint.name()?) {
        Ok(stream) => stream,
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Err(Error::NotRunning);
        }
        Err(e) => return Err(e.into()),
    };
    let mut line = serde_json::to_string(&Envelope {
        version: VERSION,
        message: message.clone(),
    })
    .map_err(|e| Error::Malformed(e.to_string()))?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// Listens at `endpoint` on a background thread and calls `on_message` with each message
/// received, for as long as the process runs.
///
/// A socket file left by an app that crashed is replaced. Fails if another app already
/// listens there; check with [`send`] first.
pub fn listen(
    endpoint: &Endpoint,
    on_message: impl Fn(Message) + Send + 'static,
) -> Result<std::thread::JoinHandle<()>> {
    if let Some(dir) = endpoint.file.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let listener = ListenerOptions::new()
        .name(endpoint.name()?)
        .try_overwrite(true)
        .create_sync()?;
    let thread = std::thread::Builder::new()
        .name("velo-ipc".into())
        .spawn(move || {
            for stream in listener.incoming().filter_map(std::io::Result::ok) {
                let mut line = String::new();
                if BufReader::new(stream).read_line(&mut line).is_err() {
                    continue;
                }
                match parse(&line) {
                    Ok(message) => on_message(message),
                    Err(error) => eprintln!("velo: ignoring a message: {error}"),
                }
            }
        })?;
    Ok(thread)
}

/// Reads one line of the protocol.
fn parse(line: &str) -> Result<Message> {
    let envelope: Envelope =
        serde_json::from_str(line.trim()).map_err(|e| Error::Malformed(e.to_string()))?;
    if envelope.version != VERSION {
        return Err(Error::Malformed(format!(
            "protocol version {} (this Velo speaks {VERSION})",
            envelope.version
        )));
    }
    Ok(envelope.message)
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use super::*;

    #[test]
    fn messages_are_versioned_json_lines() {
        let line = serde_json::to_string(&Envelope {
            version: VERSION,
            message: Message::Activate,
        })
        .unwrap();
        assert_eq!(line, r#"{"version":1,"type":"activate"}"#);
        assert_eq!(parse(&line).unwrap(), Message::Activate);
        assert!(parse(r#"{"version":2,"type":"activate"}"#).is_err());
        assert!(parse(r#"{"version":1,"type":"explode"}"#).is_err());
    }

    #[test]
    fn a_message_reaches_the_listener_and_nothing_listening_means_not_running() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = Endpoint::for_data_dir(dir.path());
        assert!(matches!(
            send(&endpoint, &Message::Activate),
            Err(Error::NotRunning)
        ));

        let (tx, rx) = mpsc::channel();
        listen(&endpoint, move |message| {
            let _ = tx.send(message);
        })
        .unwrap();
        send(&endpoint, &Message::Activate).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::Activate
        );
    }

    #[test]
    fn endpoints_differ_per_data_folder() {
        let a = Endpoint::for_data_dir(std::path::Path::new("/home/a/.local/share/velo"));
        let b = Endpoint::for_data_dir(std::path::Path::new("/home/b/.local/share/velo"));
        assert_ne!(a, b);
        assert_eq!(
            a,
            Endpoint::for_data_dir(std::path::Path::new("/home/a/.local/share/velo"))
        );
    }
}
