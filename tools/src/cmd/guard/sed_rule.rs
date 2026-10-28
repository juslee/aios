//! The protected-sed rule (git-push-guard.py `Analyzer.check_sed` L1381-1453 and
//! L2636-3064): sed that may write `.claude/settings*` or `.claude/hooks/*` asks
//! unless every script it runs is read-only. It reads sed's options under both
//! GNU and BSD rules, finds where a wrapper's command starts (`run_positions`),
//! and reads sed scripts command by command.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::analyzer::{basename, expand_vars, unresolved, Analyzer, State, ARG_WRAPPERS, WRAPPERS};
use super::fnmatch::fnmatch_case;
use super::pattern::{re, ASSIGNMENT, MENTIONS_CLAUDE, SED_PROTECTED};
use super::pyos::{self, Fault};
use super::shell::{Command, Expanded};
use super::verdict;
use crate::pystr;

pub const SED_NAMES: [&str; 2] = ["sed", "gsed"];

/// `SED_ASK`, with the problem in place of `{}`.
fn sed_ask(problem: &str) -> String {
    format!(
        "sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt ({problem})"
    )
}

/// Options of a strict read (the owner's list: -n -E -r -s -u -z and their long
/// forms), the same on GNU and BSD sed or rejected by the one that lacks them.
/// -e and --expression take the script; everything else asks.
const SED_SHORT_OPTS: &str = "nErsuz";
const SED_LONG_OPTS: [&str; 6] = [
    "--quiet",
    "--silent",
    "--regexp-extended",
    "--separate",
    "--unbuffered",
    "--null-data",
];
/// Script commands that only print, move between lines or use the hold space.
const SED_READ_COMMANDS: &str = "pP=qQlnNdDgGhHx";
/// GNU sed's long options (a unique prefix selects one).
const SED_GNU_LONG: [&str; 18] = [
    "--binary",
    "--debug",
    "--expression",
    "--file",
    "--follow-symlinks",
    "--help",
    "--in-place",
    "--line-length",
    "--null-data",
    "--posix",
    "--quiet",
    "--regexp-extended",
    "--sandbox",
    "--separate",
    "--silent",
    "--unbuffered",
    "--version",
    "--zero-terminated",
];
/// Names in .claude a glob may reach: the hooks directory and settings files.
const SED_GLOB_NAMES: [&str; 4] = ["hooks", "settings", "settings.json", "settings.local.json"];

/// An expansion at the start of a word as the parser keeps it: `$(...)` and
/// `` `...` `` (their bodies are parsed apart), `${...}`, `$NAME`, `$1`, `$@` and
/// the others (`SED_LEADING_EXPANSION`, matched at the start).
static SED_LEADING_EXPANSION: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?:\$\(\.\.\.\)|`\.\.\.`|\$\{[^}]*\}|\$[A-Za-z_][A-Za-z0-9_]*|\$[0-9@*#?$!-])")
});

/// How a wrapper finds the command it runs (`WRAPPER_OPTIONS`): its short
/// options that take a value, its long options that take one, and how many
/// operands come before the command. A wrapper not listed may run any later
/// word.
type WrapperSpec = (&'static str, &'static [&'static str], usize);

fn wrapper_options(name: &str) -> Option<WrapperSpec> {
    Some(match name {
        "env" => ("uCPS", &["--unset", "--chdir", "--split-string"], 0),
        "command" | "builtin" | "nohup" | "setsid" | "unbuffer" | "chronic" => ("", &[], 0),
        "exec" => ("a", &[], 0),
        "nice" => ("n", &["--adjustment"], 0),
        "timeout" | "gtimeout" => ("sk", &["--signal", "--kill-after"], 1),
        "time" => ("fo", &["--format", "--output"], 0),
        "stdbuf" => ("ioe", &["--input", "--output", "--error"], 0),
        "sudo" => (
            "ugCDhprtUTR",
            &[
                "--user",
                "--group",
                "--close-from",
                "--chdir",
                "--host",
                "--prompt",
                "--role",
                "--type",
                "--other-user",
                "--command-timeout",
                "--chroot",
            ],
            0,
        ),
        "doas" => ("uC", &[], 0),
        "caffeinate" => ("wt", &[], 0),
        "flock" => (
            "wEc",
            &["--timeout", "--wait", "--conflict-exit-code", "--command"],
            1,
        ),
        "xargs" => (
            "aEdIJLnPRSs",
            &[
                "--arg-file",
                "--delimiter",
                "--max-args",
                "--max-procs",
                "--max-chars",
                "--process-slot-var",
            ],
            0,
        ),
        "arch" => ("ed", &[], 0),
        _ => return None,
    })
}

const FIND_EXEC: [&str; 4] = ["-exec", "-execdir", "-ok", "-okdir"];

/// `run_positions`: the indexes in `words` where the command `words[idx]` runs
/// may start: `idx`, and when `words[idx]` is a wrapper, the command it runs
/// after its options, their values and its operands, recursively; for find, the
/// command after each -exec/-execdir/-ok/-okdir. A wrapper whose options the
/// guard does not model may run any later word.
pub fn run_positions(words: &[String], idx: usize) -> BTreeSet<usize> {
    let mut out = BTreeSet::new();
    let mut starts = vec![idx];
    while let Some(mut k) = starts.pop() {
        while k < words.len() {
            out.insert(k);
            let name = basename(&words[k]);
            if name == "find" {
                starts.extend(
                    (k + 1..words.len())
                        .filter(|&j| FIND_EXEC.contains(&words[j].as_str()))
                        .map(|j| j + 1),
                );
                break;
            }
            if !WRAPPERS.contains(&name) {
                break;
            }
            let Some(spec) = wrapper_options(name) else {
                out.extend(k + 1..words.len());
                break;
            };
            k = wrapper_command_start(words, k + 1, name, spec);
        }
    }
    out
}

/// `wrapper_command_start`: the index of the command a wrapper (`name`, options
/// from `words[k]`) runs. A long option takes a value when it is (a prefix of)
/// one in the spec; a short one when its letter ends a cluster.
fn wrapper_command_start(words: &[String], mut k: usize, name: &str, spec: WrapperSpec) -> usize {
    let (shorts, longs, operands) = spec;
    while k < words.len() {
        let w = words[k].as_str();
        if w == "--" {
            k += 1;
            break;
        }
        if (name == "env" || name == "sudo") && ASSIGNMENT.is_match(w) {
            k += 1;
            continue;
        }
        if !w.starts_with('-') || w == "-" {
            break;
        }
        k += 1;
        if w.starts_with("--") {
            if !w.contains('=') && longs.iter().any(|o| o.starts_with(w)) {
                k += 1;
            }
            continue;
        }
        let cluster: Vec<char> = w.chars().skip(1).collect();
        for (pos, &ch) in cluster.iter().enumerate() {
            if shorts.contains(ch) {
                if pos == cluster.len() - 1 {
                    k += 1;
                }
                break;
            }
        }
    }
    while name == "env" && k < words.len() && ASSIGNMENT.is_match(&words[k]) {
        k += 1;
    }
    k + operands
}

/// A sed script: its text, the word's expansion state, and its index in the
/// arguments when it is an operand.
type Script = (String, Expanded, Option<usize>);

/// `sed_read`: sed's scripts and file operands under GNU rules (options anywhere
/// before `--`, long options, -e -f -l take a value, -i an attached suffix only)
/// or BSD rules (options stop at the first operand, -e -f -i -I take a value).
/// The scripts are `None` when one is unknown (-f, a word that may split); the
/// files are indexes in `args`. A sed that rejects its options reads nothing.
fn sed_read(args: &[String], flags: &[Expanded], gnu: bool) -> (Option<Vec<Script>>, Vec<usize>) {
    let mut scripts: Vec<Script> = Vec::new();
    let mut operands: Vec<usize> = Vec::new();
    let takes = if gnu { "efl" } else { "efiI" };
    let rest_from = |operands: &[usize], i: usize| -> Vec<usize> {
        operands.iter().copied().chain(i..args.len()).collect()
    };
    let mut i = 0;
    let mut done = false;
    while i < args.len() {
        let a = args[i].as_str();
        let mut e = flags[i];
        i += 1;
        if e == Expanded::Bare && unresolved(a) {
            return (None, rest_from(&operands, i));
        }
        if done || a == "-" || !a.starts_with('-') {
            operands.push(i - 1);
            done = done || !gnu;
            continue;
        }
        if a == "--" {
            done = true;
            continue;
        }
        if a.starts_with("--") {
            let (name, eq, value) = super::analyzer::partition(a, '=');
            let exact: Vec<&str> = SED_GNU_LONG
                .iter()
                .copied()
                .filter(|o| *o == name)
                .collect();
            let full = if exact.is_empty() {
                SED_GNU_LONG
                    .iter()
                    .copied()
                    .filter(|o| o.starts_with(name))
                    .collect()
            } else {
                exact
            };
            if !gnu || full.len() != 1 {
                return (Some(Vec::new()), Vec::new());
            }
            if ["--expression", "--file", "--line-length"].contains(&full[0]) {
                let mut value = value.to_string();
                if !eq {
                    if i >= args.len() {
                        return (Some(Vec::new()), Vec::new());
                    }
                    value = args[i].clone();
                    e = flags[i];
                    i += 1;
                }
                if full[0] == "--file" {
                    return (None, rest_from(&operands, i));
                }
                if full[0] == "--expression" {
                    scripts.push((value, e, None));
                }
            }
            continue;
        }
        let cluster: Vec<char> = a.chars().skip(1).collect();
        for (pos, &ch) in cluster.iter().enumerate() {
            if !takes.contains(ch) {
                continue;
            }
            let mut value: String = cluster[pos + 1..].iter().collect();
            if value.is_empty() {
                if i >= args.len() {
                    return (Some(Vec::new()), Vec::new());
                }
                value = args[i].clone();
                e = flags[i];
                i += 1;
            }
            if ch == 'f' {
                return (None, rest_from(&operands, i));
            }
            if ch == 'e' {
                scripts.push((value, e, None));
            }
            break;
        }
    }
    if scripts.is_empty() {
        if let Some((&j, files)) = operands.split_first() {
            return (
                Some(vec![(args[j].clone(), flags[j], Some(j))]),
                files.to_vec(),
            );
        }
    }
    (Some(scripts), operands)
}

/// `sed_files`: the words GNU or BSD sed may read as file operands. A word that
/// one reading takes as the script and the other as a file (`sed -i ''
/// s/a/b/ f`: BSD's -i takes '', GNU's does not) counts only when it exists
/// against cwd, as sed edits no file that is not there.
fn sed_files(args: &[String], flags: &[Expanded], cwd: &str) -> Result<Vec<String>, Fault> {
    let readings = [sed_read(args, flags, true), sed_read(args, flags, false)];
    let script_at: BTreeSet<usize> = readings
        .iter()
        .flat_map(|(scripts, _)| scripts.iter().flatten().filter_map(|s| s.2))
        .collect();
    let mut out = Vec::new();
    for (_, files) in &readings {
        for &j in files {
            if !script_at.contains(&j)
                || pyos::lexists(&pyos::join(cwd, &pyos::expanduser(&args[j])?))
            {
                out.push(args[j].clone());
            }
        }
    }
    Ok(out)
}

/// `sed_may_open`: true when, under the GNU or the BSD reading of its options,
/// sed may open a file or run a command of its script's choosing.
fn sed_may_open(args: &[String], flags: &[Expanded]) -> bool {
    [true, false]
        .into_iter()
        .any(|gnu| match sed_read(args, flags, gnu).0 {
            None => true,
            Some(scripts) => scripts
                .iter()
                .any(|(t, e, _)| e.any() || sed_script_opens(t)),
        })
}

fn at(s: &[char], i: usize) -> Option<char> {
    s.get(i).copied()
}

fn is_in(c: Option<char>, set: &str) -> bool {
    c.is_some_and(|c| set.contains(c))
}

/// `sed_script_opens`: true when a literal sed script may open a file or run a
/// command: w, W, r, R, e, an s command with a w or e flag, or anything this
/// reader cannot follow (an address form or command it does not know, a label or
/// branch followed by `;`, which GNU ends there and BSD does not).
fn sed_script_opens(script: &str) -> bool {
    let s: Vec<char> = script.chars().collect();
    let n = s.len();
    let mut i = 0;
    loop {
        while is_in(at(&s, i), " \t\n;") {
            i += 1;
        }
        if i >= n {
            return false;
        }
        let (next, mut problem) = sed_address(&s, i, false);
        i = next;
        if problem.is_none() && at(&s, i) == Some(',') {
            (i, problem) = sed_address(&s, i + 1, true);
        }
        if problem.is_some() {
            return true;
        }
        while is_in(at(&s, i), " \t!") {
            i += 1;
        }
        if i >= n {
            return false;
        }
        let c = s[i];
        i += 1;
        if "\n;{}".contains(c) {
            continue;
        }
        if "#:btTv".contains(c) {
            let end = (i..n).find(|&p| s[p] == '\n').unwrap_or(n);
            if c != '#' && s[i.min(end)..end].contains(&';') {
                return true;
            }
            i = end;
            continue;
        }
        if "aic".contains(c) {
            while i < n && s[i] != '\n' {
                i += if s[i] == '\\' { 2 } else { 1 };
            }
            continue;
        }
        if "sy".contains(c) {
            if i >= n || "\\\n".contains(s[i]) {
                return true;
            }
            for part in 0..2 {
                let (next, problem) = sed_delimited(&s, i, s[i], c == 's' && part == 0);
                if problem.is_some() {
                    return true;
                }
                i = next - 1; // the closing delimiter opens the next part
            }
            i += 1;
            if c == 's' {
                while is_in(at(&s, i), "gpiImM0123456789") {
                    i += 1;
                }
            }
        } else if "lLqQ".contains(c) {
            while is_in(at(&s, i), " \t0123456789") {
                i += 1;
            }
        } else if !SED_READ_COMMANDS.contains(c) && !"Fz".contains(c) {
            return true;
        }
        while is_in(at(&s, i), " \t") {
            i += 1;
        }
        if i < n && !"\n;}".contains(s[i]) {
            return true;
        }
    }
}

/// `sed_problem`: `None` when every script sed runs is read-only and no option
/// edits files; otherwise what may write. GNU sed reads options anywhere before
/// `--`; BSD sed stops at the first operand and reads later option words as files
/// (and that operand as the script), so an option after an operand asks. When the
/// command names a protected path (`named`), a quoted expansion before `--` that
/// may start with `-` asks too.
fn sed_problem(args: &[String], flags: &[Expanded], named: bool) -> Option<String> {
    let mut scripts: Vec<(String, Expanded)> = Vec::new();
    let mut operands: Vec<(String, Expanded)> = Vec::new();
    let mut i = 0;
    let mut options_done = false;
    while i < args.len() {
        let a = args[i].as_str();
        let mut e = flags[i];
        i += 1;
        if e == Expanded::Bare && unresolved(a) {
            return Some(format!(
                "{a} comes from a shell expansion and may split into options"
            ));
        }
        if named && e.any() && !options_done && sed_may_be_option(a) {
            return Some(format!(
                "{a} comes from a shell expansion and may be an option"
            ));
        }
        if options_done || a == "-" || !a.starts_with('-') {
            operands.push((a.to_string(), e));
            continue;
        }
        if a == "--" {
            options_done = true;
            continue;
        }
        if let Some((first, _)) = operands.first() {
            return Some(format!(
                "option {a} after the operand {} (BSD sed reads it as a file and the first operand as the script)",
                pystr::repr(first)
            ));
        }
        if a.starts_with("--") {
            let (name, eq, value) = super::analyzer::partition(a, '=');
            if name == "--expression" {
                let mut value = value.to_string();
                if !eq {
                    if i >= args.len() {
                        return Some("--expression without a script".to_string());
                    }
                    value = args[i].clone();
                    e = flags[i];
                    i += 1;
                }
                scripts.push((value, e));
            } else if !SED_LONG_OPTS.contains(&name) || eq {
                return Some(format!("option {a}"));
            }
            continue;
        }
        let cluster: Vec<char> = a.chars().skip(1).collect();
        for (pos, &ch) in cluster.iter().enumerate() {
            if ch == 'e' {
                let mut value: String = cluster[pos + 1..].iter().collect();
                if value.is_empty() {
                    if i >= args.len() {
                        return Some("-e without a script".to_string());
                    }
                    value = args[i].clone();
                    e = flags[i];
                    i += 1;
                }
                scripts.push((value, e));
                break;
            }
            if !SED_SHORT_OPTS.contains(ch) {
                return Some(format!("option -{ch}"));
            }
        }
    }
    if scripts.is_empty() {
        let Some(first) = operands.into_iter().next() else {
            return Some("no script".to_string());
        };
        scripts.push(first);
    }
    for (text, e) in &scripts {
        if e.any() {
            return Some(format!(
                "the script {} comes from a shell expansion",
                pystr::repr(text)
            ));
        }
        if let Some(problem) = sed_script_problem(text) {
            return Some(problem);
        }
    }
    None
}

/// `sed_may_be_option`: true when a word with an expansion may start with `-`:
/// it starts with the expansion, and the expansion is not a path's first
/// component (`"$DIR/x"`).
fn sed_may_be_option(word: &str) -> bool {
    if !word.starts_with('$') && !word.starts_with('`') {
        return false;
    }
    match SED_LEADING_EXPANSION.find(word) {
        Some(m) => !word[m.end()..].starts_with('/'),
        None => true,
    }
}

/// `sed_script_problem`: `None` when the script is only commands from
/// [`SED_READ_COMMANDS`], braces and `!`, each with an optional address or range
/// of line numbers, `$`, first~step or /regex/, separated by `;` or newlines.
/// Anything else is reported.
fn sed_script_problem(script: &str) -> Option<String> {
    let s: Vec<char> = script.chars().collect();
    let n = s.len();
    let mut i = 0;
    loop {
        while is_in(at(&s, i), " \t\n;") {
            i += 1;
        }
        if i >= n {
            return None;
        }
        let (next, problem) = sed_address(&s, i, false);
        if problem.is_some() {
            return problem;
        }
        i = next;
        if at(&s, i) == Some(',') {
            let (next, problem) = sed_address(&s, i + 1, true);
            if problem.is_some() {
                return problem;
            }
            i = next;
        }
        while is_in(at(&s, i), " \t") {
            i += 1;
        }
        if at(&s, i) == Some('!') {
            i += 1;
            while is_in(at(&s, i), " \t") {
                i += 1;
            }
        }
        if i >= n || "\n;".contains(s[i]) {
            continue;
        }
        let c = s[i];
        i += 1;
        if c == '{' || c == '}' {
            continue;
        }
        let shown = pystr::repr(&c.to_string());
        if !SED_READ_COMMANDS.contains(c) {
            return Some(format!("the {shown} command"));
        }
        while is_in(at(&s, i), " \t") {
            i += 1;
        }
        if i < n && !"\n;}".contains(s[i]) {
            return Some(format!("text after the {shown} command"));
        }
    }
}

/// `sed_address`: one address at `s[i]`: (index after it, problem).
fn sed_address(s: &[char], mut i: usize, required: bool) -> (usize, Option<String>) {
    let digit = |c: Option<char>| c.is_some_and(|c| c.is_ascii_digit());
    if digit(at(s, i)) {
        while digit(at(s, i)) {
            i += 1;
        }
        if at(s, i) == Some('~') {
            let mut j = i + 1;
            while digit(at(s, j)) {
                j += 1;
            }
            if j == i + 1 {
                return (i, Some("a first~step address without a step".to_string()));
            }
            i = j;
        }
        return (i, None);
    }
    if at(s, i) == Some('$') {
        return (i + 1, None);
    }
    if at(s, i) == Some('/') {
        return sed_delimited(s, i, '/', true);
    }
    if required {
        return (
            i,
            Some("an address form other than a line number, $, first~step or /regex/".to_string()),
        );
    }
    (i, None)
}

/// `sed_delimited`: the text that `s[i]` (`delim`) opens and the next unescaped
/// delim closes: (index after the closing delim, problem). GNU sed ends a regex
/// at the first unescaped delimiter; BSD sed skips one inside a bracket
/// expression, so with `brackets` a bracket that BSD reads as still open at that
/// point makes the two disagree on where the commands start.
fn sed_delimited(s: &[char], i: usize, delim: char, brackets: bool) -> (usize, Option<String>) {
    let n = s.len();
    let mut j = i + 1;
    while j < n && s[j] != delim {
        if s[j] == '\n' {
            return (j, Some(format!("a newline inside a {delim}regex{delim}")));
        }
        j += if s[j] == '\\' { 2 } else { 1 };
    }
    if j >= n {
        return (
            j,
            Some(format!("a {delim}regex{delim} without its closing {delim}")),
        );
    }
    let body = &s[i + 1..j];
    if body.contains(&'\n') {
        return (j, Some(format!("a newline inside a {delim}regex{delim}")));
    }
    let mut k = 0;
    while brackets && k < body.len() {
        if body[k] == '\\' {
            let escaped = body
                .get(k + 1)
                .is_some_and(|&c| c == '[' || c == delim || c == '\\' || c == 'n');
            k += if escaped { 2 } else { 1 };
        } else if body[k] == '[' {
            match bsd_bracket_end(body, k) {
                Some(end) => k = end,
                None => {
                    return (
                        j,
                        Some(
                            "a /regex/ whose bracket expression GNU and BSD sed end differently"
                                .to_string(),
                        ),
                    )
                }
            }
        } else {
            k += 1;
        }
    }
    (j + 1, None)
}

/// `bsd_bracket_end`: the index after the bracket expression opened at
/// `body[k]`, read the way BSD sed's compile_ccl reads it, or `None` when it does
/// not close.
fn bsd_bracket_end(body: &[char], mut k: usize) -> Option<usize> {
    let n = body.len();
    k += 1;
    if k < n && body[k] == '^' {
        k += 1;
    }
    if k < n && body[k] == ']' {
        k += 1;
    }
    while k < n {
        if body[k] == ']' {
            return Some(k + 1);
        }
        if body[k] == '[' && k + 1 < n && ".:=".contains(body[k + 1]) {
            let close = [body[k + 1], ']'];
            let end = (k + 2..n.saturating_sub(1)).find(|&p| body[p..p + 2] == close)?;
            k = end + 2;
            continue;
        }
        k += 1;
    }
    None
}

/// `sed_protected`: a path word names .claude/settings* or .claude/hooks*, as
/// written or resolved against cwd.
fn sed_protected(word: &str, cwd: &str) -> Result<bool, Fault> {
    if SED_PROTECTED.is_match(word) {
        return Ok(true);
    }
    let resolved = pyos::normpath(&pyos::join(cwd, &pyos::expanduser(word)?));
    Ok(SED_PROTECTED.is_match(&resolved))
}

/// `sed_glob_protected`: a glob word (`*`, `?` or `[`) may match
/// .claude/settings* or a path under .claude/hooks, resolved against cwd: a
/// component that may be .claude (it starts with a literal `.`, as a shell glob
/// does not match a leading dot) followed by one that may match hooks or a
/// settings file.
fn sed_glob_protected(word: &str, cwd: &str) -> Result<bool, Fault> {
    if !word.contains(['*', '?', '[']) {
        return Ok(false);
    }
    let resolved = pyos::normpath(&pyos::join(cwd, &pyos::expanduser(word)?)).to_lowercase();
    let parts: Vec<&str> = resolved.split('/').collect();
    Ok(parts.windows(2).any(|pair| {
        let (c, nxt) = (pair[0], pair[1]);
        c.starts_with('.')
            && fnmatch_case(".claude", c)
            && SED_GLOB_NAMES.iter().any(|name| fnmatch_case(name, nxt))
    }))
}

/// `sed_protected_cwd`: cwd is a .claude directory or inside .claude/hooks.
fn sed_protected_cwd(cwd: &str) -> bool {
    let path = pyos::normpath(cwd);
    SED_PROTECTED.is_match(&format!("{path}/")) || basename(&path).to_lowercase() == ".claude"
}

impl Analyzer<'_> {
    /// `check_sed`: sed run as `words[k]` (`flags`: the words' expansion state;
    /// `via`: the wrappers that run it) that may touch .claude/settings* or
    /// .claude/hooks/* asks unless every script is read-only (the method's
    /// docstring in the Python guard lists when it may touch them).
    pub fn check_sed(
        &mut self,
        words: &[String],
        flags: &[Expanded],
        k: usize,
        cmd: &Command,
        state: &State,
        via: &BTreeSet<String>,
    ) -> Result<(), Fault> {
        let cwd = state.cwd.as_str();
        let mut args = &words[k + 1..];
        let mut flags = &flags[k + 1..];
        if via.contains("find") {
            for j in 0..args.len() {
                if args[j] == ";" || (args[j] == "+" && j > 0 && args[j - 1] == "{}") {
                    args = &args[..j];
                    flags = &flags[..j];
                    break;
                }
            }
        }
        let mut known = state.vars.clone();
        known
            .entry("PWD".to_string())
            .or_insert_with(|| cwd.to_string());
        if !known.contains_key("HOME") {
            known.insert("HOME".to_string(), pyos::expanduser("~")?);
        }
        // A digit or `-` is a descriptor (2>&1, >&-), not a file.
        let files = |targets: &[String]| -> Vec<String> {
            targets
                .iter()
                .map(|w| expand_vars(w, &known))
                .filter(|w| !(pystr::is_digit(w) || w == "-"))
                .collect()
        };
        let protected = |w: &str| -> Result<bool, Fault> {
            Ok(sed_protected(w, cwd)? || sed_glob_protected(w, cwd)?)
        };
        // Targets keep their unresolved expansions: "$ROOT/.claude/settings.json"
        // names the path as plainly as a literal does.
        let mut outs = Vec::new();
        for w in files(&cmd.out_files) {
            if protected(&w)? {
                outs.push(w);
            }
        }
        let ins = files(&cmd.stdin_files);
        // The command's own text names a protected path: its source, with the
        // bodies of its substitutions, and its words with the variables the guard
        // knows.
        let named = std::iter::once(&cmd.src)
            .chain(words)
            .chain(&cmd.heredocs)
            .chain(&ins)
            .any(|w| SED_PROTECTED.is_match(w));
        // A body that names them may write them whatever sed's script is.
        let in_body = cmd
            .bodies
            .iter()
            .chain(&cmd.heredocs)
            .any(|b| SED_PROTECTED.is_match(b));
        let mut hit = !outs.is_empty() || named || in_body;
        if !hit {
            let mut operands = sed_files(args, flags, cwd)?;
            operands.extend(ins.iter().cloned());
            for w in &operands {
                if !unresolved(w) && protected(w)? {
                    hit = true;
                    break;
                }
            }
        }
        let mut texts: Vec<&str> = self
            .texts
            .iter()
            .filter(|(_, script)| !script)
            .map(|(t, _)| t.as_str())
            .collect();
        if state.script {
            texts.push(&cmd.src);
        }
        let mentions = texts.iter().any(|t| MENTIONS_CLAUDE.is_match(t));
        let protected_cwd = sed_protected_cwd(cwd);
        if !hit && (mentions || protected_cwd) {
            hit = via.iter().any(|w| ARG_WRAPPERS.contains(&w.as_str()))
                || args
                    .iter()
                    .zip(flags)
                    .any(|(w, e)| (e.any() && unresolved(w)) || w.contains("{}"));
        }
        if !hit && protected_cwd {
            hit = sed_may_open(args, flags);
        }
        if !hit {
            return Ok(());
        }
        let mut problem = if in_body {
            Some(
                "a substitution, here-string or heredoc body names .claude/settings* or .claude/hooks/*"
                    .to_string(),
            )
        } else {
            sed_problem(args, flags, named)
        };
        if problem.is_none() {
            if let Some(out) = outs.first() {
                problem = Some(format!("its output is redirected into {out}"));
            }
        }
        if let Some(problem) = problem {
            self.verdict.ask(verdict::SED, sed_ask(&problem));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn run_positions_match_python() {
        // sorted(run_positions(shlex.split(t), 0)) on CPython 3.13.
        for (text, want) in [
            ("sed -n 1p f", vec![0]),
            ("env A=1 -u X sed x", vec![0, 4]),
            ("timeout -s KILL 5 sed x", vec![0, 4]),
            ("sudo -u root nice -n 5 sed x", vec![0, 3, 6]),
            ("find . -exec sed x {} ; -ok gsed y", vec![0, 3, 8]),
            ("parallel sed ::: a", vec![0, 1, 2, 3]),
            ("xargs -0 -I{} sed x", vec![0, 3]),
            ("flock -w 3 lock sed x", vec![0, 4]),
            ("env --chdir / -- sed", vec![0, 4]),
            ("time -f %e sed", vec![0, 3]),
        ] {
            let words: Vec<String> = text.split(' ').map(str::to_string).collect();
            let got: Vec<usize> = run_positions(&words, 0).into_iter().collect();
            assert_eq!(got, want, "{text}");
        }
    }

    #[test]
    fn sed_scripts_read_like_python() {
        // sed_script_problem(s) and sed_script_opens(s) on CPython 3.13.
        for (script, problem, opens) in [
            ("1p", None, false),
            ("$!N;P;D", None, false),
            ("/a/,/b/p", None, false),
            ("0~3d", None, false),
            ("1~p", Some("a first~step address without a step"), true),
            (
                "1,xp",
                Some("an address form other than a line number, $, first~step or /regex/"),
                true,
            ),
            ("s/a/b/", Some("the 's' command"), false),
            ("s/a/b/w out", Some("the 's' command"), true),
            ("s/[/]/x/", Some("the 's' command"), true),
            ("y/ab/cd/", Some("the 'y' command"), false),
            ("w /tmp/x", Some("the 'w' command"), true),
            ("1r f", Some("the 'r' command"), true),
            ("p x", Some("text after the 'p' command"), true),
            ("{p}", None, false),
            ("/a\nb/p", Some("a newline inside a /regex/"), true),
            ("/abc", Some("a /regex/ without its closing /"), true),
            (
                "/[[:alpha:]/p",
                Some("a /regex/ whose bracket expression GNU and BSD sed end differently"),
                true,
            ),
            ("b end; p", Some("the 'b' command"), true),
            ("# c; p", Some("the '#' command"), false),
            ("a\\\nfoo", Some("the 'a' command"), false),
            ("e ls", Some("the 'e' command"), true),
            ("q5", Some("text after the 'q' command"), false),
            ("l 3", Some("text after the 'l' command"), false),
            ("F", Some("the 'F' command"), false),
        ] {
            assert_eq!(
                sed_script_problem(script).as_deref(),
                problem,
                "problem {script:?}"
            );
            assert_eq!(sed_script_opens(script), opens, "opens {script:?}");
        }
    }

    #[test]
    fn sed_options_read_like_python() {
        // sed_problem(args, flags) with every word unexpanded, on CPython 3.13.
        let plain =
            |args: &[&str]| sed_problem(&strings(args), &vec![Expanded::No; args.len()], false);
        assert_eq!(plain(&["-n", "1p", "f"]), None);
        assert_eq!(plain(&["-i", "1p", "f"]).as_deref(), Some("option -i"));
        assert_eq!(plain(&["-n", "--", "1p"]), None);
        assert_eq!(plain(&["--quiet", "-e", "1p"]), None);
        assert_eq!(
            plain(&["--in-place", "1p"]).as_deref(),
            Some("option --in-place")
        );
        assert_eq!(plain(&["-ne"]).as_deref(), Some("-e without a script"));
        assert_eq!(plain(&[]).as_deref(), Some("no script"));
        assert_eq!(
            plain(&["1p", "f", "-i"]).as_deref(),
            Some("option -i after the operand '1p' (BSD sed reads it as a file and the first operand as the script)")
        );
        let quoted = sed_problem(
            &strings(&["$S", "f"]),
            &[Expanded::Quoted, Expanded::No],
            false,
        );
        assert_eq!(
            quoted.as_deref(),
            Some("the script '$S' comes from a shell expansion")
        );
        let bare = sed_problem(&strings(&["$S"]), &[Expanded::Bare], false);
        assert_eq!(
            bare.as_deref(),
            Some("$S comes from a shell expansion and may split into options")
        );
        let option = sed_problem(
            &strings(&["$OPT", "1p"]),
            &[Expanded::Quoted, Expanded::No],
            true,
        );
        assert_eq!(
            option.as_deref(),
            Some("$OPT comes from a shell expansion and may be an option")
        );
        assert!(
            sed_may_be_option("${X}y") && !sed_may_be_option("$DIR/x") && !sed_may_be_option("x$Y")
        );
        assert!(sed_may_be_option("$(...)") && !sed_may_be_option("$(...)/f"));
    }

    #[test]
    fn protected_paths_match_python() {
        // sed_protected, sed_glob_protected and sed_protected_cwd on CPython 3.13.
        assert!(sed_protected(".claude/settings.json", "/r").expect("no fault"));
        assert!(sed_protected("settings.json", "/r/.claude").expect("no fault"));
        assert!(sed_protected("../hooks/x", "/r/.claude/agents").expect("no fault"));
        assert!(!sed_protected("agents/x.md", "/r/.claude").expect("no fault"));
        assert!(sed_glob_protected(".c*/settings*", "/r").expect("no fault"));
        assert!(sed_glob_protected("/r/.?laude/HOOKS/*", "/").expect("no fault"));
        assert!(!sed_glob_protected("*/settings.json", "/r").expect("no fault"));
        assert!(!sed_glob_protected(".claude/agents/*", "/r").expect("no fault"));
        assert!(sed_protected_cwd("/r/.claude") && sed_protected_cwd("/r/.claude/hooks/tests"));
        assert!(!sed_protected_cwd("/r/.claude/agents"));
    }
}
