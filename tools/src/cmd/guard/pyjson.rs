//! `json.loads` as the hook's Python runs it: CPython 3.13's C scanner, as on
//! the 3.13 branch since 3.13.8 (`Modules/_json.c`: `scan_once_unicode`, `scanstring_unicode`,
//! `_parse_object_unicode`, `_parse_array_unicode`, `_match_number_unicode`) and
//! `json/decoder.py`'s `decode`, so an unreadable payload gets Python's message,
//! position included (`<msg>: line L column C (char P)`, P in characters).
//!
//! The values become `serde_json::Value`s. Three things a `Value` cannot hold are
//! mapped so that every check the guard makes on a payload value (`is None`,
//! `isinstance(..., str)` and `dict`, and truthiness) comes out as in Python:
//!
//! - a lone surrogate from a `\uD800`-style escape becomes U+FFFD (a `String`
//!   cannot hold one; the same divergence as `$'\uD800'` in the shell model);
//! - `NaN`, `Infinity`, `-Infinity`, and numbers that overflow to an infinite
//!   float, become `true` (truthy, and not a string, object or null, like
//!   Python's float);
//! - integers outside `i64`/`u64` become the nearest float.

use serde_json::{Map, Number, Value};

/// CPython raises `RecursionError` once the scanner nests deeper than this
/// (`_Py_EnterRecursiveCall`; the hook's frames included) [VERIFIED on 3.13.17
/// on Linux: 9,997 nested arrays or objects decode and 9,998 do not].
pub const MAX_NESTING: usize = 9_997;

/// The deepest nesting at which the C scanner can still raise a
/// `JSONDecodeError`: building the exception calls back into Python, which
/// needs three more levels, so an error raised deeper is a `RecursionError`
/// ("maximum recursion depth exceeded") [VERIFIED on 3.13.17 on Linux, for
/// errors in arrays, objects and strings].
const MAX_RAISE_NESTING: usize = MAX_NESTING - 3;

/// CPython's default `sys.get_int_max_str_digits()`.
const INT_MAX_STR_DIGITS: usize = 4_300;

/// Why `loads` failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadError {
    /// A `ValueError` (`JSONDecodeError` or the integer digit limit): its text.
    Value(String),
    /// The `RecursionError` that escapes the hook: its text.
    Recursion(String),
}

enum ScanError {
    /// `StopIteration(idx)`: no JSON value starts at idx.
    Stop(usize),
    Fail(LoadError),
}

type Scan<T> = Result<(T, usize), ScanError>;

/// The stack the scanner runs on: [`MAX_NESTING`] levels of recursion need far
/// more than a default thread's 2 MiB (or the main thread's 8 MiB). Only the
/// pages used are committed.
const SCAN_STACK_BYTES: usize = 256 << 20;

/// `json.loads(text)` for a `str`. The scanner runs on its own thread with a
/// [`SCAN_STACK_BYTES`] stack, so the nesting limit is Python's and not the
/// caller's stack size; only when no thread can start does it run on the
/// caller's.
pub fn loads(text: &str) -> Result<Value, LoadError> {
    std::thread::scope(|scope| {
        let scanner = std::thread::Builder::new()
            .name("guard-json".to_string())
            .stack_size(SCAN_STACK_BYTES)
            .spawn_scoped(scope, || loads_here(text));
        match scanner {
            Ok(handle) => handle
                .join()
                .unwrap_or_else(|panic| std::panic::resume_unwind(panic)),
            Err(_) => loads_here(text),
        }
    })
}

/// [`loads`] on the calling thread's stack.
fn loads_here(text: &str) -> Result<Value, LoadError> {
    let s: Vec<char> = text.chars().collect();
    if s.first() == Some(&'\u{feff}') {
        return Err(decode_error(
            "Unexpected UTF-8 BOM (decode using utf-8-sig)",
            &s,
            0,
        ));
    }
    let start = skip_ws(&s, 0);
    let (value, end) = match scan_once(&s, start, 0) {
        Ok(found) => found,
        Err(ScanError::Stop(at)) => return Err(decode_error("Expecting value", &s, at)),
        Err(ScanError::Fail(e)) => return Err(e),
    };
    let end = skip_ws(&s, end);
    if end != s.len() {
        return Err(decode_error("Extra data", &s, end));
    }
    Ok(value)
}

/// `JSONDecodeError(msg, doc, pos)` as a `ValueError` text.
fn decode_error(msg: &str, s: &[char], pos: usize) -> LoadError {
    let before = &s[..pos];
    let lineno = before.iter().filter(|&&c| c == '\n').count() + 1;
    let colno = match before.iter().rposition(|&c| c == '\n') {
        Some(nl) => pos - nl,
        None => pos + 1,
    };
    LoadError::Value(format!("{msg}: line {lineno} column {colno} (char {pos})"))
}

/// `raise_errmsg(msg, s, pos)` inside `depth` nested containers.
fn fail<T>(msg: &str, s: &[char], pos: usize, depth: usize) -> Result<T, ScanError> {
    if depth > MAX_RAISE_NESTING {
        return Err(ScanError::Fail(LoadError::Recursion(
            "maximum recursion depth exceeded".to_string(),
        )));
    }
    Err(ScanError::Fail(decode_error(msg, s, pos)))
}

fn is_ws(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\r')
}

fn skip_ws(s: &[char], mut idx: usize) -> usize {
    while idx < s.len() && is_ws(s[idx]) {
        idx += 1;
    }
    idx
}

/// `scan_once_unicode`: one JSON term starting at `idx`.
fn scan_once(s: &[char], idx: usize, depth: usize) -> Scan<Value> {
    let len = s.len();
    if idx >= len {
        return Err(ScanError::Stop(idx));
    }
    let at = |k: usize, word: &str| s[idx..].iter().take(k).copied().eq(word.chars());
    match s[idx] {
        '"' => {
            let (text, end) = scanstring(s, idx + 1, depth)?;
            return Ok((Value::String(text), end));
        }
        '{' | '[' => {
            if depth >= MAX_NESTING {
                let kind = if s[idx] == '{' { "object" } else { "array" };
                return Err(ScanError::Fail(LoadError::Recursion(format!(
                    "maximum recursion depth exceeded while decoding a JSON {kind} from a unicode string"
                ))));
            }
            return if s[idx] == '{' {
                parse_object(s, idx + 1, depth + 1)
            } else {
                parse_array(s, idx + 1, depth + 1)
            };
        }
        'n' if idx + 3 < len && at(4, "null") => return Ok((Value::Null, idx + 4)),
        't' if idx + 3 < len && at(4, "true") => return Ok((Value::Bool(true), idx + 4)),
        'f' if idx + 4 < len && at(5, "false") => return Ok((Value::Bool(false), idx + 5)),
        'N' if idx + 2 < len && at(3, "NaN") => return Ok((Value::Bool(true), idx + 3)),
        'I' if idx + 7 < len && at(8, "Infinity") => return Ok((Value::Bool(true), idx + 8)),
        '-' if idx + 8 < len && at(9, "-Infinity") => return Ok((Value::Bool(true), idx + 9)),
        _ => {}
    }
    match_number(s, idx)
}

/// `scanstring_unicode` (strict): the string whose opening quote is just
/// before `end` (inside `depth` containers), and the index after its closing
/// quote.
fn scanstring(s: &[char], mut end: usize, depth: usize) -> Scan<String> {
    let len = s.len();
    let begin = end - 1;
    let mut out = String::new();
    loop {
        let mut next = end;
        let mut stop = None;
        while next < len {
            let d = s[next];
            if d == '"' || d == '\\' {
                stop = Some(d);
                break;
            }
            if u32::from(d) <= 0x1f {
                return fail("Invalid control character at", s, next, depth);
            }
            next += 1;
        }
        let Some(c) = stop else {
            return fail("Unterminated string starting at", s, begin, depth);
        };
        out.extend(&s[end..next]);
        next += 1;
        if c == '"' {
            return Ok((out, next));
        }
        if next == len {
            return fail("Unterminated string starting at", s, begin, depth);
        }
        let esc = s[next];
        if esc != 'u' {
            end = next + 1;
            let ch = match esc {
                '"' | '\\' | '/' => esc,
                'b' => '\u{8}',
                'f' => '\u{c}',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                _ => return fail("Invalid \\escape", s, end - 2, depth),
            };
            out.push(ch);
            continue;
        }
        next += 1;
        end = next + 4;
        if end > len {
            return fail("Invalid \\uXXXX escape", s, next - 1, depth);
        }
        let Some(mut c) = hex4(&s[next..end]) else {
            return fail("Invalid \\uXXXX escape", s, end - 5, depth);
        };
        next = end;
        if (0xd800..=0xdbff).contains(&c) && end + 6 < len && s[next] == '\\' && s[next + 1] == 'u'
        {
            end += 6;
            let Some(c2) = hex4(&s[next + 2..end]) else {
                return fail("Invalid \\uXXXX escape", s, end - 5, depth);
            };
            if (0xdc00..=0xdfff).contains(&c2) {
                c = 0x10000 + (((c - 0xd800) << 10) | (c2 - 0xdc00));
            } else {
                end -= 6;
            }
        }
        out.push(char::from_u32(c).unwrap_or('\u{fffd}'));
    }
}

fn hex4(digits: &[char]) -> Option<u32> {
    digits
        .iter()
        .try_fold(0u32, |acc, d| Some(acc << 4 | d.to_digit(16)?))
}

/// `_parse_object_unicode`, from just after the `{`.
fn parse_object(s: &[char], mut idx: usize, depth: usize) -> Scan<Value> {
    let mut map = Map::new();
    idx = skip_ws(s, idx);
    if s.get(idx) != Some(&'}') {
        loop {
            if s.get(idx) != Some(&'"') {
                return fail(
                    "Expecting property name enclosed in double quotes",
                    s,
                    idx,
                    depth,
                );
            }
            let (key, next) = scanstring(s, idx + 1, depth)?;
            idx = skip_ws(s, next);
            if s.get(idx) != Some(&':') {
                return fail("Expecting ':' delimiter", s, idx, depth);
            }
            idx = skip_ws(s, idx + 1);
            let (value, next) = scan_once(s, idx, depth)?;
            map.insert(key, value);
            idx = skip_ws(s, next);
            if s.get(idx) == Some(&'}') {
                break;
            }
            if s.get(idx) != Some(&',') {
                return fail("Expecting ',' delimiter", s, idx, depth);
            }
            let comma = idx;
            idx = skip_ws(s, idx + 1);
            if s.get(idx) == Some(&'}') {
                return fail(
                    "Illegal trailing comma before end of object",
                    s,
                    comma,
                    depth,
                );
            }
        }
    }
    Ok((Value::Object(map), idx + 1))
}

/// `_parse_array_unicode`, from just after the `[`.
fn parse_array(s: &[char], mut idx: usize, depth: usize) -> Scan<Value> {
    let mut values = Vec::new();
    idx = skip_ws(s, idx);
    if s.get(idx) != Some(&']') {
        loop {
            let (value, next) = scan_once(s, idx, depth)?;
            values.push(value);
            idx = skip_ws(s, next);
            if s.get(idx) == Some(&']') {
                break;
            }
            if s.get(idx) != Some(&',') {
                return fail("Expecting ',' delimiter", s, idx, depth);
            }
            let comma = idx;
            idx = skip_ws(s, idx + 1);
            if s.get(idx) == Some(&']') {
                return fail(
                    "Illegal trailing comma before end of array",
                    s,
                    comma,
                    depth,
                );
            }
        }
    }
    Ok((Value::Array(values), idx + 1))
}

/// `_match_number_unicode`: a number starting at `start`, or `StopIteration`.
fn match_number(s: &[char], start: usize) -> Scan<Value> {
    let len = s.len();
    let digit = |k: usize| k < len && s[k].is_ascii_digit();
    let mut idx = start;
    if s[idx] == '-' {
        idx += 1;
        if idx >= len {
            return Err(ScanError::Stop(start));
        }
    }
    if ('1'..='9').contains(&s[idx]) {
        idx += 1;
        while digit(idx) {
            idx += 1;
        }
    } else if s[idx] == '0' {
        idx += 1;
    } else {
        return Err(ScanError::Stop(start));
    }
    let mut is_float = false;
    if idx + 1 < len && s[idx] == '.' && s[idx + 1].is_ascii_digit() {
        is_float = true;
        idx += 2;
        while digit(idx) {
            idx += 1;
        }
    }
    if idx + 1 < len && (s[idx] == 'e' || s[idx] == 'E') {
        let e_start = idx;
        idx += 1;
        if idx + 1 < len && (s[idx] == '-' || s[idx] == '+') {
            idx += 1;
        }
        while digit(idx) {
            idx += 1;
        }
        if s[idx - 1].is_ascii_digit() {
            is_float = true;
        } else {
            idx = e_start;
        }
    }
    let text: String = s[start..idx].iter().collect();
    let value = if is_float {
        float_value(text.parse().expect("a JSON float literal"))
    } else {
        let digits = text.trim_start_matches('-').len();
        if digits > INT_MAX_STR_DIGITS {
            return Err(ScanError::Fail(LoadError::Value(format!(
                "Exceeds the limit ({INT_MAX_STR_DIGITS} digits) for integer string conversion: value has {digits} digits; use sys.set_int_max_str_digits() to increase the limit"
            ))));
        }
        match (text.parse::<i64>(), text.parse::<u64>()) {
            (Ok(n), _) => Value::Number(n.into()),
            (_, Ok(n)) => Value::Number(n.into()),
            _ => float_value(text.parse().expect("a JSON integer literal")),
        }
    };
    Ok((value, idx))
}

/// A float as a `Value`: infinite ones become `true` (see the module docs).
fn float_value(f: f64) -> Value {
    Number::from_f64(f).map_or(Value::Bool(true), Value::Number)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn errors_match_python() {
        // str(exc) for json.loads(text) on CPython 3.13.17 (None: it decodes).
        let rows: &[(&str, Option<&str>)] = &[
            ("", Some("Expecting value: line 1 column 1 (char 0)")),
            (" ", Some("Expecting value: line 1 column 2 (char 1)")),
            (
                "not json",
                Some("Expecting value: line 1 column 1 (char 0)"),
            ),
            ("[]", None),
            ("{}", None),
            ("[", Some("Expecting value: line 1 column 2 (char 1)")),
            ("[ ", Some("Expecting value: line 1 column 3 (char 2)")),
            (
                "{",
                Some("Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ),
            (
                "{\"",
                Some("Unterminated string starting at: line 1 column 2 (char 1)"),
            ),
            (
                "{\"a\"",
                Some("Expecting ':' delimiter: line 1 column 5 (char 4)"),
            ),
            ("{\"a\":", Some("Expecting value: line 1 column 6 (char 5)")),
            (
                "{\"a\":1",
                Some("Expecting ',' delimiter: line 1 column 7 (char 6)"),
            ),
            (
                "{\"a\":1,",
                Some("Expecting property name enclosed in double quotes: line 1 column 8 (char 7)"),
            ),
            (
                "{\"a\":1,}",
                Some("Illegal trailing comma before end of object: line 1 column 7 (char 6)"),
            ),
            (
                "[1,]",
                Some("Illegal trailing comma before end of array: line 1 column 3 (char 2)"),
            ),
            (
                "[1 2]",
                Some("Expecting ',' delimiter: line 1 column 4 (char 3)"),
            ),
            (
                "{\"a\" 1}",
                Some("Expecting ':' delimiter: line 1 column 6 (char 5)"),
            ),
            (
                "{1:2}",
                Some("Expecting property name enclosed in double quotes: line 1 column 2 (char 1)"),
            ),
            (
                "\"x",
                Some("Unterminated string starting at: line 1 column 1 (char 0)"),
            ),
            (
                "\"\\x\"",
                Some("Invalid \\escape: line 1 column 2 (char 1)"),
            ),
            (
                "\"\\u12\"",
                Some("Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
            ),
            (
                "\"\\u12g4\"",
                Some("Invalid \\uXXXX escape: line 1 column 3 (char 2)"),
            ),
            (
                "\"\\u0041",
                Some("Unterminated string starting at: line 1 column 1 (char 0)"),
            ),
            (
                "\"\\ud800\\udc00",
                Some("Unterminated string starting at: line 1 column 1 (char 0)"),
            ),
            (
                "\"a\u{1}b\"",
                Some("Invalid control character at: line 1 column 3 (char 2)"),
            ),
            ("-", Some("Expecting value: line 1 column 1 (char 0)")),
            ("-x", Some("Expecting value: line 1 column 1 (char 0)")),
            ("01", Some("Extra data: line 1 column 2 (char 1)")),
            ("1.", Some("Extra data: line 1 column 2 (char 1)")),
            ("1e", Some("Extra data: line 1 column 2 (char 1)")),
            ("1e+", Some("Extra data: line 1 column 2 (char 1)")),
            ("1.5e3x", Some("Extra data: line 1 column 6 (char 5)")),
            ("NaN", None),
            ("-Infinity", None),
            ("Infinityx", Some("Extra data: line 1 column 9 (char 8)")),
            ("nul", Some("Expecting value: line 1 column 1 (char 0)")),
            ("tru", Some("Expecting value: line 1 column 1 (char 0)")),
            ("fals", Some("Expecting value: line 1 column 1 (char 0)")),
            (
                "\u{feff}{}",
                Some("Unexpected UTF-8 BOM (decode using utf-8-sig): line 1 column 1 (char 0)"),
            ),
            ("{} x", Some("Extra data: line 1 column 4 (char 3)")),
            ("{}\n\n x", Some("Extra data: line 3 column 2 (char 5)")),
            (
                "\r\n{\r\n\"a\" x}",
                Some("Expecting ':' delimiter: line 3 column 5 (char 9)"),
            ),
            (
                "[{\"a\":",
                Some("Expecting value: line 1 column 7 (char 6)"),
            ),
            (
                "{\"a\":[1,{\"b\":}]}",
                Some("Expecting value: line 1 column 14 (char 13)"),
            ),
            (
                "{\"a\":\"\\u00e9\\ud83d\\ude00\",\"b\":[true,false,null,-0.5e-3,12]}",
                None,
            ),
            (
                "\"\\",
                Some("Unterminated string starting at: line 1 column 1 (char 0)"),
            ),
            (
                "\"abc\\",
                Some("Unterminated string starting at: line 1 column 1 (char 0)"),
            ),
            (
                "[1,\n2,\n,3]",
                Some("Expecting value: line 3 column 1 (char 7)"),
            ),
        ];
        for &(text, want) in rows {
            let got = match loads(text) {
                Ok(_) => None,
                Err(LoadError::Value(msg)) => Some(msg),
                Err(LoadError::Recursion(msg)) => panic!("{text:?}: RecursionError {msg}"),
            };
            assert_eq!(got.as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn values_keep_the_types_the_guard_checks() {
        let v = loads(r#"{"s":"\u00e9\ud83d\ude00\ud800","n":-0.0,"i":12,"big":123456789012345678901234567890,"inf":1e999,"nan":NaN,"a":[null,true],"a":1}"#)
            .expect("decodes");
        assert_eq!(v["s"], "\u{e9}\u{1f600}\u{fffd}");
        assert_eq!(v["n"].as_f64(), Some(0.0));
        assert_eq!(v["i"].as_i64(), Some(12));
        assert!(v["big"].as_f64().is_some_and(|f| f > 1e29));
        assert_eq!(v["inf"], Value::Bool(true));
        assert_eq!(v["nan"], Value::Bool(true));
        assert_eq!(v["a"], 1, "the last duplicate key wins, as in a dict");
    }

    #[test]
    fn nesting_limits_match_python() {
        // CPython 3.13.17 on Linux: exit 1 (RecursionError) or a decode.
        let recursion = |text: &str| matches!(loads(text), Err(LoadError::Recursion(_)));
        let arrays = |n: usize, tail: &str| format!("{}{tail}", "[".repeat(n));
        let objects = |n: usize, tail: &str| format!("{}{tail}", "{\"a\":".repeat(n));
        assert!(!recursion(&format!(
            "{}{}",
            "[".repeat(9997),
            "]".repeat(9997)
        )));
        assert!(recursion(&arrays(9998, "")));
        assert!(!recursion(&arrays(9997, "")));
        assert!(!recursion(&objects(9997, "")));
        assert!(!recursion(&arrays(9994, "1 2")));
        assert!(recursion(&arrays(9995, "1 2")));
        assert!(!recursion(&arrays(9994, "\"\\x\"")));
        assert!(recursion(&arrays(9995, "\"\\x\"")));
        assert!(!recursion(&objects(9994, "1")));
        assert!(recursion(&objects(9995, "1")));
        assert!(recursion(&objects(9994, "{1")));
        assert!(!recursion(&objects(9993, "{1")));
        assert!(!recursion(&arrays(9997, "x")));
        let digits = format!(
            "{}{}{}",
            "[".repeat(9997),
            "1".repeat(4301),
            "]".repeat(9997)
        );
        assert!(matches!(loads(&digits), Err(LoadError::Value(_))));
    }

    #[test]
    fn the_digit_limit_matches_python() {
        let want = "Exceeds the limit (4300 digits) for integer string conversion: value has 4301 digits; use sys.set_int_max_str_digits() to increase the limit";
        for text in [
            format!("[{}]", "1".repeat(4301)),
            format!("[-{}]", "1".repeat(4301)),
        ] {
            assert_eq!(loads(&text), Err(LoadError::Value(want.to_string())));
        }
        assert!(loads(&format!("[{}]", "1".repeat(4300))).is_ok());
        assert!(loads(&format!("[{}.5]", "1".repeat(5000))).is_ok());
    }
}
