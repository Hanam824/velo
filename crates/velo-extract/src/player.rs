//! YouTube's web player script: finding the functions in it that unscramble stream
//! addresses, and running them.
//!
//! Some clients get stream addresses with a scrambled signature (`signatureCipher`) and an
//! `n` parameter that YouTube throttles until it is transformed. The functions that undo both
//! live in the player script (`base.js`). This module cuts them out with searches that know
//! the shapes YouTube has used, together with what they depend on, and runs them in
//! [`boa_engine`]. When YouTube changes the shape, the searches fail with
//! [`Error::PlayerChanged`](crate::Error::PlayerChanged) and need updating.

use std::sync::LazyLock;

use boa_engine::{Context, Source};
use regex::Regex;

use crate::{Error, Result};

/// The most loop iterations the player's functions may run, so a broken script can't hang.
const LOOP_LIMIT: u64 = 1_000_000;

/// The pieces of one player version that Velo needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Player {
    /// The player version, such as `a1b2c3d4`.
    pub id: String,
    /// The number the player sends as `signatureTimestamp`, if found.
    pub signature_timestamp: Option<u64>,
    /// A script that defines [`SIGNATURE`] and [`N_FUNCTION`], cut from the player.
    script: String,
}

/// The name the cut-out signature function gets.
const SIGNATURE: &str = "veloSignature";

/// The name the cut-out n function gets.
const N_FUNCTION: &str = "veloN";

/// A name in JavaScript.
const NAME: &str = r"[a-zA-Z0-9_$]+";

impl Player {
    /// Finds the signature and n functions in player script `js` of version `id`.
    pub fn from_js(id: &str, js: &str) -> Result<Self> {
        let signature = signature_function(js)?;
        let n = n_function(js)?;
        let mut script = String::new();
        for dependency in dependencies(js, &[&signature, &n]) {
            script.push_str(&dependency);
            script.push('\n');
        }
        script.push_str(&format!(
            "var {SIGNATURE}={signature};\nvar {N_FUNCTION}={n};\n"
        ));
        static TIMESTAMP: LazyLock<Regex> =
            LazyLock::new(|| Regex::new(r"(?:signatureTimestamp|sts)\s*:\s*(\d{5})").unwrap());
        let signature_timestamp = TIMESTAMP.captures(js).and_then(|c| c[1].parse().ok());
        Ok(Self {
            id: id.to_string(),
            signature_timestamp,
            script,
        })
    }

    /// Unscrambles each of `signatures` and transforms each of `ns`, in one run of the
    /// player's functions; returns them in the same order.
    ///
    /// Takes a few milliseconds of CPU; call it off the async runtime's worker threads.
    pub fn solve(
        &self,
        signatures: &[String],
        ns: &[String],
    ) -> Result<(Vec<String>, Vec<String>)> {
        let mut context = Context::default();
        context
            .runtime_limits_mut()
            .set_loop_iteration_limit(LOOP_LIMIT);
        context
            .eval(Source::from_bytes(&self.script))
            .map_err(|e| Error::PlayerFailed(e.to_string()))?;
        let mut call = |function: &str, input: &String| -> Result<String> {
            let code = format!(
                "{function}({})",
                serde_json::to_string(input).expect("a string serializes")
            );
            let value = context
                .eval(Source::from_bytes(&code))
                .map_err(|e| Error::PlayerFailed(e.to_string()))?;
            value
                .as_string()
                .map(|s| s.to_std_string_escaped())
                .ok_or_else(|| Error::PlayerFailed(format!("{function} didn't return a string")))
        };
        let signatures = signatures
            .iter()
            .map(|s| call(SIGNATURE, s))
            .collect::<Result<_>>()?;
        let ns = ns
            .iter()
            .map(|n| call(N_FUNCTION, n))
            .collect::<Result<_>>()?;
        Ok((signatures, ns))
    }
}

/// The player version named in the `iframe_api` script, such as `a1b2c3d4`.
pub fn player_id(iframe_api: &str) -> Option<String> {
    static ID: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"player\\?/([0-9a-zA-Z_-]{8,})\\?/").unwrap());
    ID.captures(iframe_api).map(|c| c[1].to_string())
}

/// The path of player version `id`'s script on YouTube.
pub fn player_path(id: &str) -> String {
    format!("s/player/{id}/player_ias.vflset/en_US/base.js")
}

/// The signature function, as `function(a){...}`.
fn signature_function(js: &str) -> Result<String> {
    // `NAME=function(a){a=a.split("")` or `function NAME(a){a=a.split("")`.
    static SHAPES: LazyLock<[Regex; 2]> = LazyLock::new(|| {
        [
            Regex::new(&format!(
                r#"(?:^|[^a-zA-Z0-9_$.])({NAME})\s*=\s*function\(\s*({NAME})\s*\)\s*\{{\s*({NAME})\s*=\s*({NAME})\.split\(\s*""\s*\)"#
            ))
            .unwrap(),
            Regex::new(&format!(
                r#"function\s+({NAME})\(\s*({NAME})\s*\)\s*\{{\s*({NAME})\s*=\s*({NAME})\.split\(\s*""\s*\)"#
            ))
            .unwrap(),
        ]
    });
    for shape in SHAPES.iter() {
        for captures in shape.captures_iter(js) {
            // The same argument splits into itself: `a=a.split("")`.
            if captures[2] == captures[3] && captures[3] == captures[4] {
                let start = captures.get(0).unwrap().start();
                let function = js[start..]
                    .find("function")
                    .map(|i| start + i)
                    .ok_or_else(|| changed("the signature function"))?;
                let text = function_text(js, function)
                    .ok_or_else(|| changed("the end of the signature function"))?;
                // `function NAME(a)` becomes `function(a)`, to assign it a name of our own.
                return Ok(format!("function{}", &text[text.find('(').unwrap()..]));
            }
        }
    }
    Err(changed("the signature function"))
}

/// The n function, as `function(a){...}` without the guard that makes it a no-op outside
/// the player.
fn n_function(js: &str) -> Result<String> {
    // `.get("n"))&&(b=NAME[0](b)` or `.get("n"))&&(b=NAME(b)`.
    static CALL: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r#"\.get\(\s*"n"\s*\)\s*\)\s*&&\s*\(\s*{NAME}\s*=\s*({NAME})(?:\[(\d+)\])?\(\s*{NAME}\s*\)"#
        ))
        .unwrap()
    });
    let captures = CALL
        .captures(js)
        .ok_or_else(|| changed("the n function call"))?;
    let mut name = captures[1].to_string();
    if let Some(index) = captures.get(2) {
        // `var NAME=[a,b,c]`: the function is the element at the index.
        let array = Regex::new(&format!(
            r"var\s+{}\s*=\s*\[([^\]]*)\]",
            regex::escape(&name)
        ))
        .unwrap();
        let elements = array
            .captures(js)
            .ok_or_else(|| changed("the n function array"))?[1]
            .to_string();
        let index: usize = index.as_str().parse().unwrap_or(usize::MAX);
        name = elements
            .split(',')
            .nth(index)
            .map(|e| e.trim().to_string())
            .ok_or_else(|| changed("the n function in its array"))?;
    }
    let definition = Regex::new(&format!(
        r"(?:^|[^a-zA-Z0-9_$.]){}\s*=\s*function\(",
        regex::escape(&name)
    ))
    .unwrap();
    let start = definition
        .find(js)
        .ok_or_else(|| changed("the n function"))?
        .start();
    let function = start + js[start..].find("function").unwrap();
    let text = function_text(js, function).ok_or_else(|| changed("the end of the n function"))?;
    // `if(typeof X==="undefined")return a;` returns the input unchanged unless a global of
    // the whole player exists.
    static GUARD: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r#";\s*if\s*\(\s*typeof\s+{NAME}\s*===?\s*(?:"undefined"|'undefined'|{NAME}\[\d+\])\s*\)\s*return\s+{NAME};"#
        ))
        .unwrap()
    });
    Ok(GUARD.replace_all(text, ";").into_owned())
}

/// The top-level declarations the functions use: helper objects called as `NAME.method(`
/// and tables read as `NAME[`.
fn dependencies(js: &str, functions: &[&str]) -> Vec<String> {
    static USES: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(&format!(
            r"(?:^|[^a-zA-Z0-9_$.])({NAME})\s*(?:\.{NAME}\s*\(|\[)"
        ))
        .unwrap()
    });
    let mut names: Vec<String> = Vec::new();
    for function in functions {
        for captures in USES.captures_iter(function) {
            let name = captures[1].to_string();
            if !names.contains(&name) {
                names.push(name);
            }
        }
    }
    names
        .iter()
        // One-letter names are the functions' own arguments and locals; the rest of these
        // are built in.
        .filter(|name| name.len() > 1 && !BUILT_IN.contains(&name.as_str()))
        .filter_map(|name| declaration(js, name))
        .collect()
}

/// Names the functions may use that the player doesn't declare.
const BUILT_IN: [&str; 10] = [
    "Array", "Date", "JSON", "Math", "Number", "Object", "RegExp", "String", "parseInt", "window",
];

/// `var NAME=...;` for a top-level `var` declaration of `name`, if there is one.
fn declaration(js: &str, name: &str) -> Option<String> {
    let pattern = Regex::new(&format!(r"var\s+{}\s*=", regex::escape(name))).unwrap();
    let start = pattern.find(js)?.start();
    let end = statement_end(js, start)?;
    Some(js[start..end].to_string())
}

/// The text of the function whose `function` keyword starts at `start`, through its closing
/// brace.
fn function_text(js: &str, start: usize) -> Option<&str> {
    let open = start + js[start..].find('{')?;
    let close = matching_brace(js, open)?;
    Some(&js[start..=close])
}

/// The index of the `}` that closes the `{` at `open`, skipping strings, regular
/// expressions and comments.
fn matching_brace(js: &str, open: usize) -> Option<usize> {
    let mut depth = 0usize;
    let mut scanner = Scanner::new(js, open);
    while let Some((index, byte)) = scanner.next_code_byte() {
        match byte {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
            _ => {}
        }
    }
    None
}

/// The index just past the `;` that ends the statement starting at `start`, outside any
/// brackets, strings, regular expressions and comments.
fn statement_end(js: &str, start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut scanner = Scanner::new(js, start);
    while let Some((index, byte)) = scanner.next_code_byte() {
        match byte {
            b'{' | b'(' | b'[' => depth += 1,
            b'}' | b')' | b']' => depth -= 1,
            b';' if depth == 0 => return Some(index + 1),
            _ => {}
        }
    }
    None
}

/// Walks JavaScript source byte by byte, stepping over strings, template literals, regular
/// expression literals and comments.
struct Scanner<'a> {
    bytes: &'a [u8],
    index: usize,
    /// The last byte of code seen (not whitespace), which tells a regular expression from a
    /// division.
    previous: u8,
}

impl<'a> Scanner<'a> {
    fn new(js: &'a str, start: usize) -> Self {
        Self {
            bytes: js.as_bytes(),
            index: start,
            previous: b';',
        }
    }

    /// The next byte that is code, with its index.
    fn next_code_byte(&mut self) -> Option<(usize, u8)> {
        loop {
            let index = self.index;
            let byte = *self.bytes.get(index)?;
            self.index += 1;
            match byte {
                b'"' | b'\'' | b'`' => self.skip_quoted(byte)?,
                b'/' if self.bytes.get(index + 1) == Some(&b'/') => {
                    while self.bytes.get(self.index).is_some_and(|&b| b != b'\n') {
                        self.index += 1;
                    }
                }
                b'/' if self.bytes.get(index + 1) == Some(&b'*') => {
                    self.index += 1;
                    while !(self.bytes.get(self.index)? == &b'*'
                        && self.bytes.get(self.index + 1) == Some(&b'/'))
                    {
                        self.index += 1;
                    }
                    self.index += 2;
                }
                b'/' if b"(,=:[!&|?{};+-*%<>~^".contains(&self.previous) => {
                    self.skip_regex()?;
                    self.previous = b'/';
                }
                b if b.is_ascii_whitespace() => {}
                _ => {
                    self.previous = byte;
                    return Some((index, byte));
                }
            }
        }
    }

    /// Steps past a string or template literal opened by `quote`.
    fn skip_quoted(&mut self, quote: u8) -> Option<()> {
        loop {
            let byte = *self.bytes.get(self.index)?;
            self.index += 1;
            match byte {
                b'\\' => self.index += 1,
                b if b == quote => {
                    self.previous = quote;
                    return Some(());
                }
                _ => {}
            }
        }
    }

    /// Steps past a regular expression literal whose opening `/` was just read.
    fn skip_regex(&mut self) -> Option<()> {
        let mut in_class = false;
        loop {
            let byte = *self.bytes.get(self.index)?;
            self.index += 1;
            match byte {
                b'\\' => self.index += 1,
                b'[' => in_class = true,
                b']' => in_class = false,
                b'/' if !in_class => break,
                b'\n' => return None,
                _ => {}
            }
        }
        // Flags such as `g`.
        while self
            .bytes
            .get(self.index)
            .is_some_and(|b| b.is_ascii_alphabetic())
        {
            self.index += 1;
        }
        Some(())
    }
}

/// The error for a player whose `what` can't be found.
fn changed(what: &str) -> Error {
    Error::PlayerChanged(format!("couldn't find {what}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn braces_inside_strings_regexes_and_comments_are_skipped() {
        let js = r#"x=function(a){var s="}",t='{',r=/[}]/g,u=`}`;/* } */ // }
            if(a/2>1){return {b:1}}return s};y=2"#;
        let start = js.find("function").unwrap();
        assert!(function_text(js, start).unwrap().ends_with("return s}"));
        assert_eq!(
            &js[..statement_end(js, 0).unwrap()],
            &js[..js.find(";y").unwrap() + 1][..]
        );
    }

    #[test]
    fn the_player_version_comes_from_the_iframe_api() {
        assert_eq!(
            player_id(r"https:\/\/www.youtube.com\/s\/player\/a1b2c3d4\/www-widgetapi.vflset")
                .as_deref(),
            Some("a1b2c3d4")
        );
        assert_eq!(
            player_id("/s/player/0f1e2d3c/base.js").as_deref(),
            Some("0f1e2d3c")
        );
        assert_eq!(player_id("nothing here"), None);
        assert_eq!(
            player_path("a1b2c3d4"),
            "s/player/a1b2c3d4/player_ias.vflset/en_US/base.js"
        );
    }

    /// The hand-written player in the test fixtures.
    const BASE_JS: &str = include_str!("../tests/fixtures/base.js");

    #[test]
    fn the_functions_are_cut_out_with_what_they_use_and_run() {
        let player = Player::from_js("a1b2c3d4", BASE_JS).unwrap();
        assert_eq!(player.signature_timestamp, Some(20160));
        assert!(player.script.contains("var Xy = {"), "{}", player.script);
        assert!(player.script.contains("var Tz = "), "{}", player.script);
        assert!(
            !player.script.contains("typeof Qwe"),
            "the guard is removed"
        );
        assert!(
            !player.script.contains("var fK"),
            "only what the functions use"
        );

        // Expected values worked out by hand from the fixture's operations: the signature is
        // swapped at 3, reversed, cut by 2 and swapped at 19; n is rotated right by 4, then
        // reversed.
        let (signatures, ns) = player
            .solve(
                &["0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz".into()],
                &["abcdefghij12".into(), "zyxwvutsrqpo".into()],
            )
            .unwrap();
        assert_eq!(
            signatures,
            ["ewvutsrqponmlkjihgfxdcbaZYXWVUTSRQPONMLKJIHGFEDCBA9876540213"]
        );
        assert_eq!(ns, ["hgfedcba21ji", "stuvwxyzopqr"]);
    }

    #[test]
    fn a_function_declared_by_name_is_found_too() {
        let js = r#"function Ab(a){a=a.split("");a.reverse();return a.join("")}
            var Nf=function(a){return a+"!"};x.get("n"))&&(b=Nf(b),c.set("n",b))"#;
        let player = Player::from_js("x", js).unwrap();
        assert_eq!(player.signature_timestamp, None);
        let (signatures, ns) = player.solve(&["abc".into()], &["n".into()]).unwrap();
        assert_eq!((signatures, ns), (vec!["cba".into()], vec!["n!".into()]));
    }

    #[test]
    fn a_runaway_function_is_stopped() {
        let js = r#"Ab=function(a){a=a.split("");for(;;){}return a.join("")};
            var Nf=function(a){return a};x.get("n"))&&(b=Nf(b))"#;
        let player = Player::from_js("x", js).unwrap();
        assert!(matches!(
            player.solve(&["abc".into()], &[]),
            Err(Error::PlayerFailed(_))
        ));
    }

    #[test]
    fn a_script_without_the_functions_is_a_changed_player() {
        assert!(matches!(
            Player::from_js("x", "var a=1;"),
            Err(Error::PlayerChanged(_))
        ));
    }
}
