//! Python `str` semantics that the docs-check port depends on.
//!
//! `scripts/docs/check.py` runs on CPython, so the port must strip, split and
//! slice text exactly as Python does: `str.isspace` (check.py's strips and
//! `split()` calls), `str.splitlines()` (L257, L328, L457, L474, L512, L824,
//! L865, L902, L939, L946, L1178, L1285, L1343), text-mode reading (L399-401),
//! `str.expandtabs(4)` (L275), `str.isdigit()` (L854, L963, L1000), `int()` /
//! `str(int())` and `urllib.parse.unquote` (L571, L617, L620, L661).
//!
//! Accepted divergences (contract §1.9; no tracked file reaches them):
//! `is_ascii_digits` and `parse_uint` accept ASCII digits only, where Python's
//! `str.isdigit()` and `int()` also accept other Unicode decimal digits, and
//! numbers that do not fit `u64` are treated as no match. The `regex` crate's
//! `\s` lacks U+001C..U+001F, so ported code uses these helpers instead of a
//! pattern wherever Python whitespace matters.

/// `str.isspace()` for one character: Unicode whitespace plus U+001C..U+001F,
/// which Python counts as whitespace and `char::is_whitespace` does not.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    lstrip(rstrip(s))
}

/// `str.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `str.split()` with no argument: split on runs of whitespace, dropping the
/// empty parts at both ends.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|part| !part.is_empty()).collect()
}

/// `str.splitlines()`: breaks at `\n`, `\r`, `\r\n`, `\x0b`, `\x0c`, `\x1c`,
/// `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029, with no trailing empty element.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let is_break = if c == '\r' {
            if chars.peek().map(|&(_, next)| next) == Some('\n') {
                chars.next();
            }
            true
        } else {
            matches!(
                c,
                '\n' | '\u{b}'
                    | '\u{c}'
                    | '\u{1c}'
                    | '\u{1d}'
                    | '\u{1e}'
                    | '\u{85}'
                    | '\u{2028}'
                    | '\u{2029}'
            )
        };
        if is_break {
            lines.push(&s[start..i]);
            start = chars.peek().map_or(s.len(), |&(j, _)| j);
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Python text mode: decode UTF-8 with replacement, then universal newlines
/// (`"\r\n"` then `"\r"` become `"\n"`).
pub fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.into_owned()
    }
}

/// `s[:n]`, counting code points as Python does.
pub fn char_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The number of leading spaces of `line.expandtabs(4)`.
pub fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width = width / 4 * 4 + 4,
            _ => break,
        }
    }
    width
}

/// `str.isdigit()` restricted to ASCII digits (contract §1.9).
pub fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `int(s)` for a run of ASCII digits; `None` when `s` is not one, or overflows.
pub fn parse_uint(s: &str) -> Option<u64> {
    if !is_ascii_digits(s) {
        return None;
    }
    s.parse::<u64>().ok()
}

/// `str(int(digits))` for a run of ASCII digits of any length: leading zeros go.
pub fn int_str(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `urllib.parse.unquote(s)`: only ASCII runs are unquoted, each run's bytes
/// are decoded as UTF-8 with replacement, and non-ASCII runs pass through.
pub fn url_unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let ascii = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        if ascii > 0 {
            out.push_str(&unquote_ascii(&rest[..ascii]));
            rest = &rest[ascii..];
        }
        let other = rest.find(|c: char| c.is_ascii()).unwrap_or(rest.len());
        out.push_str(&rest[..other]);
        rest = &rest[other..];
    }
    out
}

/// `unquote_to_bytes` on one ASCII run, then a lossy UTF-8 decode.
fn unquote_ascii(run: &str) -> String {
    let bytes = run.as_bytes();
    let mut raw: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = hex_byte(bytes.get(i + 1), bytes.get(i + 2)) {
                raw.push(byte);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

fn hex_byte(hi: Option<&u8>, lo: Option<&u8>) -> Option<u8> {
    let hi = char::from(*hi?).to_digit(16)?;
    let lo = char::from(*lo?).to_digit(16)?;
    Some((hi * 16 + lo) as u8)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every expected value below was recorded from CPython 3.14 by calling the
    // `str` method (or `urllib.parse.unquote`) that the function ports.

    #[test]
    fn is_space_matches_python() {
        for c in [
            ' ', '\t', '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}',
            '\u{85}', '\u{a0}', '\u{2028}', '\u{2029}', '\u{3000}',
        ] {
            assert!(is_space(c), "Python says {c:?} is whitespace");
        }
        for c in ['a', '0', '\0', '\u{200b}', '\u{180e}'] {
            assert!(!is_space(c), "Python says {c:?} is not whitespace");
        }
    }

    #[test]
    fn strip_family_matches_python() {
        assert_eq!(strip(" \u{1c} a b \t\n"), "a b");
        assert_eq!(lstrip("\u{a0}x "), "x ");
        assert_eq!(rstrip(" x\u{2028}"), " x");
        assert_eq!(strip("   "), "");
        assert_eq!(strip(""), "");
        assert_eq!(strip("no-whitespace"), "no-whitespace");
    }

    #[test]
    fn split_ws_matches_python() {
        assert_eq!(split_ws(" a\u{1c}b  c "), vec!["a", "b", "c"]);
        assert!(split_ws("").is_empty());
        assert!(split_ws("   ").is_empty());
        assert_eq!(split_ws("one"), vec!["one"]);
    }

    #[test]
    fn splitlines_matches_python() {
        let text = "a\nb\r\nc\rd\u{b}e\u{c}f\u{1c}g\u{1d}h\u{1e}i\u{85}j\u{2028}k\u{2029}l";
        assert_eq!(
            splitlines(text),
            vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"]
        );
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("a\n"), vec!["a"]);
        assert_eq!(splitlines("\n"), vec![""]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(splitlines("trailing"), vec!["trailing"]);
    }

    #[test]
    fn decode_text_is_lossy_with_universal_newlines() {
        assert_eq!(decode_text(b"a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(decode_text("café".as_bytes()), "café");
        assert_eq!(decode_text(b"x\xe2\x82y"), "x\u{fffd}y");
        assert_eq!(decode_text(b""), "");
    }

    #[test]
    fn char_prefix_counts_code_points() {
        assert_eq!(char_prefix("héllo", 3), "hél");
        assert_eq!(char_prefix("ab", 5), "ab");
        assert_eq!(char_prefix("ab", 0), "");
        assert_eq!(char_prefix("", 3), "");
    }

    #[test]
    fn indent_width_expands_tabs_to_four() {
        for (line, width) in [
            ("\tx", 4),
            ("  \tx", 4),
            ("    x", 4),
            ("\t\tx", 8),
            (" x", 1),
            ("a\tb", 0),
            ("   ", 3),
            ("", 0),
        ] {
            assert_eq!(indent_width(line), width, "indent of {line:?}");
        }
    }

    #[test]
    fn digits_and_ints_match_python() {
        assert!(is_ascii_digits("0123"));
        assert!(!is_ascii_digits(""));
        assert!(!is_ascii_digits("12a"));
        // Accepted divergence: Python's str.isdigit() is true for these.
        assert!(!is_ascii_digits("١٢"));

        assert_eq!(parse_uint("007"), Some(7));
        assert_eq!(parse_uint("18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_uint("18446744073709551616"), None);
        assert_eq!(parse_uint(""), None);
        assert_eq!(parse_uint("1x"), None);

        assert_eq!(int_str("007"), "7");
        assert_eq!(int_str("0000"), "0");
        assert_eq!(int_str("42"), "42");
        assert_eq!(
            int_str("0012300000000000000000000000045"),
            "12300000000000000000000000045"
        );
    }

    #[test]
    fn url_unquote_matches_python() {
        for (raw, want) in [
            ("a%2Fb", "a/b"),
            ("%2", "%2"),
            ("%zz", "%zz"),
            ("100%", "100%"),
            ("caf%C3%A9", "café"),
            ("a%C3%A9b", "aéb"),
            ("%E2%82", "\u{fffd}"),
            ("x y", "x y"),
            ("a%20b%", "a b%"),
            ("%41%42", "AB"),
            ("é%41", "éA"),
            ("", ""),
        ] {
            assert_eq!(url_unquote(raw), want, "unquote({raw:?})");
        }
    }
}
