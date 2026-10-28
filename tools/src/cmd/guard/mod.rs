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
//!   the protected paths; [`fnmatch`]: Python's `fnmatch.fnmatchcase`.
//! - [`repo`]: the read-only git queries (`Repo`); [`pyos`]: the Python runtime
//!   behaviour the guard depends on; [`pattern`]: its regular expressions;
//!   [`verdict`]: its decision.

pub mod analyzer;
pub mod fnmatch;
pub mod gh_rules;
pub mod git_rules;
pub mod pattern;
pub mod pyos;
pub mod repo;
pub mod sed_rule;
pub mod shell;
pub mod shlex;
pub mod verdict;

use analyzer::{Analyzer, Ctx, Stop};
use pattern::{FALLBACK_WORDS, MENTIONS_GIT, MENTIONS_TEAM};
use verdict::Verdict;

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

    type Row = (&'static str, Level, &'static [&'static str], bool);

    /// Each row's decision in a fresh directory outside any git repository, as
    /// `decide(command, cwd)` gave it on CPython 3.13.
    fn check_rows(label: &str, rows: &[Row]) {
        let dir = std::env::temp_dir().join(format!("aios-guard-{label}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let cwd = dir.to_string_lossy().into_owned();
        let ctx = Ctx::default();
        for &(command, level, reasons, push_deny) in rows {
            let v = decide(command, &cwd, &ctx).expect("no fault");
            assert_eq!(v.level, level, "{command}");
            assert_eq!(v.reasons, reasons, "{command}");
            assert_eq!(v.push_deny, push_deny, "{command}");
        }
        std::fs::remove_dir(&dir).expect("the empty temp dir");
    }

    #[test]
    fn git_and_gh_decisions_match_python() {
        check_rows(
            "git",
            &[
            ("git push origin main", Level::Deny, &["push would update main (destination 'main')"], true),
            ("git push -f origin claude/x", Level::Deny, &["plain force push (--force/-f); use --force-with-lease on a claude/* branch"], true),
            ("git -c alias.p=push p origin claude/x", Level::Ask, &["inline -c/--config-env on git push can change where it pushes"], false),
            ("gh pr merge 5", Level::Ask, &["gh pr merge: merging is reserved for the user"], false),
            ("gh pr merge --admin 5", Level::Deny, &["gh pr merge --admin bypasses branch protection; merging is reserved for the user"], true),
            ("gh api -X PATCH repos/x/y", Level::Ask, &["gh api PATCH repos/x/y changes GitHub state"], false),
            ("git push", Level::Ask, &["cannot resolve the current branch for this push (directory is not a git repository yet)"], false),
            ("git push 'oops", Level::Ask, &["git-push-guard could not parse the command (unterminated single quote)"], false),
            ("ls", Level::None, &[], false),
            ("git rebase -x make main", Level::Ask, &["git rebase --exec runs a shell command after each commit"], false),
            ("GIT_DIR=x git push origin claude/x", Level::Ask, &["GIT_DIR set in this command can change where git pushes"], false),
            ("gh api graphql -f query='mutation { mergePullRequest }'", Level::Ask, &["gh api graphql calls mergePullRequest, which changes branches, merges or repository settings"], false),
            ("git push origin +claude/x:refs/heads/ma*", Level::Deny, &["'+' refspec force-pushes (+claude/x:refs/heads/ma*)", "push would update main (destination 'refs/heads/ma*')"], true),
            ("git push --delete origin claude/y", Level::Ask, &["push deletes remote ref 'claude/y'"], false),
            ("git push --force-with-lease origin HEAD:feature", Level::Ask, &["--force-with-lease rewrites 'feature', which is outside claude/*"], false),
            ("sh -c \"git push origin main\"", Level::Deny, &["push would update main (destination 'main')"], true),
            ("gh myext run", Level::Ask, &["gh myext is an alias or extension; the guard cannot see what it runs"], false),
            ("gh issue create -R other/repo -b \"@Claude hi\"", Level::Ask, &["gh issue create targets other/repo, not juslee/aios", "gh issue create mentions @claude, which starts the Claude workflow"], false),
            ("git checkout -B x", Level::Ask, &["git checkout -B can discard uncommitted changes or reset a branch"], false),
            ("git config alias.p push && git p origin main", Level::Ask, &["git p is not a git command; it may be an alias defined earlier in this command"], false),
            ("git push --dry-run origin main", Level::None, &[], false),
            ("git push --mirror origin", Level::Deny, &["--all/--branches/--mirror pushes every branch, including main", "cannot resolve the current branch for this push (directory is not a git repository yet)"], true),
            ("git push origin :main", Level::Deny, &["push would delete main ('main')"], true),
            ("env -S \"git push origin main\"", Level::Deny, &["push would update main (destination 'main')"], true),
            ],
        );
    }

    #[test]
    fn sed_decisions_match_python() {
        check_rows(
            "sed",
            &[
            ("sed -i s/a/b/ .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sed -n 1p .claude/settings.json", Level::None, &[], false),
            ("sed -n 1p f > .claude/hooks/x", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (its output is redirected into .claude/hooks/x)"], false),
            ("cd .claude && sed -i s/x/y/ settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("find .claude -name '*.json' -exec sed -i s/a/b/ {} +", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sed 's/a/b/' .claude/hooks/*", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (the 's' command)"], false),
            ("sed -n 1p <<< \"$(cat > .claude/settings.json)\"", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (a substitution, here-string or heredoc body names .claude/settings* or .claude/hooks/*)"], false),
            ("xargs sed -i s/a/b/ < list # .claude", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sed -n \"$S\" .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt ($S comes from a shell expansion and may be an option)"], false),
            ("gsed -e 1p -i .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sudo sed -i 1d /x/.claude/settings.local.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sed -n 1p .c*/settings*", Level::None, &[], false),
            ("cd .claude/hooks && sed -n w\\ x f", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (the 'w' command)"], false),
            ("sed -n 1p \"$OPT\" .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt ($OPT comes from a shell expansion and may be an option)"], false),
            ("sh -c 'sed -i 1d \"$1\"' _ .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (option -i)"], false),
            ("sed -n '/[[:alpha:]/p' .claude/settings.json", Level::Ask, &["sed may write a protected path (.claude/settings*, .claude/hooks/*): only read-only sed scripts run without a prompt (a /regex/ whose bracket expression GNU and BSD sed end differently)"], false),
            ("sed --expression=1p --quiet .claude/hooks/aios", Level::None, &[], false),
            ],
        );
    }
}
