//! Python `str` semantics that the docs-check port depends on.
//!
//! `scripts/docs/check.py` runs on CPython, so the port must strip, split and
//! slice text exactly as Python does: `str.isspace` (check.py's strips and
//! `split()` calls), `str.splitlines()` (L257, L328, L457, L474, L512, L824,
//! L865, L902, L939, L946, L1178, L1285, L1343), text-mode reading (`text=True`
//! at L397, and `open(..., errors="replace")` with `read()` at L405-406),
//! `str.expandtabs(4)` (L275), `str.isdigit()` (L854, L963, L1000), `int()` /
//! `str(int())` and `urllib.parse.unquote` (L571, L617, L620, L661). The regex
//! classes `\s` and `\d` are `crate::pyre`'s; `is_space` and `is_decimal_char`
//! are the same two classes for one character.
//!
//! Digits are Python's decimal digits: every Unicode Nd character (`١`, `３`, ...),
//! not only ASCII, as for `\d`, `str.isdecimal()` and `int()`. Accepted divergences
//! (no tracked file reaches them):
//!
//! - check.py tests its table cells with `str.isdigit()`, which is also true for
//!   the 128 characters of Numeric_Type=Digit that are not decimal (superscripts
//!   such as `²`, circled digits such as `①`); `int()` rejects those with
//!   `ValueError`. The ports test `is_decimal` instead, because neither Rust std nor
//!   the `regex` crate exposes Numeric_Type and matching it would need a hand-kept
//!   Unicode table. The call sites list what check.py does with such a cell (exit 2,
//!   or counting a §8 row) where aios skips it.
//! - `parse_uint` treats a number that does not fit `u64` as no match, and
//!   CPython 3.11+'s `int()` raises past 4300 digits, so at every site ported
//!   through these helpers, check.py exits 2 where aios parses, saturates or
//!   treats the value as no match.

use std::sync::LazyLock;

use regex::Regex;

/// `str.isspace()` for one character, the characters of Python's `\s`
/// (`crate::pyre::SPACE`): Unicode White_Space plus U+001C..U+001F, which Python
/// counts as whitespace and `char::is_whitespace` does not.
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

/// Python's `\d` for one character (`str.isdecimal()`): Unicode Nd. It asks the
/// `regex` crate's own `\d` table, so it agrees with `crate::pyre` patterns by
/// construction.
pub fn is_decimal_char(c: char) -> bool {
    static DECIMAL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A\d\z").expect("valid regex"));
    c.is_ascii_digit() || (!c.is_ascii() && DECIMAL.is_match(c.encode_utf8(&mut [0; 4])))
}

/// `unicodedata.decimal(c)`, or `None` when `c` is not a decimal digit. Unicode
/// encodes every decimal digit set as ten consecutive code points from 0 to 9 (a
/// stability policy), so a maximal run of Nd code points is whole sets end to end
/// (the mathematical digits U+1D7CE..U+1D7FF are five), and a digit's value is its
/// offset from the start of its run, modulo 10.
pub fn decimal_value(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return c.to_digit(10);
    }
    if !is_decimal_char(c) {
        return None;
    }
    let mut start = u32::from(c);
    while let Some(prev) = start.checked_sub(1).and_then(char::from_u32) {
        if !is_decimal_char(prev) {
            break;
        }
        start -= 1;
    }
    Some((u32::from(c) - start) % 10)
}

/// `str.isdecimal()`: non-empty and every character a decimal digit. check.py's
/// `str.isdigit()` calls are ported with this (see the module doc for `²`).
pub fn is_decimal(s: &str) -> bool {
    !s.is_empty() && s.chars().all(is_decimal_char)
}

/// `int(s)` for a run of decimal digits (what `\d+` matches and `is_decimal`
/// accepts); `None` when `s` is not one, or overflows `u64`.
pub fn parse_uint(s: &str) -> Option<u64> {
    if s.is_empty() {
        return None;
    }
    s.chars().try_fold(0u64, |value, c| {
        value
            .checked_mul(10)?
            .checked_add(u64::from(decimal_value(c)?))
    })
}

/// `str(int(digits))` for a run of decimal digits of any length: ASCII digits,
/// leading zeros dropped. CPython 3.11+'s `int()` raises past 4300 digits instead
/// (check.py exits 2); see the module doc.
pub fn int_str(digits: &str) -> String {
    debug_assert!(
        is_decimal(digits),
        "int_str needs decimal digits: {digits:?}"
    );
    let ascii: String = digits
        .chars()
        .map(|c| {
            decimal_value(c)
                .and_then(|d| char::from_digit(d, 10))
                .unwrap_or(c)
        })
        .collect();
    let trimmed = ascii.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `s` without the whitespace `int(s)` ignores around a number: Unicode
/// White_Space (`char::is_whitespace`), which is `str.isspace()` without
/// U+001C..U+001F. CPython turns non-ASCII whitespace into a space and then skips
/// only C whitespace (space and `\t\n\x0b\x0c\r`), and U+001C..U+001F are ASCII,
/// so `int("\x1c3")` raises.
pub fn int_strip(s: &str) -> &str {
    s.trim()
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
    fn character_classes_agree_with_the_pattern_classes() {
        // `is_space` and `is_decimal_char` are `\s` and `\d` for one character; the
        // pattern classes are pinned to CPython in `crate::pyre`'s tests.
        let space = crate::pyre::compile(r"\A\s\z").expect("valid pattern");
        let digit = crate::pyre::compile(r"\A\d\z").expect("valid pattern");
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let text = c.encode_utf8(&mut [0; 4]).to_string();
            assert_eq!(is_space(c), space.is_match(&text), "is_space({c:?})");
            assert_eq!(
                is_decimal_char(c),
                digit.is_match(&text),
                "is_decimal_char({c:?})"
            );
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
        assert!(is_decimal("0123"));
        assert!(!is_decimal(""));
        assert!(!is_decimal("12a"));
        // Arabic-Indic, fullwidth, Devanagari and mathematical digits are decimal.
        assert!(is_decimal("١٢"));
        assert!(is_decimal("３２"));
        assert!(is_decimal("\u{966}\u{1D7D8}"));
        // Accepted divergence (module doc): Python's str.isdigit() is true for `²` and
        // `①`, but they are not decimal, and int() rejects them.
        assert!(!is_decimal("²"));
        assert!(!is_decimal("\u{2460}"));

        assert_eq!(parse_uint("007"), Some(7));
        assert_eq!(parse_uint("18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_uint("18446744073709551616"), None);
        assert_eq!(parse_uint(""), None);
        assert_eq!(parse_uint("1x"), None);
        assert_eq!(parse_uint("+5"), None);
        assert_eq!(parse_uint("١٢"), Some(12));
        assert_eq!(parse_uint("３２"), Some(32));
        assert_eq!(parse_uint("\u{966}\u{967}"), Some(1));
        assert_eq!(parse_uint("²"), None);

        assert_eq!(int_str("007"), "7");
        assert_eq!(int_str("0000"), "0");
        assert_eq!(int_str("42"), "42");
        assert_eq!(
            int_str("0012300000000000000000000000045"),
            "12300000000000000000000000045"
        );
        assert_eq!(int_str("٠٠٧"), "7");
        assert_eq!(int_str("\u{1D7D8}\u{1D7D9}"), "1");
    }

    #[test]
    fn decimal_values_match_unicodedata() {
        // unicodedata.decimal() on CPython 3.14, including digit sets that abut
        // (U+116D0..U+116E3 is two, U+1D7CE..U+1D7FF five) and one that does not
        // start at a multiple of ten (Ol Onal, U+1E5F1..U+1E5FA).
        for (c, want) in [
            ('0', Some(0)),
            ('9', Some(9)),
            ('\u{663}', Some(3)),
            ('\u{FF13}', Some(3)),
            ('\u{966}', Some(0)),
            ('\u{1D7CE}', Some(0)),
            ('\u{1D7D8}', Some(0)),
            ('\u{1D7FF}', Some(9)),
            ('\u{116DA}', Some(0)),
            ('\u{116E3}', Some(9)),
            ('\u{1E5F1}', Some(0)),
            ('\u{1E5FA}', Some(9)),
            ('²', None),
            ('\u{2460}', None),
            ('a', None),
        ] {
            assert_eq!(decimal_value(c), want, "decimal({c:?})");
        }
        // Over every code point: 760 decimal digits whose values sum to 76 * 45,
        // as `sum(unicodedata.decimal(chr(c)) for c in ... if chr(c).isdecimal())`.
        let values: Vec<u32> = (0..=0x10_FFFF)
            .filter_map(char::from_u32)
            .filter_map(decimal_value)
            .collect();
        assert_eq!(values.len(), 760);
        assert_eq!(values.iter().sum::<u32>(), 3420);
    }

    #[test]
    fn int_strip_matches_python_int() {
        // int() on CPython 3.14: `int(" 3 ")`, `int("\u2003" "3" "\u3000")` and
        // `int("\x85 7")` parse; `int("\x1c3")` and `int("3\x1f")` raise.
        assert_eq!(int_strip(" 3 "), "3");
        assert_eq!(int_strip("\u{2003}3\u{3000}"), "3");
        assert_eq!(int_strip("\u{85} 7"), "7");
        assert_eq!(int_strip("\u{1c}3"), "\u{1c}3");
        assert_eq!(int_strip("3\u{1f}"), "3\u{1f}");
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
