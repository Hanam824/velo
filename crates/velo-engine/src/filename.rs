//! Picks the file name for a download from the server's `Content-Disposition` or the URL.

use std::borrow::Cow;

use percent_encoding::percent_decode_str;
use reqwest::Url;

/// Name used when neither the server nor the URL gives one.
const FALLBACK: &str = "download";

/// Longest file name, in bytes, that every supported file system accepts.
const MAX_LEN: usize = 255;

/// Returns the file name to save a download under.
///
/// The `filename*` and then `filename` parameter of `content_disposition` wins, then the last
/// non-empty segment of the URL's path, then `"download"`. The result is always passed through
/// [`sanitize_file_name`], so it is safe on every platform.
pub fn file_name(content_disposition: Option<&str>, url: &Url) -> String {
    content_disposition
        .and_then(from_content_disposition)
        .and_then(|name| sanitize_file_name(&name))
        .or_else(|| from_url(url).and_then(|name| sanitize_file_name(&name)))
        .unwrap_or_else(|| FALLBACK.into())
}

/// Makes a server-supplied name safe to use as a file name on Windows, macOS and Linux alike.
///
/// Keeps only the part after the last `/` or `\`, replaces control characters and
/// `< > : " | ? *` with `_`, trims spaces and trailing dots, prefixes Windows device names such as
/// `CON` with `_` and shortens the name to 255 bytes, keeping a short extension. Returns `None`
/// if nothing usable is left.
pub fn sanitize_file_name(name: &str) -> Option<String> {
    let name = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let name: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    let name = name.trim_start().trim_end_matches([' ', '.']);
    if name.is_empty() {
        return None;
    }

    let (stem, ext) = match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 16 => (stem, Some(ext)),
        _ => (name, None),
    };
    let reserved = |s: &str| {
        let upper = s.to_ascii_uppercase();
        matches!(upper.as_str(), "CON" | "PRN" | "AUX" | "NUL")
            || (upper.len() == 4
                && (upper.starts_with("COM") || upper.starts_with("LPT"))
                && upper.as_bytes()[3].is_ascii_digit())
    };
    let prefix = if reserved(stem.split('.').next().unwrap_or(stem)) {
        "_"
    } else {
        ""
    };

    let suffix_len = ext.map_or(0, |e| e.len() + 1);
    let mut stem = format!("{prefix}{stem}");
    if stem.len() + suffix_len > MAX_LEN {
        let mut cut = MAX_LEN - suffix_len;
        while !stem.is_char_boundary(cut) {
            cut -= 1;
        }
        stem.truncate(cut);
    }
    Some(match ext {
        Some(ext) => format!("{stem}.{ext}"),
        None => stem,
    })
}

/// The file name in a `Content-Disposition` header value, preferring the RFC 6266 `filename*`
/// parameter over `filename`.
fn from_content_disposition(value: &str) -> Option<String> {
    let mut plain = None;
    for param in split_params(value) {
        let Some((key, val)) = param.split_once('=') else {
            continue;
        };
        match key.trim().to_ascii_lowercase().as_str() {
            "filename*" => {
                if let Some(name) = decode_ext_value(val.trim()) {
                    return Some(name);
                }
            }
            "filename" => plain = Some(unquote(val.trim())),
            _ => {}
        }
    }
    plain.filter(|name| !name.is_empty())
}

/// Splits a header value on `;`, ignoring `;` inside quoted strings.
fn split_params(value: &str) -> Vec<&str> {
    let mut params = Vec::new();
    let (mut start, mut quoted, mut escaped) = (0, false, false);
    for (i, c) in value.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                params.push(&value[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    params.push(&value[start..]);
    params
}

/// Removes the quotes and backslash escapes from a quoted string; returns other values as is.
fn unquote(value: &str) -> String {
    let Some(inner) = value.strip_prefix('"').and_then(|v| v.strip_suffix('"')) else {
        return value.to_string();
    };
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        out.push(if c == '\\' {
            chars.next().unwrap_or(c)
        } else {
            c
        });
    }
    out
}

/// Decodes an RFC 8187 extended value, `charset'language'percent-encoded`. Only UTF-8 and
/// ISO-8859-1 are supported.
fn decode_ext_value(value: &str) -> Option<String> {
    let mut parts = value.splitn(3, '\'');
    let (charset, _language, encoded) = (parts.next()?, parts.next()?, parts.next()?);
    let bytes: Vec<u8> = percent_decode_str(encoded).collect();
    let name = if charset.eq_ignore_ascii_case("utf-8") {
        String::from_utf8(bytes).ok()?
    } else if charset.eq_ignore_ascii_case("iso-8859-1") {
        bytes.into_iter().map(char::from).collect()
    } else {
        return None;
    };
    (!name.is_empty()).then_some(name)
}

/// The last non-empty segment of the URL's path, percent-decoded.
fn from_url(url: &Url) -> Option<String> {
    let segment = url.path_segments()?.rev().find(|s| !s.is_empty())?;
    Some(match percent_decode_str(segment).decode_utf8_lossy() {
        Cow::Borrowed(s) => s.to_string(),
        Cow::Owned(s) => s,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn url(s: &str) -> Url {
        Url::parse(s).unwrap()
    }

    #[test]
    fn content_disposition_wins_over_the_url() {
        let u = url("http://example.test/get?id=1");
        assert_eq!(
            file_name(Some(r#"attachment; filename="report.pdf""#), &u),
            "report.pdf"
        );
        assert_eq!(
            file_name(
                Some(
                    r#"attachment; filename="fallback.txt"; filename*=UTF-8''na%C3%AFve%20file.txt"#
                ),
                &u
            ),
            "naïve file.txt"
        );
        assert_eq!(
            file_name(Some("attachment; filename*=iso-8859-1'en'caf%E9.txt"), &u),
            "café.txt"
        );
        assert_eq!(
            file_name(Some(r#"inline; filename="a;b \"c\".txt""#), &u),
            "a;b _c_.txt"
        );
    }

    #[test]
    fn the_url_path_is_used_when_the_header_has_no_name() {
        assert_eq!(
            file_name(
                Some("attachment"),
                &url("http://example.test/dir/My%20File.zip?x=1")
            ),
            "My File.zip"
        );
        assert_eq!(file_name(None, &url("http://example.test/dir/")), "dir");
        assert_eq!(file_name(None, &url("http://example.test/")), "download");
    }

    #[test]
    fn names_are_made_safe_on_every_platform() {
        assert_eq!(
            sanitize_file_name("../../etc/passwd").as_deref(),
            Some("passwd")
        );
        assert_eq!(
            sanitize_file_name(r"C:\Users\x\evil.exe").as_deref(),
            Some("evil.exe")
        );
        assert_eq!(
            sanitize_file_name("a<b>c:d?.txt").as_deref(),
            Some("a_b_c_d_.txt")
        );
        assert_eq!(sanitize_file_name("tab\there").as_deref(), Some("tab_here"));
        assert_eq!(sanitize_file_name("  name. . ").as_deref(), Some("name"));
        assert_eq!(sanitize_file_name("con.txt").as_deref(), Some("_con.txt"));
        assert_eq!(sanitize_file_name("COM1").as_deref(), Some("_COM1"));
        assert_eq!(
            sanitize_file_name("console.txt").as_deref(),
            Some("console.txt")
        );
        assert_eq!(sanitize_file_name(".hidden").as_deref(), Some(".hidden"));
        assert_eq!(sanitize_file_name(".."), None);
        assert_eq!(sanitize_file_name(""), None);
    }

    #[test]
    fn long_names_are_cut_to_255_bytes_keeping_the_extension() {
        let long = format!("{}.iso", "é".repeat(200));
        let name = sanitize_file_name(&long).unwrap();
        assert!(name.len() <= MAX_LEN);
        assert!(name.ends_with("é.iso"));
    }
}
