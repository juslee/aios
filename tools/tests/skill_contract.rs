//! The skills' contract with `aios brief` and `aios checkpoint` (R3, Task 10g).
//!
//! `/merge-and-cleanup` pipes `just brief --no-fetch` into an awk program, and
//! `/justin:pause` tells its reader what the checkpoint prints and what its exit 3
//! means. Both texts are prose a model follows, so a rename of an output line or a
//! reworded skill would break the loop without failing a build. These tests read the
//! skill files in the repository and hold them against what the binary prints, in
//! the scenarios the parity tests already pin (`brief_parity.rs`,
//! `checkpoint_parity.rs`):
//!
//! - the merge skill's awk line is extracted from `SKILL.md` and run through `sh -c`
//!   on the report of `prs_every_blocker` and `pr_labelled_needs_human`;
//! - the pause skill's command, first-line format, both last-line forms and the
//!   `checkpoint it:` command match the output of `handoff_saved` and
//!   `other_worktree_dirty_from_main`;
//! - the pause skill's two exit-3 cues match the secret stop of
//!   `sensitive_name_new_doc`, the shim's refusal text and the shim test that pins
//!   the refusal's empty stdout.

mod common;

use common::agent::{Outcome, Scenario};
use common::fixture::repo_root;
use regex::Regex;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

fn skill(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn merge_skill() -> String {
    skill(".claude/skills/merge-and-cleanup/SKILL.md")
}

fn pause_skill() -> String {
    skill(".claude/skills/justin/skills/pause/SKILL.md")
}

/// The brief scenario of `brief_parity.rs`'s `base`: the committed `gh` answers and the
/// docs-check answer of `tests/fixtures/brief/<name>/`, a full report of 11 sections.
fn brief_base(name: &str) -> Scenario {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/brief")
        .join(name);
    assert!(dir.is_dir(), "no fixture directory {}", dir.display());
    let docs = dir.join("docs");
    let read = |file: &str| std::fs::read(docs.join(file)).unwrap_or_default();
    let rc = std::fs::read_to_string(docs.join("rc")).map_or(0, |text| {
        text.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}: docs/rc is not a number"))
    });
    assert!(!docs.join("stale").exists(), "{name}: not a stale scenario");
    Scenario::brief(name)
        .gh_fixtures_from_dir(&dir.join("gh"))
        .docs(read("out"), read("err"), rc)
        .expects_sections(11)
}

/// `checkpoint_parity.rs`'s `on_x`: a `claude/x` branch checked out from `main`.
fn on_x(name: &str) -> Scenario {
    Scenario::checkpoint(name).branch("claude/x")
}

fn run_brief(name: &str) -> Outcome {
    let outcome = common::agent::run_aios(&brief_base(name), "brief", &["--no-fetch"]);
    common::agent::assert_not_vacuous(&outcome);
    assert_eq!(outcome.code, 0, "{}", outcome.stderr_str());
    outcome
}

fn run_checkpoint(scenario: &Scenario, args: &[&str]) -> Outcome {
    let outcome = common::agent::run_aios(scenario, "checkpoint", args);
    common::agent::assert_not_vacuous(&outcome);
    outcome
}

/// The merge skill's awk command, as the skill spells it: the line holding
/// `awk -v n=`, from `just brief --no-fetch | awk` to the end of the line, with
/// `<number>` still in it.
fn merge_awk_command(text: &str) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| l.contains("awk -v n=")).collect();
    assert_eq!(
        lines.len(),
        1,
        "one `awk -v n=` line in the skill: {lines:?}"
    );
    let line = lines[0].trim();
    let command = line
        .strip_prefix("just brief --no-fetch | ")
        .unwrap_or_else(|| {
            panic!("the awk line is not piped from `just brief --no-fetch`: {line}")
        });
    assert!(command.starts_with("awk -v n="), "{command}");
    command.to_string()
}

/// Run the skill's awk for PR `number` over `report` with `sh -c`, as the skill's pipe
/// does, and return its stdout.
fn run_awk(command: &str, number: u32, report: &[u8]) -> String {
    let command = command.replace("<number>", &number.to_string());
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(&command)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn sh");
    let mut stdin = child.stdin.take().expect("stdin");
    // The awk program exits at its first match and may close the pipe before the end.
    let _ = stdin.write_all(report);
    drop(stdin);
    let out = child.wait_with_output().expect("wait for sh");
    assert!(
        out.status.success(),
        "`{command}` failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(
        out.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("UTF-8 awk output")
}

#[test]
fn merge_and_cleanup_awk_reads_every_pr_verdict() {
    let command = merge_awk_command(&merge_skill());

    // Every PR of `prs_every_blocker`, one verdict each: the first `merge-ready:` line
    // under the PR's own heading, and never a neighbour's (the heading match ends in
    // a space, so `#1` is not `#10`).
    let blockers = run_brief("prs_every_blocker");
    let report = &blockers.stdout;
    let verdicts: [(u32, &str); 4] = [
        (
            10,
            "  merge-ready: no (draft; gated by #52; no checks reported; mergeable: CONFLICTING; merge state: DIRTY; changes requested; 2 unresolved review thread(s))\n",
        ),
        (
            11,
            "  merge-ready: no (gated by #52, #50; 4 failing; 2 pending; mergeable: null; merge state: BLOCKED; review threads unknown)\n",
        ),
        (12, "  merge-ready: no (named in needs-human #51)\n"),
        (13, "  merge-ready: yes\n"),
    ];
    for (number, line) in verdicts {
        assert_eq!(run_awk(&command, number, report), line, "PR #{number}");
    }
    // A PR the report does not list has no verdict: the skill's "gate unknown" branch.
    // #1 is a prefix of #10 and #50 is an issue, not a PR.
    for number in [1, 14] {
        assert_eq!(run_awk(&command, number, report), "", "PR #{number}");
    }

    // The label is a verdict of its own.
    let labelled = run_brief("pr_labelled_needs_human");
    assert_eq!(
        run_awk(&command, 7, &labelled.stdout),
        "  merge-ready: no (labelled needs-human)\n"
    );

    // The skill names each verdict it acts on, in the words the report prints.
    let text = merge_skill();
    for cue in [
        "gated by #N",
        "named in needs-human #N",
        "labelled needs-human",
    ] {
        assert!(text.contains(cue), "the merge skill does not name `{cue}`");
    }
}

/// The code span that starts right after the first `lead` in `text`, and the text
/// after its closing backtick.
fn next_span<'a>(text: &'a str, lead: &str) -> (&'a str, &'a str) {
    let at = text
        .find(lead)
        .unwrap_or_else(|| panic!("the pause skill lacks `{lead}`"));
    let rest = text[at + lead.len()..]
        .strip_prefix('`')
        .unwrap_or_else(|| panic!("no code span after `{lead}`"));
    let end = rest.find('`').expect("a closing backtick");
    (&rest[..end], &rest[end + 1..])
}

/// A regex for a format the skill spells with `<branch>`, `<sha>` and `...`.
fn format_regex(format: &str) -> Regex {
    let pattern = regex::escape(format)
        .replace("<branch>", "[^ ]+")
        .replace("<sha>", "[0-9a-f]{7,40}")
        .replace(r"\.\.\.", ".+");
    Regex::new(&format!("^{pattern}$")).unwrap_or_else(|e| panic!("{format}: {e}"))
}

#[test]
fn pause_skill_names_just_checkpoint_and_line_formats() {
    let text = pause_skill();
    assert!(
        text.contains("just checkpoint --handoff saved"),
        "the pause skill does not run `just checkpoint --handoff saved`"
    );

    let (first, _) = next_span(&text, "Its first line is ");
    assert_eq!(
        first,
        "checkpoint: <branch> @ <sha> | wip: ... | pushed: ... | handoff: ..."
    );
    let (safe, rest) = next_span(&text, "Its last line is either ");
    let (kept, _) = next_span(rest, " or ");
    assert_eq!(safe, "Safe to /clear — run /justin:start after");
    assert_eq!(kept, "/clear keeps all of it on disk; ...");

    // The clean run: first line in the skill's format, last line the first form.
    let saved = run_checkpoint(&on_x("handoff_saved"), &["--handoff", "saved"]);
    assert_eq!(saved.code, 0, "{}", saved.stderr_str());
    let stdout = saved.stdout_str();
    let first_line = stdout.lines().next().expect("a first line");
    assert!(
        format_regex(first).is_match(first_line),
        "first line `{first_line}` is not `{}`",
        first
    );
    assert!(first_line.ends_with("| handoff: saved"), "{first_line}");
    assert_eq!(stdout.lines().last(), Some(safe), "{stdout}");

    // A run that leaves work in another worktree: the second last-line form, and the
    // `checkpoint it:` command in the form the skill quotes.
    let dirty = run_checkpoint(
        &on_x("other_worktree_dirty_from_main")
            .worktree(".claude/worktrees/w1", "claude/w1")
            .file(".claude/worktrees/w1/work.txt", "work\n")
            .commit_in(".claude/worktrees/w1", "w1 work")
            .file(".claude/worktrees/w1/dirty.txt", "dirty\n"),
        &[],
    );
    assert_eq!(dirty.code, 1, "{}", dirty.stderr_str());
    let stdout = dirty.stdout_str();
    let first_line = stdout.lines().next().expect("a first line");
    assert!(
        format_regex(first).is_match(first_line),
        "first line `{first_line}` is not `{}`",
        first
    );
    let last_line = stdout.lines().last().expect("a last line");
    assert!(format_regex(kept).is_match(last_line), "{last_line}");
    assert_ne!(last_line, safe);

    let (form, _) = next_span(&text, "It has the form ");
    assert_eq!(
        form,
        "(cd '<worktree>' && just --justfile '<main checkout>/justfile' --working-directory . checkpoint)"
    );
    let it = stdout
        .lines()
        .find_map(|l| l.trim_start().strip_prefix("checkpoint it: "))
        .unwrap_or_else(|| panic!("no `checkpoint it:` line in:\n{stdout}"));
    let shape = regex::escape(form)
        .replace("<worktree>", "[^']+")
        .replace("<main checkout>", "[^']+");
    assert!(
        Regex::new(&format!("^{shape}$")).unwrap().is_match(it),
        "`{it}` is not `{}`",
        form
    );
}

fn shim_path() -> PathBuf {
    repo_root().join(".claude/hooks/aios")
}

#[test]
fn pause_skill_tells_the_two_exit_3s_apart() {
    let text = pause_skill();
    // Both cues are in the skill, each with the stream it appears on.
    assert!(
        text.contains("Empty stdout and an `aios: ` line on stderr"),
        "the pause skill lacks the shim-refusal cue"
    );
    assert!(
        text.contains("A `Stopped before committing` block on stdout"),
        "the pause skill lacks the secret-stop cue"
    );

    // The secret stop: exit 3, the stdout cue, and no `aios: ` line on stderr.
    let stop = run_checkpoint(
        &on_x("sensitive_name_new_doc").file("docs/deploy-secret.md", "how to deploy\n"),
        &[],
    );
    assert_eq!(stop.code, 3, "{}", stop.stderr_str());
    assert!(
        stop.stdout_str()
            .lines()
            .any(|l| l.starts_with("Stopped before committing")),
        "{}",
        stop.stdout_str()
    );
    assert!(
        !stop.stderr_str().lines().any(|l| l.starts_with("aios: ")),
        "{}",
        stop.stderr_str()
    );

    // The shim's refusal: the skill quotes its text, and the shim prints it with an
    // `aios: ` prefix on stderr.
    let refusal = "aios checkpoint never commits or pushes with a stale binary";
    assert!(
        text.contains(&format!("`{refusal}`")),
        "the pause skill does not quote the refusal"
    );
    let shim = std::fs::read_to_string(shim_path()).expect("read the shim");
    assert!(
        shim.lines().any(|l| l.contains(&format!(
            "echo \"aios: rebuilding $bin failed; {refusal} (run: just tools)\" >&2"
        ))),
        "the shim does not print the refusal on stderr"
    );

    // The shim test that runs it pins exit 3 and an empty stdout.
    let shim_tests =
        std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/shim.rs"))
            .expect("read shim.rs");
    let name = "fn checkpoint_on_a_stale_binary_whose_rebuild_fails_exits_3()";
    let at = shim_tests
        .find(name)
        .expect("the stale-checkpoint shim test exists");
    let body = &shim_tests[at..];
    let body = &body[..body.find("\n#[test]").unwrap_or(body.len())];
    assert!(body.contains("assert_eq!(code(&out), 3)"), "{body}");
    assert!(body.contains("stdout(&out).is_empty()"), "{body}");
    assert!(body.contains(refusal), "{body}");
}
