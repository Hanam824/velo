//! Velo's native-messaging host: the program a browser starts for Velo's extension.
//!
//! The browser and the host exchange JSON messages on the host's standard input and output,
//! each preceded by its length as a 32-bit number in the machine's byte order. The host reads
//! one request at a time and answers each with one response, until the browser closes the
//! connection. It hands downloads to the app over [`velo_ipc`], starting the app if needed, and
//! answers format questions itself with [`velo_extract`], so the format menu works even while
//! the app starts.
//!
//! Every request has a `type`; see [`Request`] for them and the answers. The protocol version
//! is [`PROTOCOL`]; an incompatible change raises it, and the extension checks it with `hello`.

use std::io::{Read, Write};

use serde::Deserialize;
use serde_json::{Value, json};

/// The version of the messages this host speaks.
pub const PROTOCOL: u32 = 1;

/// The longest message accepted or sent, in bytes; browsers refuse longer ones from hosts.
pub const MAX_MESSAGE: usize = 1024 * 1024;

/// Errors reading or writing messages.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// Standard input or output failed.
    #[error("couldn't talk to the browser: {0}")]
    Io(#[from] std::io::Error),
    /// A message is longer than [`MAX_MESSAGE`].
    #[error("a message of {0} bytes is too long")]
    TooLong(usize),
    /// A message isn't JSON.
    #[error("a message isn't JSON: {0}")]
    Json(#[from] serde_json::Error),
}

/// Result with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Reads one message, or `None` once the browser has closed the connection.
pub fn read_message(input: &mut impl Read) -> Result<Option<Value>> {
    let mut length = [0u8; 4];
    match input.read_exact(&mut length) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let length = u32::from_ne_bytes(length) as usize;
    if length > MAX_MESSAGE {
        return Err(Error::TooLong(length));
    }
    let mut message = vec![0; length];
    input.read_exact(&mut message)?;
    Ok(Some(serde_json::from_slice(&message)?))
}

/// Writes one message.
pub fn write_message(output: &mut impl Write, message: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(message)?;
    if bytes.len() > MAX_MESSAGE {
        return Err(Error::TooLong(bytes.len()));
    }
    output.write_all(&(bytes.len() as u32).to_ne_bytes())?;
    output.write_all(&bytes)?;
    output.flush()?;
    Ok(())
}

/// A request from the extension.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    /// Who are you? Answered with `{"type": "hello", "protocol", "app_version",
    /// "capture_extensions"}`: the file types the user wants captured.
    Hello,
    /// Download `url`, asking the user in Download File Info. Answered with `result`.
    Download {
        /// The address.
        url: String,
        /// The page the link was on.
        #[serde(default)]
        referer: Option<String>,
        /// The cookies the browser has for the address.
        #[serde(default)]
        cookies: Option<String>,
        /// The name the page suggested.
        #[serde(default)]
        file_name: Option<String>,
    },
    /// Which formats does the YouTube video at `url` have? Answered with `{"type": "formats",
    /// "title", "author", "choices": [{"label", "itags"}]}`, best first.
    ListFormats {
        /// The YouTube address.
        url: String,
    },
    /// Download formats `itags` of the YouTube video at `url` without asking. Answered with
    /// `result`.
    DownloadFormat {
        /// The YouTube address.
        url: String,
        /// One stream, or video then sound.
        itags: Vec<u32>,
    },
}

/// What the host needs from the rest of Velo; the real one talks to the app and YouTube.
pub trait Backend {
    /// Hands `message` to the app, starting it if it isn't running.
    fn send(&self, message: velo_ipc::Message) -> Result<(), String>;
    /// The video at YouTube address `url`.
    fn video(&self, url: &str) -> Result<velo_extract::Video, String>;
    /// The file types the user wants captured, from the settings.
    fn capture_extensions(&self) -> Vec<String>;
}

/// The answer to `message`, which is a request or something the host doesn't understand.
pub fn answer(message: Value, backend: &impl Backend) -> Value {
    let request = match serde_json::from_value::<Request>(message) {
        Ok(request) => request,
        Err(error) => return json!({ "type": "error", "error": error.to_string() }),
    };
    match request {
        Request::Hello => json!({
            "type": "hello",
            "protocol": PROTOCOL,
            "app_version": env!("CARGO_PKG_VERSION"),
            "capture_extensions": backend.capture_extensions(),
        }),
        Request::Download {
            url,
            referer,
            cookies,
            file_name,
        } => result(backend.send(velo_ipc::Message::Add {
            url,
            save_dir: None,
            file_name,
            start: true,
            silent: false,
            itags: Vec::new(),
            referer,
            cookies,
        })),
        Request::ListFormats { url } => match backend.video(&url) {
            Ok(video) => {
                let choices: Vec<Value> = video
                    .choices()
                    .iter()
                    .map(|c| json!({ "label": c.label(), "itags": c.itags() }))
                    .collect();
                json!({
                    "type": "formats",
                    "title": video.title,
                    "author": video.author,
                    "choices": choices,
                })
            }
            Err(error) => json!({ "type": "error", "error": error }),
        },
        Request::DownloadFormat { url, itags } => result(backend.send(velo_ipc::Message::Add {
            url,
            save_dir: None,
            file_name: None,
            start: true,
            silent: true,
            itags,
            referer: None,
            cookies: None,
        })),
    }
}

/// A `result` answer.
fn result(outcome: Result<(), String>) -> Value {
    match outcome {
        Ok(()) => json!({ "type": "result", "ok": true }),
        Err(error) => json!({ "type": "result", "ok": false, "error": error }),
    }
}

/// Answers requests from `input` on `output` until the browser closes the connection.
pub fn serve(input: &mut impl Read, output: &mut impl Write, backend: &impl Backend) -> Result<()> {
    while let Some(message) = read_message(input)? {
        write_message(output, &answer(message, backend))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use velo_extract::{Format, StreamKind, Video};

    use super::*;

    /// Records what it is asked and answers from fixed data.
    #[derive(Default)]
    struct Fake {
        sent: RefCell<Vec<velo_ipc::Message>>,
        refuse: bool,
    }

    impl Backend for Fake {
        fn send(&self, message: velo_ipc::Message) -> Result<(), String> {
            self.sent.borrow_mut().push(message);
            if self.refuse {
                Err("Velo couldn't start".into())
            } else {
                Ok(())
            }
        }

        fn video(&self, url: &str) -> Result<Video, String> {
            if !url.contains("youtu") {
                return Err("not a YouTube video address".into());
            }
            let format = |itag, kind, mime: &str, height| Format {
                itag,
                url: Some(format!("https://media.test/{itag}")),
                signature_cipher: None,
                mime_type: mime.into(),
                codecs: vec!["avc1.640028".into()],
                kind,
                width: None,
                height,
                fps: Some(30),
                bitrate: Some(128_000),
                content_length: None,
                quality_label: None,
                audio_sample_rate: None,
            };
            Ok(Video {
                id: "dQw4w9WgXcQ".into(),
                title: "A video".into(),
                author: "Someone".into(),
                duration: Some(10),
                formats: vec![
                    format(137, StreamKind::Video, "video/mp4", Some(1080)),
                    format(140, StreamKind::Audio, "audio/mp4", None),
                ],
                client: "ios",
            })
        }

        fn capture_extensions(&self) -> Vec<String> {
            vec!["zip".into(), "iso".into()]
        }
    }

    /// The bytes of `messages` as the browser sends them.
    fn framed(messages: &[Value]) -> Vec<u8> {
        let mut bytes = Vec::new();
        for message in messages {
            write_message(&mut bytes, message).unwrap();
        }
        bytes
    }

    #[test]
    fn messages_are_length_prefixed_json() {
        let message = json!({"type": "hello"});
        let bytes = framed(std::slice::from_ref(&message));
        assert_eq!(&bytes[..4], &16u32.to_ne_bytes());
        let mut input = bytes.as_slice();
        assert_eq!(read_message(&mut input).unwrap(), Some(message));
        assert_eq!(read_message(&mut input).unwrap(), None);

        let mut huge = ((MAX_MESSAGE + 1) as u32).to_ne_bytes().to_vec();
        huge.extend_from_slice(b"{}");
        assert!(matches!(
            read_message(&mut huge.as_slice()),
            Err(Error::TooLong(_))
        ));
    }

    #[test]
    fn every_request_gets_its_answer() {
        let fake = Fake::default();
        let input = framed(&[
            json!({"type": "hello"}),
            json!({"type": "download", "url": "https://a.test/x.zip", "referer": "https://a.test/", "cookies": "s=1"}),
            json!({"type": "list_formats", "url": "https://youtu.be/dQw4w9WgXcQ"}),
            json!({"type": "download_format", "url": "https://youtu.be/dQw4w9WgXcQ", "itags": [137, 140]}),
            json!({"type": "explode"}),
        ]);
        let mut output = Vec::new();
        serve(&mut input.as_slice(), &mut output, &fake).unwrap();

        let mut answers = Vec::new();
        let mut reader = output.as_slice();
        while let Some(answer) = read_message(&mut reader).unwrap() {
            answers.push(answer);
        }
        assert_eq!(answers.len(), 5);
        assert_eq!(answers[0]["protocol"], PROTOCOL);
        assert_eq!(answers[0]["capture_extensions"], json!(["zip", "iso"]));
        assert_eq!(answers[1], json!({"type": "result", "ok": true}));
        assert_eq!(answers[2]["title"], "A video");
        assert_eq!(answers[2]["choices"][0]["itags"], json!([137, 140]));
        assert_eq!(answers[3], json!({"type": "result", "ok": true}));
        assert_eq!(answers[4]["type"], "error");

        let sent = fake.sent.borrow();
        assert!(matches!(
            &sent[0],
            velo_ipc::Message::Add { url, silent: false, referer: Some(r), cookies: Some(c), .. }
                if url == "https://a.test/x.zip" && r == "https://a.test/" && c == "s=1"
        ));
        assert!(matches!(
            &sent[1],
            velo_ipc::Message::Add { silent: true, start: true, itags, .. } if itags == &[137, 140]
        ));
    }

    #[test]
    fn failures_are_answered_not_fatal() {
        let fake = Fake {
            refuse: true,
            ..Fake::default()
        };
        assert_eq!(
            answer(
                json!({"type": "download", "url": "https://a.test/x"}),
                &fake
            ),
            json!({"type": "result", "ok": false, "error": "Velo couldn't start"})
        );
        assert_eq!(
            answer(
                json!({"type": "list_formats", "url": "https://a.test/"}),
                &fake
            )["type"],
            "error"
        );
    }
}
