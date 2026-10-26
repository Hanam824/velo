//! The local-socket protocol between the Velo app, `velo-cli` and the browser native-messaging host.
//!
//! The running app listens on an [`Endpoint`]; other processes connect, send one
//! [`Message`] as a line of JSON, read one reply line saying whether the app took it, and
//! disconnect. Every line carries a protocol version, so an older app can refuse what it doesn't
//! understand instead of misreading it.
//!
//! The socket is a named pipe on Windows, an abstract socket on Linux and a socket file on
//! macOS, chosen at run time by what the system supports.

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use velo_core::QueueId;

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
    /// The app understood the message but refused it.
    #[error("{0}")]
    Refused(String),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// A request to the running app.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Message {
    /// Show the main window and bring it to the front; sent by a second launch of the app.
    Activate,
    /// Do nothing but answer, to learn whether the app runs.
    Ping,
    /// Add a download of `url`.
    Add {
        /// The address to download.
        url: String,
        /// The folder to save into; the app's choice for the file's category if `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        save_dir: Option<PathBuf>,
        /// The name to save as; the server's name if `None`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        file_name: Option<String>,
        /// Start it at once; otherwise it waits in the main queue.
        start: bool,
        /// Add it without asking; otherwise the app shows Download File Info first.
        silent: bool,
        /// For a YouTube address added without asking: the formats (itags) to download, one
        /// stream or video then sound; the best choice if empty.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        itags: Vec<u32>,
        /// The page the link was on, sent as `Referer`.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        referer: Option<String>,
        /// Cookies for the address, sent as `Cookie`; kept in memory only.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        cookies: Option<String>,
    },
    /// Start a queue: `queue`, or the main queue if `None`.
    StartQueue {
        /// The queue to start.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        queue: Option<QueueId>,
    },
}

/// The app's answer to a [`Message`]: `Ok` if it took it, or why it refused it.
pub type Reply = std::result::Result<(), String>;

/// A message as sent over the socket: the protocol version and the message.
#[derive(Debug, Serialize, Deserialize)]
struct Envelope {
    version: u32,
    #[serde(flatten)]
    message: Message,
}

/// A reply as sent over the socket.
#[derive(Debug, Serialize, Deserialize)]
struct ReplyLine {
    version: u32,
    ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<String>,
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

/// Whether an app listens at `endpoint`.
pub fn is_running(endpoint: &Endpoint) -> bool {
    send(endpoint, &Message::Ping).is_ok()
}

/// Sends `message` to the app listening at `endpoint` and waits for its reply.
///
/// Fails with [`Error::NotRunning`] if nothing listens there, and with [`Error::Refused`] if
/// the app refused the message.
pub fn send(endpoint: &Endpoint, message: &Message) -> Result<()> {
    let stream = match Stream::connect(endpoint.name()?) {
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
    let mut stream = BufReader::new(stream);
    write_line(
        stream.get_mut(),
        &Envelope {
            version: VERSION,
            message: message.clone(),
        },
    )?;
    let mut line = String::new();
    stream.read_line(&mut line)?;
    match parse_reply(&line)? {
        Ok(()) => Ok(()),
        Err(reason) => Err(Error::Refused(reason)),
    }
}

/// Writes `value` as one line of JSON.
fn write_line(stream: &mut Stream, value: &impl Serialize) -> Result<()> {
    let mut line = serde_json::to_string(value).map_err(|e| Error::Malformed(e.to_string()))?;
    line.push('\n');
    stream.write_all(line.as_bytes())?;
    stream.flush()?;
    Ok(())
}

/// Listens at `endpoint` on a background thread and answers each message received with what
/// `on_message` returns, for as long as the process runs.
///
/// `on_message` runs on that thread, so it must hand UI work to the UI thread. A socket file
/// left by an app that crashed is replaced. Fails if another app already listens there; check
/// with [`send`] first.
pub fn listen(
    endpoint: &Endpoint,
    on_message: impl Fn(Message) -> Reply + Send + 'static,
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
                let mut stream = BufReader::new(stream);
                let mut line = String::new();
                if stream.read_line(&mut line).is_err() {
                    continue;
                }
                let reply = parse(&line)
                    .map_err(|error| error.to_string())
                    .and_then(&on_message);
                // The sender may have gone; it then doesn't need the answer.
                let _ = write_line(
                    stream.get_mut(),
                    &ReplyLine {
                        version: VERSION,
                        ok: reply.is_ok(),
                        error: reply.err(),
                    },
                );
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

/// Reads a reply line. An empty line means the app closed the connection without answering,
/// which counts as taken.
fn parse_reply(line: &str) -> Result<Reply> {
    if line.trim().is_empty() {
        return Ok(Ok(()));
    }
    let reply: ReplyLine =
        serde_json::from_str(line.trim()).map_err(|e| Error::Malformed(e.to_string()))?;
    Ok(if reply.ok {
        Ok(())
    } else {
        Err(reply.error.unwrap_or_default())
    })
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

        let add = parse(r#"{"version":1,"type":"add","url":"https://a.test/x.zip","start":true,"silent":false}"#)
            .unwrap();
        assert_eq!(
            add,
            Message::Add {
                url: "https://a.test/x.zip".into(),
                save_dir: None,
                file_name: None,
                start: true,
                silent: false,
                itags: Vec::new(),
                referer: None,
                cookies: None,
            }
        );
        assert_eq!(
            parse(r#"{"version":1,"type":"start_queue"}"#).unwrap(),
            Message::StartQueue { queue: None }
        );
        assert_eq!(parse_reply("").unwrap(), Ok(()));
        assert_eq!(
            parse_reply(r#"{"version":1,"ok":false,"error":"no"}"#).unwrap(),
            Err("no".into())
        );
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
            let refused = matches!(message, Message::StartQueue { .. });
            let _ = tx.send(message);
            if refused {
                Err("no such queue".into())
            } else {
                Ok(())
            }
        })
        .unwrap();
        send(&endpoint, &Message::Activate).unwrap();
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(5)).unwrap(),
            Message::Activate
        );
        let add = Message::Add {
            url: "https://a.test/x.zip".into(),
            save_dir: Some("/tmp/in".into()),
            file_name: Some("y.zip".into()),
            start: false,
            silent: true,
            itags: vec![137, 140],
            referer: Some("https://a.test/".into()),
            cookies: Some("s=1".into()),
        };
        send(&endpoint, &add).unwrap();
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), add);
        let start = Message::StartQueue {
            queue: Some(QueueId(3)),
        };
        assert!(matches!(
            send(&endpoint, &start),
            Err(Error::Refused(reason)) if reason == "no such queue"
        ));
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
