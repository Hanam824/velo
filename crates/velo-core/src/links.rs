//! Download addresses as text: the lists Velo imports and exports, and which copied addresses
//! it offers to download.

/// Whether `text` looks like one http or https address: the scheme, something after it, and no
/// spaces.
pub fn is_http_url(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    ["http://", "https://"]
        .iter()
        .any(|scheme| lower.len() > scheme.len() && lower.starts_with(scheme))
        && !text.chars().any(char::is_whitespace)
}

/// The addresses in a list, one per line: blank lines, lines starting with `#` and lines that
/// aren't http or https addresses are skipped, and repeats are kept once, first come first.
pub fn read_list(text: &str) -> Vec<String> {
    let mut urls: Vec<String> = Vec::new();
    for line in text.lines().map(str::trim) {
        if is_http_url(line) && !urls.iter().any(|u| u == line) {
            urls.push(line.to_string());
        }
    }
    urls
}

/// A list of `urls`, one per line, that [`read_list`] reads back.
pub fn write_list<'a>(urls: impl IntoIterator<Item = &'a str>) -> String {
    urls.into_iter().map(|url| format!("{url}\n")).collect()
}

/// The lowercase extension of the file an address names, such as `zip` for
/// `https://a.test/x.ZIP?v=2`.
pub fn extension(url: &str) -> Option<String> {
    let path = url.split(['?', '#']).next()?;
    let (_, after_scheme) = path.split_once("://")?;
    // The host alone names no file.
    let (_, path) = after_scheme.split_once('/')?;
    let name = path.rsplit('/').next()?;
    let (stem, extension) = name.rsplit_once('.')?;
    (!stem.is_empty() && !extension.is_empty()).then(|| extension.to_ascii_lowercase())
}

/// The address in copied `text`, if it is one http or https address of a file whose extension
/// is in `extensions` (lowercase, without the dot).
pub fn capture(text: &str, extensions: &[String]) -> Option<String> {
    let text = text.trim();
    if !is_http_url(text) {
        return None;
    }
    let extension = extension(text)?;
    extensions
        .iter()
        .any(|e| e.eq_ignore_ascii_case(&extension))
        .then(|| text.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_single_http_addresses_count() {
        assert!(is_http_url("https://a.test/x.zip"));
        assert!(is_http_url("HTTP://a.test"));
        assert!(!is_http_url("https://"));
        assert!(!is_http_url("ftp://a.test/x"));
        assert!(!is_http_url("https://a.test/x y"));
        assert!(!is_http_url("see https://a.test/x"));
    }

    #[test]
    fn lists_round_trip_and_skip_comments_blanks_and_repeats() {
        let text = "# exported by Velo\n\nhttps://a.test/1.zip\r\n  http://b.test/2.iso  \nnot a url\nhttps://a.test/1.zip\n";
        let urls = read_list(text);
        assert_eq!(urls, ["https://a.test/1.zip", "http://b.test/2.iso"]);
        assert_eq!(
            read_list(&write_list(urls.iter().map(String::as_str))),
            urls
        );
        assert_eq!(write_list([]), "");
    }

    #[test]
    fn extensions_come_from_the_last_path_segment() {
        assert_eq!(
            extension("https://a.test/x.ZIP?v=2#top").as_deref(),
            Some("zip")
        );
        assert_eq!(
            extension("https://a.test/dir.d/file.tar.gz").as_deref(),
            Some("gz")
        );
        assert_eq!(extension("https://a.test/dir.d/file"), None);
        assert_eq!(extension("https://a.test/.hidden"), None);
        assert_eq!(extension("https://a.test"), None);
        assert_eq!(extension("https://a.test/?f=x.zip"), None);
    }

    #[test]
    fn copied_addresses_are_captured_by_extension() {
        let extensions = vec!["zip".to_string(), "mp4".to_string()];
        assert_eq!(
            capture("  https://a.test/movie.MP4\n", &extensions).as_deref(),
            Some("https://a.test/movie.MP4")
        );
        assert_eq!(capture("https://a.test/page.html", &extensions), None);
        assert_eq!(
            capture("https://a.test/a.zip https://a.test/b.zip", &extensions),
            None
        );
        assert_eq!(capture("some text", &extensions), None);
    }
}
