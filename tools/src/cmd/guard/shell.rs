//! The guard's shell model: a port of `ShellParser`, `brace_expand` and
//! `parse_commands` (git-push-guard.py L438-872).
//!
//! The parser splits shell text into simple commands, lists of words with quoting
//! removed. Command substitutions, process substitutions and backticks are parsed
//! recursively, and their commands come before the command that contains them.
//! Heredoc and here-string bodies are attached to their command as data, as are
//! `< file` redirections, the targets of the other redirections, and whether the
//! command reads a pipe. `case` patterns are skipped; their bodies are commands.
//!
//! The port keeps Python's indexing: the text is a slice of `char`s, as a Python
//! `str` is a sequence of code points, and every quirk of the state machine is
//! kept, because the guard's decisions follow from it. For example, the flags of
//! a word taken as a redirection's descriptor (`2>`) stay set for the next word,
//! and an operator with nothing after it (`echo >`) leaves the next word of the
//! text a redirection target, even in the next command.
//!
//! One accepted divergence: `$'\uD800'` (an escape of a UTF-16 surrogate) gives a
//! lone surrogate in Python, which a Rust `String` cannot hold; the port puts
//! U+FFFD there.

use std::collections::VecDeque;
use std::fmt;

/// How deep `$(...)`, backticks, `sh -c`, `eval`, scripts and aliases may nest
/// (`MAX_DEPTH`).
pub const MAX_DEPTH: usize = 8;

/// Words that are shell syntax, not a command (`RESERVED`).
pub const RESERVED: [&str; 17] = [
    "{", "}", "!", "if", "then", "else", "elif", "fi", "do", "done", "while", "until", "time",
    "in", "case", "esac", "function",
];

/// The longest word list brace expansion produces (`brace_expand`'s `limit`).
const BRACE_LIMIT: usize = 64;

/// How many `brace_expand` calls Python can nest before it raises
/// `RecursionError`, which `decide` handles like a `ParseError`. Python's
/// recursion limit (1000) counts every frame on the stack, so the bound depends
/// on how deep the hook is when `ShellParser.end_word` calls `brace_expand`.
/// This is the bound for a word of the hook's top-level Bash command, measured
/// on CPython 3.13: `echo` + 992 `{a,b}` groups parses, and 993 groups raise.
/// Words parsed deeper in the hook (`$(...)`, `bash -c`) hit Python's limit a
/// few groups earlier (984 to 992 groups measured), and Python 3.9 fails one
/// group earlier, with ` while calling a Python object` after the message.
/// Those bands are accepted divergences (`guard::protocol`). The bound also
/// keeps the recursion off the end of Rust's own stack.
const BRACE_FRAMES: usize = 993;

/// Text the guard cannot parse (`ParseError`). The message is Python's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError(pub &'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for ParseError {}

/// A word's expansion state (`Command.expanded`): no `$` or backtick expansion
/// outside single quotes, one outside any quotes (`BARE_EXPANSION`: the shell
/// splits its result into words), or only inside double quotes
/// (`QUOTED_EXPANSION`: it stays one word).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Expanded {
    #[default]
    No,
    Bare,
    Quoted,
}

impl Expanded {
    /// Python's truthiness of the state: `False` is the only false one.
    pub fn any(self) -> bool {
        self != Expanded::No
    }

    /// Python's `self or other`.
    fn or(self, other: Expanded) -> Expanded {
        if self.any() {
            self
        } else {
            other
        }
    }
}

/// One simple command (`Command`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Command {
    pub words: Vec<String>,
    /// Per word, its expansion state.
    pub expanded: Vec<Expanded>,
    /// Heredoc and here-string bodies, in the order they are read.
    pub heredocs: Vec<String>,
    /// Targets of `<` and `0<`.
    pub stdin_files: Vec<String>,
    /// Targets of every other redirection (`>`, `>>`, `>|`, `&>`, `<>`, `n>&m`):
    /// the files the command may write.
    pub out_files: Vec<String>,
    pub piped_in: bool,
    /// The command's own source text.
    pub src: String,
    /// The source text of each `$( )`, `$(( ))`, backtick, `<( )` and `>( )` body
    /// in the command (the words keep only `$(...)` or `` `...` ``).
    pub bodies: Vec<String>,
}

/// Where a `case` is: reading its subject, a pattern, or a body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Case {
    Subject,
    Pattern,
    Body,
}

/// The command a pending heredoc belongs to: the one being read, or one already
/// in the output (by index, as the output only grows).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Owner {
    Current,
    Out(usize),
}

/// `parse_commands(text, depth)`: the simple commands of `text`, inner commands
/// first.
pub fn parse_commands(text: &str, depth: usize) -> Result<Vec<Command>, ParseError> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    Parser::new(&chars, 0, &mut out, false, depth)?.parse()?;
    Ok(out)
}

/// `ShellParser`.
struct Parser<'s, 'o> {
    s: &'s [char],
    i: usize,
    out: &'o mut Vec<Command>,
    nested: bool,
    depth: usize,
    parens: usize,
    cmd: Command,
    word: Option<String>,
    quoted: bool,
    word_expanded: Expanded,
    out_next: bool,
    stdin_next: bool,
    herestring_next: bool,
    pending: VecDeque<(String, bool, Owner)>,
    brace: bool,
    case: Vec<Case>,
    start: usize,
}

impl<'s, 'o> Parser<'s, 'o> {
    fn new(
        s: &'s [char],
        start: usize,
        out: &'o mut Vec<Command>,
        nested: bool,
        depth: usize,
    ) -> Result<Parser<'s, 'o>, ParseError> {
        if depth > MAX_DEPTH {
            return Err(ParseError("nesting too deep"));
        }
        Ok(Parser {
            s,
            i: start,
            out,
            nested,
            depth,
            parens: 0,
            cmd: Command::default(),
            word: None,
            quoted: false,
            word_expanded: Expanded::No,
            out_next: false,
            stdin_next: false,
            herestring_next: false,
            pending: VecDeque::new(),
            brace: false,
            case: Vec::new(),
            start,
        })
    }

    fn n(&self) -> usize {
        self.s.len()
    }

    /// `s[a:b]`, with Python's clamping.
    fn slice(&self, a: usize, b: usize) -> String {
        let b = b.min(self.n());
        if a >= b {
            String::new()
        } else {
            self.s[a..b].iter().collect()
        }
    }

    fn peek(&self, k: usize) -> Option<char> {
        self.s.get(self.i + k).copied()
    }

    /// `s.startswith(prefix, i)`.
    fn starts_with(&self, prefix: &str, at: usize) -> bool {
        let want: Vec<char> = prefix.chars().collect();
        self.s.get(at..).is_some_and(|rest| rest.starts_with(&want))
    }

    /// `s.find(c, from)`.
    fn find(&self, c: char, from: usize) -> Option<usize> {
        self.s
            .get(from..)
            .and_then(|rest| rest.iter().position(|&x| x == c))
            .map(|k| from + k)
    }

    fn add(&mut self, text: &str, quoted: bool) {
        self.word.get_or_insert_with(String::new).push_str(text);
        self.quoted = self.quoted || quoted;
    }

    fn end_word(&mut self) -> Result<(), ParseError> {
        let Some(word) = self.word.take() else {
            return Ok(());
        };
        let brace = std::mem::take(&mut self.brace);
        let quoted = std::mem::take(&mut self.quoted);
        let expanded = std::mem::take(&mut self.word_expanded);
        if self.out_next {
            self.out_next = false;
            self.cmd.out_files.push(word);
        } else if self.stdin_next {
            self.stdin_next = false;
            self.cmd.stdin_files.push(word);
        } else if self.herestring_next {
            self.herestring_next = false;
            self.cmd.heredocs.push(word);
        } else if !self.case_keyword(&word, quoted) {
            let words = if brace {
                brace_expand(&word)?
            } else {
                vec![word]
            };
            self.cmd
                .expanded
                .extend(std::iter::repeat_n(expanded, words.len()));
            self.cmd.words.extend(words);
        }
        Ok(())
    }

    /// Tracks `case WORD in PATTERN) BODY ;; ... esac`. True when the word
    /// belongs to the case syntax rather than to a command.
    fn case_keyword(&mut self, word: &str, quoted: bool) -> bool {
        let state = self.case.last().copied();
        let at_start = self
            .cmd
            .words
            .iter()
            .all(|w| RESERVED.contains(&w.as_str()));
        if state == Some(Case::Subject) {
            if word == "in" && !quoted {
                *self.case.last_mut().expect("a case") = Case::Pattern;
            }
            return true;
        }
        if state == Some(Case::Pattern) {
            if word == "esac" && !quoted {
                self.case.pop();
            }
            return true;
        }
        if quoted || !at_start {
            return false;
        }
        if word == "case" {
            self.case.push(Case::Subject);
            return true;
        }
        if state == Some(Case::Body) && word == "esac" {
            self.case.pop();
            return true;
        }
        false
    }

    fn end_command(&mut self) -> Result<(), ParseError> {
        self.end_word()?;
        self.cmd.src = self.slice(self.start, self.i);
        self.start = self.i;
        let waiting = self.pending.iter().any(|p| p.2 == Owner::Current);
        let cmd = std::mem::take(&mut self.cmd);
        if !cmd.words.is_empty()
            || !cmd.heredocs.is_empty()
            || !cmd.stdin_files.is_empty()
            || waiting
        {
            let at = self.out.len();
            for p in self.pending.iter_mut() {
                if p.2 == Owner::Current {
                    p.2 = Owner::Out(at);
                }
            }
            self.out.push(cmd);
        }
        Ok(())
    }

    fn read_heredocs(&mut self) {
        while let Some((delim, strip_tabs, owner)) = self.pending.pop_front() {
            let mut body = Vec::new();
            while self.i < self.n() {
                let j = self.find('\n', self.i);
                let line = match j {
                    Some(j) => self.slice(self.i, j),
                    None => self.slice(self.i, self.n()),
                };
                self.i = j.map_or(self.n(), |j| j + 1);
                let cmp = if strip_tabs {
                    line.trim_start_matches('\t')
                } else {
                    &line
                };
                if cmp == delim {
                    break;
                }
                body.push(line);
            }
            let text = body.join("\n");
            match owner {
                Owner::Current => self.cmd.heredocs.push(text),
                Owner::Out(k) => self.out[k].heredocs.push(text),
            }
        }
    }

    fn read_heredoc_delim(&mut self) -> Result<String, ParseError> {
        while self.i < self.n() && matches!(self.s[self.i], ' ' | '\t') {
            self.i += 1;
        }
        let mut buf = String::new();
        let mut any = false;
        while self.i < self.n() && !" \t\n;&|()<>".contains(self.s[self.i]) {
            let c = self.s[self.i];
            any = true;
            if c == '\'' || c == '"' {
                let Some(j) = self.find(c, self.i + 1) else {
                    return Err(ParseError("unterminated heredoc delimiter"));
                };
                buf.push_str(&self.slice(self.i + 1, j));
                self.i = j + 1;
            } else if c == '\\' {
                buf.push_str(&self.slice(self.i + 1, self.i + 2));
                self.i += 2;
            } else {
                buf.push(c);
                self.i += 1;
            }
        }
        if !any {
            return Err(ParseError("missing heredoc delimiter"));
        }
        Ok(buf)
    }

    fn read_single(&mut self) -> Result<String, ParseError> {
        let Some(j) = self.find('\'', self.i + 1) else {
            return Err(ParseError("unterminated single quote"));
        };
        let text = self.slice(self.i + 1, j);
        self.i = j + 1;
        Ok(text)
    }

    fn read_double(&mut self) -> Result<String, ParseError> {
        let mut i = self.i + 1;
        let mut buf = String::new();
        while i < self.n() {
            let c = self.s[i];
            if c == '"' {
                self.i = i + 1;
                return Ok(buf);
            }
            if c == '\\' && i + 1 < self.n() && "$`\"\\\n".contains(self.s[i + 1]) {
                if self.s[i + 1] != '\n' {
                    buf.push(self.s[i + 1]);
                }
                i += 2;
            } else if c == '$' && i + 1 < self.n() && self.s[i + 1] == '(' {
                self.i = i;
                buf.push_str(self.read_substitution(2)?);
                self.word_expanded = self.word_expanded.or(Expanded::Quoted);
                i = self.i;
            } else if c == '`' {
                self.i = i;
                buf.push_str(self.read_backtick()?);
                self.word_expanded = self.word_expanded.or(Expanded::Quoted);
                i = self.i;
            } else {
                if c == '$' && expansion_start(self.s.get(i + 1).copied()) {
                    self.word_expanded = self.word_expanded.or(Expanded::Quoted);
                }
                buf.push(c);
                i += 1;
            }
        }
        Err(ParseError("unterminated double quote"))
    }

    fn read_ansi_c(&mut self) -> Result<String, ParseError> {
        let mut i = self.i + 2;
        let mut buf = String::new();
        while i < self.n() {
            let c = self.s[i];
            if c == '\'' {
                self.i = i + 1;
                return Ok(buf);
            }
            if c != '\\' || i + 1 >= self.n() {
                buf.push(c);
                i += 1;
                continue;
            }
            let esc = self.s[i + 1];
            i += 2;
            if let Some(simple) = ansi_simple(esc) {
                buf.push(simple);
                continue;
            }
            let hex_max = match esc {
                'x' => 2,
                'u' => 4,
                'U' => 8,
                _ => 0,
            };
            if hex_max > 0 {
                let digits = self.run_of(i, hex_max, |c| c.is_ascii_hexdigit());
                if digits.is_empty() {
                    buf.push('\\');
                    buf.push(esc);
                } else {
                    let value = u32::from_str_radix(&digits, 16).expect("hex digits");
                    if (0xD800..=0xDFFF).contains(&value) {
                        buf.push('\u{FFFD}');
                    } else if let Some(ch) = char::from_u32(value) {
                        buf.push(ch);
                    }
                    i += digits.chars().count();
                }
            } else if ('0'..='7').contains(&esc) {
                let digits = self.run_of(i, 2, |c| ('0'..='7').contains(&c));
                let value =
                    u32::from_str_radix(&format!("{esc}{digits}"), 8).expect("octal") & 0xFF;
                buf.push(char::from_u32(value).expect("a byte value is a char"));
                i += digits.chars().count();
            } else if esc == 'c' && i < self.n() {
                let value = u32::from(self.s[i]) & 0x1F;
                buf.push(char::from_u32(value).expect("a control character"));
                i += 1;
            } else {
                buf.push('\\');
                buf.push(esc);
            }
        }
        Err(ParseError("unterminated $'...'"))
    }

    /// The longest run (at most `max`) of characters from `at` that `ok` accepts.
    fn run_of(&self, at: usize, max: usize, ok: impl Fn(char) -> bool) -> String {
        self.s
            .get(at..)
            .unwrap_or(&[])
            .iter()
            .take(max)
            .take_while(|&&c| ok(c))
            .collect()
    }

    /// At `$(`, `<(` or `>(`: parse the body as nested commands.
    fn read_substitution(&mut self, skip: usize) -> Result<&'static str, ParseError> {
        if skip == 2 && self.starts_with("$((", self.i) {
            let mut depth = 0i64;
            let mut j = self.i + 1;
            while j < self.n() {
                if self.s[j] == '(' {
                    depth += 1;
                } else if self.s[j] == ')' {
                    depth -= 1;
                    if depth == 0 {
                        let body = self.slice(self.i + 3, j - 1);
                        self.cmd.bodies.push(body);
                        self.i = j + 1;
                        return Ok("$((...))");
                    }
                }
                j += 1;
            }
            return Err(ParseError("unterminated $(("));
        }
        let end =
            Parser::new(self.s, self.i + skip, &mut *self.out, true, self.depth + 1)?.parse()?;
        let body = self.slice(self.i + skip, end);
        self.cmd.bodies.push(body);
        self.i = end + 1;
        Ok("$(...)")
    }

    fn read_backtick(&mut self) -> Result<&'static str, ParseError> {
        let mut i = self.i + 1;
        let mut buf = String::new();
        while i < self.n() {
            let c = self.s[i];
            if c == '\\' && i + 1 < self.n() && "`$\\".contains(self.s[i + 1]) {
                buf.push(self.s[i + 1]);
                i += 2;
            } else if c == '`' {
                let inner: Vec<char> = buf.chars().collect();
                self.cmd.bodies.push(buf);
                Parser::new(&inner, 0, &mut *self.out, false, self.depth + 1)?.parse()?;
                self.i = i + 1;
                return Ok("`...`");
            } else {
                buf.push(c);
                i += 1;
            }
        }
        Err(ParseError("unterminated backtick"))
    }

    /// At `<`, `>` or `&>`: consume the operator. The next word is its target: a
    /// stdin file for `<` and `0<`, an output file otherwise, unless it is a
    /// process substitution.
    fn read_redirect(&mut self) -> Result<(), ParseError> {
        let mut fd = None;
        if self.word.as_deref().is_some_and(crate::pystr::is_digit) {
            fd = self.word.take();
        } else {
            self.end_word()?;
        }
        let c = self.peek(0);
        if matches!(c, Some('<' | '>')) && self.peek(1) == Some('(') {
            let text = self.read_substitution(2)?;
            self.add(text, false);
            self.end_word()?;
            return Ok(());
        }
        if c == Some('&') {
            self.i += 2;
            if self.peek(0) == Some('>') {
                self.i += 1;
            }
        } else {
            self.i += 1;
            // Python's `self.peek() in ">&|"` is also true past the end, where
            // peek() is "".
            let next = self.peek(0);
            if next.is_none_or(|n| ">&|".contains(n)) || (c == Some('<') && next == Some('>')) {
                self.i += 1;
            } else if c == Some('<') && fd.as_deref().is_none_or(|fd| fd == "0") {
                self.stdin_next = true;
                return Ok(());
            }
        }
        self.out_next = true;
        Ok(())
    }

    fn parse(&mut self) -> Result<usize, ParseError> {
        while self.i < self.n() {
            let c = self.s[self.i];
            if "|()".contains(c) && self.case.last() == Some(&Case::Pattern) {
                self.end_word()?; // may be `esac`, which closes the case
            }
            let state = self.case.last().copied();
            if matches!(c, ' ' | '\t' | '\r') {
                self.end_word()?;
                self.i += 1;
            } else if c == '\n' {
                self.end_command()?;
                self.i += 1;
                self.read_heredocs();
            } else if c == '\\' {
                if self.peek(1) != Some('\n') {
                    let next = self.peek(1).map(String::from).unwrap_or_default();
                    self.add(&next, true);
                }
                self.i += 2;
            } else if c == '\'' {
                let text = self.read_single()?;
                self.add(&text, true);
            } else if c == '"' {
                let text = self.read_double()?;
                self.add(&text, true);
            } else if c == '$' && self.peek(1) == Some('\'') {
                let text = self.read_ansi_c()?;
                self.add(&text, true);
            } else if c == '$' && self.peek(1) == Some('"') {
                self.i += 1; // $"..." is a translatable double-quoted string
            } else if c == '$' && self.peek(1) == Some('(') {
                let text = self.read_substitution(2)?;
                self.add(text, false);
                self.word_expanded = Expanded::Bare;
            } else if c == '`' {
                let text = self.read_backtick()?;
                self.add(text, false);
                self.word_expanded = Expanded::Bare;
            } else if c == '#' && self.word.is_none() {
                self.i = self.find('\n', self.i).unwrap_or(self.n());
            } else if self.starts_with("<<<", self.i) {
                self.end_word()?;
                self.i += 3;
                self.herestring_next = true;
            } else if self.starts_with("<<", self.i) {
                self.end_word()?;
                self.i += 2;
                let strip_tabs = self.peek(0) == Some('-');
                if strip_tabs {
                    self.i += 1;
                }
                let delim = self.read_heredoc_delim()?;
                self.pending.push_back((delim, strip_tabs, Owner::Current));
            } else if c == '<' || c == '>' || (c == '&' && self.peek(1) == Some('>')) {
                self.read_redirect()?;
            } else if state == Some(Case::Pattern) && (c == '|' || c == '(') {
                self.i += 1; // pattern alternatives and the optional "("
            } else if state == Some(Case::Pattern) && c == ')' {
                *self.case.last_mut().expect("a case") = Case::Body;
                self.i += 1;
            } else if c == ';' || c == '&' || c == '|' {
                let two = self.slice(self.i, self.i + 2);
                if state == Some(Case::Body) && (two == ";;" || two == ";&") {
                    self.end_command()?;
                    *self.case.last_mut().expect("a case") = Case::Pattern;
                    self.i += 2;
                    continue;
                }
                self.end_command()?;
                if c == '|' && two != "||" {
                    self.cmd.piped_in = true;
                }
                self.i += if matches!(two.as_str(), "&&" | "||" | ";;" | "|&" | ";&") {
                    2
                } else {
                    1
                };
            } else if c == '(' {
                self.end_command()?;
                self.parens += 1;
                self.i += 1;
            } else if c == ')' {
                self.end_command()?;
                if self.parens == 0 && self.nested {
                    return Ok(self.i);
                }
                self.parens = self.parens.saturating_sub(1);
                self.i += 1;
            } else {
                if c == '{' {
                    self.brace = true;
                } else if c == '$' && expansion_start(self.peek(1)) {
                    self.word_expanded = Expanded::Bare;
                }
                self.add(&c.to_string(), false);
                self.i += 1;
            }
        }
        if self.nested {
            return Err(ParseError("unterminated $("));
        }
        self.end_command()?;
        Ok(self.i)
    }
}

/// `EXPANSION_START.match(s, i)`: a `$` followed by this starts an expansion.
fn expansion_start(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_alphanumeric() || "_{@*#?$!-".contains(c))
}

/// The one-character escapes of `$'...'`.
fn ansi_simple(esc: char) -> Option<char> {
    Some(match esc {
        'n' => '\n',
        't' => '\t',
        'r' => '\r',
        'a' => '\u{7}',
        'b' => '\u{8}',
        'e' | 'E' => '\u{1b}',
        'f' => '\u{c}',
        'v' => '\u{b}',
        '\\' => '\\',
        '\'' => '\'',
        '"' => '"',
        '?' => '?',
        _ => return None,
    })
}

/// Bash brace expansion for comma lists (`pu{sh,ll}`), so that
/// `git {push,origin,main}` is seen as `git push origin main` (`brace_expand`, at
/// most 64 words). Nesting deeper than `BRACE_FRAMES` calls is Python's
/// `RecursionError`.
pub fn brace_expand(word: &str) -> Result<Vec<String>, ParseError> {
    let chars: Vec<char> = word.chars().collect();
    expand(&chars, BRACE_LIMIT, 1)
}

/// `brace_expand`'s body; `frames` counts the calls on the stack, this one
/// included.
fn expand(word: &[char], limit: usize, frames: usize) -> Result<Vec<String>, ParseError> {
    if frames > BRACE_FRAMES {
        return Err(ParseError("maximum recursion depth exceeded"));
    }
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut commas = Vec::new();
    for (i, &c) in word.iter().enumerate() {
        if c == '{' {
            if depth == 0 {
                start = i;
                commas.clear();
            }
            depth += 1;
        } else if c == '}' && depth > 0 {
            depth -= 1;
            if depth == 0 && !commas.is_empty() {
                let prefix = &word[..start];
                let suffix = &word[i + 1..];
                let mut bounds = vec![start];
                bounds.extend(&commas);
                bounds.push(i);
                let mut out = Vec::new();
                for pair in bounds.windows(2) {
                    let (a, b) = (pair[0], pair[1]);
                    for tail in expand(suffix, limit, frames + 1)? {
                        let mut combined = prefix.to_vec();
                        combined.extend_from_slice(&word[a + 1..b]);
                        combined.extend(tail.chars());
                        out.extend(expand(&combined, limit, frames + 1)?);
                        if out.len() >= limit {
                            out.truncate(limit);
                            return Ok(out);
                        }
                    }
                }
                return Ok(out);
            }
        } else if c == ',' && depth == 1 {
            commas.push(i);
        }
    }
    Ok(vec![word.iter().collect()])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// Python's parse of every case in `tests/fixtures/guard/shell.jsonl`, recorded
    /// by `guard_parity`'s `record_shell` from the guard's own `parse_commands`.
    const FIXTURE: &str = include_str!("../../../tests/fixtures/guard/shell.jsonl");

    fn strings(v: &Value) -> Vec<String> {
        v.as_array()
            .expect("a list")
            .iter()
            .map(|s| s.as_str().expect("a string").to_string())
            .collect()
    }

    fn expanded(v: &Value) -> Vec<Expanded> {
        v.as_array()
            .expect("a list")
            .iter()
            .map(|e| match e {
                Value::Bool(false) => Expanded::No,
                Value::String(s) if s == "bare" => Expanded::Bare,
                Value::String(s) if s == "quoted" => Expanded::Quoted,
                other => panic!("an expansion state: {other}"),
            })
            .collect()
    }

    fn command(v: &Value) -> Command {
        Command {
            words: strings(&v["words"]),
            expanded: expanded(&v["expanded"]),
            heredocs: strings(&v["heredocs"]),
            stdin_files: strings(&v["stdin_files"]),
            out_files: strings(&v["out_files"]),
            piped_in: v["piped_in"].as_bool().expect("piped_in"),
            src: v["src"].as_str().expect("src").to_string(),
            bodies: strings(&v["bodies"]),
        }
    }

    #[test]
    fn parses_like_python() {
        let mut failures = Vec::new();
        let mut cases = 0;
        for line in FIXTURE.lines() {
            let case: Value = serde_json::from_str(line).expect("a JSON line");
            let text = case["text"].as_str().expect("text");
            let depth = case["depth"].as_u64().expect("depth") as usize;
            let want = match case.get("error") {
                Some(e) => Err(e.as_str().expect("an error message").to_string()),
                None => Ok(case["commands"]
                    .as_array()
                    .expect("commands")
                    .iter()
                    .map(command)
                    .collect::<Vec<_>>()),
            };
            let got = parse_commands(text, depth).map_err(|e| e.0.to_string());
            cases += 1;
            if got != want {
                failures.push(format!(
                    "{text:?} (depth {depth}):\n  python: {want:?}\n  rust:   {got:?}"
                ));
            }
        }
        assert!(cases > 1000, "only {cases} cases: the fixture lost cases");
        assert!(
            failures.is_empty(),
            "{} of {cases} parses differ:\n{}",
            failures.len(),
            failures.join("\n")
        );
    }

    #[test]
    fn a_surrogate_escape_becomes_the_replacement_character() {
        // Python keeps a lone surrogate here (the module doc's one divergence).
        let cmds = parse_commands("echo $'a\\uD800b\\UDFFF'", 0).expect("parses");
        assert_eq!(cmds[0].words, ["echo", "a\u{FFFD}b\u{FFFD}"]);
    }

    #[test]
    fn brace_expansion_matches_python() {
        let cases: [(&str, &[&str]); 7] = [
            ("pu{sh,ll}", &["push", "pull"]),
            ("{a,b}{c,d}", &["ac", "ad", "bc", "bd"]),
            ("{a}", &["{a}"]),
            ("x{a,{b,c}}y", &["xay", "xby", "xcy"]),
            ("{,}", &["", ""]),
            ("}{a,b", &["}{a,b"]),
            ("a{b,c", &["a{b,c"]),
        ];
        for (word, want) in cases {
            assert_eq!(brace_expand(word).unwrap(), want, "brace_expand({word:?})");
        }
        let many = "{a,b}".repeat(7);
        assert_eq!(brace_expand(&many).unwrap().len(), 64);
    }
}
