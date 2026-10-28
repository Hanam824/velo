//! Describing things for a log without their secrets.
//!
//! Download addresses carry signatures and the viewer's IP, headers carry cookies and
//! passwords, and paths carry the user's name. Log them only through these functions.

use std::path::Path;

/// Describes a download address without its secrets.
///
/// Keeps the host, the path and the names of the query parameters, and the values of the few
/// that say what kind of stream it is (`c`, `itag`, `mime`, `expire`, `clen`); signatures,
/// tokens and the viewer's address stay out.
pub fn url(url: &str) -> String {
    let (head, query) = url.split_once('?').unwrap_or((url, ""));
    let params: Vec<String> = query
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| match p.split_once('=') {
            Some((k, v)) if ["c", "itag", "mime", "expire", "clen"].contains(&k) => {
                format!("{k}={v}")
            }
            Some((k, _)) => k.to_string(),
            None => p.to_string(),
        })
        .collect();
    format!("{head}?{}", params.join("&"))
}

/// The value of header `name` as it may appear in a log: `<redacted>` for the ones that hold
/// credentials, the value itself for the rest.
pub fn header(name: &str, value: &str) -> String {
    const SECRET: [&str; 4] = [
        "authorization",
        "proxy-authorization",
        "cookie",
        "set-cookie",
    ];
    if SECRET.iter().any(|s| name.eq_ignore_ascii_case(s)) {
        "<redacted>".into()
    } else {
        value.into()
    }
}

/// `path` with the user's home folder replaced by `~`.
pub fn path(path: &Path) -> String {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    match home.as_deref().map(Path::new) {
        Some(home) if !home.as_os_str().is_empty() => match path.strip_prefix(home) {
            Ok(rest) => format!("~/{}", rest.display()),
            Err(_) => path.display().to_string(),
        },
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_keep_only_the_harmless_parts() {
        assert_eq!(
            url("https://a.example/videoplayback?expire=9&ip=1.2.3.4&c=IOS&sig=secret&itag=140"),
            "https://a.example/videoplayback?expire=9&ip&c=IOS&sig&itag=140"
        );
        assert_eq!(url("https://a.example/f.bin"), "https://a.example/f.bin?");
    }

    #[test]
    fn credentials_in_headers_are_hidden() {
        assert_eq!(header("Cookie", "a=1"), "<redacted>");
        assert_eq!(header("authorization", "Basic xyz"), "<redacted>");
        assert_eq!(
            header("Referer", "https://a.example/"),
            "https://a.example/"
        );
    }

    #[test]
    fn the_home_folder_becomes_a_tilde() {
        let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"))
        else {
            return;
        };
        let inside = Path::new(&home).join("Downloads").join("a.bin");
        assert_eq!(path(&inside), "~/Downloads/a.bin");
        assert_eq!(path(Path::new("/elsewhere/a.bin")), "/elsewhere/a.bin");
    }
}
