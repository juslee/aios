//! The PATH jail of the agent-script tests (contract §5).
//!
//! A scenario runs a script, or the `aios` binary, with `PATH=<fake_bin>:<jail_bin>`
//! and nothing else on it: no `/usr/bin`, no `/bin`. `fake_bin` holds the fakes
//! (`gh`, `aios-fake-docs`, ...); `jail_bin` holds symlinks to exactly the host
//! tools the program is allowed to run. So a test can never reach the host's `gh`
//! (GitHub's ubuntu image ships one, and the job has no token), and a tool the
//! program needs but the list lacks fails loudly instead of being found by luck.
//!
//! The tool lists are checked against the scripts themselves: [`script_commands`]
//! lists the external commands a shell script runs, and `tests/agent_support.rs`
//! compares that with each list. This module uses only `super::{isolated, unique_dir}`
//! (contract A3), so it builds on a tree that has no other agent-test support.

use std::collections::BTreeSet;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::Command;

/// The external commands of the fake `gh` script (`fake_gh::SCRIPT`).
pub const FAKE_GH_TOOLS: &[&str] = &["cat", "cp", "dirname", "grep", "mkdir", "tr"];

/// The external commands of the loop's fake `claude` script (`loop_fake::FAKE_CLAUDE`).
pub const FAKE_CLAUDE_TOOLS: [&str; 4] = ["cat", "env", "mkdir", "sleep"];

/// `FAKE_GH_TOOLS` and `FAKE_CLAUDE_TOOLS` together; [`bin`] always links them.
pub const FAKE_TOOLS: &[&str] = &[
    "cat", "cp", "dirname", "env", "grep", "mkdir", "sleep", "tr",
];

/// The external commands of the former `scripts/agent/brief.sh` (blob at `2d4a9f7`),
/// less `gh` and `.claude/hooks/aios` (fakes) and `sh` (always linked).
pub const BRIEF_TOOLS: &[&str] = &[
    "awk", "basename", "cat", "date", "find", "git", "grep", "head", "jq", "ls", "mkdir", "mktemp",
    "rm", "sed", "sort", "stat", "tail", "touch", "tr", "wc",
];

/// The external commands of the former `scripts/agent/checkpoint.sh` (blob at `faf6d20`),
/// less `gh`.
pub const CHECKPOINT_TOOLS: &[&str] = &[
    "awk", "basename", "cut", "dirname", "git", "grep", "head", "mktemp", "paste", "rm", "sed",
    "sort", "tr", "wc",
];

/// The external commands of `scripts/agent/qemu-lock.sh`.
pub const QEMU_LOCK_TOOLS: &[&str] = &[
    "awk", "basename", "cat", "cut", "date", "dirname", "find", "git", "grep", "mkdir", "mv",
    "pgrep", "ps", "rm", "sed", "sleep", "sysctl", "tr",
];

/// `BRIEF_TOOLS`, `CHECKPOINT_TOOLS` and `QEMU_LOCK_TOOLS` together. No `flock`,
/// `lockf`, `uniq`, `xargs` or `readlink`: no script line runs them.
pub const ORACLE_TOOLS: &[&str] = &[
    "awk", "basename", "cat", "cut", "date", "dirname", "find", "git", "grep", "head", "jq", "ls",
    "mkdir", "mktemp", "mv", "paste", "pgrep", "ps", "rm", "sed", "sleep", "sort", "stat",
    "sysctl", "tail", "touch", "tr", "wc",
];

/// Names the jail never links, so no test can reach a real one.
const REFUSED: &[&str] = &["gh", "claude"];

/// `name` on the host's PATH (the test process's own), as a path inside a PATH
/// entry: not canonicalised, so a multi-call binary still sees its own name as
/// `argv[0]` when it is invoked through the jail's link.
pub fn host_which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| is_executable_file(candidate))
}

fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// `<dir>/bin`, holding a symlink to each of `tools`, to `sh` and to every entry of
/// [`FAKE_TOOLS`], resolved from the host PATH. Panics on a missing tool and
/// refuses `gh` and `claude`.
pub fn bin(dir: &Path, tools: &[&str]) -> PathBuf {
    let bin = dir.join("bin");
    std::fs::create_dir_all(&bin).expect("create the jail bin directory");
    let wanted: BTreeSet<&str> = tools
        .iter()
        .copied()
        .chain(["sh"])
        .chain(FAKE_TOOLS.iter().copied())
        .collect();
    for tool in wanted {
        assert!(
            !REFUSED.contains(&tool),
            "the jail never links {tool}: it is a fake or absent"
        );
        let target = host_which(tool)
            .unwrap_or_else(|| panic!("jail: {tool} is not on the host PATH (a tool list entry)"));
        let link = bin.join(tool);
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&target, &link)
            .unwrap_or_else(|e| panic!("jail: link {tool} -> {}: {e}", target.display()));
    }
    bin
}

/// Make `cmd` hermetic: `common::isolated()`, then no `GH_*`, `GITHUB_*`, `AIOS_*` or
/// `CLAUDE*` variable from the parent (a scenario sets what it needs afterwards),
/// `PATH=<fake_bin>:<jail_bin>` and nothing else on it, `TZ=UTC` and `LC_ALL=C`. It
/// sets no git identity: the runners add one after it.
pub fn hermetic<'c>(cmd: &'c mut Command, fake_bin: &Path, jail_bin: &Path) -> &'c mut Command {
    super::isolated(cmd);
    let set_on_cmd: Vec<_> = cmd.get_envs().map(|(name, _)| name.to_owned()).collect();
    for name in std::env::vars_os().map(|(name, _)| name).chain(set_on_cmd) {
        let bytes = name.as_bytes();
        if [&b"GH_"[..], b"GITHUB_", b"AIOS_", b"CLAUDE"]
            .iter()
            .any(|prefix| bytes.starts_with(prefix))
        {
            cmd.env_remove(&name);
        }
    }
    let path = std::env::join_paths([fake_bin, jail_bin]).expect("join the jail PATH");
    cmd.env("PATH", path).env("TZ", "UTC").env("LC_ALL", "C")
}

/// Panic when `stderr` shows a command the PATH could not find: `command not found`
/// (bash, macOS sh), a line ending `: not found` (dash), or `env: <name>: No such
/// file or directory` for a bare name. A missing tool behind `2>/dev/null` stays
/// invisible here, so `agent_support` also compares every script's commands with its
/// tool list.
pub fn assert_no_command_not_found(stderr: &[u8]) {
    let text = String::from_utf8_lossy(stderr);
    for line in text.lines() {
        let line = line.trim_end();
        let missing = line.contains("command not found")
            || line.ends_with(": not found")
            || bare_enoent(line);
        assert!(
            !missing,
            "a command was not found on the jail PATH: {line:?}\n--- stderr\n{text}"
        );
    }
}

/// `<launcher>: <bare name>: No such file or directory`, as `env foo` prints it.
fn bare_enoent(line: &str) -> bool {
    let Some(head) = line.strip_suffix(": No such file or directory") else {
        return false;
    };
    let mut fields: Vec<&str> = head.split(": ").collect();
    let Some(name) = fields.pop() else {
        return false;
    };
    let name = name.trim_matches(|c| matches!(c, '\'' | '"' | '`' | '\u{2018}' | '\u{2019}'));
    let launcher = fields.last().copied().unwrap_or("");
    !name.is_empty()
        && !name.contains('/')
        && [
            "env", "xargs", "exec", "command", "nohup", "timeout", "time", "nice",
        ]
        .contains(&launcher)
}

/// Shell keywords and builtins: words that start a simple command but are not an
/// external tool. The one list in both plans (A1); a caller appends the names of the
/// functions its script defines.
pub const SH_WORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "case", "esac", "for", "select", "while", "until", "do",
    "done", "in", "function", "time", "{", "}", "!", "[[", "]]", ":", ".", "[", "]", "alias", "bg",
    "break", "cd", "command", "continue", "echo", "eval", "exec", "exit", "export", "false", "fg",
    "getopts", "hash", "jobs", "kill", "local", "printf", "pwd", "read", "readonly", "return",
    "set", "shift", "source", "test", "times", "trap", "true", "type", "ulimit", "umask",
    "unalias", "unset", "wait",
];

/// The first word of every simple command in the shell script `src` that is not in
/// `not_commands`: the external commands (and called functions) the script runs.
///
/// A small lexer, not one regex:
/// - a command starts at a line start and after `|`, `;`, `&&`, `||`, `&`, `(`, `{`,
///   `!`, `then`, `do`, `else`, `if`, `elif`, `while`, `until`, and after a `case`
///   pattern's `)`;
/// - leading `NAME=value` assignments and redirections are skipped, so
///   `GH_PROMPT_DISABLED=1 gh ...` yields `gh`;
/// - every command substitution, `$(...)` and backticks, is scanned as a script of
///   its own wherever it appears: inside double quotes, in a here-document with an
///   unquoted delimiter, in a parameter default, inside `$((...))`;
/// - comments, single-quoted text (awk and jq programs), the literal parts of
///   double-quoted strings, escaped characters, and here-documents with a quoted
///   delimiter are skipped;
/// - the first argument of `trap`, when it is one single-quoted string, is a script
///   (`trap 'rm -rf "$TMP"' EXIT` runs `rm`).
pub fn script_commands(src: &str, not_commands: &[&str]) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    scan_text(src.as_bytes(), &mut found);
    for word in not_commands {
        found.remove(*word);
    }
    found
}

fn scan_text(text: &[u8], out: &mut BTreeSet<String>) {
    Lexer::new(text, out).script(End::Eof);
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum End {
    Eof,
    Paren,
    Backtick,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaseState {
    Header,
    Patterns,
    Body,
}

struct Heredoc {
    delim: Vec<u8>,
    quoted: bool,
    strip_tabs: bool,
}

#[derive(Default)]
struct Word {
    /// The literal characters, and the content of single-quoted parts.
    lit: Vec<u8>,
    /// Any quote, escape or expansion was seen.
    dynamic: bool,
    /// The word is exactly one single-quoted string: its content.
    single: Option<Vec<u8>>,
}

struct Lexer<'s, 'o> {
    b: &'s [u8],
    i: usize,
    out: &'o mut BTreeSet<String>,
    in_backtick: bool,
}

const WORD_END: &[u8] = b" \t\r\n;&|()<>";

impl<'s, 'o> Lexer<'s, 'o> {
    fn new(b: &'s [u8], out: &'o mut BTreeSet<String>) -> Self {
        Lexer {
            b,
            i: 0,
            out,
            in_backtick: false,
        }
    }

    /// The byte `off` ahead, or 0 past the end.
    fn peek(&self, off: usize) -> u8 {
        self.b.get(self.i + off).copied().unwrap_or(0)
    }

    fn script(&mut self, end: End) {
        let outer_backtick = self.in_backtick;
        if end == End::Backtick {
            self.in_backtick = true;
        }
        let mut cmd_pos = true;
        let mut cmd_name: Option<Vec<u8>> = None;
        let mut argc = 0usize;
        let mut depth = 0usize;
        let mut cases: Vec<CaseState> = Vec::new();
        let mut heredocs: Vec<Heredoc> = Vec::new();
        while self.i < self.b.len() {
            let c = self.b[self.i];
            let in_patterns = cases.last() == Some(&CaseState::Patterns);
            match c {
                b' ' | b'\t' | b'\r' => self.i += 1,
                b'\\' if self.peek(1) == b'\n' => self.i += 2,
                b'\n' => {
                    self.i += 1;
                    self.flush_heredocs(&mut heredocs);
                    if !in_patterns {
                        cmd_pos = true;
                    }
                }
                b'#' => {
                    while self.i < self.b.len() && self.b[self.i] != b'\n' {
                        self.i += 1;
                    }
                }
                b';' => {
                    let arm_end = matches!(self.peek(1), b';' | b'&');
                    self.i += if arm_end { 2 } else { 1 };
                    if arm_end && cases.last() == Some(&CaseState::Body) {
                        *cases.last_mut().expect("a case") = CaseState::Patterns;
                    } else if !in_patterns {
                        cmd_pos = true;
                    }
                }
                b'&' if self.peek(1) == b'>' => self.redirect(&mut heredocs),
                b'&' => {
                    self.i += if self.peek(1) == b'&' { 2 } else { 1 };
                    cmd_pos = true;
                }
                b'|' => {
                    self.i += if self.peek(1) == b'|' { 2 } else { 1 };
                    if !in_patterns {
                        cmd_pos = true;
                    }
                }
                b'(' => {
                    self.i += 1;
                    if !in_patterns {
                        depth += 1;
                        cmd_pos = true;
                    }
                }
                b')' => {
                    self.i += 1;
                    if in_patterns {
                        *cases.last_mut().expect("a case") = CaseState::Body;
                        cmd_pos = true;
                    } else if depth > 0 {
                        depth -= 1;
                        cmd_pos = true;
                    } else if end == End::Paren {
                        break;
                    } else {
                        cmd_pos = true;
                    }
                }
                b'<' | b'>' => self.redirect(&mut heredocs),
                b'`' if end == End::Backtick => {
                    self.i += 1;
                    break;
                }
                _ => {
                    let start = self.i;
                    let is_assign = self.assignment_at(start);
                    let word = self.word(is_assign);
                    if self.i == start {
                        self.i += 1;
                        continue;
                    }
                    // `2>file`: the digits are a file descriptor, not a word.
                    if !word.dynamic
                        && !word.lit.is_empty()
                        && word.lit.iter().all(u8::is_ascii_digit)
                        && matches!(self.peek(0), b'<' | b'>')
                    {
                        continue;
                    }
                    let literal: Option<&[u8]> = (!word.dynamic).then_some(&word.lit[..]);
                    match cases.last().copied() {
                        Some(CaseState::Header) => {
                            if literal == Some(b"in") {
                                *cases.last_mut().expect("a case") = CaseState::Patterns;
                            }
                            continue;
                        }
                        Some(CaseState::Patterns) => {
                            if literal == Some(b"esac") {
                                cases.pop();
                                cmd_pos = false;
                            }
                            continue;
                        }
                        _ => {}
                    }
                    if is_assign && cmd_pos {
                        continue;
                    }
                    if !cmd_pos {
                        if cmd_name.as_deref() == Some(b"trap") && argc == 0 {
                            if let Some(text) = &word.single {
                                scan_text(text, self.out);
                            }
                        }
                        argc += 1;
                        continue;
                    }
                    match literal {
                        Some(
                            b"if" | b"then" | b"else" | b"elif" | b"while" | b"until" | b"do"
                            | b"{" | b"!" | b"time",
                        ) => continue,
                        Some(b"fi" | b"done" | b"}") => {
                            cmd_pos = false;
                            continue;
                        }
                        Some(b"esac") => {
                            cases.pop();
                            cmd_pos = false;
                            continue;
                        }
                        Some(b"case") => {
                            cases.push(CaseState::Header);
                            cmd_pos = false;
                            continue;
                        }
                        Some(b"for" | b"select") => {
                            cmd_pos = false;
                            continue;
                        }
                        _ => {}
                    }
                    cmd_name = literal.filter(|w| !w.is_empty()).map(<[u8]>::to_vec);
                    if let Some(name) = &cmd_name {
                        self.out.insert(String::from_utf8_lossy(name).into_owned());
                    }
                    argc = 0;
                    cmd_pos = false;
                }
            }
        }
        self.in_backtick = outer_backtick;
    }

    /// `NAME=` or `NAME+=` at `at`.
    fn assignment_at(&self, at: usize) -> bool {
        let rest = &self.b[at..];
        let Some(first) = rest.first() else {
            return false;
        };
        if !(first.is_ascii_alphabetic() || *first == b'_') {
            return false;
        }
        let name_len = rest
            .iter()
            .take_while(|c| c.is_ascii_alphanumeric() || **c == b'_')
            .count();
        matches!(&rest[name_len..], [b'=', ..] | [b'+', b'=', ..])
    }

    /// One shell word, consuming every quote and expansion it holds.
    fn word(&mut self, is_assign: bool) -> Word {
        let mut word = Word::default();
        let start = self.i;
        let mut seen_eq = false;
        while self.i < self.b.len() {
            let c = self.b[self.i];
            if WORD_END.contains(&c) || (c == b'`' && self.in_backtick) {
                break;
            }
            match c {
                b'\\' => {
                    if self.peek(1) == b'\n' {
                        self.i += 2;
                    } else {
                        word.dynamic = true;
                        if let Some(&escaped) = self.b.get(self.i + 1) {
                            word.lit.push(escaped);
                        }
                        self.i += 2;
                    }
                }
                b'\'' => {
                    let content = self.skip_single();
                    word.dynamic = true;
                    word.lit.extend_from_slice(&content);
                    let at_end = self.i >= self.b.len() || WORD_END.contains(&self.b[self.i]);
                    if start + content.len() + 2 == self.i && at_end {
                        word.single = Some(content);
                    }
                }
                b'"' => {
                    self.i += 1;
                    self.dquote();
                    word.dynamic = true;
                }
                b'$' => {
                    self.dollar();
                    word.dynamic = true;
                }
                b'`' => {
                    self.i += 1;
                    self.script(End::Backtick);
                    word.dynamic = true;
                }
                b'=' if is_assign && !seen_eq => {
                    seen_eq = true;
                    word.lit.push(b'=');
                    self.i += 1;
                    if self.peek(0) == b'(' {
                        self.i += 1;
                        self.skip_array();
                        word.dynamic = true;
                    }
                }
                _ => {
                    word.lit.push(c);
                    self.i += 1;
                }
            }
        }
        word
    }

    /// At an opening `'`: consume through the closing one and return the content.
    fn skip_single(&mut self) -> Vec<u8> {
        self.i += 1;
        let from = self.i;
        while self.i < self.b.len() && self.b[self.i] != b'\'' {
            self.i += 1;
        }
        let content = self.b[from..self.i].to_vec();
        if self.i < self.b.len() {
            self.i += 1;
        }
        content
    }

    /// Inside double quotes (the opening one consumed): through the closing one.
    fn dquote(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'"' => {
                    self.i += 1;
                    return;
                }
                b'\\' => self.i += 2,
                b'$' => self.dollar(),
                b'`' => {
                    self.i += 1;
                    self.script(End::Backtick);
                }
                _ => self.i += 1,
            }
        }
    }

    /// At a `$`: a command substitution is scanned, a `${...}` expansion and an
    /// arithmetic expansion skipped (still scanning any substitution inside them).
    fn dollar(&mut self) {
        match self.peek(1) {
            b'(' if self.peek(2) == b'(' => self.arith(),
            b'(' => {
                self.i += 2;
                self.script(End::Paren);
            }
            b'{' => {
                self.i += 2;
                self.skip_braced();
            }
            b'#' | b'?' | b'$' | b'!' | b'@' | b'*' | b'-' => self.i += 2,
            c if c.is_ascii_digit() => self.i += 2,
            c if c.is_ascii_alphabetic() || c == b'_' => {
                self.i += 1;
                while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                    self.i += 1;
                }
            }
            _ => self.i += 1,
        }
    }

    /// At `$((`: through the matching `))`.
    fn arith(&mut self) {
        self.i += 3;
        let mut depth = 2usize;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'(' => {
                    depth += 1;
                    self.i += 1;
                }
                b')' => {
                    depth -= 1;
                    self.i += 1;
                    if depth == 0 {
                        return;
                    }
                }
                b'$' => self.dollar(),
                b'\'' => {
                    self.skip_single();
                }
                b'"' => {
                    self.i += 1;
                    self.dquote();
                }
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
    }

    /// After `${`: through the matching `}`.
    fn skip_braced(&mut self) {
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'}' => {
                    self.i += 1;
                    return;
                }
                b'\\' => self.i += 2,
                b'\'' => {
                    self.skip_single();
                }
                b'"' => {
                    self.i += 1;
                    self.dquote();
                }
                b'$' => self.dollar(),
                b'`' => {
                    self.i += 1;
                    self.script(End::Backtick);
                }
                _ => self.i += 1,
            }
        }
    }

    /// After `NAME=(`: through the matching `)` of an array assignment.
    fn skip_array(&mut self) {
        let mut depth = 1usize;
        while self.i < self.b.len() {
            match self.b[self.i] {
                b'(' => {
                    depth += 1;
                    self.i += 1;
                }
                b')' => {
                    depth -= 1;
                    self.i += 1;
                    if depth == 0 {
                        return;
                    }
                }
                b'\'' => {
                    self.skip_single();
                }
                b'"' => {
                    self.i += 1;
                    self.dquote();
                }
                b'$' => self.dollar(),
                b'\\' => self.i += 2,
                _ => self.i += 1,
            }
        }
    }

    /// At `<`, `>` or `&>`: the operator and its target word; a `<<` also queues
    /// its here-document for the next newline.
    fn redirect(&mut self, heredocs: &mut Vec<Heredoc>) {
        let c = self.b[self.i];
        let mut here = false;
        let mut strip_tabs = false;
        if c == b'&' {
            self.i += 2;
            if self.peek(0) == b'>' {
                self.i += 1;
            }
        } else if c == b'<' && self.peek(1) == b'<' {
            if self.peek(2) == b'<' {
                self.i += 3;
            } else {
                self.i += 2;
                here = true;
                if self.peek(0) == b'-' {
                    self.i += 1;
                    strip_tabs = true;
                }
            }
        } else if (c == b'>' && matches!(self.peek(1), b'>' | b'&' | b'|'))
            || (c == b'<' && matches!(self.peek(1), b'&' | b'>'))
        {
            self.i += 2;
        } else {
            self.i += 1;
        }
        while matches!(self.peek(0), b' ' | b'\t') {
            self.i += 1;
        }
        let target = self.word(false);
        if here {
            heredocs.push(Heredoc {
                delim: target.lit,
                quoted: target.dynamic,
                strip_tabs,
            });
        }
    }

    /// At the start of the line after a newline: consume the queued here-document
    /// bodies and scan the ones with an unquoted delimiter for substitutions.
    fn flush_heredocs(&mut self, heredocs: &mut Vec<Heredoc>) {
        let all = self.b;
        for doc in std::mem::take(heredocs) {
            let mut body: Vec<u8> = Vec::new();
            while self.i < all.len() {
                let end = all[self.i..]
                    .iter()
                    .position(|&b| b == b'\n')
                    .map_or(all.len(), |p| self.i + p);
                let line = &all[self.i..end];
                self.i = (end + 1).min(all.len());
                let compared = if doc.strip_tabs {
                    let tabs = line.iter().take_while(|&&b| b == b'\t').count();
                    &line[tabs..]
                } else {
                    line
                };
                if compared == doc.delim.as_slice() {
                    break;
                }
                body.extend_from_slice(line);
                body.push(b'\n');
            }
            if !doc.quoted {
                let mut inner = Lexer::new(&body, &mut *self.out);
                while inner.i < inner.b.len() {
                    match inner.b[inner.i] {
                        b'\\' => inner.i += 2,
                        b'$' => inner.dollar(),
                        b'`' => {
                            inner.i += 1;
                            inner.script(End::Backtick);
                        }
                        _ => inner.i += 1,
                    }
                }
            }
        }
    }
}
