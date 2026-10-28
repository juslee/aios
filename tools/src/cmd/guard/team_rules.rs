//! Rule 11's team rules for commands (git-push-guard.py L1556-1812, L1846-2001
//! and L3071-3166, L3223-3306): rule 1 (every QEMU start goes through the lock
//! wrapper, and only the verifier's), rule 2 (no pattern kills, no toolchain
//! changes) and rule 4 rows 4-6 (agents reset, commit and move refs only in their
//! own temporary worktree).

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::Regex;

use super::analyzer::{basename, partition, unresolved, Analyzer, Flow, State, Stop, ARG_WRAPPERS};
use super::git_rules::{git_env_risky, scan_git_options};
use super::pattern::{ci, re, B_AFTER, PID_FINDER};
use super::pyos::{self, Fault};
use super::repo::Repo;
use super::shell::{Command, Expanded, ParseError, MAX_DEPTH};
use super::shlex;
use super::verdict;
use crate::pystr;

/// The recipes that start QEMU.
const QEMU_RECIPES: [&str; 7] = [
    "run",
    "run-display",
    "run-gpu",
    "run-input",
    "run-direct",
    "debug",
    "soak",
];
/// just options that take a value (so it is not mistaken for a recipe name).
const JUST_VALUE_LONGS: [&str; 24] = [
    "working-directory",
    "justfile",
    "shell",
    "shell-arg",
    "chooser",
    "color",
    "command-color",
    "dotenv-filename",
    "dotenv-path",
    "dump-format",
    "list-heading",
    "list-prefix",
    "highlight",
    "timestamp-format",
    "tempdir",
    "alias-style",
    "ceiling",
    "cygpath",
    "evaluate-format",
    "group",
    "indentation",
    "jobs",
    "justfile-name",
    "completions",
];
const JUST_VALUE_SHORTS: &str = "dfEF";
/// just options that print or edit instead of running recipes.
const JUST_NO_RUN_LONGS: [&str; 18] = [
    "dry-run",
    "show",
    "evaluate",
    "list",
    "summary",
    "dump",
    "json",
    "variables",
    "groups",
    "usage",
    "changelog",
    "completions",
    "man",
    "fmt",
    "init",
    "edit",
    "help",
    "version",
];
const JUST_NO_RUN_SHORTS: &str = "nlsehV";
/// qemu-system-* options that print and exit before any machine starts, as the
/// first argument.
const QEMU_INFO_ARGS: [&str; 5] = ["--version", "-version", "-h", "--help", "-help"];
/// QEMU-starting scripts deleted here (#246, R4) but still present in older
/// checkouts used as soak arms.
const QEMU_SCRIPTS: [&str; 2] = ["soak-matrix.sh", "soak-qemu.sh"];
const WRAPPER_PATH: &str = "scripts/agent/qemu-lock.sh";
/// qemu-lock.sh run's options, each taking the next word as its value.
const WRAPPER_VALUE_OPTS: [&str; 6] = [
    "--team",
    "--mode",
    "--label",
    "--eta-min",
    "--max-load",
    "--settle-min",
];
const GIT_COMMITTERS: [&str; 5] = ["commit", "merge", "cherry-pick", "revert", "am"];
/// Rule 4: what an agent never runs in the main checkout besides committing.
const MAIN_CHECKOUT_REF_CHANGERS: [&str; 6] = [
    "rebase",
    "pull",
    "update-ref",
    "reset",
    "commit-tree",
    "fast-import",
];
/// Environment variables that move git to another repository, working tree or
/// index (rule 4 denies them on an agent's reset or commit).
const PLACEMENT_ENV: [&str; 4] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_COMMON_DIR",
    "GIT_INDEX_FILE",
];
const RUSTUP_TOP_CHANGES: [&str; 3] = ["update", "install", "uninstall"];
/// More rustup commands that install or switch toolchains; denied to agents only.
const RUSTUP_AGENT_TOP: [&str; 4] = ["set", "run", "upgrade", "up"];
/// rustup proxies: `<proxy> +<toolchain>` runs, and may install, that toolchain.
const RUST_PROXIES: [&str; 13] = [
    "cargo",
    "rustc",
    "rustdoc",
    "rustfmt",
    "cargo-fmt",
    "cargo-clippy",
    "clippy-driver",
    "cargo-miri",
    "rust-gdb",
    "rust-gdbgui",
    "rust-lldb",
    "rust-analyzer",
    "rls",
];
const AGENT_TOOLCHAIN_DENY: &str =
    "agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)";
const PKILL_VALUE_OPTS: [&str; 13] = [
    "-F", "-G", "-g", "-J", "-j", "-M", "-N", "-P", "-s", "-T", "-t", "-U", "-u",
];
const REGEX_META: &str = ".^$*+?()[]{}|\\";
const QEMU_BINARY: &str = "qemu-system-aarch64";

/// `RUSTUP_SUB_CHANGES[top]`: subcommands that change the toolchain.
fn rustup_sub_changes(top: &str) -> &'static [&'static str] {
    match top {
        "toolchain" => &["install", "uninstall", "remove"],
        "component" => &["add", "remove"],
        _ => &[],
    }
}

/// `RUSTUP_AGENT_SUB[top]`: more subcommands denied to agents.
fn rustup_agent_sub(top: &str) -> &'static [&'static str] {
    match top {
        "toolchain" => &["install", "add", "uninstall", "remove", "link"],
        "target" => &["add", "remove", "install", "uninstall"],
        "self" => &["update", "uninstall", "upgrade-data"],
        _ => &[],
    }
}

/// `TEMP_WORKTREE_RE`, matched whole: the temporary worktree names Claude Code
/// gives (agent-<id> for an Agent-tool spawn, wf_<run>-<n> for a workflow agent).
static TEMP_WORKTREE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"^(?:agent-[A-Za-z0-9]+|wf_[0-9a-f]{8}-[0-9a-f]{3}-(?:0|[1-9][0-9]{0,3}))$")
});

/// `PLACEMENT_CONFIG.match` (`re.I`): config keys that move git elsewhere.
static PLACEMENT_CONFIG: LazyLock<Regex> = LazyLock::new(|| {
    re(&format!(
        r"^(?:{}\n?$|{}\n?$|{}|{})",
        ci("core.worktree"),
        ci("core.bare"),
        ci("include."),
        ci("includeif.")
    ))
});

/// Whether a command starts QEMU, as rule 1 reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Qemu {
    /// qemu-lock.sh run.
    Wrapper,
    /// Any other QEMU start.
    Other,
    /// Part of the command comes from an expansion, so the guard cannot tell.
    Unclear,
}

/// `classify_only`: `soak --classify LOG...` classifies saved logs and starts no
/// QEMU. Only a --classify before the `--` that ends the options counts.
fn classify_only(args: &[String]) -> bool {
    args.iter()
        .take_while(|a| *a != "--")
        .any(|a| a == "--classify" || a.starts_with("--classify="))
}

/// `aios_starts_qemu`: `aios [options] soak ...` (not --classify), the tools
/// binary's soak; unclear when the subcommand comes from an expansion.
fn aios_starts_qemu(args: &[String]) -> Option<Qemu> {
    let at = args.iter().position(|a| !a.starts_with('-'))?;
    let first = &args[at];
    if unresolved(first) {
        return Some(Qemu::Unclear);
    }
    (first == "soak" && !classify_only(&args[at + 1..])).then_some(Qemu::Other)
}

/// `cargo_starts_qemu`: `cargo run|r ... -- [options] soak ...`, the tools
/// binary's soak whichever way the package is named.
fn cargo_starts_qemu(args: &[String]) -> Option<Qemu> {
    let mut i = usize::from(args.first().is_some_and(|a| a.starts_with('+')));
    let mut sub = None;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" {
            return None;
        }
        if a.starts_with('-') {
            if ["--config", "-Z", "-C", "--color"].contains(&a) {
                i += 1;
            }
            continue;
        }
        sub = Some(a);
        break;
    }
    if !matches!(sub, Some("run" | "r")) {
        return None;
    }
    let dashes = i + args.get(i..)?.iter().position(|a| a == "--")?;
    aios_starts_qemu(&args[dashes + 1..])
}

/// `is_qemu_script`: a soak script, anything named qemu-lock.sh, or a script
/// called the lock wrapper's way (`run --team ...`, so a renamed copy counts).
pub fn is_qemu_script(path: &str, argv: &[String]) -> bool {
    let name = basename(path);
    if QEMU_SCRIPTS.contains(&name) || name == "qemu-lock.sh" {
        return true;
    }
    if argv.first().map(String::as_str) != Some("run") {
        return false;
    }
    let opts = match argv.iter().position(|a| a == "--") {
        Some(end) => &argv[..end],
        None => argv,
    };
    opts.iter()
        .any(|a| a == "--team" || a.starts_with("--team="))
}

/// `may_split`: `word` (`flag` its expansion state) can become other words at
/// run time: an unquoted expansion that is unresolved or has whitespace, or,
/// outside a value position, a quoted unresolved one (which may be `--`).
fn may_split(word: &str, flag: Expanded, value: bool) -> bool {
    if !flag.any() {
        return false;
    }
    if flag == Expanded::Bare && (unresolved(word) || word.chars().any(pystr::is_space)) {
        return true;
    }
    !value && unresolved(word)
}

/// `wrapped_start`: (start, clear) for the command qemu-lock.sh `run` executes
/// (argv[0] is the subcommand): where it starts, just after the `--` that ends
/// the options (`None` without one), and whether the guard can tell where the
/// options end.
fn wrapped_start(argv: &[String], flags: &[Expanded]) -> (Option<usize>, bool) {
    let flag = |j: usize| flags.get(j).copied().unwrap_or_default();
    let mut i = 1;
    while i + 1 < argv.len() && WRAPPER_VALUE_OPTS.contains(&argv[i].as_str()) {
        i += 2;
    }
    let mut clear = !(2..i)
        .step_by(2)
        .any(|j| may_split(&argv[j], flag(j), true));
    if i < argv.len() && may_split(&argv[i], flag(i), false) {
        clear = false;
    }
    let start = argv
        .get(i..)
        .and_then(|rest| rest.iter().position(|a| a == "--"))
        .map(|p| i + p + 1);
    (start, clear)
}

/// `main_checkout_change`: the git commands an agent never runs in the main
/// checkout, as named in the deny text.
fn main_checkout_change(sub: &str, args: &[String]) -> Option<String> {
    if GIT_COMMITTERS.contains(&sub) {
        return Some("commit".to_string());
    }
    if MAIN_CHECKOUT_REF_CHANGERS.contains(&sub) {
        return Some(sub.to_string());
    }
    let positional: Vec<&String> = args.iter().filter(|a| !a.starts_with('-')).collect();
    let found = match sub {
        "symbolic-ref" => (positional.len() >= 2
            || !scan_git_options(args, &[("delete", false)], "d", "", "").is_empty())
        .then_some(sub),
        "stash" => (!matches!(
            positional.first().map(|p| p.as_str()),
            Some("list" | "show")
        ))
        .then_some(sub),
        "branch" => (!scan_git_options(args, &[("force", false)], "DfMC", "u", "").is_empty())
            .then_some("branch -D/-f/-M/-C"),
        "checkout" => {
            (!scan_git_options(args, &[], "B", "bB", "").is_empty()).then_some("checkout -B")
        }
        "switch" => (!scan_git_options(args, &[("force-create", true)], "C", "cC", "").is_empty())
            .then_some("switch -C"),
        _ => None,
    };
    found.map(str::to_string)
}

/// `qemu_lock_dir`: `<git-common-dir>/aios-agent/qemu.lock` of the repository
/// `cwd` is in.
fn qemu_lock_dir(repo: &Repo) -> Result<Option<String>, Fault> {
    let out = repo
        .run(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?
        .unwrap_or_default();
    let common = pystr::strip(&out);
    Ok((!common.is_empty()).then(|| pyos::join(&pyos::join(common, "aios-agent"), "qemu.lock")))
}

/// `lock_message`: who holds the lock, from its owner file. The owner file is
/// written after the wrapper's mkdir, so it can be missing while the lock is
/// held; it only supplies this message.
fn lock_message(lock: &str) -> String {
    let mut fields: HashMap<String, String> = HashMap::new();
    if let Ok(text) = pyos::read_text_replace(&pyos::join(lock, "owner")) {
        for line in pystr::splitlines(&text) {
            let (key, eq, value) = partition(line, '=');
            if eq {
                fields.insert(
                    pystr::strip(key).to_string(),
                    pystr::strip(value).to_string(),
                );
            }
        }
    }
    let get = |key: &str| fields.get(key).map(String::as_str);
    let Some(team) = get("team") else {
        return "QEMU lock held (owner file not written yet)".to_string();
    };
    let eta_raw = get("eta").unwrap_or("?");
    let eta = local_minute(eta_raw).unwrap_or_else(|| eta_raw.to_string());
    format!(
        "QEMU lock held by {team}/{} mode={} until {eta} (rule 11)",
        get("branch").unwrap_or("?"),
        get("mode").unwrap_or("?")
    )
}

/// `time.strftime("%Y-%m-%d %H:%M", time.localtime(int(value)))`, or `None`
/// where Python raises (not an `int()` literal, or a time the platform cannot
/// represent). The standard library has no local-time conversion, so the
/// system's `date` formats the instant under the same `TZ` the C library reads.
fn local_minute(value: &str) -> Option<String> {
    let seconds = python_int(value)?;
    let mut date = std::process::Command::new("date");
    if cfg!(target_os = "macos") {
        date.arg("-r").arg(seconds.to_string());
    } else {
        date.arg("-d").arg(format!("@{seconds}"));
    }
    let out = date
        .arg("+%Y-%m-%d %H:%M")
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8(out.stdout).ok()?;
    Some(text.trim_end_matches('\n').to_string())
}

/// `int(s)` for a base-10 literal: surrounding whitespace, an optional sign,
/// decimal digits (any script) with single underscores between them. `None` for
/// anything `int()` rejects, or a value past `i128` (which `localtime` rejects
/// anyway).
fn python_int(s: &str) -> Option<i128> {
    let s = pystr::int_strip(s);
    let (negative, digits) = match s.chars().next()? {
        '-' => (true, &s[1..]),
        '+' => (false, &s[1..]),
        _ => (false, s),
    };
    if digits.is_empty()
        || digits.starts_with('_')
        || digits.ends_with('_')
        || digits.contains("__")
    {
        return None;
    }
    let mut value: i128 = 0;
    for c in digits.chars().filter(|&c| c != '_') {
        let d = i128::from(pystr::decimal_value(c)?);
        value = value.checked_mul(10)?.checked_add(d)?;
    }
    Some(if negative { -value } else { value })
}

/// `temp_worktree`: the name of the temporary worktree `repo` is in: <name> is
/// agent-<id> or wf_<8 hex>-<3 hex>-<n>, the working tree is exactly
/// <main>/.claude/worktrees/<name>, the branch is worktree-<name>, and git's
/// admin directory is <common>/worktrees/<name>.
pub fn temp_worktree(repo: &Repo) -> Result<Option<String>, Fault> {
    let roots = repo.roots()?;
    let [top, main] = roots.as_slice() else {
        return Ok(None);
    };
    let name = pyos::basename(top);
    let expected = pyos::join(&pyos::join(&pyos::join(main, ".claude"), "worktrees"), name);
    if !TEMP_WORKTREE.is_match(name) || *top != expected {
        return Ok(None);
    }
    let (admin, common) = repo.git_dirs()?;
    let Some(common) = common else {
        return Ok(None);
    };
    if admin != Some(pyos::join(&pyos::join(&common, "worktrees"), name)) {
        return Ok(None);
    }
    let branch = repo.current_branch()?;
    Ok((branch.as_deref() == Some(&format!("worktree-{name}"))).then(|| name.to_string()))
}

/// `main_checkout`: the working tree is the main worktree.
fn main_checkout(repo: &Repo) -> Result<bool, Fault> {
    let roots = repo.roots()?;
    Ok(roots.len() == 2 && roots[0] == roots[1])
}

impl Analyzer<'_> {
    /// `check_team_command`: rules 1 and 2 for the command whose name is
    /// `words[k]`; `via` holds the names of the wrappers that run it. True when
    /// the command is fully handled and nothing after it must be read (a
    /// recognised QEMU start or script).
    #[allow(clippy::too_many_arguments)]
    pub fn check_team_command(
        &mut self,
        words: &[String],
        flags: &[Expanded],
        k: usize,
        cmd: &Command,
        state: &mut State,
        depth: usize,
        via: &BTreeSet<String>,
    ) -> Result<bool, Stop> {
        let word = words[k].as_str();
        let name = basename(word);
        let args = &words[k + 1..];
        if name == "pkill" || name == "killall" {
            self.check_pkill(name, args);
            return Ok(false);
        }
        if name == "kill" {
            self.check_kill(args, cmd, state, via);
            return Ok(false);
        }
        if name == "rustup" || name == "rustup-init" {
            self.check_rustup(name, args);
            return Ok(false);
        }
        let mut kind = None;
        if RUST_PROXIES.contains(&name) {
            self.check_rust_proxy(args);
            if name == "cargo" {
                kind = cargo_starts_qemu(args);
            }
        } else if name == QEMU_BINARY {
            let info = args
                .first()
                .is_some_and(|a| QEMU_INFO_ARGS.contains(&a.as_str()));
            kind = (!info).then_some(Qemu::Other);
        } else if name == "just" {
            kind = self.just_starts_qemu(args, state, depth)?;
        } else if name == "aios"
            || (unresolved(word) && args.first().map(String::as_str) == Some("soak"))
        {
            let Some(kind) = aios_starts_qemu(args) else {
                return Ok(false);
            };
            self.qemu_decision(kind, None)?;
            return Ok(true);
        } else if is_qemu_script(word, args) {
            self.check_qemu_script(word, args, &flags[k + 1..], cmd, state, depth, true, via)?;
            return Ok(true);
        }
        if let Some(kind) = kind {
            self.qemu_decision(kind, None)?;
        }
        Ok(false)
    }

    /// `check_qemu_script`: a script that starts QEMU: the soak scripts, and the
    /// lock wrapper (only its `run`). Only the genuine wrapper (genuine_wrapper)
    /// is trusted and left unread; for every `run`, the command after its `--` is
    /// analysed (analyze_wrapped).
    #[allow(clippy::too_many_arguments)]
    pub fn check_qemu_script(
        &mut self,
        path: &str,
        argv: &[String],
        flags: &[Expanded],
        cmd: &Command,
        state: &mut State,
        depth: usize,
        shebang: bool,
        via: &BTreeSet<String>,
    ) -> Flow {
        let name = basename(path);
        if QEMU_SCRIPTS.contains(&name) {
            if !(name == "soak-qemu.sh" && classify_only(argv)) {
                self.qemu_decision(Qemu::Other, None)?;
            }
            return Ok(());
        }
        let genuine = if unresolved(path) {
            None
        } else {
            self.genuine_wrapper(path, &state.cwd)?
        };
        let sub_unclear = argv.first().is_some_and(|first| {
            may_split(first, flags.first().copied().unwrap_or_default(), false)
        });
        if sub_unclear {
            self.verdict.ask(
                verdict::R1,
                format!(
                    "cannot tell which qemu-lock.sh subcommand runs ({} comes from a shell expansion)",
                    argv[0]
                ),
            );
        }
        if argv.first().map(String::as_str) == Some("run") || sub_unclear {
            if unresolved(path) {
                self.verdict.ask(
                    verdict::R1,
                    format!(
                        "the QEMU lock wrapper's path ({path}) comes from a shell expansion; the guard cannot tell it is scripts/agent/qemu-lock.sh (rule 11)"
                    ),
                );
            }
            self.qemu_decision(Qemu::Wrapper, genuine.as_deref())?;
            self.analyze_wrapped(argv, flags, cmd, state, depth, via)?;
        }
        if !unresolved(path) && genuine.is_none() {
            self.read_script(path, state, depth, shebang)?;
        }
        Ok(())
    }

    /// `analyze_wrapped`: the command a qemu-lock.sh `run` executes (the words
    /// after the `--` that ends its options) is analysed like one typed on its
    /// own, except that a QEMU start in it is the wrapper's. An argument wrapper
    /// that runs the call appends its words to the wrapped command, so it is
    /// analysed with that wrapper in front.
    fn analyze_wrapped(
        &mut self,
        argv: &[String],
        flags: &[Expanded],
        cmd: &Command,
        state: &State,
        depth: usize,
        via: &BTreeSet<String>,
    ) -> Flow {
        let (start, clear) = wrapped_start(argv, flags);
        if !clear {
            self.verdict.ask(
                verdict::R1,
                "cannot tell where the command qemu-lock.sh run wraps starts (an option word comes from a shell expansion)",
            );
        }
        let supplier = via
            .iter()
            .filter(|w| ARG_WRAPPERS.contains(&w.as_str()))
            .min()
            .cloned();
        let start = match start {
            Some(s) if s != argv.len() => s,
            _ => {
                if let Some(supplier) = supplier {
                    self.verdict.ask(
                        verdict::R1,
                        format!(
                            "the command qemu-lock.sh run wraps is supplied by {supplier}; the guard cannot see it"
                        ),
                    );
                }
                return Ok(()); // no command: the wrapper refuses to run
            }
        };
        let mut inner = Command::default();
        if let Some(supplier) = &supplier {
            inner.words.push(supplier.clone());
            inner.expanded.push(Expanded::No);
        }
        inner.words.extend_from_slice(&argv[start..]);
        inner
            .expanded
            .extend_from_slice(flags.get(start..).unwrap_or(&[]));
        inner.src = cmd.src.clone();
        inner.bodies = cmd.bodies.clone();
        let mut sub = state.clone();
        let outer = self.wrapped;
        self.wrapped = true;
        let result = if depth + 1 > MAX_DEPTH {
            Err(Stop::Parse(ParseError("nesting too deep")))
        } else {
            self.analyze_command(&inner, &mut sub, depth + 1)
        };
        self.wrapped = outer;
        match result {
            Err(Stop::Parse(e)) => {
                self.verdict.ask(
                    verdict::R1,
                    format!(
                        "could not parse the command qemu-lock.sh run wraps ({})",
                        e.0
                    ),
                );
                Ok(())
            }
            other => other,
        }
    }

    /// `genuine_wrapper`: the root whose scripts/agent/qemu-lock.sh `path`
    /// resolves to: the main checkout, the project directory, or the agent's own
    /// temporary worktree; for a main thread also the working tree of `cwd` when
    /// it is a linked worktree of the same repository. The path as typed must be
    /// named qemu-lock.sh and must not itself be a symlink.
    fn genuine_wrapper(&self, path: &str, cwd: &str) -> Result<Option<String>, Fault> {
        let typed = pyos::join(cwd, &pyos::expanduser(path)?);
        if pyos::basename(&typed) != "qemu-lock.sh" || pyos::islink(&typed) {
            return Ok(None);
        }
        let target = pyos::realpath(&typed)?;
        let mut roots: Vec<String> = Vec::new();
        let payload_cwd = self.ctx.cwd.as_str();
        if !payload_cwd.is_empty() {
            let repo = self.repo(payload_cwd, &[]);
            let found = repo.roots()?;
            if let Some(last) = found.last() {
                roots.push(last.clone());
            }
            if self.is_agent() && self.own_temp_worktree(&repo)? {
                roots.push(found[0].clone());
            }
            if !self.is_agent() {
                if let Some(last) = found.last() {
                    let here = self.repo(cwd, &[]).roots()?;
                    if here.len() == 2 && here[1] == *last {
                        roots.push(here[0].clone());
                    }
                }
            }
        }
        if let Some(project) = std::env::var_os("CLAUDE_PROJECT_DIR").filter(|p| !p.is_empty()) {
            roots.push(project.to_string_lossy().into_owned());
        }
        for root in roots {
            let wrapper = pyos::join(&root, WRAPPER_PATH);
            let real = pyos::join(
                &pyos::realpath(pyos::dirname(&wrapper))?,
                pyos::basename(&wrapper),
            );
            if target == real && !pyos::islink(&real) {
                return Ok(Some(root));
            }
        }
        Ok(None)
    }

    /// `just_starts_qemu`: Other when the just invocation runs a QEMU recipe,
    /// Unclear when a recipe name comes from an expansion, `None` when it runs
    /// none or only prints. `just --command CMD` and `--dotenv-command CMD` run
    /// CMD, which is analysed as a command line.
    fn just_starts_qemu(
        &mut self,
        args: &[String],
        state: &State,
        depth: usize,
    ) -> Result<Option<Qemu>, Stop> {
        let cwd = state.cwd.clone();
        let mut hits: Vec<&String> = Vec::new();
        let mut no_run = false;
        let mut i = 0;
        let mut positional_only = false;
        while i < args.len() {
            let a = args[i].as_str();
            i += 1;
            if positional_only || !a.starts_with('-') || a == "-" {
                if !a.contains('=') || positional_only {
                    hits.push(&args[i - 1]);
                }
                // From the first recipe name or override on, every word is a
                // recipe argument, so only options before it can make just print
                // instead of run.
                positional_only = true;
                continue;
            }
            if a == "--" {
                positional_only = true;
            } else if let Some(long) = a.strip_prefix("--") {
                let (name, eq, value) = partition(long, '=');
                if name == "command" {
                    let mut rest: Vec<String> = Vec::new();
                    if eq {
                        rest.push(value.to_string());
                    }
                    rest.extend_from_slice(&args[i..]);
                    self.analyze_text(&shlex::join(&rest), &cwd, depth + 1, false)?;
                    return Ok(None);
                }
                if name == "dotenv-command" {
                    let mut value = value.to_string();
                    if !eq {
                        value = args.get(i).cloned().unwrap_or_default();
                        i += 1;
                    }
                    self.analyze_text(&value, &cwd, depth + 1, false)?;
                    continue;
                }
                if JUST_NO_RUN_LONGS.contains(&name) {
                    no_run = true;
                }
                if name == "set" && !eq {
                    i += 2;
                } else if !eq && JUST_VALUE_LONGS.contains(&name) {
                    i += 1;
                }
            } else {
                let cluster: Vec<char> = a.chars().skip(1).collect();
                for (pos, &ch) in cluster.iter().enumerate() {
                    let pos = pos + 1;
                    if ch == 'c' {
                        let mut rest: Vec<String> = Vec::new();
                        if pos < cluster.len() {
                            rest.push(cluster[pos..].iter().collect());
                        }
                        rest.extend_from_slice(args.get(i..).unwrap_or(&[]));
                        self.analyze_text(&shlex::join(&rest), &cwd, depth + 1, false)?;
                        return Ok(None);
                    }
                    if JUST_NO_RUN_SHORTS.contains(ch) {
                        no_run = true;
                    }
                    if JUST_VALUE_SHORTS.contains(ch) {
                        if pos == cluster.len() {
                            i += 1;
                        }
                        break;
                    }
                }
            }
        }
        if no_run {
            return Ok(None);
        }
        if hits.iter().any(|h| unresolved(h)) {
            return Ok(Some(Qemu::Unclear));
        }
        let recipes: BTreeSet<&str> = hits
            .iter()
            .map(|h| h.as_str())
            .filter(|h| QEMU_RECIPES.contains(h))
            .collect();
        if recipes.is_empty() {
            return Ok(None);
        }
        if recipes.len() == 1 && recipes.contains("soak") && classify_only(args) {
            return Ok(None);
        }
        Ok(Some(Qemu::Other))
    }

    /// `qemu_decision`: rule 1 rows 1-7 for a command that starts QEMU. Inside the
    /// command a wrapper `run` executes there is nothing to decide. The lock is
    /// found from `lock_root` (the genuine wrapper's root), else from the
    /// payload's cwd.
    fn qemu_decision(&mut self, kind: Qemu, lock_root: Option<&str>) -> Result<(), Fault> {
        if self.wrapped {
            return Ok(());
        }
        if kind == Qemu::Unclear {
            self.verdict.ask(
                verdict::R1,
                "cannot tell whether this starts QEMU: part of the command comes from a shell expansion; every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)",
            );
            return Ok(());
        }
        let root = lock_root.unwrap_or(self.ctx.cwd.as_str());
        let held = if root.is_empty() {
            None
        } else {
            qemu_lock_dir(&self.repo(root, &[]))?
        };
        match held {
            Some(lock) if pyos::exists(&lock) => {
                self.verdict.rule_deny(verdict::R1, lock_message(&lock));
            }
            _ if kind != Qemu::Wrapper => {
                self.verdict.rule_deny(
                    verdict::R1,
                    "every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)",
                );
            }
            _ if self.is_agent() => match self.ctx.agent_type.as_ref().and_then(|t| t.as_str()) {
                Some(t) if !t.is_empty() => {
                    if t != "verifier" {
                        self.verdict
                            .rule_deny(verdict::R1, "only the verifier starts QEMU (rule 11)");
                    }
                }
                _ => self.verdict.rule_deny(
                    verdict::R1,
                    "cannot tell which agent this is; only the verifier starts QEMU",
                ),
            },
            _ if !self.ctx.team.is_empty() => {
                self.verdict.rule_deny(
                    verdict::R1,
                    "team leads route boots to their verifier (rule 11)",
                );
            }
            _ => {}
        }
        Ok(())
    }

    /// `check_pkill`: rule 2, pattern kills. A pattern naming QEMU (or a piece
    /// of its binary name) is denied to everyone; agents never pattern-kill at
    /// all; a main thread is asked when a pattern is computed, a regex, or names
    /// the soak harness.
    fn check_pkill(&mut self, name: &str, args: &[String]) {
        static HARNESS: LazyLock<Regex> =
            LazyLock::new(|| re(&format!("{}|{}", ci("aios"), ci("soak"))));
        let mut patterns: Vec<&String> = Vec::new();
        let mut skip = false;
        for a in args {
            if skip {
                skip = false;
            } else if PKILL_VALUE_OPTS.contains(&a.as_str()) {
                skip = true;
            } else if !a.starts_with('-') || unresolved(a) {
                patterns.push(a);
            }
        }
        let names_qemu = args.iter().any(|a| a.to_lowercase().contains("qemu"))
            || patterns
                .iter()
                .filter(|p| !unresolved(p))
                .any(|p| p.chars().count() >= 3 && QEMU_BINARY.contains(p.to_lowercase().as_str()));
        if names_qemu {
            self.verdict.rule_deny(
                verdict::R2,
                "never pattern-kill QEMU; TERM the lock owner's pid (rule 11)",
            );
        } else if self.is_agent() {
            self.verdict.rule_deny(
                verdict::R2,
                format!(
                    "agents never pattern-kill ({name}); stop only a pid you own, such as the lock owner's pid with a TERM (rule 11)"
                ),
            );
        } else if patterns.iter().any(|p| {
            unresolved(p) || p.chars().any(|c| REGEX_META.contains(c)) || HARNESS.is_match(p)
        }) {
            self.verdict.ask(
                verdict::R2,
                format!(
                    "{name} with a pattern that comes from an expansion, is a regex or names the soak harness can match QEMU; TERM the lock owner's pid instead (rule 11)"
                ),
            );
        }
    }

    /// `check_kill`: rule 2, `kill $(pgrep ...)`, `pgrep ... | xargs kill`, and
    /// a kill of a computed pid where the text also runs a pid lookup are
    /// pattern kills. The text is the whole command typed, or inside a script the
    /// kill's own line.
    fn check_kill(
        &mut self,
        args: &[String],
        cmd: &Command,
        state: &State,
        via: &BTreeSet<String>,
    ) {
        let mut pattern = via.iter().any(|w| ARG_WRAPPERS.contains(&w.as_str()));
        if !pattern && args.iter().any(|a| unresolved(a)) {
            let text = if state.script { &cmd.src } else { &state.text };
            pattern = PID_FINDER.is_match(text)
                || state.pid_vars.iter().any(|n| {
                    re(&format!(r"\$\{{?{}{B_AFTER}", regex::escape(n))).is_match(&cmd.src)
                });
        }
        if !pattern {
            return;
        }
        if self.is_agent() {
            self.verdict.rule_deny(
                verdict::R2,
                "agents never pattern-kill (kill with pids from pgrep, ps or xargs); stop only a pid you own, such as the lock owner's pid with a TERM (rule 11)",
            );
        } else {
            self.verdict.ask(
                verdict::R2,
                "kill with pids found by pattern (pgrep, ps or xargs) can hit QEMU; TERM the lock owner's pid instead (rule 11)",
            );
        }
    }

    /// `check_rustup`: rule 2 row 2, toolchain changes.
    fn check_rustup(&mut self, name: &str, args: &[String]) {
        if name == "rustup-init" {
            if self.is_agent() {
                self.verdict.rule_deny(verdict::R2, AGENT_TOOLCHAIN_DENY);
            }
            return;
        }
        let pos: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-') && !a.starts_with('+'))
            .map(String::as_str)
            .collect();
        let Some((&top, rest)) = pos.split_first() else {
            return;
        };
        let changes = RUSTUP_TOP_CHANGES.contains(&top)
            || rest.iter().any(|p| rustup_sub_changes(top).contains(p));
        if self.is_agent() {
            let denied = changes
                || RUSTUP_AGENT_TOP.contains(&top)
                || rest.iter().any(|p| rustup_agent_sub(top).contains(p))
                || (top == "default" && !rest.is_empty())
                || (top == "override" && rest.first() != Some(&"list"));
            if denied {
                self.verdict.rule_deny(verdict::R2, AGENT_TOOLCHAIN_DENY);
            } else if unresolved(top) {
                self.verdict.ask(
                    verdict::R2,
                    "the rustup subcommand comes from a shell expansion; agents never change the toolchain (rule 11, Toolchain)",
                );
            }
        } else if changes {
            self.verdict.ask(
                verdict::R2,
                "rustup would change the toolchain, which only Renovate PRs and the owner do (rule 11, Toolchain)",
            );
        }
    }

    /// `check_rust_proxy`: rule 2 row 2, `cargo +<toolchain>` and `RUSTUP_*`
    /// variables make a rustup proxy run, and may install, another toolchain.
    fn check_rust_proxy(&mut self, args: &[String]) {
        if !self.is_agent() {
            return;
        }
        let plus = args.first().is_some_and(|a| a.starts_with('+'));
        if plus || self.env_names().iter().any(|n| n.starts_with("RUSTUP_")) {
            self.verdict.rule_deny(verdict::R2, AGENT_TOOLCHAIN_DENY);
        }
    }

    /// `own_temp_worktree`: `repo` is a temporary worktree the agent owns: the
    /// payload's cwd is inside it, and for agent-<id> also <id> is the payload's
    /// agent_id (or agent_id is agent-<id>). A wf_ worktree has no id check.
    fn own_temp_worktree(&self, repo: &Repo) -> Result<bool, Fault> {
        let Some(name) = temp_worktree(repo)? else {
            return Ok(false);
        };
        let home = self.ctx.cwd.as_str();
        if home.is_empty() {
            return Ok(false);
        }
        let home_roots = self.repo(home, &[]).roots()?;
        if home_roots.first() != repo.roots()?.first() {
            return Ok(false);
        }
        if name.starts_with("wf_") {
            return Ok(true);
        }
        let id = &name["agent-".len()..];
        Ok(self
            .ctx
            .agent_id
            .as_ref()
            .and_then(|a| a.as_str())
            .is_some_and(|a| a == id || a == name))
    }

    /// `git_redirect`: what in this command can move git to another repository,
    /// working tree or index. Empty when nothing does.
    fn git_redirect(&self, extra: &[String], inline_config: &[String]) -> String {
        let mut names: BTreeSet<String> = self
            .env_names()
            .into_iter()
            .filter(|n| n.starts_with("GIT_") && git_env_risky(n))
            .collect();
        for n in PLACEMENT_ENV {
            if std::env::var_os(n).is_some_and(|v| !v.is_empty()) {
                names.insert(n.to_string());
            }
        }
        let mut parts: Vec<String> = Vec::new();
        if !names.is_empty() {
            parts.push(names.into_iter().collect::<Vec<_>>().join(", "));
        }
        if !extra.is_empty() {
            let opts: BTreeSet<&str> = extra
                .iter()
                .map(|e| e.split('=').next().unwrap_or(""))
                .collect();
            parts.push(opts.into_iter().collect::<Vec<_>>().join(" "));
        }
        let risky: Vec<&str> = inline_config
            .iter()
            .map(|c| partition(c, '=').0)
            .filter(|k| unresolved(k) || PLACEMENT_CONFIG.is_match(k))
            .collect();
        if !risky.is_empty() {
            parts.push(format!("-c {}", risky.join(", ")));
        }
        parts.join(" and ")
    }

    /// `check_placement`: rule 4 rows 4-6. From an agent, git reset --hard and
    /// the commands that commit, move refs or discard work run only in the
    /// temporary worktree it owns; GIT_DIR and the other redirects are denied
    /// outright.
    pub fn check_placement(
        &mut self,
        sub: &str,
        rest: &[String],
        workdir: &str,
        extra: &[String],
        inline_config: &[String],
    ) -> Result<(), Fault> {
        let tag = verdict::R4;
        let hard =
            sub == "reset" && !scan_git_options(rest, &[("hard", false)], "", "", "").is_empty();
        if !self.is_agent() {
            if hard {
                self.verdict.ask(
                    tag,
                    "git reset --hard discards uncommitted work; the settings allow rule for it exists only for an isolated agent's first command (rule 11)",
                );
            }
            return Ok(());
        }
        let change = main_checkout_change(sub, rest);
        if !hard && change.is_none() {
            return Ok(());
        }
        let redirect = self.git_redirect(extra, inline_config);
        if hard {
            if !redirect.is_empty() {
                self.verdict.rule_deny(
                    tag,
                    format!(
                        "agents reset only their own temporary worktree; {redirect} in this command can point git at another one (rule 11)"
                    ),
                );
            } else if unresolved(workdir) || !self.own_temp_worktree(&self.repo(workdir, &[]))? {
                self.verdict.rule_deny(
                    tag,
                    "agents reset only their own temporary worktree (rule 11)",
                );
            }
            return Ok(());
        }
        let change = change.unwrap_or_default();
        if !redirect.is_empty() {
            self.verdict.rule_deny(
                tag,
                format!(
                    "agents never change where git writes: {redirect} in this command can point git {sub} at another repository, working tree, index or config (rule 11)"
                ),
            );
        } else if unresolved(workdir) {
            self.verdict.ask(
                tag,
                format!(
                    "git {sub} runs in a directory that comes from a shell expansion; agents commit and move refs only in their own temporary worktree"
                ),
            );
        } else if self.own_temp_worktree(&self.repo(workdir, &[]))? {
        } else if main_checkout(&self.repo(workdir, &[]))? {
            if change == "commit" {
                self.verdict.rule_deny(
                    tag,
                    "agents never commit in the main checkout: you were spawned without isolation (rule 11)",
                );
            } else {
                self.verdict.rule_deny(
                    tag,
                    format!(
                        "agents never run git {change} in the main checkout (it moves refs or discards work): you were spawned without isolation (rule 11)"
                    ),
                );
            }
        } else if change == "commit" {
            self.verdict.rule_deny(
                tag,
                "agents commit only in their own temporary worktree, the one their cwd is in (rule 11)",
            );
        } else {
            self.verdict.rule_deny(
                tag,
                format!(
                    "agents run git {change} only in their own temporary worktree, the one their cwd is in (rule 11)"
                ),
            );
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
    fn qemu_starts_match_python() {
        // aios_starts_qemu, cargo_starts_qemu, is_qemu_script on CPython 3.13.
        assert_eq!(
            aios_starts_qemu(&strings(&["-q", "soak", "runs=1"])),
            Some(Qemu::Other)
        );
        assert_eq!(
            aios_starts_qemu(&strings(&["soak", "--classify", "x"])),
            None
        );
        assert_eq!(aios_starts_qemu(&strings(&["$X"])), Some(Qemu::Unclear));
        assert_eq!(aios_starts_qemu(&strings(&["brief"])), None);
        assert_eq!(
            cargo_starts_qemu(&strings(&[
                "+nightly",
                "run",
                "-p",
                "aios-tools",
                "--",
                "soak"
            ])),
            Some(Qemu::Other)
        );
        assert_eq!(
            cargo_starts_qemu(&strings(&["--config", "x", "r", "--", "soak"])),
            Some(Qemu::Other)
        );
        assert_eq!(cargo_starts_qemu(&strings(&["build", "--", "soak"])), None);
        assert_eq!(cargo_starts_qemu(&strings(&["run"])), None);
        assert!(is_qemu_script("x/qemu-lock.sh", &[]));
        assert!(is_qemu_script(
            "./w.sh",
            &strings(&["run", "--team=x", "--", "a"])
        ));
        assert!(!is_qemu_script(
            "./w.sh",
            &strings(&["run", "--", "--team"])
        ));
    }

    #[test]
    fn wrapped_start_matches_python() {
        // wrapped_start(argv, flags) on CPython 3.13.
        let no = Expanded::No;
        let quoted = Expanded::Quoted;
        let bare = Expanded::Bare;
        let argv = strings(&["run", "--team", "solo", "--", "just", "soak"]);
        assert_eq!(wrapped_start(&argv, &[no; 6]), (Some(4), true));
        let argv = strings(&["run", "--label", "--", "--", "x"]);
        assert_eq!(wrapped_start(&argv, &[no; 5]), (Some(4), true));
        let argv = strings(&["run", "--team", "$T", "--", "x"]);
        assert_eq!(
            wrapped_start(&argv, &[no, no, bare, no, no]),
            (Some(4), false)
        );
        assert_eq!(
            wrapped_start(&argv, &[no, no, quoted, no, no]),
            (Some(4), true)
        );
        let argv = strings(&["run", "$O", "x"]);
        assert_eq!(wrapped_start(&argv, &[no, quoted, no]), (None, false));
    }

    #[test]
    fn helpers_match_python() {
        assert_eq!(main_checkout_change("am", &[]).as_deref(), Some("commit"));
        assert_eq!(main_checkout_change("stash", &strings(&["list"])), None);
        assert_eq!(main_checkout_change("stash", &[]).as_deref(), Some("stash"));
        assert_eq!(
            main_checkout_change("branch", &strings(&["-fq", "x"])).as_deref(),
            Some("branch -D/-f/-M/-C")
        );
        assert_eq!(
            main_checkout_change("switch", &strings(&["--force-c", "x"])).as_deref(),
            Some("switch -C")
        );
        assert_eq!(
            main_checkout_change("symbolic-ref", &strings(&["HEAD"])),
            None
        );
        assert_eq!(python_int(" +1_700 "), Some(1700));
        assert_eq!(python_int("-5"), Some(-5));
        assert_eq!(python_int("\u{663}"), Some(3));
        assert_eq!(python_int("1__0"), None);
        assert_eq!(python_int("_1"), None);
        assert_eq!(python_int("?"), None);
        assert!(
            TEMP_WORKTREE.is_match("agent-a1B2") && TEMP_WORKTREE.is_match("wf_0123abcd-abc-12")
        );
        assert!(
            !TEMP_WORKTREE.is_match("wf_0123abcd-abc-012") && !TEMP_WORKTREE.is_match("agent-")
        );
        assert!(
            PLACEMENT_CONFIG.is_match("Core.Bare\n") && PLACEMENT_CONFIG.is_match("includeIf.x")
        );
        assert!(!PLACEMENT_CONFIG.is_match("core.barex"));
    }

    fn git(dir: &std::path::Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .stdout(std::process::Stdio::null())
            .status()
            .expect("git runs");
        assert!(status.success(), "git {args:?} in {dir:?}");
    }

    fn temp_name(path: &std::path::Path) -> Option<String> {
        let repos = super::super::repo::Repos::default();
        temp_worktree(&repos.get(&path.to_string_lossy(), &[])).expect("no fault")
    }

    /// WorkflowWorktrees' direct `temp_worktree()` checks (the fixture's 21
    /// that are not hook calls): only <main>/.claude/worktrees/<name> on
    /// branch worktree-<name>, with git's admin directory of the same name,
    /// is a temporary worktree.
    #[test]
    fn temporary_worktrees_match_python() {
        let tmp = std::env::temp_dir().join(format!("aios-guard-temp-wt-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let repo = tmp.join("work");
        std::fs::create_dir_all(&repo).expect("repo dir");
        git(&repo, &["init", "-q", "-b", "main"]);
        git(
            &repo,
            &[
                "-c",
                "user.name=t",
                "-c",
                "user.email=t@e",
                "commit",
                "-q",
                "--allow-empty",
                "-m",
                "init",
            ],
        );
        let trees = repo.join(".claude").join("worktrees");
        let add = |path: &std::path::Path, branch: &str| {
            git(
                &repo,
                &[
                    "worktree",
                    "add",
                    "-q",
                    "-b",
                    branch,
                    &path.to_string_lossy(),
                ],
            );
            path.to_path_buf()
        };
        // The temporary worktrees: a workflow agent's and an Agent-tool agent's.
        let wf = add(
            &trees.join("wf_0123abcd-0ef-1"),
            "worktree-wf_0123abcd-0ef-1",
        );
        assert_eq!(temp_name(&wf).as_deref(), Some("wf_0123abcd-0ef-1"));
        std::fs::create_dir_all(wf.join("sub")).expect("sub");
        assert_eq!(
            temp_name(&wf.join("sub")).as_deref(),
            Some("wf_0123abcd-0ef-1")
        );
        let agent = add(&trees.join("agent-x"), "worktree-agent-x");
        assert_eq!(temp_name(&agent).as_deref(), Some("agent-x"));
        assert_eq!(temp_name(&repo), None);
        let mut not_temporary = Vec::new();
        for name in [
            "wf_x",
            "wf_0123abc-0ef-1",
            "wf_0123abcde-0ef-1",
            "wf_0123abcd-0e-1",
            "wf_0123abcd-0eff-1",
            "wf_ABCDEF01-0ef-11",
            "wf_0123abcd-0ef-",
            "wf_0123abcd-0ef-01",
            "wf_0123abcd-0ef-1x",
            "wf_0123abcd_0ef_1",
            "wf-0123abcd-0ef-1",
            "agent-a.b",
        ] {
            not_temporary.push(add(&trees.join(name), &format!("worktree-{name}")));
        }
        // The right name on another branch.
        not_temporary.push(add(&trees.join("wf_0123abcd-0ef-3"), "claude/w3"));
        // A worktree-wf_* branch outside .claude/worktrees, nested below it,
        // or under another name in it.
        not_temporary.push(add(
            &tmp.join("wf_0123abcd-0ef-4"),
            "worktree-wf_0123abcd-0ef-4",
        ));
        not_temporary.push(add(
            &trees.join("sub").join("wf_0123abcd-0ef-5"),
            "worktree-wf_0123abcd-0ef-5",
        ));
        not_temporary.push(add(
            &repo.join(".claude").join("wf_0123abcd-0ef-6"),
            "worktree-wf_0123abcd-0ef-6",
        ));
        not_temporary.push(add(&trees.join("lead"), "worktree-wf_0123abcd-0ef-7"));
        // A branch worktree.
        not_temporary.push(add(&tmp.join("W"), "claude/w"));
        // A symlink at a wf_ path to a worktree that lives elsewhere.
        let real = add(&tmp.join("real"), "worktree-wf_0123abcd-0ef-8");
        let link = trees.join("wf_0123abcd-0ef-8");
        std::os::unix::fs::symlink(&real, &link).expect("symlink");
        not_temporary.push(link);
        // Worktrees moved onto a temporary path keep their admin directory's
        // name, which no longer matches.
        for name in ["wf_0123abcd-0ef-9", "agent-z"] {
            let src = add(&tmp.join(format!("mv-{name}")), &format!("worktree-{name}"));
            let dst = trees.join(name);
            git(
                &repo,
                &[
                    "worktree",
                    "move",
                    &src.to_string_lossy(),
                    &dst.to_string_lossy(),
                ],
            );
            not_temporary.push(dst);
        }
        assert_eq!(not_temporary.len(), 21);
        for path in &not_temporary {
            assert_eq!(temp_name(path), None, "{path:?}");
        }
        std::fs::remove_dir_all(&tmp).expect("remove the temp dir");
    }
}
