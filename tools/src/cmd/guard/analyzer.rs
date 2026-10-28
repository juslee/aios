//! The shell level of the analysis (`Analyzer`, git-push-guard.py L1106-1555
//! and L1814-1845, L2005-2051, L2565-2586, L3169-3221): it walks the commands of
//! a text, tracks variables, `cd`/`pushd` and the state earlier commands leave,
//! and hands each command to the rule that reads it (git and gh here, the sed
//! and team rules in their own modules). Shell strings (`sh -c`, `eval`, `env
//! -S`, tmux, `screen -X stuff`), script files and git aliases are analysed as
//! text of their own, one level deeper.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::rc::Rc;
use std::sync::LazyLock;

use regex::Regex;

use super::pattern::{self, re, ASSIGNMENT, AWK_EXEC, MENTIONS_GIT, MENTIONS_TEAM, PID_FINDER};
use super::pyos::{self, Fault, ReadError};
use super::repo::{Repo, Repos};
use super::sed_rule::{run_positions, SED_NAMES};
use super::shell::{parse_commands, Command, Expanded, ParseError, MAX_DEPTH, RESERVED};
use super::shlex;
use super::verdict::{self, Tag, Verdict};

/// The largest script, body or request file the guard reads (`FILE_LIMIT`).
pub const FILE_LIMIT: u64 = 256 * 1024;

pub const SHELLS: [&str; 8] = [
    "sh", "bash", "zsh", "dash", "ksh", "mksh", "fish", "busybox",
];
pub const INTERPRETERS: [&str; 14] = [
    "awk",
    "gawk",
    "nawk",
    "mawk",
    "perl",
    "python",
    "python3",
    "ruby",
    "node",
    "osascript",
    "php",
    "lua",
    "tclsh",
    "expect",
];
/// Commands that never execute their arguments.
pub const NON_EXEC: [&str; 35] = [
    "echo", "printf", "grep", "egrep", "fgrep", "rg", "ag", "ack", "man", "which", "type",
    "whereis", "whatis", "apropos", "info", "cat", "head", "tail", "less", "more", "wc", "ls",
    "file", "stat", "test", "[", "[[", "true", "false", ":", "jq", "diff", "cmp", "basename",
    "dirname",
];
pub const ARG_WRAPPERS: [&str; 3] = ["xargs", "parallel", "find"];
/// Commands that run their arguments as a command line.
pub const WRAPPERS: [&str; 24] = [
    "xargs",
    "parallel",
    "find",
    "env",
    "command",
    "builtin",
    "exec",
    "nohup",
    "nice",
    "timeout",
    "gtimeout",
    "time",
    "stdbuf",
    "sudo",
    "doas",
    "caffeinate",
    "script",
    "unbuffer",
    "chronic",
    "flock",
    "rustup",
    "arch",
    "setsid",
    "screen",
];
pub const AWK_NAMES: [&str; 4] = ["awk", "gawk", "nawk", "mawk"];
/// Key names tmux send-keys types as keys, not text.
const TMUX_KEYS: [&str; 9] = [
    "Enter", "C-m", "C-j", "KPEnter", "Space", "Tab", "Escape", "C-c", "C-d",
];

/// Who runs the command: `agent_id` and `agent_type` from the payload of a
/// subagent (any JSON value; `None` when absent or null), `team` from
/// `AIOS_TEAM`, `cwd` from the payload.
#[derive(Debug, Clone, Default)]
pub struct Ctx {
    pub agent_id: Option<serde_json::Value>,
    pub agent_type: Option<serde_json::Value>,
    pub team: String,
    pub cwd: String,
}

/// Why an analysis stopped: text it cannot parse (`ParseError`), or a Python
/// exception that escapes to `main()`.
#[derive(Debug)]
pub enum Stop {
    Parse(ParseError),
    Fault(Fault),
}

impl From<ParseError> for Stop {
    fn from(e: ParseError) -> Stop {
        Stop::Parse(e)
    }
}

impl From<Fault> for Stop {
    fn from(f: Fault) -> Stop {
        Stop::Fault(f)
    }
}

pub type Flow = Result<(), Stop>;

/// The state one text's commands share (Python's `state` dict).
#[derive(Debug, Clone)]
pub struct State {
    pub cwd: String,
    pub vars: HashMap<String, String>,
    pub text: String,
    pub script: bool,
    pub pid_vars: HashSet<String>,
}

pub struct Analyzer<'c> {
    pub verdict: Verdict,
    pub ctx: &'c Ctx,
    pub repos: Repos,
    /// git subcommands that may change config.
    pub config_changed: BTreeSet<String>,
    /// git subcommands that may switch branches.
    pub head_changed: BTreeSet<String>,
    /// A commit, merge, reset, ... ran earlier.
    pub commits_changed: bool,
    /// Variables assigned or exported earlier.
    pub global_env: HashSet<String>,
    /// The names each enclosing command assigns for itself.
    pub env_stack: Vec<HashSet<String>>,
    /// True while the command after a qemu-lock.sh `run`'s `--` is analysed.
    pub wrapped: bool,
    /// (text, script) of every text being analysed, outermost first.
    pub texts: Vec<(String, bool)>,
    /// Tests replace `claude_tmp_roots` with fixed roots.
    pub tmp_roots: Option<Vec<String>>,
}

/// `unresolved(word)`: the word still holds an expansion.
pub fn unresolved(word: &str) -> bool {
    word.contains('$') || word.contains('`')
}

/// `basename(word)`: the part after the last `/`.
pub fn basename(word: &str) -> &str {
    word.rsplit('/').next().unwrap_or(word)
}

/// `assignment_name(word)`: the variable an assignment word sets.
pub fn assignment_name(word: &str) -> Option<String> {
    let m = ASSIGNMENT.find(word)?;
    let head = m.as_str().split('[').next().unwrap_or("");
    Some(head.trim_end_matches(['+', '=']).to_string())
}

/// `expand_vars(word, known)`: `$NAME` and `${NAME}` set earlier in the same
/// text; anything unknown is left as it is.
pub fn expand_vars(word: &str, known: &HashMap<String, String>) -> String {
    static VAR: LazyLock<Regex> =
        LazyLock::new(|| re(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}|\$([A-Za-z_][A-Za-z0-9_]*)"));
    if !word.contains('$') {
        return word.to_string();
    }
    VAR.replace_all(word, |c: &regex::Captures| {
        let name = c.get(1).or_else(|| c.get(2)).expect("a name").as_str();
        match known.get(name) {
            Some(v) => v.clone(),
            None => c[0].to_string(),
        }
    })
    .into_owned()
}

/// `str.partition(sep)` for a one-character separator.
pub fn partition(s: &str, sep: char) -> (&str, bool, &str) {
    match s.split_once(sep) {
        Some((a, b)) => (a, true, b),
        None => (s, false, ""),
    }
}

/// `a[:1]`: the first character, or "".
pub fn first_char(s: &str) -> &str {
    s.char_indices().nth(1).map_or(s, |(i, _)| &s[..i])
}

/// `a[1:]`.
pub fn after_first(s: &str) -> &str {
    &s[first_char(s).len()..]
}

/// `str.split(None, maxsplit)`.
fn split_ws_max(s: &str, maxsplit: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    let n = chars.len();
    let mut i = 0;
    let mut out = Vec::new();
    let mut count = maxsplit;
    while count > 0 {
        while i < n && crate::pystr::is_space(chars[i]) {
            i += 1;
        }
        if i == n {
            break;
        }
        let j = i;
        while i < n && !crate::pystr::is_space(chars[i]) {
            i += 1;
        }
        out.push(chars[j..i].iter().collect());
        count -= 1;
    }
    if i < n {
        while i < n && crate::pystr::is_space(chars[i]) {
            i += 1;
        }
        if i != n {
            out.push(chars[i..].iter().collect());
        }
    }
    out
}

impl<'c> Analyzer<'c> {
    pub fn new(ctx: &'c Ctx) -> Analyzer<'c> {
        Analyzer {
            verdict: Verdict::default(),
            ctx,
            repos: Repos::default(),
            config_changed: BTreeSet::new(),
            head_changed: BTreeSet::new(),
            commits_changed: false,
            global_env: HashSet::new(),
            env_stack: Vec::new(),
            wrapped: false,
            texts: Vec::new(),
            tmp_roots: None,
        }
    }

    pub fn repo(&self, cwd: &str, extra: &[String]) -> Rc<Repo> {
        self.repos.get(cwd, extra)
    }

    pub fn is_agent(&self) -> bool {
        self.ctx.agent_id.is_some()
    }

    pub fn env_names(&self) -> BTreeSet<String> {
        let mut names: BTreeSet<String> = self.global_env.iter().cloned().collect();
        for frame in &self.env_stack {
            names.extend(frame.iter().cloned());
        }
        names
    }

    /// Something runs that the guard cannot see. Ask when the text it came from
    /// mentions git, gh or push, or a team rule's commands: the whole command
    /// typed, or, inside a script file, the command's own line.
    pub fn opaque(&mut self, state: &State, cmd: &Command, reason: &str) {
        let text = if state.script { &cmd.src } else { &state.text };
        if MENTIONS_GIT.is_match(text) || MENTIONS_TEAM.is_match(text) {
            self.verdict.ask(verdict::SHELL, reason);
        }
    }

    // -- shell level --------------------------------------------------------

    pub fn analyze_text(&mut self, text: &str, cwd: &str, depth: usize, script: bool) -> Flow {
        if depth > MAX_DEPTH {
            return Err(ParseError("nesting too deep").into());
        }
        let mut state = State {
            cwd: cwd.to_string(),
            vars: HashMap::new(),
            text: text.to_string(),
            script,
            pid_vars: HashSet::new(),
        };
        self.texts.push((text.to_string(), script));
        let result = parse_commands(text, depth)
            .map_err(Stop::from)
            .and_then(|cmds| {
                for cmd in &cmds {
                    self.analyze_command(cmd, &mut state, depth)?;
                }
                Ok(())
            });
        self.texts.pop();
        result
    }

    pub fn analyze_command(&mut self, cmd: &Command, state: &mut State, depth: usize) -> Flow {
        let mut known = state.vars.clone();
        known
            .entry("PWD".to_string())
            .or_insert_with(|| state.cwd.clone());
        if !known.contains_key("HOME") {
            known.insert("HOME".to_string(), pyos::expanduser("~")?);
        }
        let flags = &cmd.expanded;
        let words: Vec<String> = cmd
            .words
            .iter()
            .zip(flags)
            .map(|(w, e)| {
                if e.any() {
                    expand_vars(w, &known)
                } else {
                    w.clone()
                }
            })
            .collect();
        let mut idx = 0;
        while idx < words.len()
            && (ASSIGNMENT.is_match(&words[idx]) || RESERVED.contains(&words[idx].as_str()))
        {
            idx += 1;
        }
        let mut assigns: Vec<&String> = words[..idx]
            .iter()
            .filter(|w| ASSIGNMENT.is_match(w))
            .collect();
        if idx < words.len()
            && ["export", "local", "declare", "readonly", "typeset"].contains(&words[idx].as_str())
        {
            for w in &words[idx + 1..] {
                if !w.starts_with('-') {
                    self.global_env
                        .insert(assignment_name(w).unwrap_or_else(|| w.clone()));
                }
            }
            idx = words.len();
            assigns = words.iter().filter(|w| ASSIGNMENT.is_match(w)).collect();
        }
        if idx >= words.len() {
            // A variable assigned from a pid lookup ($(pgrep ...)) carries a
            // pattern kill to a later `kill $VAR`.
            let pid_lookup = PID_FINDER.is_match(&cmd.src);
            for w in assigns {
                let name = assignment_name(w).expect("an assignment");
                let end = ASSIGNMENT.find(w).expect("an assignment").end();
                state.vars.insert(name.clone(), w[end..].to_string());
                self.global_env.insert(name.clone());
                if pid_lookup {
                    state.pid_vars.insert(name);
                } else {
                    state.pid_vars.remove(&name);
                }
            }
            return Ok(());
        }
        // Every name assigned for this command (callers filter the ones that
        // matter to them: git_env_risky, gh_env_risky, RUSTUP_*).
        let frame: HashSet<String> = assigns
            .iter()
            .map(|w| assignment_name(w).expect("an assignment"))
            .collect();
        self.env_stack.push(frame);
        let result = self.dispatch(&words, flags, idx, cmd, state, depth);
        self.env_stack.pop();
        result
    }

    fn dispatch(
        &mut self,
        words: &[String],
        flags: &[Expanded],
        idx: usize,
        cmd: &Command,
        state: &mut State,
        depth: usize,
    ) -> Flow {
        let head_word = &words[idx];
        let head = basename(head_word);
        if flags[idx].any() && unresolved(head_word) {
            self.opaque(
                state,
                cmd,
                "the command name comes from a shell expansion the guard cannot resolve",
            );
        }
        if head == "cd" || head == "pushd" {
            let target = match words[idx + 1..].iter().find(|w| !w.starts_with('-')) {
                Some(t) => pyos::expanduser(t)?,
                None => pyos::expanduser(&pyos::expanduser("~")?)?,
            };
            state.cwd = pyos::normpath(&pyos::join(&state.cwd, &target));
            return Ok(());
        }
        if NON_EXEC.contains(&head) || head == "for" || head == "select" {
            return Ok(());
        }
        if head == "read" {
            self.model_read(&words[idx + 1..], cmd, state);
            return Ok(());
        }
        if head == "eval" || head == "watch" {
            let rest: Vec<&str> = words[idx + 1..]
                .iter()
                .filter(|w| !w.starts_with('-'))
                .map(String::as_str)
                .collect();
            if words[idx + 1..]
                .iter()
                .zip(&flags[idx + 1..])
                .any(|(w, e)| e.any() && unresolved(w))
            {
                self.opaque(
                    state,
                    cmd,
                    &format!(
                        "{head} runs a command string that comes from a shell expansion the guard cannot resolve"
                    ),
                );
            }
            let cwd = state.cwd.clone();
            return self.analyze_text(&rest.join(" "), &cwd, depth + 1, false);
        }
        if head == "source" || head == "." {
            if idx + 1 < words.len() {
                self.analyze_script(&words[idx + 1], state, depth, cmd, false)?;
            }
            return Ok(());
        }
        if head == "command" && command_lookup(&words[idx + 1..]) {
            return Ok(()); // command -v / -V only prints what a name resolves to
        }
        let runs: BTreeSet<usize> = if words[idx..]
            .iter()
            .any(|w| SED_NAMES.contains(&basename(w)))
        {
            run_positions(words, idx)
        } else {
            BTreeSet::new()
        };
        for k in idx..words.len() {
            let name = basename(&words[k]);
            if k > idx && head == "env" && ASSIGNMENT.is_match(&words[k]) {
                let assigned = assignment_name(&words[k]).expect("an assignment");
                self.env_stack
                    .last_mut()
                    .expect("the command's frame")
                    .insert(assigned);
                continue;
            }
            let via: BTreeSet<String> = words[idx..k]
                .iter()
                .map(|w| basename(w).to_string())
                .collect();
            if k == idx || WRAPPERS.contains(&head) {
                if name == "env" {
                    if let Some(split) = env_split_string(&words[k + 1..]) {
                        let cwd = state.cwd.clone();
                        return self.analyze_text(&split, &cwd, depth + 1, false);
                    }
                }
                if name == "tmux" {
                    return self.analyze_tmux(&words[k + 1..], state, depth);
                }
                if name == "screen" && words[k + 1..].iter().any(|w| w == "stuff") {
                    // screen -X stuff TEXT types TEXT into the session's shell.
                    let after = &words[k + 1..];
                    let at = after.iter().position(|w| w == "stuff").expect("stuff");
                    let cwd = state.cwd.clone();
                    for text in &after[at + 1..] {
                        self.analyze_text(text, &cwd, depth + 1, false)?;
                    }
                    return Ok(());
                }
                if SED_NAMES.contains(&name) && runs.contains(&k) {
                    self.check_sed(words, flags, k, cmd, state, &via)?;
                }
            }
            if name == "git" {
                let wrapper = (k > idx).then_some(head);
                let cwd = state.cwd.clone();
                return self.analyze_git(&words[k + 1..], cmd, &cwd, wrapper, depth);
            }
            if name == "gh" {
                let cwd = state.cwd.clone();
                let runs = k == idx || WRAPPERS.contains(&head);
                return Ok(self.analyze_gh(&words[k + 1..], cmd, &cwd, runs)?);
            }
            if SHELLS.contains(&name) {
                return self.analyze_shell(&words[k + 1..], &flags[k + 1..], cmd, state, depth);
            }
            if INTERPRETERS.contains(&name) {
                let cwd = state.cwd.clone();
                if AWK_NAMES.contains(&name) && self.awk_runs_commands(&words[k + 1..], &cwd)? {
                    self.verdict.ask(
                        verdict::SHELL,
                        format!(
                            "{name} program runs shell commands (system, getline or print to a pipe)"
                        ),
                    );
                }
                let program = words[k + 1..].iter().chain(&cmd.heredocs);
                if program.into_iter().any(|w| pattern::inline_git_push(w)) {
                    self.verdict.ask(
                        verdict::SHELL,
                        format!(
                            "{name} program text mentions git push; the guard cannot resolve it"
                        ),
                    );
                } else if cmd.piped_in && !words[k + 1..].iter().any(|w| !w.starts_with('-')) {
                    self.opaque(
                        state,
                        cmd,
                        &format!(
                            "{name} reads its program from a pipe; the guard cannot see what it runs"
                        ),
                    );
                }
                return Ok(());
            }
            if k == idx && words[k].contains('/') {
                return self.analyze_script(&words[k], state, depth, cmd, true);
            }
        }
        Ok(())
    }

    /// True when the awk program (inline, or from -f files) can run a shell
    /// command. Options with a separate value are skipped.
    fn awk_runs_commands(&mut self, args: &[String], cwd: &str) -> Result<bool, Fault> {
        static STRING: LazyLock<Regex> = LazyLock::new(|| re(r#""(?:[^"\\\n]|\\.)*""#));
        let mut programs: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if (a == "-f" || a == "--file") && i + 1 < args.len() {
                programs.push(
                    self.read_file(&args[i + 1], cwd)?
                        .filter(|t| !t.is_empty())
                        .unwrap_or_else(|| "system(".to_string()),
                );
                i += 2;
                continue;
            }
            if ["-v", "-F", "--assign", "--field-separator"].contains(&a) && i + 1 < args.len() {
                i += 2;
                continue;
            }
            if a.starts_with('-') && a != "-" {
                i += 1;
                continue;
            }
            if programs.is_empty() {
                programs.push(a.to_string());
            }
            break;
        }
        // String literals are data: "a | b" is not a pipe.
        Ok(programs
            .iter()
            .any(|p| AWK_EXEC.is_match(&STRING.replace_all(p, "\"\""))))
    }

    /// `read VAR <<< 'text'`: remember VAR so a later "$VAR" resolves.
    fn model_read(&mut self, args: &[String], cmd: &Command, state: &mut State) {
        static NAME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z_][A-Za-z0-9_]*$"));
        let mut names: Vec<&String> = Vec::new();
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if ["-d", "-n", "-N", "-p", "-t", "-u", "-a", "-i"].contains(&a.as_str()) {
                skip = true;
            } else if !a.starts_with('-') && NAME.is_match(a) {
                names.push(a);
            }
        }
        if names.is_empty() {
            return;
        }
        let Some(first) = cmd.heredocs.first() else {
            for n in names {
                state.vars.remove(n);
            }
            return;
        };
        let line = first.split('\n').next().unwrap_or("");
        let parts = if names.len() > 1 {
            split_ws_max(line, names.len() - 1)
        } else {
            vec![crate::pystr::strip(line).to_string()]
        };
        for (k, n) in names.into_iter().enumerate() {
            state
                .vars
                .insert(n.clone(), parts.get(k).cloned().unwrap_or_default());
        }
    }

    /// A shell run with `args` (`flags`: their expansion state).
    fn analyze_shell(
        &mut self,
        args: &[String],
        flags: &[Expanded],
        cmd: &Command,
        state: &mut State,
        depth: usize,
    ) -> Flow {
        let cwd = state.cwd.clone();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            if a == "--" {
                i += 1;
                break;
            }
            if ["-o", "+o", "-O", "+O", "--rcfile", "--init-file"].contains(&a) {
                i += 2;
                continue;
            }
            if a.starts_with("--") {
                i += 1;
                continue;
            }
            if (first_char(a) == "-" || first_char(a) == "+") && a.chars().count() > 1 {
                if after_first(a).contains('c') {
                    let mut j = i + 1;
                    while j < args.len() && first_char(&args[j]) == "-" && args[j] != "-" {
                        j += 1;
                    }
                    if j < args.len() {
                        if flags[j].any() && unresolved(&args[j]) {
                            self.opaque(
                                state,
                                cmd,
                                "a shell runs a command string that comes from a shell expansion the guard cannot resolve",
                            );
                        }
                        self.analyze_text(&args[j], &cwd, depth + 1, false)?;
                    }
                    return Ok(());
                }
                i += 1;
                continue;
            }
            break;
        }
        if i < args.len() && args[i] != "-" {
            return self.analyze_script(&args[i], state, depth, cmd, false);
        }
        if !cmd.heredocs.is_empty() {
            for body in &cmd.heredocs {
                self.analyze_text(body, &cwd, depth + 1, false)?;
            }
        } else if !cmd.stdin_files.is_empty() {
            for path in &cmd.stdin_files {
                self.analyze_script(path, state, depth, cmd, false)?;
            }
        } else if cmd.piped_in {
            self.verdict.ask(
                verdict::SHELL,
                "a shell reads commands from a pipe; the guard cannot see what it runs",
            );
        }
        Ok(())
    }

    /// A script run as `path`.
    pub fn analyze_script(
        &mut self,
        path: &str,
        state: &mut State,
        depth: usize,
        cmd: &Command,
        shebang: bool,
    ) -> Flow {
        if unresolved(path) {
            self.opaque(
                state,
                cmd,
                &format!(
                    "runs a script whose path ({path}) comes from a shell expansion the guard cannot resolve"
                ),
            );
            return Ok(());
        }
        self.read_script(path, state, depth, shebang)
    }

    /// Analyse a script file's commands. `shebang` is true when the file is
    /// executed directly: a `#!` line naming something other than a shell means
    /// it is not shell code; a file without one is run by the shell.
    pub fn read_script(&mut self, path: &str, state: &State, depth: usize, shebang: bool) -> Flow {
        let full = pyos::join(&state.cwd, &pyos::expanduser(path)?);
        if !pyos::isfile(&full) {
            return Ok(());
        }
        match pyos::getsize(&full) {
            Ok(size) if size <= FILE_LIMIT => {}
            Ok(_) | Err(ReadError::Os) => return Ok(()),
            Err(ReadError::Fault(f)) => return Err(f.into()),
        }
        let text = match pyos::read_text_replace(&full) {
            Ok(text) => text,
            Err(ReadError::Os) => return Ok(()),
            Err(ReadError::Fault(f)) => return Err(f.into()),
        };
        if text.contains('\0') {
            return Ok(()); // a binary
        }
        if shebang {
            let first = text.split('\n').next().unwrap_or("");
            if first.starts_with("#!") && !pattern::SHELL_SHEBANG.is_match(first) {
                return Ok(());
            }
        }
        let cwd = state.cwd.clone();
        self.analyze_text(&text, &cwd, depth + 1, true)
    }

    /// tmux runs shell commands it is given as strings (new-session,
    /// new-window, split-window, respawn-*, run-shell, if-shell, -c) and types
    /// send-keys text into a shell. Each argument that is not an option, and the
    /// tail of arguments from the first one, is analysed as a command line.
    fn analyze_tmux(&mut self, args: &[String], state: &State, depth: usize) -> Flow {
        let cwd = state.cwd.clone();
        let mut i = 0;
        while i < args.len() && args[i].starts_with('-') && args[i] != "--" {
            let a = args[i].as_str();
            i += 1;
            if a == "-c" && i < args.len() {
                self.analyze_text(&args[i], &cwd, depth + 1, false)?;
                i += 1;
            } else if ["-L", "-S", "-f", "-T"].contains(&a) {
                i += 1;
            }
        }
        if i < args.len() && args[i] == "--" {
            i += 1;
        }
        let mut group: Vec<String> = Vec::new();
        let rest = args.get(i..).unwrap_or(&[]);
        for a in rest.iter().chain(std::iter::once(&";".to_string())) {
            if a != ";" {
                group.push(a.clone());
                continue;
            }
            let words: Vec<String> = group.drain(..).skip(1).collect();
            let positional: Vec<usize> = (0..words.len())
                .filter(|&n| !words[n].starts_with('-'))
                .collect();
            for &n in &positional {
                self.analyze_text(&words[n], &cwd, depth + 1, false)?;
            }
            if let Some(&first) = positional.first() {
                let tail: Vec<&String> = words[first..]
                    .iter()
                    .filter(|w| !TMUX_KEYS.contains(&w.as_str()))
                    .collect();
                self.analyze_text(&shlex::join(&tail), &cwd, depth + 1, false)?;
            }
        }
        Ok(())
    }

    // -- files that a command publishes -----------------------------------

    /// Claude Code keeps session scratchpads under /tmp/claude-<uid>/<slug>, where
    /// slug is the project path with every other character replaced by '-'.
    /// Files there are the agent's own drafts, like files in the repo
    /// (`claude_tmp_roots`).
    fn claude_tmp_roots(&self, repo_roots: &[String]) -> Vec<String> {
        if let Some(roots) = &self.tmp_roots {
            return roots.clone();
        }
        let project = std::env::var("CLAUDE_PROJECT_DIR")
            .ok()
            .filter(|p| !p.is_empty())
            .or_else(|| repo_roots.last().cloned());
        let (Some(project), Some(uid)) = (project, uid()) else {
            return Vec::new();
        };
        let slug: String = project
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        vec![pyos::realpath(&pyos::join(
            &format!("/tmp/claude-{uid}"),
            &slug,
        ))]
    }

    /// Inside the working tree, the main checkout, or this project's Claude Code
    /// temp directory (session scratchpads).
    pub fn inside_repo(&self, path: &str, cwd: &str) -> Result<bool, Fault> {
        let full = pyos::realpath(&pyos::join(cwd, &pyos::expanduser(path)?));
        let roots = self.repo(cwd, &[]).roots()?;
        let tmp = self.claude_tmp_roots(&roots);
        Ok(roots
            .iter()
            .chain(&tmp)
            .any(|root| full == *root || full.starts_with(&format!("{root}/"))))
    }

    /// `what` reads `path` and sends its content to GitHub or into a commit that
    /// will be pushed. Files inside the repository are fine; anything else could
    /// carry a secret out, so ask.
    pub fn check_input(
        &mut self,
        tag: Tag,
        path: Option<&str>,
        cmd: &Command,
        cwd: &str,
        what: &str,
    ) -> Result<(), Fault> {
        let Some(path) = path else {
            return Ok(());
        };
        if path == "-" {
            if !cmd.heredocs.is_empty() {
                return Ok(());
            }
            for f in &cmd.stdin_files {
                self.check_input(tag, Some(f), &Command::default(), cwd, what)?;
            }
            if cmd.piped_in {
                self.verdict.ask(
                    tag,
                    format!("{what} reads a pipe; the guard cannot see what it publishes"),
                );
            }
            return Ok(());
        }
        if unresolved(path) {
            self.verdict.ask(
                tag,
                format!("{what} reads a file whose path comes from a shell expansion"),
            );
        } else if !self.inside_repo(path, cwd)? {
            self.verdict.ask(
                tag,
                format!(
                    "{what} reads {path}, which is outside the repository and the session scratchpad (the repository is public)"
                ),
            );
        }
        Ok(())
    }

    pub fn read_repo_file(&self, path: &str, cwd: &str) -> Result<Option<String>, Fault> {
        if path == "-" || unresolved(path) || !self.inside_repo(path, cwd)? {
            return Ok(None);
        }
        self.read_file(path, cwd)
    }

    /// `read_file`: the text of `path` (errors replaced), or `None` when it is
    /// `-`, unresolved, missing, unreadable or over [`FILE_LIMIT`].
    pub fn read_file(&self, path: &str, cwd: &str) -> Result<Option<String>, Fault> {
        if path == "-" || unresolved(path) {
            return Ok(None);
        }
        let full = pyos::join(cwd, &pyos::expanduser(path)?);
        match pyos::getsize(&full) {
            Ok(size) if size <= FILE_LIMIT => {}
            Ok(_) | Err(ReadError::Os) => return Ok(None),
            Err(ReadError::Fault(f)) => return Err(f),
        }
        match pyos::read_text_replace(&full) {
            Ok(text) => Ok(Some(text)),
            Err(ReadError::Os) => Ok(None),
            Err(ReadError::Fault(f)) => Err(f),
        }
    }
}

/// `os.getuid()`, through `id -u` (the standard library has no `getuid`).
fn uid() -> Option<String> {
    static UID: LazyLock<Option<String>> = LazyLock::new(|| {
        let out = std::process::Command::new("id")
            .arg("-u")
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output()
            .ok()?;
        let uid = String::from_utf8(out.stdout).ok()?.trim().to_string();
        (out.status.success() && !uid.is_empty() && uid.bytes().all(|b| b.is_ascii_digit()))
            .then_some(uid)
    });
    UID.clone()
}

/// `command -v NAME` / `command -V NAME` prints what NAME is; it runs nothing.
fn command_lookup(args: &[String]) -> bool {
    for a in args {
        if a == "--" || !a.starts_with('-') || a == "-" {
            return false;
        }
        if after_first(a).contains('v') || after_first(a).contains('V') {
            return true;
        }
    }
    false
}

/// `env -S STRING` / `--split-string=STRING`: env splits STRING into the command
/// line it runs. Returns that command line (with the arguments after STRING) as
/// shell text, or `None` when there is no -S.
fn env_split_string(args: &[String]) -> Option<String> {
    let rest_quoted = |from: usize| -> Vec<String> {
        args.get(from..)
            .unwrap_or(&[])
            .iter()
            .map(|r| shlex::quote(r))
            .collect()
    };
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" || !a.starts_with('-') || a == "-" {
            return None;
        }
        if let Some(long) = a.strip_prefix("--") {
            let (name, eq, value) = partition(long, '=');
            if !name.is_empty() && "split-string".starts_with(name) {
                let value = if eq {
                    value.to_string()
                } else {
                    let v = args.get(i).cloned().unwrap_or_default();
                    i += 1;
                    v
                };
                let mut parts = vec![value];
                parts.extend(rest_quoted(i));
                return Some(parts.join(" "));
            }
            if !eq
                && !name.is_empty()
                && ["unset", "chdir", "argv0"]
                    .iter()
                    .any(|o| o.starts_with(name))
            {
                i += 1;
            }
            continue;
        }
        let cluster: Vec<char> = after_first(a).chars().collect();
        for (pos, &ch) in cluster.iter().enumerate() {
            let rest: String = cluster[pos + 1..].iter().collect();
            if ch == 'S' {
                let rest = if rest.is_empty() {
                    let v = args.get(i).cloned().unwrap_or_default();
                    i += 1;
                    v
                } else {
                    rest
                };
                let mut parts = vec![rest];
                parts.extend(rest_quoted(i));
                return Some(parts.join(" "));
            }
            if "uCPa".contains(ch) {
                if rest.is_empty() {
                    i += 1;
                }
                break;
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn env_split_string_matches_python() {
        // env_split_string on CPython 3.13.
        for (args, want) in [
            (vec!["-S", "git push", "a b"], Some("git push 'a b'")),
            (vec!["-Sgit push"], Some("git push")),
            (vec!["-uX", "-S", "x"], Some("x")),
            (vec!["-u", "X", "-S", "x"], Some("x")),
            (vec!["--split=git push", "y"], Some("git push y")),
            (vec!["--chdir", "/", "--split-string", "x"], Some("x")),
            (vec!["-i", "git"], None),
            (vec!["git", "-S", "x"], None),
            (vec!["-S"], Some("")),
        ] {
            assert_eq!(
                env_split_string(&strings(&args)).as_deref(),
                want,
                "env_split_string({args:?})"
            );
        }
    }

    #[test]
    fn split_ws_max_matches_python() {
        // str.split(None, n) on CPython 3.13.
        assert_eq!(split_ws_max(" a  b c ", 1), ["a", "b c "]);
        assert_eq!(split_ws_max("a b", 5), ["a", "b"]);
        assert_eq!(split_ws_max("   ", 2), Vec::<String>::new());
        assert_eq!(split_ws_max("a\u{a0}b c", 1), ["a", "b c"]);
    }

    #[test]
    fn helpers_match_python() {
        assert_eq!(assignment_name("A[x]+=1").as_deref(), Some("A"));
        assert_eq!(assignment_name("A+=1").as_deref(), Some("A"));
        assert_eq!(assignment_name("1A=1"), None);
        let known: HashMap<String, String> = [("A".to_string(), "x".to_string())].into();
        assert_eq!(expand_vars("$A/${A}/$B/${B}/$", &known), "x/x/$B/${B}/$");
        assert!(command_lookup(&strings(&["-v", "git"])));
        assert!(!command_lookup(&strings(&["git", "-v"])));
        assert_eq!(first_char("éa"), "é");
        assert_eq!(after_first("éa"), "a");
        assert_eq!(basename("a/b/"), "");
    }
}
