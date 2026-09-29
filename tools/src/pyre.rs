//! Python `re` semantics for the `str` patterns that the ports compile.
//!
//! The ported scripts' patterns are Python `str` patterns, whose `\d` and `\s` are
//! Unicode classes. [`compile`] takes a pattern written in Python's syntax and makes
//! those two classes match exactly what CPython's `re` matches:
//!
//! - `\d` and `\D` pass through. In the `regex` crate's default Unicode mode `\d` is
//!   `\p{Nd}`, which is Python's `\d` (the `str.isdecimal()` characters). The crate's
//!   table (regex-syntax 0.8.11, Unicode 16.0) and CPython 3.14's (`unicodedata`
//!   16.0) hold the same 760 code points; the `digit_class_is_pythons` test pins them,
//!   so a regex-syntax update to a newer Unicode version fails it rather than drifting
//!   from CPython silently.
//! - `\s` becomes [`SPACE`] and `\S` becomes [`NON_SPACE`], outside and inside a
//!   character class (as a nested class, which the crate supports). The crate's `\s`
//!   is Unicode White_Space; Python's `\s` (the `str.isspace()` characters) also has
//!   U+001C..U+001F.
//!
//! Everything else passes through unchanged. Python syntax the crate lacks
//! (lookaround, backreferences) still fails to compile, so the callers rewrite it as
//! code. These differences compile silently and stay with the caller:
//!
//! - Python's non-MULTILINE `$` also matches just before a final `\n`; the crate's `$`
//!   is `\z` (write `\n?$` where the subject can end in `\n`, such as a path);
//! - `\w` and `\b` use different word classes (the `docs_check::markdown` module doc
//!   lists how they differ);
//! - `(?i)` does not fold `ı` (U+0131) or `İ` (U+0130) to `i` as `re.IGNORECASE` does;
//! - inside a class, `[` opens a nested class and `&&`, `--` and `~~` are set
//!   operations in the crate, where Python reads them as literals (escape them).

use regex::Regex;

/// Python's `\s` in a `str` pattern (`str.isspace()`): Unicode White_Space plus
/// U+001C..U+001F.
pub const SPACE: &str = r"[\s\x1c-\x1f]";

/// Python's `\S` in a `str` pattern: everything [`SPACE`] does not match.
pub const NON_SPACE: &str = r"[^\s\x1c-\x1f]";

/// `pattern` with every `\s` replaced by [`SPACE`] and every `\S` by [`NON_SPACE`].
/// An escaped backslash (`\\`) is consumed as a pair, so `\\s` stays a backslash
/// followed by `s`; `regex::escape` output is safe to include, because it never puts a
/// backslash before a letter.
pub fn translate(pattern: &str) -> String {
    let mut out = String::with_capacity(pattern.len());
    let mut chars = pattern.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push_str(SPACE),
            Some('S') => out.push_str(NON_SPACE),
            Some(next) => {
                out.push('\\');
                out.push(next);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// `re.compile(pattern)` for a Python `str` pattern: [`translate`], then
/// `Regex::new`.
pub fn compile(pattern: &str) -> Result<Regex, regex::Error> {
    Regex::new(&translate(pattern))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code point Python's `\s` matches: `[hex(c) for c in range(0x110000) if
    /// re.fullmatch(r"\s", chr(c))]` on CPython 3.14.
    const PYTHON_SPACE: [u32; 29] = [
        0x9, 0xA, 0xB, 0xC, 0xD, 0x1C, 0x1D, 0x1E, 0x1F, 0x20, 0x85, 0xA0, 0x1680, 0x2000, 0x2001,
        0x2002, 0x2003, 0x2004, 0x2005, 0x2006, 0x2007, 0x2008, 0x2009, 0x200A, 0x2028, 0x2029,
        0x202F, 0x205F, 0x3000,
    ];

    /// The ranges of code points Python's `\d` matches (CPython 3.14, Unicode 16.0):
    /// the maximal runs of `re.fullmatch(r"\d", chr(c))` over every code point.
    const PYTHON_DIGIT: [(u32, u32); 71] = [
        (0x30, 0x39),
        (0x660, 0x669),
        (0x6F0, 0x6F9),
        (0x7C0, 0x7C9),
        (0x966, 0x96F),
        (0x9E6, 0x9EF),
        (0xA66, 0xA6F),
        (0xAE6, 0xAEF),
        (0xB66, 0xB6F),
        (0xBE6, 0xBEF),
        (0xC66, 0xC6F),
        (0xCE6, 0xCEF),
        (0xD66, 0xD6F),
        (0xDE6, 0xDEF),
        (0xE50, 0xE59),
        (0xED0, 0xED9),
        (0xF20, 0xF29),
        (0x1040, 0x1049),
        (0x1090, 0x1099),
        (0x17E0, 0x17E9),
        (0x1810, 0x1819),
        (0x1946, 0x194F),
        (0x19D0, 0x19D9),
        (0x1A80, 0x1A89),
        (0x1A90, 0x1A99),
        (0x1B50, 0x1B59),
        (0x1BB0, 0x1BB9),
        (0x1C40, 0x1C49),
        (0x1C50, 0x1C59),
        (0xA620, 0xA629),
        (0xA8D0, 0xA8D9),
        (0xA900, 0xA909),
        (0xA9D0, 0xA9D9),
        (0xA9F0, 0xA9F9),
        (0xAA50, 0xAA59),
        (0xABF0, 0xABF9),
        (0xFF10, 0xFF19),
        (0x104A0, 0x104A9),
        (0x10D30, 0x10D39),
        (0x10D40, 0x10D49),
        (0x11066, 0x1106F),
        (0x110F0, 0x110F9),
        (0x11136, 0x1113F),
        (0x111D0, 0x111D9),
        (0x112F0, 0x112F9),
        (0x11450, 0x11459),
        (0x114D0, 0x114D9),
        (0x11650, 0x11659),
        (0x116C0, 0x116C9),
        (0x116D0, 0x116E3),
        (0x11730, 0x11739),
        (0x118E0, 0x118E9),
        (0x11950, 0x11959),
        (0x11BF0, 0x11BF9),
        (0x11C50, 0x11C59),
        (0x11D50, 0x11D59),
        (0x11DA0, 0x11DA9),
        (0x11F50, 0x11F59),
        (0x16130, 0x16139),
        (0x16A60, 0x16A69),
        (0x16AC0, 0x16AC9),
        (0x16B50, 0x16B59),
        (0x16D70, 0x16D79),
        (0x1CCF0, 0x1CCF9),
        (0x1D7CE, 0x1D7FF),
        (0x1E140, 0x1E149),
        (0x1E2F0, 0x1E2F9),
        (0x1E4F0, 0x1E4F9),
        (0x1E5F1, 0x1E5FA),
        (0x1E950, 0x1E959),
        (0x1FBF0, 0x1FBF9),
    ];

    fn matches(rx: &Regex, c: char) -> bool {
        rx.is_match(c.encode_utf8(&mut [0; 4]))
    }

    #[test]
    fn translate_rewrites_only_the_space_classes() {
        let cases = [
            (r"^\s*(\S+)", r"^[\s\x1c-\x1f]*([^\s\x1c-\x1f]+)"),
            (r"\(\s*([^)\s]+)", r"\([\s\x1c-\x1f]*([^)[\s\x1c-\x1f]]+)"),
            (r"^[│\s]+", r"^[│[\s\x1c-\x1f]]+"),
            (r"\\s\\S", r"\\s\\S"),
            (r"\d+\D\w\b\.", r"\d+\D\w\b\."),
            ("tail\\", "tail\\"),
            ("", ""),
        ];
        for (python, want) in cases {
            assert_eq!(translate(python), want, "{python:?}");
        }
    }

    #[test]
    fn space_class_is_pythons() {
        let bare = compile(r"\A\s\z").expect("valid pattern");
        let negated = compile(r"\A\S\z").expect("valid pattern");
        let in_class = compile(r"\A[x\s]\z").expect("valid pattern");
        let in_negated_class = compile(r"\A[^x\s]\z").expect("valid pattern");
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let python = PYTHON_SPACE.contains(&u32::from(c));
            assert_eq!(matches(&bare, c), python, "\\s on {c:?}");
            assert_eq!(matches(&negated, c), !python, "\\S on {c:?}");
            assert_eq!(matches(&in_class, c), python || c == 'x', "[x\\s] on {c:?}");
            assert_eq!(
                matches(&in_negated_class, c),
                !(python || c == 'x'),
                "[^x\\s] on {c:?}"
            );
        }
    }

    #[test]
    fn digit_class_is_pythons() {
        let digit = compile(r"\A\d\z").expect("valid pattern");
        let non_digit = compile(r"\A\D\z").expect("valid pattern");
        for c in (0..=0x10_FFFF).filter_map(char::from_u32) {
            let code = u32::from(c);
            let python = PYTHON_DIGIT
                .iter()
                .any(|&(lo, hi)| (lo..=hi).contains(&code));
            assert_eq!(matches(&digit, c), python, "\\d on {c:?}");
            assert_eq!(matches(&non_digit, c), !python, "\\D on {c:?}");
        }
    }

    #[test]
    fn compiled_patterns_match_like_python() {
        // Expected values from CPython 3.14's `re` on the same pattern and subject.
        let fence = compile(r"^\s*(`{3,}|~{3,})").expect("valid pattern");
        assert!(fence.is_match("\u{1f}```"));
        let heading = compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$").expect("valid pattern");
        let caps = heading.captures("##\u{1f}Title\u{1c}").expect("a heading");
        assert_eq!(&caps[2], "Title");
        let target = compile(r"\]\(\s*([^)\s]+)").expect("valid pattern");
        let caps = target.captures("[a](\u{1f}x.md\u{1f}y)").expect("a target");
        assert_eq!(&caps[1], "x.md");
        let number = compile(r"§\s*(\d+(?:\.\d+)*)").expect("valid pattern");
        let caps = number.captures("§\u{1f}٣.١٢").expect("a section number");
        assert_eq!(&caps[1], "٣.١٢");
        let fullwidth = compile(r"^M(\d+)$").expect("valid pattern");
        assert_eq!(
            &fullwidth.captures("M１２").expect("a milestone")[1],
            "１２"
        );
    }
}
