//! Markdown parsing helpers of `aios docs-check`, ported line by line from
//! `scripts/docs/check.py` at 33c6b3d (L161-364, L534-572, L724-730, L1338-1347).
//!
//! Lines are Python `str.splitlines()` and whitespace is Python `str.isspace()`
//! (`crate::pystr`). Where check.py indexes a line at an ASCII character
//! (backtick, bracket, backslash), this port uses byte offsets; the positions are
//! the same characters because those characters are one byte in UTF-8.
//!
//! Python regex features the `regex` crate lacks are rewritten as code:
//! `heading_number` (`HEADING_NUM_RE`, L171, a lookahead) and
//! `has_placeholder_word` (`PLACEHOLDER_WORD_RE`, L534, a lookbehind and a
//! lookahead). Every other pattern is check.py's text with `\d`
//! written as `[0-9]`.
//!
//! Accepted divergences from check.py (no tracked file and no fixture exercises
//! them; the parity goldens prove the real inputs; verified with python3):
//! - regex `\s` does not match U+001C..U+001F here (Python's does);
//! - `\d` is `[0-9]`: non-ASCII decimal digits are not digits here;
//! - `gh_slug` (`char::is_alphanumeric`) and the `regex` crate's `\w`/`\b` keep
//!   combining marks (Mn/Mc), connector punctuation (Pc) and Join_Control
//!   characters that Python's `str.isalnum()` and `\w` drop; `gh_slug` also
//!   keeps letter-like Symbol-other (So) characters such as Ⓐ (U+24B6), which
//!   Python drops too. Neither differs on numeric symbols (No, e.g. `²`): both
//!   `gh_slug` and Python's `\w` keep them — though the `regex` crate's own
//!   `\w` (used outside `gh_slug`, e.g. in `MILESTONE_RE`) lacks No digits,
//!   where Python's `\w` has them. CPython 3.14's Unicode tables and the
//!   `regex` crate's may also drift apart from each other over time;
//! - milestone numbers that do not fit `u64` are ignored;
//! - `brace_expand` has no recursion limit here: at about 1,000 or more `{…}`
//!   groups in one doc-map code span, check.py raises RecursionError (caught by
//!   `__main__`, exit 2), where aios completes.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::pystr;

/// An ATX heading outside code blocks (check.py `headings`, L306-313).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1-based line number.
    pub line: usize,
    /// Number of leading `#` characters (1-6).
    pub level: usize,
    /// Heading text (HEADING_RE group 2).
    pub text: String,
}

/// check.py L161: an opening or closing fence at any indentation.
pub static FENCE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(`{3,}|~{3,})").expect("valid regex"));
/// check.py L162.
pub static HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(#{1,6})\s+(.*?)\s*#*\s*$").expect("valid regex"));
/// check.py L163-165: group 2 is the text, group 3 the target.
pub static INLINE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(!?)\[((?:[^\[\]]|\[[^\]]*\])*)\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+(?:"[^"]*"|'[^']*'))?\s*\)"#,
    )
    .expect("valid regex")
});
/// check.py L166: reference definitions, not footnotes (`^` escaped in the class).
pub static REF_DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^ {0,3}\[([^\]\^][^\]]*)\]:\s*(\S+)").expect("valid regex"));
/// check.py L167.
pub static WIKI_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(!?)\[\[([^\]|#]*)(?:#([^\]|]*))?(?:\|[^\]]*)?\]\]").expect("valid regex")
});
/// check.py L168-170.
pub static SECTION_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[[^\]]*\]\(([^)\s]+\.md)(#[^)]*)?\)\s*\**\s*§\s*([A-Z]?[0-9]+(?:\.[0-9]+)*)")
        .expect("valid regex")
});
/// check.py L172.
pub static SCHEME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[a-zA-Z][a-zA-Z0-9+.-]*:").expect("valid regex"));
/// check.py L223.
pub static HTML_COMMENT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<!--.*?-->").expect("valid regex"));
/// check.py L224.
pub static LIST_ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:[-*+]|[0-9]+[.)])\s+").expect("valid regex"));

/// check.py L173.
pub const PLACEHOLDER_CHARS: [&str; 11] = ["<", ">", "{", "}", "$", "*", "?", "…", "...", "[", "]"];

/// check.py `HEADING_NUM_RE` (L171) without its lookahead (see `heading_number`).
static HEADING_NUM_HEAD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?:§\s*)?([A-Z]?[0-9]+(?:\.[0-9]+)*)").expect("valid regex"));
/// check.py `PLACEHOLDER_WORD_RE` (L534) rewritten: maximal runs of ASCII letters
/// (see `has_placeholder_word`).
static ASCII_LETTERS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[A-Za-z]+").expect("valid regex"));
/// check.py L295.
static INLINE_MD_LINK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"!?\[([^\]]*)\]\([^)]*\)").expect("valid regex"));
/// check.py L296.
static HTML_TAG_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<[^>]+>").expect("valid regex"));
/// check.py L317; the braces are escaped inside the class too.
static BRACE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\{([^\{\}]*)\}").expect("valid regex"));
/// check.py L349 (fullmatch).
static SEPARATOR_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(?::?-{2,}:?)$").expect("valid regex"));
/// check.py L358.
static MILESTONE_RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bM([0-9]+)\s*[–-]\s*M([0-9]+)\b").expect("valid regex"));
/// check.py L362.
static MILESTONE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bM([0-9]+)\b").expect("valid regex"));
/// check.py L729.
static LINE_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r":[0-9]+(?:[-–][0-9]+)?(?:,[0-9]+)*$").expect("valid regex"));
/// check.py L1339 (re.S).
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\r?\n(.*?)\r?\n---\s*(?:\r?\n|$)").expect("valid regex"));
/// check.py L1344.
static FRONTMATTER_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([A-Za-z_][\w\-]*):\s*(.*)$").expect("valid regex"));

/// Byte index just past the run of backticks that starts at `start`.
fn backtick_run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end] == b'`' {
        end += 1;
    }
    end
}

/// Python `haystack.find(needle, from)` in byte offsets; `None` for -1.
fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .find(needle)
        .map(|offset| from + offset)
}

/// check.py `mask_code_spans` (L176-198): inline code spans, backticks included,
/// become one space per character. The closing run is the next run of the same
/// length that is not followed by another backtick; unclosed runs stay.
pub fn mask_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n);
    let mut copied = 0;
    let mut i = 0;
    while i < n {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let j = backtick_run_end(bytes, i);
        let tick = &line[i..j];
        let mut close = find_from(line, tick, j);
        while let Some(at) = close {
            let after = at + tick.len();
            if after < n && bytes[after] == b'`' {
                close = find_from(line, tick, after + 1);
            } else {
                break;
            }
        }
        let Some(at) = close else {
            i = j;
            continue;
        };
        let end = at + tick.len();
        out.push_str(&line[copied..i]);
        out.extend(line[i..end].chars().map(|_| ' '));
        copied = end;
        i = end;
    }
    out.push_str(&line[copied..]);
    out
}

/// check.py `code_spans` (L201-220): the stripped contents of inline code spans.
/// Unlike `mask_code_spans`, the closing run is simply the next run of the same
/// length, even when another backtick follows it.
pub fn code_spans(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let j = backtick_run_end(bytes, i);
        let tick = &line[i..j];
        match find_from(line, tick, j) {
            None => i = j,
            Some(close) => {
                spans.push(pystr::strip(&line[j..close]).to_string());
                i = close + tick.len();
            }
        }
    }
    spans
}

/// check.py `mask_html_comments` (L227-229): each `<!-- ... -->` on the line
/// becomes one space per character.
pub fn mask_html_comments(line: &str) -> String {
    HTML_COMMENT_RE
        .replace_all(line, |caps: &Captures<'_>| {
            " ".repeat(caps[0].chars().count())
        })
        .into_owned()
}

/// check.py `mask_prose` (L232-234): code spans, then inline HTML comments.
pub fn mask_prose(line: &str) -> String {
    mask_html_comments(&mask_code_spans(line))
}

/// check.py `is_escaped` (L237-242): an odd number of backslashes right before
/// byte index `idx` (the index of an ASCII character).
pub fn is_escaped(s: &str, idx: usize) -> bool {
    let bytes = s.as_bytes();
    let mut n = 0;
    while let Some(pos) = idx.checked_sub(n + 1) {
        if bytes.get(pos) != Some(&b'\\') {
            break;
        }
        n += 1;
    }
    n % 2 == 1
}

/// check.py `iter_prose_lines` (L245-291): `(1-based line number, line)` for the
/// lines outside fenced blocks (any indentation), multi-line HTML comments and
/// CommonMark indented code blocks (4+ columns after a blank line, outside a
/// list). Single-line comments stay; callers mask them with `mask_prose`.
pub fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    let mut in_comment = false;
    let mut in_indented = false;
    let mut in_list = false;
    let mut prev_blank = true;
    for (index, line) in pystr::splitlines(text).into_iter().enumerate() {
        let lineno = index + 1;
        let run = FENCE_RE
            .captures(line)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str());
        if fence.is_none() && run.is_some() && !in_indented {
            fence = run;
            prev_blank = false;
            continue;
        }
        if let Some(open) = fence {
            let closes = run.is_some_and(|run| {
                run.as_bytes()[0] == open.as_bytes()[0]
                    && run.len() >= open.len()
                    && pystr::strip(line) == run
            });
            if closes {
                fence = None;
            }
            continue;
        }
        if in_comment {
            if line.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if pystr::lstrip(line).starts_with("<!--") && !line.contains("-->") {
            in_comment = true;
            continue;
        }
        let blank = pystr::strip(line).is_empty();
        let indent = pystr::indent_width(line);
        if in_indented {
            if blank || indent >= 4 {
                prev_blank = blank;
                continue;
            }
            in_indented = false;
        } else if indent >= 4 && prev_blank && !(blank || in_list) {
            in_indented = true;
            continue;
        }
        if !blank {
            if LIST_ITEM_RE.is_match(line) {
                in_list = true;
            } else if indent == 0 && (prev_blank || line.starts_with('#')) {
                in_list = false;
            }
        }
        prev_blank = blank;
        out.push((lineno, line));
    }
    out
}

/// check.py `strip_inline_md` (L294-297).
pub fn strip_inline_md(text: &str) -> String {
    let unlinked = INLINE_MD_LINK_RE.replace_all(text, "${1}");
    let untagged = HTML_TAG_RE.replace_all(&unlinked, "");
    untagged.replace('`', "").replace("**", "").replace('*', "")
}

/// check.py `gh_slug` (L300-303): GitHub's heading anchor. The L302 pattern
/// (`[^\w\- ]`) is the character filter below.
pub fn gh_slug(text: &str) -> String {
    let stripped = strip_inline_md(text);
    pystr::strip(&stripped)
        .to_lowercase()
        .chars()
        .filter(|&c| c == '_' || c == '-' || c == ' ' || c.is_alphanumeric())
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// check.py `headings` (L306-313): ATX headings on prose lines.
pub fn headings(text: &str) -> Vec<Heading> {
    prose_lines(text)
        .into_iter()
        .filter_map(|(line, content)| {
            let caps = HEADING_RE.captures(content)?;
            Some(Heading {
                line,
                level: caps[1].len(),
                text: caps[2].to_string(),
            })
        })
        .collect()
}

/// check.py `brace_expand` (L316-323): expands the first `{a,b}` group, then
/// expands each result the same way (depth-first, left to right). check.py
/// recurses; this walks an explicit work stack, pushing each group's results in
/// reverse so they pop in check.py's order, so no input can overflow the
/// thread's stack (see the module doc for the resulting divergence).
pub fn brace_expand(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut stack = vec![s.to_string()];
    while let Some(item) = stack.pop() {
        let Some(caps) = BRACE_RE.captures(&item) else {
            out.push(item);
            continue;
        };
        let (Some(whole), Some(inner)) = (caps.get(0), caps.get(1)) else {
            out.push(item);
            continue;
        };
        let prefix = &item[..whole.start()];
        let suffix = &item[whole.end()..];
        let expanded: Vec<String> = inner
            .as_str()
            .split(',')
            .map(|part| format!("{prefix}{}{suffix}", pystr::strip(part)))
            .collect();
        stack.extend(expanded.into_iter().rev());
    }
    out
}

/// check.py `section_body` (L326-339): the lines after the first line where
/// `start` matches (searched anywhere), up to the next line where `stop` matches.
pub fn section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut started = false;
    for (index, line) in pystr::splitlines(text).into_iter().enumerate() {
        if !started {
            if start.is_match(line) {
                started = true;
            }
            continue;
        }
        if stop.is_match(line) {
            break;
        }
        out.push((index + 1, line));
    }
    out
}

/// check.py `table_rows` (L342-352): the stripped cells of `|` table rows,
/// skipping separator rows (every non-empty cell is `:?-{2,}:?`).
pub fn table_rows(lines: &[(usize, &str)]) -> Vec<(usize, Vec<String>)> {
    let mut rows = Vec::new();
    for &(lineno, line) in lines {
        let s = pystr::strip(line);
        if !s.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = s
            .trim_matches('|')
            .split('|')
            .map(|cell| pystr::strip(cell).to_string())
            .collect();
        if cells
            .iter()
            .filter(|cell| !cell.is_empty())
            .all(|cell| SEPARATOR_CELL_RE.is_match(cell))
        {
            continue;
        }
        rows.push((lineno, cells));
    }
    rows
}

/// check.py `milestone_tokens` (L355-364): milestones named as `M12` or as a
/// range `M3-M5` / `M3–M5` (at most 100 wide).
pub fn milestone_tokens(text: &str) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for caps in MILESTONE_RANGE_RE.captures_iter(text) {
        let lo = pystr::parse_uint(&caps[1]);
        let hi = pystr::parse_uint(&caps[2]);
        if let (Some(lo), Some(hi)) = (lo, hi) {
            if lo <= hi && hi - lo < 100 {
                out.extend(lo..=hi);
            }
        }
    }
    for caps in MILESTONE_RE.captures_iter(text) {
        if let Some(num) = pystr::parse_uint(&caps[1]) {
            out.insert(num);
        }
    }
    out
}

/// check.py `HEADING_NUM_RE.match(text).group(1)` (L171): the section number
/// at the start of a heading. Python backtracks the greedy number when the
/// lookahead `(?=[.:\s)]|$)` fails; the only shorter prefix that can pass is the
/// one before the last `.`, whose next character is that `.`.
pub fn heading_number(text: &str) -> Option<String> {
    let num = HEADING_NUM_HEAD.captures(text)?.get(1)?;
    let passes = text[num.end()..]
        .chars()
        .next()
        .is_none_or(|c| matches!(c, '.' | ':' | ')') || pystr::is_space(c));
    let s = num.as_str();
    if passes {
        Some(s.to_string())
    } else {
        s.rfind('.').map(|dot| s[..dot].to_string())
    }
}

/// check.py `PLACEHOLDER_WORD_RE.search(path)` (L534): a match needs a
/// non-letter (or an edge) on both sides of an all-letter word, so it exists
/// exactly when some maximal ASCII-letter run equals one of the words.
pub fn has_placeholder_word(path: &str) -> bool {
    ASCII_LETTERS.find_iter(path).any(|m| {
        matches!(
            m.as_str(),
            "N" | "NN" | "K" | "X" | "XX" | "XXX" | "YYYY" | "MM" | "DD"
        )
    })
}

/// check.py `is_placeholder` (L537-538).
pub fn is_placeholder(target: &str) -> bool {
    PLACEHOLDER_CHARS.iter().any(|c| target.contains(c))
}

/// check.py `is_path_placeholder` (L541-546): template paths
/// (`docs/phases/NN-name.md`) and type names (`shared/BootInfo`) are not paths.
pub fn is_path_placeholder(path: &str) -> bool {
    if is_placeholder(path) || has_placeholder_word(path) {
        return true;
    }
    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit_once('/').map_or(trimmed, |(_, last)| last);
    !(path.ends_with('/') || last.contains('.')) && last.chars().any(char::is_uppercase)
}

/// check.py `clean_repo_path` (L727-730): drops a `::item` suffix, a
/// `:line[-line][,line...]` suffix and trailing `.,;:)`. `token` must contain
/// no newline, so `LINE_SUFFIX_RE`'s `$` (the regex crate's, matching only the
/// true end) lines up with check.py's non-MULTILINE `$`. Both call sites
/// satisfy this: `pointer_doctor.rs` passes a `split_ws` token and
/// `repo_paths.rs` a regex capture, neither of which can contain whitespace.
pub fn clean_repo_path(token: &str) -> String {
    let head = token.split("::").next().unwrap_or(token);
    let without_lines = LINE_SUFFIX_RE.replace_all(head, "");
    without_lines
        .trim_end_matches(['.', ',', ';', ':', ')'])
        .to_string()
}

/// check.py `split_target` (L561-572): `(unquoted path, fragment)` of a local link
/// target, or `None` for an empty, external (`scheme:` or `//`) or placeholder target.
pub fn split_target(raw: &str) -> Option<(String, String)> {
    let mut t = pystr::strip(raw);
    if t.starts_with('<') && t.ends_with('>') {
        t = pystr::strip(&t[1..t.len() - 1]);
    }
    if t.is_empty() || SCHEME_RE.is_match(t) || t.starts_with("//") {
        return None;
    }
    if is_placeholder(t.split('#').next().unwrap_or(t)) {
        return None;
    }
    let (path, frag) = t.split_once('#').unwrap_or((t, ""));
    let path = path.split('?').next().unwrap_or(path);
    Some((pystr::url_unquote(path), frag.to_string()))
}

/// check.py `parse_frontmatter` (L1338-1347): `key: value` lines of the leading
/// `---` block (value stripped, then surrounding quotes removed; later keys win).
pub fn parse_frontmatter(text: &str) -> Option<HashMap<String, String>> {
    let caps = FRONTMATTER_RE.captures(text)?;
    let body = caps.get(1)?.as_str();
    let mut out = HashMap::new();
    for line in pystr::splitlines(body) {
        if let Some(kv) = FRONTMATTER_KEY_RE.captures(line) {
            let value = pystr::strip(&kv[2]).trim_matches(['"', '\'']);
            out.insert(kv[1].to_string(), value.to_string());
        }
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Expected values below were recorded by calling the check.py functions of the
    // same name on the same inputs (python3, check.py at 33c6b3d).

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 20] = [
            &FENCE_RE,
            &HEADING_RE,
            &INLINE_LINK_RE,
            &REF_DEF_RE,
            &WIKI_RE,
            &SECTION_REF_RE,
            &SCHEME_RE,
            &HTML_COMMENT_RE,
            &LIST_ITEM_RE,
            &HEADING_NUM_HEAD,
            &ASCII_LETTERS,
            &INLINE_MD_LINK_RE,
            &HTML_TAG_RE,
            &BRACE_RE,
            &SEPARATOR_CELL_RE,
            &MILESTONE_RANGE_RE,
            &MILESTONE_RE,
            &LINE_SUFFIX_RE,
            &FRONTMATTER_RE,
            &FRONTMATTER_KEY_RE,
        ];
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn heading_number_matches_the_lookahead_pattern() {
        let cases = [
            ("3.3 Lock", Some("3.3")),
            ("8. Plan", Some("8")),
            ("§ 4.2: x", Some("4.2")),
            ("A1.2 T", Some("A1.2")),
            ("4)", Some("4")),
            ("4", Some("4")),
            ("1.2a", Some("1")),
            ("1.2.3x", Some("1.2")),
            ("12a", None),
            ("Milestone 3", None),
            ("§3", Some("3")),
            ("2.1\tTabbed", Some("2.1")),
            ("B12.4.1 (x)", Some("B12.4.1")),
            ("1..2", Some("1")),
        ];
        for (text, want) in cases {
            assert_eq!(heading_number(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn placeholder_word_matches_the_lookaround_pattern() {
        let cases = [
            ("docs/phases/NN-name.md", true),
            ("YYYY-MM-DD", true),
            ("aXb", false),
            ("KX", false),
            ("NNN", false),
            ("x/N", true),
            ("N_x", true),
            ("9K", true),
            ("MMX", false),
        ];
        for (path, want) in cases {
            assert_eq!(has_placeholder_word(path), want, "{path:?}");
        }
    }

    #[test]
    fn mask_code_spans_matches_check_py() {
        let cases = [
            ("a `b` c", "a     c"),
            ("``x`y`` z", "        z"),
            ("`unclosed", "`unclosed"),
            ("``a```b``", "         "),
            ("é `ü` ö", "é     ö"),
            ("x ``` y", "x ``` y"),
            ("`a` `b", "    `b"),
            ("a ``b`` `c` d", "a           d"),
        ];
        for (line, want) in cases {
            assert_eq!(mask_code_spans(line), want, "{line:?}");
        }
    }

    #[test]
    fn code_spans_matches_check_py() {
        let cases: [(&str, &[&str]); 6] = [
            ("run `just build` and ``a ` b``", &["just build", "a ` b"]),
            ("``a```b``", &["a", "b"]),
            ("` padded `", &["padded"]),
            ("`a` `b", &["a"]),
            ("no spans", &[]),
            ("``x`y`` z", &["x`y"]),
        ];
        for (line, want) in cases {
            assert_eq!(code_spans(line), want, "{line:?}");
        }
    }

    #[test]
    fn masking_keeps_columns() {
        assert_eq!(mask_html_comments("a <!-- é --> b"), "a            b");
        assert_eq!(mask_html_comments("<!--x--><!--y-->"), " ".repeat(16));
        assert_eq!(mask_html_comments("<!-- open"), "<!-- open");
        assert_eq!(mask_prose("`<!--` x <!-- y -->"), "       x           ");
        assert_eq!(mask_prose("[a](b) `[c](d)`"), "[a](b)         ");
    }

    #[test]
    fn is_escaped_counts_backslashes() {
        assert!(is_escaped("\\[", 1));
        assert!(!is_escaped("\\\\[", 2));
        assert!(!is_escaped("[", 0));
        assert!(is_escaped("a\\\\\\[", 4));
    }

    const PROSE_DOC: &str = concat!(
        "# Title\n",
        "Intro text\n",
        "```rust\n",
        "# not a heading\n",
        "```\n",
        "    indented code after blank? no, prev not blank\n",
        "text\n",
        "\n",
        "    indented code block\n",
        "\ttab indented too\n",
        "\n",
        "after indented\n",
        "- list item\n",
        "\n",
        "    continuation in list (not code)\n",
        "paragraph\n",
        "<!-- multi\n",
        "line comment -->\n",
        "<!-- single --> kept\n",
        "~~~~\n",
        "~~~ inner shorter fence\n",
        "~~~~~\n",
        "   ```\n",
        "   indented fence\n",
        "   ```\n",
        "## Heading ##\n",
        "####### seven\n",
        "#no space\n",
        "## C# language\n",
    );

    #[test]
    fn prose_lines_skip_code_and_comments() {
        let want = [
            (1, "# Title"),
            (2, "Intro text"),
            (6, "    indented code after blank? no, prev not blank"),
            (7, "text"),
            (8, ""),
            (12, "after indented"),
            (13, "- list item"),
            (14, ""),
            (15, "    continuation in list (not code)"),
            (16, "paragraph"),
            (19, "<!-- single --> kept"),
            (26, "## Heading ##"),
            (27, "####### seven"),
            (28, "#no space"),
            (29, "## C# language"),
        ];
        assert_eq!(prose_lines(PROSE_DOC), want);
    }

    #[test]
    fn headings_are_atx_headings_on_prose_lines() {
        let heading = |line, level, text: &str| Heading {
            line,
            level,
            text: text.to_string(),
        };
        assert_eq!(
            headings(PROSE_DOC),
            [
                heading(1, 1, "Title"),
                heading(26, 2, "Heading"),
                heading(29, 2, "C# language"),
            ]
        );
    }

    #[test]
    fn slugs_match_github() {
        let cases = [
            ("3.3 Lock Hierarchy", "33-lock-hierarchy"),
            (
                "`code` and **bold** [link](x.md) <br> *em*",
                "code-and-bold-link--em",
            ),
            ("Ünïcode Title_x", "ünïcode-title_x"),
            ("  Spaced  Out ", "spaced--out"),
            ("C# & C++: a/b (c)", "c--c-ab-c"),
            ("![img](p.png) Title", "img-title"),
            ("Ⅳ ½ ²", "ⅳ-½-²"),
        ];
        for (text, want) in cases {
            assert_eq!(gh_slug(text), want, "{text:?}");
        }
        assert_eq!(
            strip_inline_md("`code` and **bold** [link](x.md) <br> *em*"),
            "code and bold link  em"
        );
        assert_eq!(strip_inline_md("![img](p.png) Title"), "img Title");
    }

    #[test]
    fn brace_expand_is_depth_first() {
        let cases: [(&str, &[&str]); 6] = [
            (
                "docs/kernel/{a,b}.md",
                &["docs/kernel/a.md", "docs/kernel/b.md"],
            ),
            ("x{a,b}{c,d}", &["xac", "xad", "xbc", "xbd"]),
            ("p/{ a , b }/q", &["p/a/q", "p/b/q"]),
            ("none", &["none"]),
            ("{}", &[""]),
            ("a{b,{c,d}}e", &["abe", "ace", "abe", "ade"]),
        ];
        for (s, want) in cases {
            assert_eq!(brace_expand(s), want, "{s:?}");
        }
    }

    #[test]
    fn brace_expand_does_not_recurse_per_group() {
        // Deep enough to overflow the stack if each `{...}` group took a frame.
        let s = format!("docs/{}.md", "{a}".repeat(20_000));
        let want = format!("docs/{}.md", "a".repeat(20_000));
        assert_eq!(brace_expand(&s), vec![want]);
        // Order and repeats as check.py's recursion yields them (python3).
        assert_eq!(
            brace_expand("{a,b}{c,{d,e}}"),
            ["ac", "ad", "ac", "ae", "bc", "bd", "bc", "be"]
        );
        assert_eq!(
            brace_expand("x{ p , {q,r}s}{t,u}"),
            ["xpt", "xpu", "xqst", "xqsu", "xpt", "xpu", "xrst", "xrsu"]
        );
    }

    const TABLE_DOC: &str = concat!(
        "# D\n",
        "## 3. Locks\n",
        "### 3.3 Lock Hierarchy\n",
        "\n",
        "| Rank | Lock | Purpose |\n",
        "|---|:---:|---|\n",
        "| 1 | `ALPHA_LOCK` | First |\n",
        "|  |  |  |\n",
        "| | x | |\n",
        "  | 2 | `BETA` |\n",
        "|---|\n",
        "### 3.4 Test Locks\n",
        "text\n",
        "### 3.5 Notes\n",
    );

    #[test]
    fn section_body_and_table_rows() {
        let start = Regex::new(r"^### 3\.3 ").expect("valid regex");
        let stop = Regex::new(r"^### 3\.5 |^## 4\.").expect("valid regex");
        let body = section_body(TABLE_DOC, &start, &stop);
        let lines: Vec<usize> = body.iter().map(|&(line, _)| line).collect();
        assert_eq!(lines, (4..=13).collect::<Vec<usize>>());
        assert_eq!(body[1], (5, "| Rank | Lock | Purpose |"));
        let cells = |row: &[&str]| row.iter().map(|c| c.to_string()).collect::<Vec<String>>();
        assert_eq!(
            table_rows(&body),
            [
                (5, cells(&["Rank", "Lock", "Purpose"])),
                (7, cells(&["1", "`ALPHA_LOCK`", "First"])),
                (9, cells(&["", "x", ""])),
                (10, cells(&["2", "`BETA`"])),
            ]
        );
        let never = Regex::new(r"^## 9").expect("valid regex");
        assert!(section_body(TABLE_DOC, &never, &stop).is_empty());
    }

    #[test]
    fn milestone_tokens_expand_short_ranges() {
        let cases: [(&str, &[u64]); 4] = [
            (
                "M1–M3, M7 and M10-M12; M5-M2 (reversed), M1-M200",
                &[1, 2, 3, 5, 7, 10, 11, 12, 200],
            ),
            ("M4 - M6", &[4, 5, 6]),
            ("XM3 M03", &[3]),
            ("M1-M2-M3", &[1, 2, 3]),
        ];
        for (text, want) in cases {
            let got: Vec<u64> = milestone_tokens(text).into_iter().collect();
            assert_eq!(got, want, "{text:?}");
        }
    }

    #[test]
    fn placeholders() {
        let cases = [
            ("docs/phases/NN-name.md", false, true),
            ("shared/BootInfo", false, true),
            ("kernel/src/mm/", false, false),
            ("kernel/src/Foo.rs", false, false),
            ("docs/<x>.md", true, true),
            ("docs/a…", true, true),
            ("a/b...", true, true),
            ("kernel/src/mm/X/", false, true),
            ("scripts/YYYY-MM-DD.sh", false, true),
            ("kernel/src/main.rs", false, false),
            ("KX/y", false, false),
            ("a/Makefile", false, true),
            ("a/README", false, true),
        ];
        for (path, placeholder, path_placeholder) in cases {
            assert_eq!(is_placeholder(path), placeholder, "{path:?}");
            assert_eq!(is_path_placeholder(path), path_placeholder, "{path:?}");
        }
    }

    #[test]
    fn clean_repo_path_drops_line_suffixes() {
        let cases = [
            ("kernel/src/main.rs:12-40,", "kernel/src/main.rs:12-40"),
            ("shared/src/lib.rs::Foo", "shared/src/lib.rs"),
            ("scripts/x.sh:1,4,9", "scripts/x.sh"),
            ("kernel/src/mm/).", "kernel/src/mm/"),
            ("a:1:2", "a:1"),
            ("kernel/src/a.rs:3–5", "kernel/src/a.rs"),
            ("x.rs:12;", "x.rs:12"),
        ];
        for (token, want) in cases {
            assert_eq!(clean_repo_path(token), want, "{token:?}");
        }
    }

    #[test]
    fn split_target_matches_check_py() {
        let cases = [
            ("<a b.md>", Some(("a b.md", ""))),
            ("https://x.org/a", None),
            ("//host/x", None),
            ("#frag", Some(("", "frag"))),
            ("a%20b.md#Sec", Some(("a b.md", "Sec"))),
            ("a%20b.md?x=1#Sec", None),
            ("<>", None),
            ("docs/{x}.md", None),
            ("a.md#{x}", Some(("a.md", "{x}"))),
            ("  b.md  ", Some(("b.md", ""))),
            ("mailto:x@y", None),
            ("c%C3%A9.md", Some(("cé.md", ""))),
            ("%E9.md", Some(("\u{FFFD}.md", ""))),
            ("a.md#b#c", Some(("a.md", "b#c"))),
            ("?q#f", None),
        ];
        for (raw, want) in cases {
            let want = want.map(|(p, f): (&str, &str)| (p.to_string(), f.to_string()));
            assert_eq!(split_target(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn frontmatter_matches_check_py() {
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<String, String>>()
        };
        let full = "---\nauthor: jl\ndate: 2026-01-01\ntags: [a]\nstatus: \"final\"\n---\n# T\n";
        assert_eq!(
            parse_frontmatter(full),
            Some(map(&[
                ("author", "jl"),
                ("date", "2026-01-01"),
                ("tags", "[a]"),
                ("status", "final"),
            ]))
        );
        assert_eq!(
            parse_frontmatter("---\r\nk: 'v'\r\n---\r\n"),
            Some(map(&[("k", "v")]))
        );
        assert_eq!(parse_frontmatter("no front"), None);
        assert_eq!(parse_frontmatter("---\n---\n"), None);
        assert_eq!(
            parse_frontmatter("---\nk: v\n---"),
            Some(map(&[("k", "v")]))
        );
        assert_eq!(
            parse_frontmatter("---\na: 1\n--- \nb"),
            Some(map(&[("a", "1")]))
        );
        assert_eq!(
            parse_frontmatter("---\n  indented: x\nbad line\nx-y_z: w \nx-y_z: later\n---\n"),
            Some(map(&[("x-y_z", "later")]))
        );
    }
}
