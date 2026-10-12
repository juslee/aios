//! `aios brief` parity with the former script of `/justin:brief` (R3, Tasks 4, 6 and 7).
//!
//! The oracle is the script's blob at `agent::BRIEF_ORACLE`, read from git history and run
//! as `<bash> <script> <args>` in a twin repository under the PATH jail, with the fake
//! `gh` and the fake docs-check child (`agent::FAKE_DOCS`). The 76 scenarios of
//! [`CASES`] each describe one repository state, one argv and the `gh` answers it gets.
//! Their `gh` and docs-check answers are committed under
//! `tests/fixtures/brief/<scenario>/{gh,docs}/` (synthetic values in the shapes of real
//! `gh` output: Task 0d's capture, and [`capture_real_gh_shapes`] checks them against
//! the real thing). The goldens in `tests/golden/brief/<name>.golden` were recorded from
//! the oracle on macOS (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test
//! brief_parity -- --ignored record_brief_goldens`), through `agent::oracle_renames()` (the
//! documented D1 rename) and `agent::normalize()`. 74 scenarios are differential.
//! `cli_help` (D1) and `pr_labelled_needs_human` (D10, `agent::DIVERGENT_SCENARIOS`) are
//! aios-only: their goldens were blessed from the port, one scenario at a time, and
//! reviewed by hand.
//!
//! A golden holds these sections, in order: `exit`, `stdout`, `stderr`, `gh calls` (the
//! rendered argv, the cwd and the environment of every fake-gh call), `docs calls` (the
//! fake docs-check's argv and cwd), `marker touched` (whether the brief wrote
//! `<git-common-dir>/aios-agent/last-brief`) and `remote refs` (`git for-each-ref
//! refs/remotes` after the run).
//!
//! The tests:
//!
//! - one `#[test]` per scenario, named after it: `aios brief` against the scenario's
//!   golden (`cargo test -p aios-tools --test brief_parity <name>` runs one);
//! - `oracle_reproduces_brief_goldens`: the oracle against the 74 differential goldens;
//! - `brief_differential_against_oracle`: the oracle and aios in twin repositories over
//!   the 74 differential scenarios, comparing everything a golden holds;
//! - `brief_fixtures_label_needs_human_only_in_d10`: only the D10 scenario serves a PR
//!   labelled `needs-human`, so every other scenario stays differential;
//! - `record_brief_goldens` and `capture_real_gh_shapes`, both `#[ignore]`.
//!
//! Every scenario also runs `assert_not_vacuous` and the fake gh's `BRIEF_GH_ALLOW`
//! allowlist (the shared checks, [`check_outcome`]). Test data builds nothing
//! token-shaped (contract §10).

mod common;

use common::agent::{
    self, brief_base, brief_fixtures_dir, Outcome, Scenario, Sub, BRIEF_SECTIONS,
    DIVERGENT_SCENARIOS, EPOCH_BASE,
};
use common::parallel_map;
use std::path::{Path, PathBuf};

/// One scenario: its name (also the golden's and the fixture directory's name), the argv
/// after `brief`, how to build the repository, and an optional extra assertion on the run.
struct Case {
    name: &'static str,
    args: &'static [&'static str],
    build: fn(&str) -> Scenario,
    check: Option<fn(&Outcome)>,
}

/// The scenario table. Each `scenario!(<name>, [<args>], <builder>[, <check>])` at
/// column 0 declares one entry (the builder takes the scenario's name) and one `#[test]` of
/// the same name, which runs aios on it against its golden.
macro_rules! scenarios {
    (@check) => {
        None
    };
    (@check $check:path) => {
        Some($check)
    };
    ($(scenario!($name:ident, [$($arg:expr),* $(,)?], $build:expr $(, $check:path)? $(,)?);)*) => {
        static CASES: &[Case] = &[$(Case {
            name: stringify!($name),
            args: &[$($arg),*],
            build: $build,
            check: scenarios!(@check $($check)?),
        }),*];

        $(
            #[test]
            fn $name() {
                aios_matches_golden(stringify!($name));
            }
        )*
    };
}

/// 2026-01-01T00:00:00Z, the date of the base commits (`agent::EPOCH_BASE`): a scenario's
/// n-th commit is dated `T0 + n`, so a file mtime set to `T0 + k` falls between commits.
const T0: u64 = EPOCH_BASE;
/// The mtime a scenario gives `last-brief` before the run: a touch by the brief changes it.
const MARKER_PRESET: u64 = T0 + 2;

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/brief")
}

/// A scenario that serves no fixture and ends before any section is printed.
fn usage_error(name: &str) -> Scenario {
    Scenario::brief(name)
        .expects_gh(false)
        .expects_docs_calls(Some(0))
        .expects_sections(0)
}

/// `main` gets a second commit, pushed: origin/main is two commits long.
fn second_commit(scenario: Scenario) -> Scenario {
    scenario
        .file("second.txt", "two\n")
        .commit("Second commit")
        .git(&["push", "-q", "origin", "main"])
}

/// An open numbered phase milestone with a step and one open task.
const MILESTONE_DOC: &str =
    "## Milestone 41 - Alpha\n\n### Step 3: Do it\n\n- [x] done already\n- [ ] the open task\n";

/// A committed-later phase doc named `file` under `docs/phases`.
fn phase_doc(scenario: Scenario, file: &str, status: &str, body: &str) -> Scenario {
    scenario.file(
        &format!("docs/phases/{file}"),
        format!("# Phase\n\n**Status:** {status}\n\n{body}"),
    )
}

/// The owner file of a lock whose wrapper is gone (a pid no process has).
const DEAD_LOCK_OWNER: &str =
    "team=team-build\nmode=boot\nlabel=gate boot\nstarted=1\neta=2\npid=999999999\n";

fn handoff_lines(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// A handoff file in `.remember/` with the given mtime.
fn handoff(scenario: Scenario, name: &str, text: impl AsRef<[u8]>, mtime: u64) -> Scenario {
    let rel = format!(".remember/{name}");
    scenario.file(&rel, text).mtime(&rel, mtime)
}

// --- soak fixtures -------------------------------------------------------------------

const BOOTS_ROW: &str = "| Boots | 10 (text mode) |";

/// A finished run's `summary.md`; `commit` may hold `{rev:..}` placeholders.
fn summary_md(commit: &str, boots_row: &str) -> String {
    format!(
        "# Soak summary\n\n| Field | Value |\n| --- | --- |\n| Commit | `{commit}` |\n{boots_row}\n\n\
         CLEAN rate: 8/10 (80%)\n"
    )
}

/// A `summary.tsv`: the header, then one row per boot class.
fn summary_tsv(mode: &str, classes: &[&str]) -> String {
    let mut text = String::from("boot\tmode\tclass\n");
    for (i, class) in classes.iter().enumerate() {
        text.push_str(&format!("{}\t{mode}\t{class}\n", i + 1));
    }
    text
}

/// A run directory `dir` (relative to the root) with a `summary.md` and a `summary.tsv`
/// (each optional), both at `mtime`.
fn soak_run(
    mut scenario: Scenario,
    dir: &str,
    mtime: u64,
    md: Option<String>,
    tsv: Option<String>,
) -> Scenario {
    if let Some(md) = md {
        let rel = format!("{dir}/summary.md");
        scenario = scenario.file_revs(&rel, &md).mtime(&rel, mtime);
    }
    if let Some(tsv) = tsv {
        let rel = format!("{dir}/summary.tsv");
        scenario = scenario.file(&rel, tsv).mtime(&rel, mtime);
    }
    scenario
}

/// A finished run of `commit` with ten boots, `clean` of them CLEAN and the rest PANIC.
fn finished_run(scenario: Scenario, dir: &str, mtime: u64, commit: &str, clean: usize) -> Scenario {
    let mut classes = vec!["CLEAN"; clean];
    classes.resize(10, "PANIC");
    soak_run(
        scenario,
        dir,
        mtime,
        Some(summary_md(commit, BOOTS_ROW)),
        Some(summary_tsv("text", &classes)),
    )
}

/// An interleaved run's `arms.tsv` (two arms), `boots.tsv` and `summary.md`, at mtimes
/// that make `summary.md` the newest. Needs a second commit on `main`.
fn interleaved_run(mut scenario: Scenario, dir: &str, with_summary: bool) -> Scenario {
    let files = [
        (
            "arms.tsv",
            "arm\tcommit\nA\t{rev:HEAD~1}\nB\t{rev:HEAD}\n",
            T0 + 1000,
        ),
        (
            "boots.tsv",
            "boot\tarm\tclass\n1\tA\tCLEAN\n2\tB\tCLEAN\n3\tA\tPANIC\n4\tB\tCLEAN\n",
            T0 + 2000,
        ),
    ];
    for (name, text, mtime) in files {
        let rel = format!("{dir}/{name}");
        scenario = scenario.file_revs(&rel, text).mtime(&rel, mtime);
    }
    if with_summary {
        let rel = format!("{dir}/summary.md");
        scenario = scenario
            .file(
                &rel,
                "# Interleaved soak\n\n**Status:** complete, 4 boots (2 per arm)\n",
            )
            .mtime(&rel, T0 + 3000);
    }
    scenario
}

// --- the scenario-specific assertions ---------------------------------------------------

fn stdout_has(outcome: &Outcome, needles: &[&str]) {
    let stdout = outcome.stdout_str();
    for needle in needles {
        assert!(
            stdout.contains(needle),
            "no {needle:?} in:\n{stdout}\nstderr:\n{}",
            outcome.stderr_str()
        );
    }
}

fn offline_without_a_reason(outcome: &Outcome) {
    stdout_has(outcome, &["- GitHub unavailable: unknown error\n"]);
    assert_eq!(outcome.gh_calls.len(), 1, "only the probe is called");
}

fn every_blocker(outcome: &Outcome) {
    stdout_has(
        outcome,
        &[
            "(draft)",
            "gated by #52, #50",
            "named in needs-human #51",
            "no checks reported",
            "4 failing",
            "2 pending",
            "mergeable: CONFLICTING",
            "merge state: DIRTY",
            "changes requested",
            "2 unresolved review thread(s)",
            "review threads unknown",
            "{no team}",
            "{team-fix,team-build}",
        ],
    );
}

fn labelled_pr_is_not_ready(outcome: &Outcome) {
    // The oracle does not read the label (it would print `merge-ready: yes` for this
    // fixture): D10 is aios's, and its golden is aios-only.
    stdout_has(outcome, &["merge-ready: no (labelled needs-human)"]);
}

fn suites_query_fails_so_runs_are_never_listed(outcome: &Outcome) {
    assert!(
        outcome
            .gh_calls
            .iter()
            .all(|call| !call.line().starts_with("run list ")),
        "run list was called after a failed suites query"
    );
    stdout_has(
        outcome,
        &["- GitHub unavailable: gh: simulated suites failure\n"],
    );
}

fn queue_is_unknown(outcome: &Outcome) {
    stdout_has(outcome, &["- queue: ? agent-ready, ? agent-working\n"]);
}

fn no_origin_main_has_no_suites_query(outcome: &Outcome) {
    assert!(
        outcome
            .gh_calls
            .iter()
            .all(|call| !call.line().contains("checkSuites")),
        "the suites query needs an origin/main tip"
    );
    stdout_has(outcome, &["(origin/main unknown)"]);
}

fn docs_ran_in_the_root(outcome: &Outcome) {
    let root = outcome.twin().root().canonicalize().expect("canonicalise");
    assert_eq!(outcome.docs_calls.len(), 1);
    assert_eq!(
        outcome.docs_calls[0].cwd, root,
        "docs-check runs in the toplevel"
    );
}

fn linked_worktree_pins_main_checkout_reads(outcome: &Outcome) {
    let root = outcome.twin().root().canonicalize().expect("canonicalise");
    assert_eq!(
        outcome.docs_calls[0].cwd,
        root.join("wt-x"),
        "docs-check runs in the linked worktree"
    );
    stdout_has(
        outcome,
        &[
            "- checkout: wt-x on `claude/x`",
            "(main checkout)",
            "- state=dead",
            "build lane",
            "fix lane",
            "docs/phases/01-wt.md",
        ],
    );
    assert!(
        !outcome
            .stdout_str()
            .contains("no scripts/agent/qemu-lock.sh in the main checkout"),
        "the lock is read in the main checkout"
    );
}

fn stale_binary_is_flagged(outcome: &Outcome) {
    stdout_has(
        outcome,
        &["(from a stale aios binary whose rebuild failed; run `just tools`)"],
    );
}

scenarios! {
// GitHub reachability (6).
scenario!(offline, ["--no-fetch"], brief_base);
scenario!(offline_empty_stderr, ["--no-fetch"], brief_base, offline_without_a_reason);
scenario!(gh_missing, ["--no-fetch"], |n| brief_base(n).gh_missing());
scenario!(rate_zero, ["--no-fetch"], brief_base);
// The origin's URL is gone, so `git fetch` fails; the run fetches (no --no-fetch).
scenario!(fetch_fails, [],
    |n| brief_base(n).git(&["config", "--unset", "remote.origin.url"]));
// The origin deleted a branch the clone still tracks; the run fetches and prunes it.
scenario!(fetch_prunes, [],
    |n| brief_base(n)
        .branch("claude/gone")
        .file("gone.txt", "gone\n")
        .commit("Work on a branch the origin will delete")
        .git(&["push", "-q", "-u", "origin", "claude/gone"])
        .git(&["checkout", "-q", "main"])
        .git_in("../origin.git", &["branch", "-q", "-D", "claude/gone"]));

// Remote state (1): origin has only a claude/x branch, so there is no origin/main.
scenario!(no_origin_main, ["--no-fetch"],
    |n| finished_run(
        brief_base(n)
            .no_origin_main()
            .branch("claude/x")
            .file("x.txt", "x\n")
            .commit("Work on x")
            .git(&["push", "-q", "-u", "origin", "claude/x"]),
        "target/soak/r1-text",
        T0 + 5000,
        "{rev:HEAD}",
        10,
    ),
    no_origin_main_has_no_suites_query);

// Working directory (2).
scenario!(run_from_subdirectory, ["--no-fetch"],
    |n| handoff(
        phase_doc(brief_base(n), "01-sub.md", "In Progress", MILESTONE_DOC)
            .file("docs/knowledge/lessons/sub-note.md", "a lesson\n")
            .commit("Add docs")
            .run_cwd("docs"),
        "handoff-team-build.md",
        "build lane\n",
        T0,
    ),
    docs_ran_in_the_root);
// Run in the linked worktree wt-x: its branch deletes qemu-lock.sh and adds a phase doc,
// the main worktree keeps the script and holds the handoffs, and the lock (a dead pid)
// is in the shared common directory.
scenario!(run_from_linked_worktree, ["--no-fetch"],
    |n| handoff(
        handoff(
            brief_base(n)
                .file(".git/aios-agent/qemu.lock/owner", DEAD_LOCK_OWNER)
                .worktree("wt-x", "claude/x")
                .remove("wt-x/scripts/agent/qemu-lock.sh")
                .file("wt-x/docs/phases/01-wt.md", format!(
                    "# Phase\n\n**Status:** In Progress\n\n{MILESTONE_DOC}"
                ))
                .commit_in("wt-x", "Work in the linked worktree")
                .run_in_worktree("wt-x"),
            "handoff-team-build.md",
            "build lane\n",
            T0 + 4000,
        ),
        "handoff-team-fix.md",
        "fix lane\n",
        T0 + 3000,
    ),
    linked_worktree_pins_main_checkout_reads);

// Worktrees (1): detached, missing, not on origin, ahead and behind origin/b, and a
// local main behind origin/main.
scenario!(worktrees, ["--no-fetch"],
    |n| second_commit(
        brief_base(n)
            .git(&["worktree", "add", "-q", "--detach", ".claude/worktrees/w-det", "main"])
            .worktree(".claude/worktrees/w-local", "claude/local")
            .file(".claude/worktrees/w-local/l.txt", "local\n")
            .commit_in(".claude/worktrees/w-local", "Local work")
            .worktree(".claude/worktrees/w-b", "claude/b")
            .file(".claude/worktrees/w-b/b1.txt", "1\n")
            .commit_in(".claude/worktrees/w-b", "B one")
            .file(".claude/worktrees/w-b/b2.txt", "2\n")
            .commit_in(".claude/worktrees/w-b", "B two")
            .git_in(".claude/worktrees/w-b", &["push", "-q", "-u", "origin", "claude/b"])
            .git_in(".claude/worktrees/w-b", &["reset", "-q", "--hard", "HEAD~1"])
            .file(".claude/worktrees/w-b/b3.txt", "3\n")
            .commit_in(".claude/worktrees/w-b", "B three")
            .worktree(".claude/worktrees/w-gone", "claude/gone")
            .git(&["-c", "alias.zap=!rm -rf .claude/worktrees/w-gone", "zap"]),
    )
    .git(&["reset", "-q", "--hard", "HEAD~1"]));

// Pull requests (9).
scenario!(prs_every_blocker, ["--no-fetch"], brief_base, every_blocker);
scenario!(merge_ready_yes, ["--no-fetch"], brief_base);
scenario!(threads_unknown, ["--no-fetch"], brief_base);
scenario!(threads_zero_is_known, ["--no-fetch"], brief_base);
scenario!(gates_unknown, ["--no-fetch"], brief_base);
scenario!(pr_beyond_50_threads_unknown, ["--no-fetch"], brief_base);
scenario!(no_prs, ["--no-fetch"], brief_base);
// pr list fails after the probe, the issues and the threads succeed; the phase doc's next
// step is in a milestone, so the Phase section cannot look for its PR.
scenario!(pr_list_fails, ["--no-fetch"],
    |n| phase_doc(brief_base(n), "01-alpha.md", "In Progress", MILESTONE_DOC).commit("Add a phase doc"));
scenario!(pr_labelled_needs_human, ["--no-fetch"], brief_base, labelled_pr_is_not_ready);

// Main CI (6).
scenario!(main_ci, ["--no-fetch"], brief_base);
scenario!(main_ci_tie, ["--no-fetch"], brief_base);
scenario!(main_ci_not_tip, ["--no-fetch"], brief_base);
scenario!(main_ci_empty, ["--no-fetch"], brief_base);
scenario!(main_ci_suites_fail, ["--no-fetch"], brief_base, suites_query_fails_so_runs_are_never_listed);
scenario!(main_ci_runs_fail, ["--no-fetch"], brief_base);

// Needs human (1).
scenario!(queue_counts_unknown, ["--no-fetch"], brief_base, queue_is_unknown);

// Soak (17).
scenario!(soak_main_tip, ["--no-fetch"],
    |n| finished_run(brief_base(n), "target/soak/r1-text", T0 + 5000, "{rev:origin/main}", 10));
scenario!(soak_behind, ["--no-fetch"],
    |n| finished_run(second_commit(brief_base(n)), "target/soak/r1-text", T0 + 5000, "{rev:origin/main~1}", 9));
scenario!(soak_not_on_main, ["--no-fetch"],
    |n| finished_run(
        brief_base(n)
            .branch("claude/side")
            .file("side.txt", "side\n")
            .commit("Side work")
            .git(&["checkout", "-q", "main"]),
        "target/soak/r1-text",
        T0 + 5000,
        "{rev:claude/side}",
        7,
    ));
scenario!(soak_dirty, ["--no-fetch"],
    |n| finished_run(brief_base(n), "target/soak/r1-text", T0 + 5000, "{rev:origin/main}-dirty", 10));
scenario!(soak_partial, ["--no-fetch"],
    |n| soak_run(brief_base(n), "target/soak/p1-text", T0 + 4000, None,
        Some(summary_tsv("text", &["CLEAN", "CLEAN", "PCZERO"]))));
scenario!(soak_no_rows, ["--no-fetch"],
    |n| soak_run(brief_base(n), "target/soak/r1-text", T0 + 5000,
        Some(summary_md("{rev:origin/main}", BOOTS_ROW)), Some(summary_tsv("text", &[]))));
scenario!(soak_legacy_wedge, ["--no-fetch"],
    |n| soak_run(brief_base(n), "target/soak/r1-gpu", T0 + 5000,
        Some(summary_md("{rev:origin/main}", BOOTS_ROW)),
        Some(summary_tsv("gpu", &["CLEAN", "WEDGE", "WEDGE", "PANIC-LOCK", "WEDGE-STUCK", "DEGRADED"]))));
scenario!(soak_interleaved_clean_per_arm, ["--no-fetch"],
    |n| interleaved_run(second_commit(brief_base(n)), "target/soak/ab-1", true));
scenario!(soak_interleaved_no_arms, ["--no-fetch"],
    |n| brief_base(n)
        .file("target/soak/ab-1/arms.tsv", "arm\tcommit\n")
        .mtime("target/soak/ab-1/arms.tsv", T0 + 1000)
        .file("target/soak/ab-1/boots.tsv", "boot\tarm\tclass\n1\tA\tCLEAN\n")
        .mtime("target/soak/ab-1/boots.tsv", T0 + 2000));
scenario!(soak_interleaved_no_summary, ["--no-fetch"],
    |n| interleaved_run(second_commit(brief_base(n)), "target/soak/ab-1", false));
// An interleaved run beside a main soak: both are listed, the interleaved one on its own,
// and its arm-A/ directory (single-run files) is neither.
scenario!(soak_interleaved_beside_main, ["--no-fetch"],
    |n| finished_run(
        interleaved_run(second_commit(brief_base(n)), "target/soak/ab-1", true)
            .file("target/soak/ab-1/arm-A/summary.md", "| Commit | `0000000` |\n")
            .file("target/soak/ab-1/arm-A/summary.tsv", summary_tsv("text", &["CLEAN"])),
        "target/soak/main-1-text",
        T0 + 5000,
        "{rev:origin/main}",
        10,
    ));
// boots.tsv is missing: BSD awk stops at the missing file, gawk and mawk run END.
scenario!(soak_interleaved_no_boots, ["--no-fetch"],
    |n| second_commit(brief_base(n))
        .file_revs("target/soak/ab-1/arms.tsv", "arm\tcommit\nA\t{rev:HEAD~1}\nB\t{rev:HEAD}\n")
        .mtime("target/soak/ab-1/arms.tsv", T0 + 1000)
        .file("target/soak/ab-1/summary.md", "# Interleaved soak\n\n**Status:** aborted before the first boot\n")
        .mtime("target/soak/ab-1/summary.md", T0 + 3000));
// The Boots row has no closing ` |`: the sed does not match and the whole row is printed.
scenario!(soak_boots_row_quirk, ["--no-fetch"],
    |n| soak_run(brief_base(n), "target/soak/r1-text", T0 + 5000,
        Some(summary_md("{rev:origin/main}", "| Boots | 10 (text mode)")), None));
scenario!(soak_none, ["--no-fetch"], brief_base);
scenario!(soak_commit_is_branch_name, ["--no-fetch"],
    |n| finished_run(brief_base(n).git(&["branch", "claude/x"]), "target/soak/r1-text", T0 + 5000, "claude/x", 10));
scenario!(soak_equal_mtime_tie, ["--no-fetch"],
    |n| finished_run(
        finished_run(brief_base(n), "target/soak/r2-text", T0 + 6000, "{rev:origin/main}", 7),
        "target/soak/r1-text",
        T0 + 6000,
        "{rev:origin/main}",
        10,
    ));
scenario!(soak_equal_mtime_tie_across_worktrees, ["--no-fetch"],
    |n| finished_run(
        finished_run(
            brief_base(n).worktree("wt-x", "claude/x"),
            "wt-x/target/soak/run-text",
            T0 + 6000,
            "{rev:origin/main}",
            7,
        ),
        "target/soak/run-text",
        T0 + 6000,
        "{rev:origin/main}",
        10,
    ));

// QEMU lock (3).
scenario!(qemu_lock_free, ["--no-fetch"], brief_base);
scenario!(qemu_lock_dead_holder, ["--no-fetch"],
    |n| brief_base(n).file(".git/aios-agent/qemu.lock/owner", DEAD_LOCK_OWNER));
scenario!(qemu_lock_missing, ["--no-fetch"], |n| brief_base(n).no_qemu_lock());

// Routing log (3).
scenario!(routing_log, ["--no-fetch"],
    |n| brief_base(n)
        .env("AIOS_HOOK_STATE_DIR", "{root}/.git/hook-state")
        .file(".git/hook-state/route-shadow.jsonl", "{\"a\":1}\n{\"a\":2}\n\n{\"a\":3}\n")
        .file(
            ".git/hook-state/route-outcome.jsonl",
            "{\"kind\":\"launched\",\"agent\":\"a\"}\n{\"kind\":\"finished\"}\n\
             {\"kind\":\"launched\",\"agent\":\"b\"}\n{\"kind\": \"launched\"}\n",
        ));
// An empty value counts as unset: the records are read from the default directory.
scenario!(routing_log_empty_var, ["--no-fetch"],
    |n| brief_base(n)
        .env("AIOS_HOOK_STATE_DIR", "")
        .file(".git/aios-agent/hooks/route-shadow.jsonl", "{\"a\":1}\n{\"a\":2}\n")
        .file(".git/aios-agent/hooks/route-outcome.jsonl", "{\"kind\":\"launched\"}\n"));
scenario!(routing_log_missing, ["--no-fetch"], brief_base);

// Handoffs (2): two by mtime, a tie broken by name, no trailing newline, 61+ lines and
// `handoff-.md`; a file that is not a handoff is ignored.
scenario!(handoffs, ["--no-fetch"],
    |n| {
        let s = handoff(brief_base(n), "handoff-team-build.md", handoff_lines(65), T0 + 9000);
        let s = handoff(s, "handoff-team-fix.md", "fix lane\nno trailing newline", T0 + 8000);
        let s = handoff(s, "handoff-solo-b.md", "solo b\n", T0 + 7000);
        let s = handoff(s, "handoff-solo-a.md", "solo a\n", T0 + 7000);
        let s = handoff(s, "handoff-.md", "no key\n", T0 + 6000);
        s.file(".remember/notes.md", "not a handoff\n")
    });
scenario!(handoffs_none, ["--no-fetch"], brief_base);

// Knowledge (6). Commit n is dated T0 + n.
// The newest handoff (T0 + 2) bounds the window: the commit before it is out.
scenario!(knowledge_since_handoff, ["--no-fetch"],
    |n| handoff(
        brief_base(n)
            .file("docs/knowledge/lessons/old-lesson.md", "old\n")
            .commit("Old lesson")
            .file("filler.txt", "filler\n")
            .commit("Filler")
            .file("docs/knowledge/decisions/new-decision.md", "new\n")
            .commit("New decision")
            .file("docs/knowledge/decisions/new-decision.md", "new, edited\n")
            .file("docs/knowledge/research/new-research.md", "research\n")
            .commit("Edit and research"),
        "handoff-team-build.md",
        "build lane\n",
        T0 + 2,
    ));
// With no handoff, the marker's mtime (T0 + 2) bounds the window.
scenario!(knowledge_since_marker, ["--no-fetch"],
    |n| brief_base(n)
        .file("docs/knowledge/lessons/old-lesson.md", "old\n")
        .commit("Old lesson")
        .file("filler.txt", "filler\n")
        .commit("Filler")
        .file("docs/knowledge/decisions/new-decision.md", "new\n")
        .commit("New decision")
        .file(".git/aios-agent/last-brief", "")
        .mtime(".git/aios-agent/last-brief", MARKER_PRESET));
// Neither a handoff nor a marker: the last 7 days, and the commits are older, so only
// uncommitted notes (a modified tracked one, an untracked one) are listed.
scenario!(knowledge_7_days, ["--no-fetch"],
    |n| brief_base(n)
        .file("docs/knowledge/README.md", "knowledge\n")
        .commit("Add the knowledge readme")
        .file("docs/knowledge/README.md", "knowledge, edited\n")
        .file("docs/knowledge/draft.md", "draft\n"));
scenario!(knowledge_over_25, ["--no-fetch"],
    |n| {
        let mut s = brief_base(n);
        for i in 1..=30 {
            s = s.file(&format!("docs/knowledge/lessons/l{i:02}.md"), format!("lesson {i}\n"));
        }
        handoff(s.commit("Thirty lessons"), "handoff-team-build.md", "build lane\n", T0)
    });
// A staged rename lists the new name; an untracked path with a space is quoted by git
// and the awk keeps its last word. Both are kept as they are.
scenario!(knowledge_uncommitted_rename_and_space, ["--no-fetch"],
    |n| brief_base(n)
        .file("docs/knowledge/old-name.md", "old\n")
        .commit("Add a note")
        .git(&["mv", "docs/knowledge/old-name.md", "docs/knowledge/renamed.md"])
        .file("docs/knowledge/a b.md", "spaced\n"));
scenario!(knowledge_template_filtered, ["--no-fetch"],
    |n| handoff(
        brief_base(n)
            .file("docs/knowledge/plans/_template.md", "template\n")
            .file("docs/knowledge/plans/real-plan.md", "plan\n")
            .commit("Add a plan"),
        "handoff-team-build.md",
        "build lane\n",
        T0,
    ));

// Phase (8).
scenario!(phase_in_progress, ["--no-fetch"],
    |n| phase_doc(
        phase_doc(
            phase_doc(brief_base(n), "01-boot.md", "Complete (2026-01-01)", "## Milestone 1\n\n- [x] done\n"),
            "02-mm.md",
            "Not Started",
            "## Milestone 2\n\n- [ ] later\n",
        ),
        "03-ipc.md",
        "In Progress",
        "## Setup\n\n### Step 2: Wire it\n\n- [x] first\n  * [ ] nested open item\n- [ ] second\n",
    )
    .commit("Add phase docs"));
scenario!(phase_all_complete, ["--no-fetch"],
    |n| phase_doc(
        phase_doc(brief_base(n), "01-boot.md", "Complete", "## Milestone 1\n\n- [x] done\n"),
        "02-mm.md",
        "Complete (2026-01-02)",
        "## Milestone 2\n\n- [x] done\n",
    )
    .commit("Add phase docs"));
scenario!(phase_struck_task, ["--no-fetch"],
    |n| phase_doc(
        brief_base(n),
        "01-quality.md",
        "In Progress",
        "## Milestone 5 - Quality\n\n### Step 9: Cleanup\n\n- [ ] ~~dropped idea~~\n- [ ] the real open task\n",
    )
    .commit("Add a phase doc"));
scenario!(phase_no_step_heading, ["--no-fetch"],
    |n| phase_doc(
        brief_base(n),
        "01-hardening.md",
        "In Progress",
        "## Milestone 3 - Hardening\n\n- [ ] unstepped task\n",
    )
    .commit("Add a phase doc"));
scenario!(phase_all_checked, ["--no-fetch"],
    |n| phase_doc(brief_base(n), "01-done.md", "In Progress", "## Setup\n\n- [x] a\n- [x] b\n")
        .commit("Add a phase doc"));
scenario!(phase_open_pr_for_m, ["--no-fetch"],
    |n| phase_doc(brief_base(n), "01-alpha.md", "In Progress", MILESTONE_DOC).commit("Add a phase doc"));
scenario!(phase_open_pr_none, ["--no-fetch"],
    |n| phase_doc(brief_base(n), "01-alpha.md", "In Progress", MILESTONE_DOC).commit("Add a phase doc"));
scenario!(phase_open_pr_unknown, ["--no-fetch"],
    |n| phase_doc(brief_base(n), "01-alpha.md", "In Progress", MILESTONE_DOC).commit("Add a phase doc"));

// Docs (8). The docs-check answer is in each scenario's docs/ fixture.
scenario!(docs_clean, ["--no-fetch"], brief_base);
scenario!(docs_new_drift_key_order, ["--no-fetch"], brief_base);
scenario!(docs_prune_hint, ["--no-fetch"], brief_base);
scenario!(docs_rc2, ["--no-fetch"], brief_base);
scenario!(docs_panic101, ["--no-fetch"], brief_base);
scenario!(docs_other_rc_last_line, ["--no-fetch"], brief_base);
scenario!(docs_unreadable_json, ["--no-fetch"], brief_base);
// The oracle's fake docs-check prints the shim's stale-rebuild warning; aios gets
// AIOS_SHIM_STALE=1 instead (D6): the note is the same.
scenario!(docs_stale_note, ["--no-fetch"], brief_base, stale_binary_is_flagged);

// CLI (3).
scenario!(cli_help, ["--help"], usage_error);
// The unknown argument is read before the `--help` after it, so the run ends with the
// usage error (exit 2) and never prints the help text, which is new in aios (D1).
scenario!(cli_unknown_after_help, ["--bogus", "--help"], usage_error);
scenario!(outside_repo, ["--no-fetch"], |n| Scenario::outside_repo(Sub::Brief, n));
}

/// Whether the run wrote `last-brief`: it exists and its mtime is not the one the
/// scenario gave it (`MARKER_PRESET`) before the run.
fn marker_touched(outcome: &Outcome) -> bool {
    let marker = outcome.twin().root().join(".git/aios-agent/last-brief");
    std::fs::metadata(&marker)
        .and_then(|meta| meta.modified())
        .is_ok_and(|mtime| {
            mtime
                .duration_since(std::time::UNIX_EPOCH)
                .is_ok_and(|since| since.as_secs() != MARKER_PRESET)
        })
}

/// The golden text of one run. An oracle run goes through `agent::oracle_renames()` (the
/// documented D1 rename); an aios run is recorded as it is.
fn render_golden(outcome: &Outcome, oracle: bool) -> Vec<u8> {
    let rename = |text: &[u8]| {
        if oracle {
            agent::oracle_renames(text, outcome)
        } else {
            text.to_vec()
        }
    };
    let twin = outcome.twin();
    let ceiling = twin.root().parent().expect("the twin root has a parent");
    // `refs/remotes/origin/HEAD` is left out: a fetch creates it in recent git and not in
    // older git, and the brief never reads it.
    let remote_refs =
        agent::probe(twin.run_dir(), ceiling, &["for-each-ref", "refs/remotes"]).map(|refs| {
            refs.lines()
                .filter(|line| !line.ends_with("\trefs/remotes/origin/HEAD"))
                .collect::<Vec<_>>()
                .join("\n")
        });

    let gh_calls = agent::gh_calls_text(outcome);
    let mut docs_calls = String::new();
    for call in &outcome.docs_calls {
        let argv: Vec<String> = call
            .argv
            .iter()
            .map(|word| String::from_utf8_lossy(word).into_owned())
            .collect();
        docs_calls.push_str(&format!(
            "{}\ndocs-cwd {}\n",
            argv.join(" "),
            call.cwd.display()
        ));
    }
    if docs_calls.is_empty() {
        docs_calls.push_str("(none)\n");
    }

    let mut golden = Vec::new();
    agent::golden_section(
        &mut golden,
        "exit",
        format!("{}\n", outcome.code).as_bytes(),
    );
    agent::golden_section(&mut golden, "stdout", &rename(&outcome.stdout));
    agent::golden_section(&mut golden, "stderr", &rename(&outcome.stderr));
    agent::golden_section(&mut golden, "gh calls", gh_calls.as_bytes());
    agent::golden_section(&mut golden, "docs calls", docs_calls.as_bytes());
    agent::golden_section(
        &mut golden,
        "marker touched",
        if marker_touched(outcome) {
            b"yes\n"
        } else {
            b"no\n"
        },
    );
    agent::golden_section(
        &mut golden,
        "remote refs",
        format!("{}\n", remote_refs.as_deref().unwrap_or("(none)")).as_bytes(),
    );
    agent::normalize(&golden, outcome)
}

/// The labels of the `gh` fixtures a scenario's directory serves (none without a `gh/`).
fn served_labels(name: &str) -> std::collections::BTreeSet<String> {
    let Ok(entries) = std::fs::read_dir(brief_fixtures_dir().join(name).join("gh")) else {
        return std::collections::BTreeSet::new();
    };
    entries
        .map(|entry| {
            entry
                .expect("a directory entry")
                .path()
                .file_stem()
                .expect("a fixture file name")
                .to_string_lossy()
                .into_owned()
        })
        .collect()
}

/// The invariants every scenario shares, and the scenario's own check.
fn check_outcome(case: &Case, outcome: &Outcome) {
    agent::assert_not_vacuous(outcome);
    agent::assert_origin_invariants(outcome);
    if let Some(gh) = outcome.twin().fake_gh() {
        gh.assert_calls_match(agent::BRIEF_GH_ALLOW);
    }
    // A scenario serves exactly the fixtures its run calls: a call without one hits the
    // fake's exit 99 (assert_not_vacuous), and a fixture nobody calls is a stale answer.
    let called: std::collections::BTreeSet<String> = outcome
        .gh_calls
        .iter()
        .map(|call| call.fixture.clone())
        .collect();
    assert_eq!(
        served_labels(case.name),
        called,
        "{}: the gh fixtures served and the gh calls made differ",
        case.name
    );
    let sections = agent::section_count(&outcome.stdout);
    if outcome.code == 0 && sections > 0 {
        assert_eq!(
            sections,
            BRIEF_SECTIONS,
            "{}: a report has {BRIEF_SECTIONS} sections:\n{}",
            case.name,
            outcome.stdout_str()
        );
    }
    if let Some(check) = case.check {
        check(outcome);
    }
}

/// Run `case`'s oracle in its twin and apply the invariants every scenario shares.
fn run_case(case: &Case) -> Outcome {
    let scenario = (case.build)(case.name);
    let outcome = agent::run_oracle(&scenario, case.args);
    check_outcome(case, &outcome);
    outcome
}

/// The cases whose golden an oracle run records.
fn differential_cases() -> Vec<&'static Case> {
    CASES
        .iter()
        .filter(|case| !agent::is_divergent(case.name))
        .collect()
}

fn assert_table_is_sound() {
    assert_eq!(CASES.len(), 76, "the brief scenario table");
    let mut names: Vec<&str> = CASES.iter().map(|case| case.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), CASES.len(), "scenario names are unique");
    for (divergent, _) in DIVERGENT_SCENARIOS {
        let in_brief = ["cli_help", "pr_labelled_needs_human"].contains(divergent);
        assert_eq!(
            in_brief,
            CASES.iter().any(|case| case.name == *divergent),
            "{divergent}: DIVERGENT_SCENARIOS and the table disagree"
        );
    }
    let mut dirs: Vec<String> = std::fs::read_dir(brief_fixtures_dir())
        .expect("read the fixture directory")
        .map(|entry| {
            entry
                .expect("a directory entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    dirs.sort_unstable();
    let with_fixtures: Vec<&str> = names
        .iter()
        .copied()
        .filter(|name| !["cli_help", "cli_unknown_after_help", "outside_repo"].contains(name))
        .collect();
    assert_eq!(
        dirs, with_fixtures,
        "a fixture directory for every scenario that serves one, and no other"
    );
}

/// Every differential scenario's oracle output equals its golden. Runs on every host:
/// the goldens were recorded on macOS, and CI's ubuntu-26.04 reproduces them with GNU
/// tools (a scenario that cannot goes in `agent::PLATFORM_SCENARIOS`).
#[test]
fn oracle_reproduces_brief_goldens() {
    assert_table_is_sound();
    let dir = golden_dir();
    let cases = differential_cases();
    assert_eq!(cases.len(), 74, "the non-divergent scenarios");

    // Every scenario has a golden: the oracle's 74 and the two aios-only ones.
    let known: Vec<String> = CASES
        .iter()
        .map(|case| format!("{}.golden", case.name))
        .collect();
    let mut files = 0;
    for entry in std::fs::read_dir(&dir).expect("read the golden directory") {
        let file = entry
            .expect("a directory entry")
            .file_name()
            .to_string_lossy()
            .into_owned();
        assert!(known.contains(&file), "{file} is no scenario's golden");
        files += 1;
    }
    assert_eq!(files, CASES.len(), "a golden for every scenario");

    let failures: Vec<String> = parallel_map(&cases, |case| {
        if !agent::oracle_runs_here(case.name) {
            return None;
        }
        let got = String::from_utf8_lossy(&render_golden(&run_case(case), true)).into_owned();
        let path = dir.join(format!("{}.golden", case.name));
        let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{}: {e} (record it: AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test \
                 brief_parity -- --ignored record_brief_goldens)",
                path.display()
            )
        });
        (want != got).then(|| format!("{}: {}", case.name, agent::first_difference(&want, &got)))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(
        failures.is_empty(),
        "the oracle no longer reproduces these goldens:\n{}",
        failures.join("\n")
    );
}

/// Only the D10 scenario serves a PR labelled `needs-human`: the oracle ignores that
/// label, so any other scenario serving one would diverge from aios by design.
#[test]
fn brief_fixtures_label_needs_human_only_in_d10() {
    assert_table_is_sound();
    for case in CASES {
        let path = brief_fixtures_dir().join(case.name).join("gh/pr-list.out");
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        if text.is_empty() {
            // pr_list_fails: gh failed, and a failure prints nothing.
            continue;
        }
        let prs: serde_json::Value = serde_json::from_str(&text)
            .unwrap_or_else(|e| panic!("{}: not JSON: {e}", path.display()));
        let labelled = prs.as_array().is_some_and(|prs| {
            prs.iter().any(|pr| {
                pr["labels"].as_array().is_some_and(|labels| {
                    labels
                        .iter()
                        .any(|label| label["name"].as_str() == Some("needs-human"))
                })
            })
        });
        assert_eq!(
            labelled,
            agent::is_divergent(case.name),
            "{}: a PR labelled needs-human belongs to the D10 scenario only",
            case.name
        );
    }
}

// --- aios against the goldens and against the oracle ---------------------------------------

fn case_named(name: &str) -> &'static Case {
    CASES
        .iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("no scenario named {name}"))
}

/// aios on scenario `name` against its golden. The goldens of the two aios-only scenarios
/// are blessed from this run (`AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test
/// brief_parity <name>`, one scenario at a time, reviewed by hand); the 74 differential
/// goldens are recorded from the oracle only, so a bless run leaves them.
fn aios_matches_golden(name: &str) {
    let case = case_named(name);
    let scenario = (case.build)(case.name);
    let outcome = agent::run_aios(&scenario, "brief", case.args);
    check_outcome(case, &outcome);
    let got = String::from_utf8_lossy(&render_golden(&outcome, false)).into_owned();
    let path = golden_dir().join(format!("{name}.golden"));
    if agent::is_divergent(name) && common::soak::bless() {
        std::fs::create_dir_all(golden_dir()).expect("create the golden directory");
        std::fs::write(&path, &got).expect("write an aios-only golden");
    }
    let want = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{}: {e} (an aios-only golden is blessed with AIOS_BLESS_GOLDENS=1 cargo test -p \
             aios-tools --test brief_parity {name})",
            path.display()
        )
    });
    assert!(
        want == got,
        "{name}: aios no longer matches its golden: {}\n--- stdout\n{}\n--- stderr\n{}",
        agent::first_difference(&want, &got),
        outcome.stdout_str(),
        outcome.stderr_str()
    );
}

/// The oracle and aios, in twin repositories, over the 74 differential scenarios: the
/// exit status, stdout and stderr, the `gh` calls (so the call sequence, argv, cwd and
/// environment), the docs-check calls, the marker and the remote refs.
#[test]
fn brief_differential_against_oracle() {
    assert_table_is_sound();
    let cases = differential_cases();
    assert_eq!(cases.len(), 74, "the non-divergent scenarios");
    let failures: Vec<String> = parallel_map(&cases, |case| {
        if !agent::oracle_runs_here(case.name) {
            return None;
        }
        let scenario = (case.build)(case.name);
        let oracle = agent::run_oracle(&scenario, case.args);
        check_outcome(case, &oracle);
        let aios = agent::run_aios(&scenario, "brief", case.args);
        check_outcome(case, &aios);
        let (want, got) = (
            String::from_utf8_lossy(&render_golden(&oracle, true)).into_owned(),
            String::from_utf8_lossy(&render_golden(&aios, false)).into_owned(),
        );
        (want != got).then(|| format!("{}: {}", case.name, agent::first_difference(&want, &got)))
    })
    .into_iter()
    .flatten()
    .collect();
    assert!(
        failures.is_empty(),
        "aios differs from the oracle in these scenarios:\n{}",
        failures.join("\n")
    );
}

/// Record the 74 differential goldens from the oracle. Review the diff.
#[test]
#[ignore = "rewrites the goldens from the oracle: AIOS_BLESS_GOLDENS=1 ... -- --ignored record_brief_goldens"]
fn record_brief_goldens() {
    assert!(
        common::soak::bless(),
        "recording rewrites the goldens: set AIOS_BLESS_GOLDENS=1"
    );
    assert_table_is_sound();
    let dir = golden_dir();
    std::fs::create_dir_all(&dir).expect("create the golden directory");
    let cases = differential_cases();
    let goldens = parallel_map(&cases, |case| render_golden(&run_case(case), true));
    for (case, golden) in cases.iter().zip(goldens) {
        std::fs::write(dir.join(format!("{}.golden", case.name)), golden).expect("write a golden");
    }
}

// --- the real gh shapes ------------------------------------------------------------------

/// The repository the capture reads, named explicitly so that it works from any clone
/// (a clone whose `origin` is a local path has no repository `gh` can resolve).
const REPO: &str = "juslee/aios";

/// The brief oracle's review-threads query (its L177-186), with literal variables.
const THREADS_QUERY: &str = "
        query($owner: String!, $name: String!) {
          repository(owner: $owner, name: $name) {
            pullRequests(states: OPEN, first: 50, orderBy: {field: CREATED_AT, direction: DESC}) {
              nodes { number reviewThreads(first: 100) { nodes { isResolved } } }
            }
          }
        }";
const THREADS_JQ: &str = "[.data.repository.pullRequests.nodes[]
                  | {key: (.number | tostring), value: ([.reviewThreads.nodes[] | select(.isResolved | not)] | length)}]
                 | from_entries";
/// The tip's check-suites query (the oracle's L269-279).
const SUITES_QUERY: &str = "
        query($owner: String!, $name: String!, $oid: GitObjectID!) {
          repository(owner: $owner, name: $name) {
            object(oid: $oid) {
              ... on Commit {
                checkSuites(first: 100) {
                  nodes { status conclusion branch { name } workflowRun { event createdAt workflow { name } } }
                }
              }
            }
          }
        }";
/// The checkpoint oracle's `pr list --head` selection (its L219-220).
const HEAD_JQ: &str = r##".[] | select(.isDraft | not) | "#\(.number)""##;

/// Objects' keys by `__typename` (`""` for an object without one), at any depth.
fn collect_keys(
    value: &serde_json::Value,
    keys: &mut std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
) {
    match value {
        serde_json::Value::Object(map) => {
            let tag = map
                .get("__typename")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string();
            keys.entry(tag).or_default().extend(map.keys().cloned());
            for child in map.values() {
                collect_keys(child, keys);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_keys(item, keys);
            }
        }
        _ => {}
    }
}

/// The JSON type a `--jq` result has: a committed fixture of such a call is compared by
/// type only (its keys are PR numbers, not field names).
fn json_type(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

/// Read-only `gh` against the real repository: one response per call of `aios brief`, written
/// to `target/r3-gh-capture/` (uncommitted), and each committed fixture's shape checked
/// against it. A `--jq` call (rate-limit, the threads map, the queue counts) is compared
/// by JSON type. The other calls' fixtures must use only keys the real response has,
/// compared per `__typename` (`CheckRun` against `CheckRun`, and so on); a response with
/// no object of a kind (no open PR, say) cannot judge that kind, which the output says.
/// Every call goes through `aios_tools::gh::run_program`, so `is_read` refuses anything
/// but a read. Run by the lead: it needs the network and a logged-in `gh`.
#[test]
#[ignore = "uses the network and the real read-only gh: cargo test -p aios-tools --test brief_parity -- --ignored capture_real_gh_shapes"]
fn capture_real_gh_shapes() {
    use aios_tools::gh::{run_program, GhEnv, GhOutcome};
    use aios_tools::merge_ready::PR_FIELDS;
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsStr;

    let (owner, name) = REPO.split_once('/').expect("owner/name");
    let cwd = Path::new(env!("CARGO_MANIFEST_DIR"));
    let out_dir = cwd.join("../target/r3-gh-capture");
    std::fs::create_dir_all(&out_dir).expect("create the capture directory");

    let gh = |args: &[&str]| -> String {
        match run_program(OsStr::new("gh"), args, cwd, &GhEnv::INHERIT) {
            GhOutcome::Ok(stdout) => String::from_utf8(stdout).expect("gh printed UTF-8"),
            other => panic!("gh {args:?} gave {other:?}"),
        }
    };
    let owner_arg = format!("owner={owner}");
    let name_arg = format!("name={name}");
    let tip = gh(&["api", &format!("repos/{REPO}/commits/main"), "--jq", ".sha"])
        .trim()
        .to_string();
    let oid_arg = format!("oid={tip}");
    let threads_query = format!("query={THREADS_QUERY}");
    let suites_query = format!("query={SUITES_QUERY}");
    let calls: Vec<(&str, Vec<&str>)> = vec![
        (
            "rate-limit",
            vec!["api", "rate_limit", "--jq", ".resources.core.remaining"],
        ),
        (
            "issues-needs-human",
            vec![
                "issue",
                "list",
                "-R",
                REPO,
                "--state",
                "open",
                "--label",
                "needs-human",
                "--limit",
                "1000",
                "--json",
                "number,title,body,updatedAt",
            ],
        ),
        (
            "graphql-threads",
            vec![
                "api",
                "graphql",
                "-F",
                &owner_arg,
                "-F",
                &name_arg,
                "-f",
                &threads_query,
                "--jq",
                THREADS_JQ,
            ],
        ),
        (
            "pr-list",
            vec![
                "pr", "list", "-R", REPO, "--state", "open", "--limit", "30", "--json", PR_FIELDS,
            ],
        ),
        (
            "graphql-suites",
            vec![
                "api",
                "graphql",
                "-F",
                &owner_arg,
                "-F",
                &name_arg,
                "-F",
                &oid_arg,
                "-f",
                &suites_query,
            ],
        ),
        (
            "run-list",
            vec![
                "run",
                "list",
                "-R",
                REPO,
                "--branch",
                "main",
                "--event",
                "push",
                "--limit",
                "40",
                "--json",
                "workflowName,status,conclusion,headSha,createdAt,event",
            ],
        ),
        (
            "issues-agent-ready",
            vec![
                "issue",
                "list",
                "-R",
                REPO,
                "--state",
                "open",
                "--label",
                "agent-ready",
                "--limit",
                "1000",
                "--json",
                "number",
                "--jq",
                "length",
            ],
        ),
        (
            "issues-agent-working",
            vec![
                "issue",
                "list",
                "-R",
                REPO,
                "--state",
                "open",
                "--label",
                "agent-working",
                "--limit",
                "1000",
                "--json",
                "number",
                "--jq",
                "length",
            ],
        ),
        (
            "pr-list-head",
            vec![
                "pr",
                "list",
                "-R",
                REPO,
                "--head",
                "main",
                "--state",
                "open",
                "--json",
                "number,isDraft",
                "--jq",
                HEAD_JQ,
            ],
        ),
    ];
    let jq_labels = [
        "rate-limit",
        "graphql-threads",
        "issues-agent-ready",
        "issues-agent-working",
    ];

    let mut problems = Vec::new();
    for (label, args) in &calls {
        let real = gh(args);
        std::fs::write(out_dir.join(format!("{label}.json")), &real).expect("write the capture");
        let real_json: Option<serde_json::Value> = serde_json::from_str(&real).ok();
        let mut fixtures = 0;
        for entry in std::fs::read_dir(brief_fixtures_dir()).expect("read the fixtures") {
            let path = entry
                .expect("a directory entry")
                .path()
                .join(format!("gh/{label}.out"));
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(fixture) = serde_json::from_str::<serde_json::Value>(&text) else {
                continue;
            };
            fixtures += 1;
            let Some(real_json) = &real_json else {
                continue;
            };
            if jq_labels.contains(label) {
                if json_type(&fixture) != json_type(real_json) {
                    problems.push(format!(
                        "{}: a {} where gh gives a {}",
                        path.display(),
                        json_type(&fixture),
                        json_type(real_json)
                    ));
                }
                continue;
            }
            let mut want: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            let mut got: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
            collect_keys(&fixture, &mut want);
            collect_keys(real_json, &mut got);
            for (tag, keys) in &want {
                let Some(real_keys) = got.get(tag) else {
                    println!(
                        "{label}: the real response has no {:?} object; {} not compared",
                        tag,
                        path.display()
                    );
                    continue;
                };
                let extra: Vec<&String> = keys.difference(real_keys).collect();
                if !extra.is_empty() {
                    problems.push(format!(
                        "{}: {:?} objects carry {extra:?}, which gh does not give",
                        path.display(),
                        tag
                    ));
                }
            }
        }
        println!(
            "{label}: {} bytes, {fixtures} committed fixtures compared",
            real.len()
        );
    }
    assert!(
        problems.is_empty(),
        "fixtures that do not fit the real shapes:\n{}",
        problems.join("\n")
    );
}
