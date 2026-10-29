//! `aios guard`: the Rust port of `.claude/hooks/git-push-guard.py` (R5).
//!
//! The port is exact (plan D1): every decision, exit code and reason byte is the
//! Python guard's, the gaps of issue #241 included, until the R5b cutover. Each
//! module names the Python lines it ports.
//!
//! - [`shell`]: the shell model (`ShellParser`, `brace_expand`, `parse_commands`).
//! - [`shlex`]: Python's `shlex.quote` and `shlex.split`.
//! - [`analyzer`]: the shell level (`Analyzer`): variables, `cd`, wrappers, shell
//!   strings and script files.
//! - [`git_rules`] and [`gh_rules`]: the git and gh levels; [`sed_rule`]: sed on
//!   the protected paths; [`team_rules`]: rule 11's QEMU, kill, toolchain and
//!   placement rules; [`spawn_rules`]: its Agent spawn and EnterWorktree rules
//!   (`decide_tool`); [`fnmatch`]: Python's `fnmatch.fnmatchcase`.
//! - [`repo`]: the read-only git queries (`Repo`); [`pyos`]: the Python runtime
//!   behaviour the guard depends on; [`pattern`]: its regular expressions;
//!   [`verdict`]: its decision.
//! - [`protocol`]: the hook protocol (`main` and `emit`); [`pyjson`]: the
//!   payload's `json.loads`, with Python's error messages.

pub mod analyzer;
pub mod fnmatch;
pub mod gh_rules;
pub mod git_rules;
pub mod pattern;
pub mod protocol;
pub mod pyjson;
pub mod pyos;
pub mod repo;
pub mod sed_rule;
pub mod shell;
pub mod shlex;
pub mod spawn_rules;
pub mod team_rules;
pub mod verdict;

use std::collections::HashMap;
use std::io::{Read, Write};

use analyzer::{Analyzer, Ctx, Stop};
use pattern::{FALLBACK_WORDS, MENTIONS_GIT, MENTIONS_TEAM};
use verdict::Verdict;

/// `aios guard`: the PreToolUse hook for Bash, Monitor, Agent, Task and
/// EnterWorktree. It takes no arguments.
#[derive(clap::Args)]
pub struct Args {}

/// Run the hook: read one payload from `stdin`, write the decision, and return
/// the exit code (0, 2 for a deny, 1 where Python's hook crashes or cannot
/// write its output).
pub fn run(
    stdin: &mut dyn Read,
    env: &HashMap<String, String>,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> u8 {
    let mut input = Vec::new();
    let input = stdin.read_to_end(&mut input).map(|_| input);
    let getcwd = || std::env::current_dir().map(|d| d.to_string_lossy().into_owned());
    let response = protocol::respond(input, env, getcwd);
    protocol::write(&response, out, err).unwrap_or(1)
}

/// `decide(command, cwd, ctx)`: the verdict for one command text, or the Python
/// exception that escaped the analysis. Text the guard cannot parse asks when
/// it mentions anything a rule covers (the keyword gate, plan D4).
pub fn decide(command: &str, cwd: &str, ctx: &Ctx) -> Result<Verdict, pyos::Fault> {
    let mut ctx = ctx.clone();
    if ctx.cwd.is_empty() {
        ctx.cwd = cwd.to_string();
    }
    let mut analyzer = Analyzer::new(&ctx);
    match analyzer.analyze_text(command, cwd, 0, false) {
        Ok(()) => {}
        Err(Stop::Fault(fault)) => return Err(fault),
        Err(Stop::Parse(e)) => {
            if MENTIONS_GIT.is_match(command)
                || MENTIONS_TEAM.is_match(command)
                || FALLBACK_WORDS.is_match(command)
            {
                analyzer.verdict.ask(
                    verdict::PARSE_FALLBACK,
                    format!("git-push-guard could not parse the command ({})", e.0),
                );
            }
        }
    }
    Ok(analyzer.verdict)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verdict::Level;

    /// A command, then its level, reasons and `push_deny` as Python's `decide`
    /// gave them, then the rule tags the port adds (step 7b; Python has none,
    /// so the tags are each reason's rule).
    type Row = (
        &'static str,
        Level,
        &'static [&'static str],
        bool,
        &'static [verdict::Tag],
    );

    /// Each row's decision in a fresh directory outside any git repository, as
    /// `decide(command, cwd)` gave it on CPython 3.13.
    fn check_rows(label: &str, rows: &[Row]) {
        check_rows_as(label, &Ctx::default(), rows);
    }

    /// [`check_rows`] for the caller `ctx` (`decide(command, cwd, ctx)`).
    fn check_rows_as(label: &str, ctx: &Ctx, rows: &[Row]) {
        let dir = std::env::temp_dir().join(format!("aios-guard-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let cwd = dir.to_string_lossy().into_owned();
        for &(command, level, reasons, push_deny, tags) in rows {
            let v = decide(command, &cwd, ctx).expect("no fault");
            assert_eq!(v.level, level, "{label}: {command}");
            assert_eq!(v.reasons, reasons, "{label}: {command}");
            assert_eq!(v.push_deny, push_deny, "{label}: {command}");
            assert!(
                v.tags.iter().eq(tags),
                "{label}: {command}: tags {:?}",
                v.tags
            );
        }
        std::fs::remove_dir(&dir).expect("the empty temp dir");
    }

    #[test]
    fn git_and_gh_decisions_match_python() {
        check_rows(
            "git",
            &[
            ("git push origin main", Level::Deny, &["push would update main (destination 'main')"], true, &["push"]),
            ("git push -f origin claude/x", Level::Deny, &["plain force push (--force/-f); use --force-with-lease on a claude/* branch"], true, &["push"]),
            ("git -c alias.p=push p origin claude/x", Level::Ask, &["inline -c/--config-env on git push can change where it pushes"], false, &["push"]),
            ("gh pr merge 5", Level::Ask, &["gh pr merge: merging is reserved for the user"], false, &["gh"]),
            ("gh pr merge --admin 5", Level::Deny, &["gh pr merge --admin bypasses branch protection; merging is reserved for the user"], true, &["gh"]),
            ("gh api -X PATCH repos/x/y", Level::Ask, &["gh api PATCH repos/x/y changes GitHub state"], false, &["gh"]),
            ("git push", Level::Ask, &["cannot resolve the current branch for this push (directory is not a git repository yet)"], false, &["push"]),
            ("git push 'oops", Level::Ask, &["git-push-guard could not parse the command (unterminated single quote)"], false, &["parse-fallback"]),
            ("ls", Level::None, &[], false, &[]),
            ("git rebase -x make main", Level::Ask, &["git rebase --exec runs a shell command after each commit"], false, &["git"]),
            ("GIT_DIR=x git push origin claude/x", Level::Ask, &["GIT_DIR set in this command can change where git pushes"], false, &["push"]),
            ("gh api graphql -f query='mutation { mergePullRequest }'", Level::Ask, &["gh api graphql calls mergePullRequest, which changes branches, merges or repository settings"], false, &["gh"]),
            ("git push origin +claude/x:refs/heads/ma*", Level::Deny, &["'+' refspec force-pushes (+claude/x:refs/heads/ma*)", "push would update main (destination 'refs/heads/ma*')"], true, &["push"]),
            ("git push --delete origin claude/y", Level::Ask, &["push deletes remote ref 'claude/y'"], false, &["push"]),
            ("git push --force-with-lease origin HEAD:feature", Level::Ask, &["--force-with-lease rewrites 'feature', which is outside claude/*"], false, &["push"]),
            ("sh -c \"git push origin main\"", Level::Deny, &["push would update main (destination 'main')"], true, &["push"]),
            ("gh myext run", Level::Ask, &["gh myext is an alias or extension; the guard cannot see what it runs"], false, &["gh"]),
            ("gh issue create -R other/repo -b \"@Claude hi\"", Level::Ask, &["gh issue create targets other/repo, not juslee/aios", "gh issue create mentions @claude, which starts the Claude workflow"], false, &["gh"]),
            ("git checkout -B x", Level::Ask, &["git checkout -B can discard uncommitted changes or reset a branch"], false, &["git"]),
            ("git config alias.p push && git p origin main", Level::Ask, &["git p is not a git command; it may be an alias defined earlier in this command"], false, &["git"]),
            ("git push --dry-run origin main", Level::None, &[], false, &[]),
            ("git push --mirror origin", Level::Deny, &["--all/--branches/--mirror pushes every branch, including main", "cannot resolve the current branch for this push (directory is not a git repository yet)"], true, &["push"]),
            ("git push origin :main", Level::Deny, &["push would delete main ('main')"], true, &["push"]),
            ("env -S \"git push origin main\"", Level::Deny, &["push would update main (destination 'main')"], true, &["push"]),
            ],
        );
    }

    /// Brace expansion nested past Python's recursion limit is a parse failure
    /// (`shell::BRACE_FRAMES`): `echo` with 992 `{a,b}` groups still parses on
    /// CPython 3.13 and 993 do not, flat or nested, and 50,000 groups ask
    /// instead of overflowing the stack. Without a keyword, a parse failure
    /// says nothing.
    #[test]
    fn brace_recursion_matches_python() {
        let flat = |n: usize| format!("echo {} && git push origin main", "{a,b}".repeat(n));
        let nested = |n: usize| {
            format!(
                "echo x{}b{} && git push origin main",
                "{a,".repeat(n),
                "}".repeat(n)
            )
        };
        let parse = "git-push-guard could not parse the command (maximum recursion depth exceeded)";
        let push = "push would update main (destination 'main')";
        let rows: [(String, Level, &[&str], &[verdict::Tag]); 6] = [
            (flat(992), Level::Deny, &[push], &[verdict::PUSH]),
            (flat(993), Level::Ask, &[parse], &[verdict::PARSE_FALLBACK]),
            (
                flat(50_000),
                Level::Ask,
                &[parse],
                &[verdict::PARSE_FALLBACK],
            ),
            (nested(992), Level::Deny, &[push], &[verdict::PUSH]),
            (
                nested(993),
                Level::Ask,
                &[parse],
                &[verdict::PARSE_FALLBACK],
            ),
            (
                format!("echo {}", "{a,b}".repeat(993)),
                Level::None,
                &[],
                &[],
            ),
        ];
        let cwd = std::env::temp_dir().to_string_lossy().into_owned();
        for (command, level, reasons, tags) in rows {
            let v = decide(&command, &cwd, &Ctx::default()).expect("no fault");
            let head = &command[..40];
            assert_eq!(v.level, level, "{head}");
            assert_eq!(v.reasons, reasons, "{head}");
            assert!(v.tags.iter().eq(tags), "{head}: tags {:?}", v.tags);
        }
    }

    #[test]
    fn sed_decisions_match_python() {
        check_rows(
            "sed",
            &[
            ("sed -i s/a/b/ .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sed -n 1p .claude/settings.json", Level::None, &[], false, &[]),
            ("sed -n 1p f > .claude/hooks/x", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (its output is redirected into .claude/hooks/x)"], false, &["sed"]),
            ("cd .claude && sed -i s/x/y/ settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("find .claude -name '*.json' -exec sed -i s/a/b/ {} +", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sed 's/a/b/' .claude/hooks/*", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (the 's' command)"], false, &["sed"]),
            ("sed -n 1p <<< \"$(cat > .claude/settings.json)\"", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (a substitution, here-string or heredoc body names .claude/settings* or .claude/hooks/*)"], false, &["sed"]),
            ("xargs sed -i s/a/b/ < list # .claude", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sed -n \"$S\" .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt ($S comes from a shell expansion and may be an option)"], false, &["sed"]),
            ("gsed -e 1p -i .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sudo sed -i 1d /x/.claude/settings.local.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sed -n 1p .c*/settings*", Level::None, &[], false, &[]),
            ("cd .claude/hooks && sed -n w\\ x f", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (the 'w' command)"], false, &["sed"]),
            ("sed -n 1p \"$OPT\" .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt ($OPT comes from a shell expansion and may be an option)"], false, &["sed"]),
            ("sh -c 'sed -i 1d \"$1\"' _ .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false, &["sed"]),
            ("sed -n '/[[:alpha:]/p' .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (a /regex/ whose bracket expression GNU and BSD sed end differently)"], false, &["sed"]),
            ("sed --expression=1p --quiet .claude/hooks/aios", Level::None, &[], false, &[]),
            ],
        );
    }

    #[test]
    fn team_rules_for_the_owner_match_python() {
        let ctx = Ctx::default();
        check_rows_as(
            "owner",
            &ctx,
            &[
            ("just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just --dry-run soak", Level::None, &[], false, &[]),
            ("just -f other/justfile run", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just build", Level::None, &[], false, &[]),
            ("qemu-system-aarch64 -M virt", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("qemu-system-aarch64 --version", Level::None, &[], false, &[]),
            ("cargo run -p aios-tools -- soak runs=1", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("aios soak --classify log.txt", Level::None, &[], false, &[]),
            ("aios soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("$AIOS soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("bash scripts/agent/qemu-lock.sh run --team team-fix -- just soak", Level::None, &[], false, &[]),
            ("bash scripts/agent/qemu-lock.sh status", Level::None, &[], false, &[]),
            ("scripts/soak-qemu.sh", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("scripts/soak-qemu.sh --classify x", Level::None, &[], false, &[]),
            ("./w.sh run --team x -- echo hi", Level::None, &[], false, &[]),
            ("pkill qemu", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("killall -9 qemu-system-aarch64", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("kill 123", Level::None, &[], false, &[]),
            ("kill $PID", Level::None, &[], false, &[]),
            ("kill $(pgrep qemu)", Level::Ask, &["kill with pids found by pattern (pgrep, ps or xargs) can hit QEMU; TERM the lock owner's pid instead (rule 11)"], false, &["r2"]),
            ("rustup update", Level::Ask, &["rustup would change the toolchain, which only Renovate PRs and the owner do (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup toolchain install nightly", Level::Ask, &["rustup would change the toolchain, which only Renovate PRs and the owner do (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup target add x", Level::None, &[], false, &[]),
            ("rustup show", Level::None, &[], false, &[]),
            ("cargo +nightly build", Level::None, &[], false, &[]),
            ("rustc +stable -V", Level::None, &[], false, &[]),
            ("git reset --hard HEAD", Level::Ask, &["git reset --hard discards uncommitted work; the settings allow rule for it exists only for an isolated agent's first command (rule 11)"], false, &["r4"]),
            ("git commit -m x", Level::None, &[], false, &[]),
            ("git -c core.worktree=/x commit -m x", Level::None, &[], false, &[]),
            ("GIT_DIR=/x git commit -m x", Level::None, &[], false, &[]),
            ("git switch -C x", Level::Ask, &["git switch -C can discard uncommitted changes or reset a branch"], false, &["git"]),
            ("timeout 60 just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("nohup just run &", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("xargs -I{} kill {}", Level::Ask, &["kill with pids found by pattern (pgrep, ps or xargs) can hit QEMU; TERM the lock owner's pid instead (rule 11)"], false, &["r2"]),
            ("sh -c 'just soak'", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ],
        );
    }

    #[test]
    fn team_rules_for_an_agent_match_python() {
        let ctx = Ctx {
            agent_id: Some(serde_json::json!("a1")),
            agent_type: Some(serde_json::json!("verifier")),
            ..Ctx::default()
        };
        check_rows_as(
            "agent",
            &ctx,
            &[
            ("just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just --dry-run soak", Level::None, &[], false, &[]),
            ("just -f other/justfile run", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just build", Level::None, &[], false, &[]),
            ("qemu-system-aarch64 -M virt", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("qemu-system-aarch64 --version", Level::None, &[], false, &[]),
            ("cargo run -p aios-tools -- soak runs=1", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("aios soak --classify log.txt", Level::None, &[], false, &[]),
            ("aios soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("$AIOS soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("bash scripts/agent/qemu-lock.sh run --team team-fix -- just soak", Level::None, &[], false, &[]),
            ("bash scripts/agent/qemu-lock.sh status", Level::None, &[], false, &[]),
            ("scripts/soak-qemu.sh", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("scripts/soak-qemu.sh --classify x", Level::None, &[], false, &[]),
            ("./w.sh run --team x -- echo hi", Level::None, &[], false, &[]),
            ("pkill qemu", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("killall -9 qemu-system-aarch64", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("kill 123", Level::None, &[], false, &[]),
            ("kill $PID", Level::None, &[], false, &[]),
            ("kill $(pgrep qemu)", Level::Deny, &["agents never pattern-kill (kill with pids from pgrep, ps or xargs); stop only a pid you own, such as the lock owner's pid with a TERM (rule 11)"], false, &["r2"]),
            ("rustup update", Level::Deny, &["agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup toolchain install nightly", Level::Deny, &["agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup target add x", Level::Deny, &["agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup show", Level::None, &[], false, &[]),
            ("cargo +nightly build", Level::Deny, &["agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)"], false, &["r2"]),
            ("rustc +stable -V", Level::Deny, &["agents never change the toolchain; report a missing toolchain to your lead (rule 11, Toolchain)"], false, &["r2"]),
            ("git reset --hard HEAD", Level::Deny, &["agents reset only their own temporary worktree (rule 11)"], false, &["r4"]),
            ("git commit -m x", Level::Deny, &["agents commit only in their own temporary worktree, the one their cwd is in (rule 11)"], false, &["r4"]),
            ("git -c core.worktree=/x commit -m x", Level::Deny, &["agents never change where git writes: -c core.worktree in this command can point git commit at another repository, working tree, index or config (rule 11)"], false, &["r4"]),
            ("GIT_DIR=/x git commit -m x", Level::Deny, &["agents never change where git writes: GIT_DIR in this command can point git commit at another repository, working tree, index or config (rule 11)"], false, &["r4"]),
            ("git switch -C x", Level::Deny, &["agents run git switch -C only in their own temporary worktree, the one their cwd is in (rule 11)", "git switch -C can discard uncommitted changes or reset a branch"], false, &["git", "r4"]),
            ("timeout 60 just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("nohup just run &", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("xargs -I{} kill {}", Level::Deny, &["agents never pattern-kill (kill with pids from pgrep, ps or xargs); stop only a pid you own, such as the lock owner's pid with a TERM (rule 11)"], false, &["r2"]),
            ("sh -c 'just soak'", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ],
        );
    }

    #[test]
    fn team_rules_for_a_team_lead_match_python() {
        let ctx = Ctx {
            team: "team-fix".to_string(),
            ..Ctx::default()
        };
        check_rows_as(
            "lead",
            &ctx,
            &[
            ("just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just --dry-run soak", Level::None, &[], false, &[]),
            ("just -f other/justfile run", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("just build", Level::None, &[], false, &[]),
            ("qemu-system-aarch64 -M virt", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("qemu-system-aarch64 --version", Level::None, &[], false, &[]),
            ("cargo run -p aios-tools -- soak runs=1", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("aios soak --classify log.txt", Level::None, &[], false, &[]),
            ("aios soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("$AIOS soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("bash scripts/agent/qemu-lock.sh run --team team-fix -- just soak", Level::Deny, &["team leads route boots to their verifier (rule 11)"], false, &["r1"]),
            ("bash scripts/agent/qemu-lock.sh status", Level::None, &[], false, &[]),
            ("scripts/soak-qemu.sh", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("scripts/soak-qemu.sh --classify x", Level::None, &[], false, &[]),
            ("./w.sh run --team x -- echo hi", Level::Deny, &["team leads route boots to their verifier (rule 11)"], false, &["r1"]),
            ("pkill qemu", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("killall -9 qemu-system-aarch64", Level::Deny, &["never pattern-kill QEMU; TERM the lock owner's pid (rule 11)"], false, &["r2"]),
            ("kill 123", Level::None, &[], false, &[]),
            ("kill $PID", Level::None, &[], false, &[]),
            ("kill $(pgrep qemu)", Level::Ask, &["kill with pids found by pattern (pgrep, ps or xargs) can hit QEMU; TERM the lock owner's pid instead (rule 11)"], false, &["r2"]),
            ("rustup update", Level::Ask, &["rustup would change the toolchain, which only Renovate PRs and the owner do (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup toolchain install nightly", Level::Ask, &["rustup would change the toolchain, which only Renovate PRs and the owner do (rule 11, Toolchain)"], false, &["r2"]),
            ("rustup target add x", Level::None, &[], false, &[]),
            ("rustup show", Level::None, &[], false, &[]),
            ("cargo +nightly build", Level::None, &[], false, &[]),
            ("rustc +stable -V", Level::None, &[], false, &[]),
            ("git reset --hard HEAD", Level::Ask, &["git reset --hard discards uncommitted work; the settings allow rule for it exists only for an isolated agent's first command (rule 11)"], false, &["r4"]),
            ("git commit -m x", Level::None, &[], false, &[]),
            ("git -c core.worktree=/x commit -m x", Level::None, &[], false, &[]),
            ("GIT_DIR=/x git commit -m x", Level::None, &[], false, &[]),
            ("git switch -C x", Level::Ask, &["git switch -C can discard uncommitted changes or reset a branch"], false, &["git"]),
            ("timeout 60 just soak", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("nohup just run &", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ("xargs -I{} kill {}", Level::Ask, &["kill with pids found by pattern (pgrep, ps or xargs) can hit QEMU; TERM the lock owner's pid instead (rule 11)"], false, &["r2"]),
            ("sh -c 'just soak'", Level::Deny, &["every QEMU start goes through scripts/agent/qemu-lock.sh run (rule 11)"], false, &["r1"]),
            ],
        );
    }

    /// Two gaps of the Python guard that the port keeps (plan D1), pinned at
    /// today's decision for R5c: a script named soak-qemu.sh run with
    /// --classify is never read, and a qemu-lock.sh outside the trusted
    /// locations decides its `run` as the wrapper, so the QEMU start it
    /// wraps is not checked. Python gives `none` for every row.
    #[test]
    fn qemu_script_gaps_are_pinned() {
        let dir = std::env::temp_dir().join(format!("aios-guard-gaps-{}", std::process::id()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).expect("bin");
        std::fs::write(
            bin.join("soak-qemu.sh"),
            "#!/bin/sh\ngit push origin main\n",
        )
        .expect("script");
        std::fs::write(bin.join("qemu-lock.sh"), "#!/bin/sh\nexec \"$@\"\n").expect("script");
        let cwd = dir.to_string_lossy().into_owned();
        let agent = Ctx {
            agent_id: Some(serde_json::json!("a")),
            agent_type: Some(serde_json::json!("verifier")),
            ..Ctx::default()
        };
        for ctx in [&Ctx::default(), &agent] {
            for command in [
                "bin/soak-qemu.sh --classify x",
                "bin/qemu-lock.sh run --team team-fix -- qemu-system-aarch64 -M virt",
                "bash bin/qemu-lock.sh run --team team-fix -- just soak",
            ] {
                let v = decide(command, &cwd, ctx).expect("no fault");
                assert_eq!(v.level, Level::None, "{command} {ctx:?}");
                assert!(v.reasons.is_empty(), "{command} {ctx:?}");
            }
        }
        std::fs::remove_dir_all(&dir).expect("remove the temp dir");
    }

    /// GhCommands.test_scratchpad_body_files_are_the_agents_own: a body file in
    /// the session scratchpad (`claude_tmp_roots`, replaced here as the Python
    /// test mocks it) is the agent's own; elsewhere outside the repository it
    /// asks. Python's decisions in the same layout.
    #[test]
    fn scratchpad_body_files_match_python() {
        let tmp = std::env::temp_dir().join(format!("aios-guard-scratch-{}", std::process::id()));
        let repo = tmp.join("work");
        let scratch = tmp.join("claude-tmp");
        std::fs::create_dir_all(&repo).expect("repo dir");
        std::fs::create_dir_all(&scratch).expect("scratch dir");
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main"])
            .arg(&repo)
            .status()
            .expect("git");
        assert!(init.success());
        let body = scratch.join("pr-body.md");
        std::fs::write(&body, "Summary\n").expect("body");
        let (repo, body) = (repo.to_string_lossy(), body.to_string_lossy());
        let scratch = pyos::realpath(&scratch.to_string_lossy()).expect("no NUL");
        let ctx = Ctx {
            cwd: repo.to_string(),
            ..Ctx::default()
        };
        for (sub, verb) in [("create --title t", "create"), ("edit 5", "edit")] {
            let command = format!("gh pr {sub} --body-file {body}");
            let mut analyzer = Analyzer::new(&ctx);
            analyzer.tmp_roots = Some(vec![scratch.clone()]);
            analyzer
                .analyze_text(&command, &repo, 0, false)
                .expect("no stop");
            assert_eq!(analyzer.verdict.level, Level::None, "{command}");
            let v = decide(&command, &repo, &ctx).expect("no fault");
            assert_eq!(v.level, Level::Ask, "{command}");
            assert_eq!(
                v.reasons,
                [format!("gh pr {verb} reads {body}, which is outside the repository and the session scratchpad (the repository is public)")]
            );
        }
        std::fs::remove_dir_all(&tmp).expect("remove the temp dir");
    }
}
