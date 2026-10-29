//! The guard's regular expressions, compiled so they match what Python's `re`
//! matches on a `str` (git-push-guard.py's module constants and inline
//! patterns).
//!
//! Three Python behaviours need help beyond `crate::pyre`:
//!
//! - `\b` and `\w`. Python's `\w` is exactly `[\p{L}\p{N}_]` (every code point
//!   checked on CPython 3.13), while the `regex` crate's also counts marks and
//!   join controls. So a `\b` before a word literal is written [`B_BEFORE`] and
//!   one after it [`B_AFTER`], which consume the neighbouring character. Every
//!   pattern here only answers "is there a match" (or reports a match that has no
//!   `\b`), so consuming a neighbour never changes the answer; [`inline_git_push`]
//!   is the one pattern with two words, and it is code.
//! - `re.IGNORECASE`. Python also folds U+0130 and U+0131 to `i`, which the
//!   crate's `(?i)` does not, so [`ci`] spells every ASCII letter as the class of
//!   what Python's `re.I` matches for it (`i`: U+0130, U+0131; `k`: U+212A; `s`:
//!   U+017F; every other letter only its two cases), checked over every code point
//!   on CPython 3.13.
//! - `$` outside MULTILINE also matches before a final `\n`; the patterns that can
//!   meet one write `\n?$`.

use std::sync::LazyLock;

use regex::Regex;

use crate::pyre;

/// Python's `\w` as a class body.
pub const WORD: &str = r"\p{L}\p{N}_";

/// `\b` before a literal that starts with a word character.
pub const B_BEFORE: &str = r"(?:^|[^\p{L}\p{N}_])";

/// `\b` after a literal that ends with a word character.
pub const B_AFTER: &str = r"(?:$|[^\p{L}\p{N}_])";

/// A Python `str` pattern, compiled (`crate::pyre::compile`).
pub fn re(pattern: &str) -> Regex {
    pyre::compile(pattern).unwrap_or_else(|e| panic!("guard pattern {pattern:?}: {e}"))
}

/// Python's `\w` for one character.
pub fn is_word(c: char) -> bool {
    static W: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\A[\p{L}\p{N}_]\z").expect("valid"));
    W.is_match(c.encode_utf8(&mut [0; 4]))
}

/// `literal` matched as Python's `re.IGNORECASE` matches it: each ASCII letter
/// becomes the class of the characters `re.I` equates with it, and everything
/// else is escaped.
pub fn ci(literal: &str) -> String {
    let mut out = String::new();
    for c in literal.chars() {
        if c.is_ascii_alphabetic() {
            let lower = c.to_ascii_lowercase();
            let upper = c.to_ascii_uppercase();
            let extra = match lower {
                'i' => "\u{130}\u{131}",
                'k' => "\u{212A}",
                's' => "\u{17F}",
                _ => "",
            };
            out.push_str(&format!("[{lower}{upper}{extra}]"));
        } else {
            out.push_str(&regex::escape(&c.to_string()));
        }
    }
    out
}

/// `MENTIONS_GIT`: text that suggests a command string or script is about git
/// or gh.
pub static MENTIONS_GIT: LazyLock<Regex> =
    LazyLock::new(|| re(&format!("git|push|{B_BEFORE}gh{B_AFTER}")));

/// `MENTIONS_TEAM` (`re.I`): text that suggests it is about a team rule (QEMU,
/// kills, the toolchain).
pub static MENTIONS_TEAM: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        "{}|{B_BEFORE}{}{B_AFTER}|{}|{}|{}",
        ci("qemu"),
        ci("just"),
        ci("pkill"),
        ci("killall"),
        ci("rustup")
    ))
});

/// The extra words of `decide`'s parse-failure gate:
/// `\b(kill|soak|cargo|tmux|screen|setsid|g?sed)\b|\.claude`.
pub static FALLBACK_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"{B_BEFORE}(?:kill|soak|cargo|tmux|screen|setsid|g?sed){B_AFTER}|\.claude"
    ))
});

/// `PID_FINDER`: commands whose output, in a `$(...)` or a pipe into kill, is a
/// pid list found by pattern.
pub static PID_FINDER: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        "{B_BEFORE}(?:pgrep|pidof|pkill|lsof|fuser|ps){B_AFTER}"
    ))
});

/// `\b(sh|bash|zsh|dash|ksh)\b`: a `#!` line that names a shell.
pub static SHELL_SHEBANG: LazyLock<Regex> =
    LazyLock::new(|| re(&format!("{B_BEFORE}(?:sh|bash|zsh|dash|ksh){B_AFTER}")));

/// `AWK_EXEC`: awk statements that run a shell command,
/// `\bsystem\s*\(|\|\s*getline|\|&|\bprintf?\b[^;{}]*\|\s*["a-zA-Z_$(]`. After
/// `print` or `printf`, `\b` holds when the next character is not a word
/// character; that character is either the `|` itself or the first of `[^;{}]*`.
pub static AWK_EXEC: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r#"{B_BEFORE}system\s*\(|\|\s*getline|\|&|{B_BEFORE}printf?(?:[^{WORD};{{}}][^;{{}}]*)?\|\s*["a-zA-Z_$(]"#
    ))
});

/// `ASSIGNMENT`: `NAME=`, `NAME+=`, `NAME[sub]=` at the start of a word.
pub static ASSIGNMENT: LazyLock<Regex> =
    LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*(?:\[[^\]]*\])?\+?="));

/// `SED_PROTECTED` (`re.I`): `.claude/settings*` or `.claude/hooks*`.
pub static SED_PROTECTED: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"\.{}/+(?:{}|{})",
        ci("claude"),
        ci("settings"),
        ci("hooks")
    ))
});

/// `re.search(r"\.claude", t, re.I)`.
pub static MENTIONS_CLAUDE: LazyLock<Regex> = LazyLock::new(|| re(&format!(r"\.{}", ci("claude"))));

/// `INLINE_GIT_PUSH.search(text)`, `\bgit\b.*\bpush\b` with `re.S`: a word `git`
/// followed, anywhere later, by a word `push`. The earliest `git` leaves the most
/// room, and the character after it is not a word character, so a `push` that
/// follows it has a non-word character before it inside the rest of the text.
pub fn inline_git_push(text: &str) -> bool {
    static GIT: LazyLock<Regex> = LazyLock::new(|| re(&format!("{B_BEFORE}(git){B_AFTER}")));
    static PUSH: LazyLock<Regex> = LazyLock::new(|| re(&format!("[^{WORD}]push{B_AFTER}")));
    GIT.captures(text)
        .and_then(|c| c.get(1))
        .is_some_and(|git| PUSH.is_match(&text[git.end()..]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_class_is_letters_numbers_and_underscore() {
        for c in ['a', 'Z', '_', '9', 'é', 'ß', '²', '½', 'Ⅻ', '٣', '中'] {
            assert!(is_word(c), "{c:?} is \\w in Python");
        }
        for c in [' ', '-', '/', '\u{301}', '\u{200d}', '$', '\u{a0}'] {
            assert!(!is_word(c), "{c:?} is not \\w in Python");
        }
    }

    #[test]
    fn word_boundaries_follow_python() {
        // re.search on CPython 3.13: a combining mark is not \w, so it ends a word.
        assert!(MENTIONS_GIT.is_match("gh pr list"));
        assert!(MENTIONS_GIT.is_match("x;gh\u{301}"));
        assert!(!MENTIONS_GIT.is_match("ghost"));
        assert!(!MENTIONS_GIT.is_match("highway"));
        assert!(MENTIONS_TEAM.is_match("JUST run"));
        assert!(MENTIONS_TEAM.is_match("pk\u{131}ll x"));
        assert!(MENTIONS_TEAM.is_match("ruſtup"));
        assert!(!MENTIONS_TEAM.is_match("adjust"));
        assert!(FALLBACK_WORDS.is_match("gsed -i"));
        assert!(FALLBACK_WORDS.is_match("x .claude"));
        assert!(!FALLBACK_WORDS.is_match("xsed"));
        assert!(PID_FINDER.is_match("kill $(ps aux)"));
        assert!(!PID_FINDER.is_match("kill $(psx)"));
    }

    #[test]
    fn inline_git_push_matches_python() {
        // bool(INLINE_GIT_PUSH.search(t)) on CPython 3.13.
        for (text, want) in [
            ("git push", true),
            ("os.system('git  push origin main')", true),
            ("gitpush", false),
            ("git\npush", true),
            ("push git", false),
            ("git pushx", false),
            ("agit push", false),
            ("agit git push", true),
            ("git x_push", false),
            ("git é push", true),
        ] {
            assert_eq!(inline_git_push(text), want, "{text:?}");
        }
    }

    #[test]
    fn awk_exec_matches_python() {
        // bool(AWK_EXEC.search(t)) on CPython 3.13.
        for (text, want) in [
            ("{ system(\"ls\") }", true),
            ("{ x_system(1) }", false),
            ("{ \"date\" | getline d }", true),
            ("{ print |& \"cat\" }", true),
            ("{ print $0 | \"sh\" }", true),
            ("{ printf \"%s\" | \"sh\" }", true),
            ("{ print| \"sh\" }", true),
            ("{ printx | \"sh\" }", false),
            ("{ print $0; x | \"sh\" }", false),
            ("{ print $0 }", false),
        ] {
            assert_eq!(AWK_EXEC.is_match(text), want, "{text:?}");
        }
    }

    #[test]
    fn ignorecase_classes_match_python() {
        let re_i = re(&format!("^{}$", ci("Claude/Settings")));
        assert!(re_i.is_match("claude/settings"));
        assert!(re_i.is_match("CLAUDE/SETT\u{130}NGS"));
        assert!(re_i.is_match("claude/ſettings"));
        assert!(!re_i.is_match("claude-settings"));
        assert!(SED_PROTECTED.is_match("x/.Claude//HOOKS"));
        assert!(!SED_PROTECTED.is_match(".claude/agents"));
    }
}
