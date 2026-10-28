//! Self-tests of the shared agent-script test support (`tests/common/{jail,fake_gh,
//! agent}.rs`, contract §5): the PATH jail and its tool lists, the fake `gh`, the
//! oracles read from git history and the twin repositories. The parity tests of
//! `aios brief` and `aios checkpoint` build on these, so each test here pins one
//! property they rely on.

mod common;

use common::agent::{self, Scenario, Sub, Twin, Which};
use common::fake_gh::{self, FakeGh};
use common::jail;
use common::unique_dir;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// The brief oracle's functions at `BRIEF_ORACLE`: mtime L68, fmt_epoch L70, section L74,
/// rel_path L76, unpushed_desc L84, gh_unavailable L161, soak_commit L323,
/// soak_relation L328, print_soak L358, print_interleaved L387.
const BRIEF_FUNCTIONS: &[&str] = &[
    "mtime",
    "fmt_epoch",
    "section",
    "rel_path",
    "unpushed_desc",
    "gh_unavailable",
    "soak_commit",
    "soak_relation",
    "print_soak",
    "print_interleaved",
];

/// The checkpoint oracle's functions at `CHECKPOINT_ORACLE`: in_progress L91, added_lines
/// L107, secret_scan L116, split_allowed L127, describe_paths L133, other_worktrees
/// L137, count_dirty L154, count_unpushed L155.
const CHECKPOINT_FUNCTIONS: &[&str] = &[
    "in_progress",
    "added_lines",
    "secret_scan",
    "split_allowed",
    "describe_paths",
    "other_worktrees",
    "count_dirty",
    "count_unpushed",
];

/// qemu-lock.sh's functions at `BRIEF_ORACLE`: qemu_pids L86, owner_lines L91,
/// holder_state L102, load1 L118, above L129, is_stale L133, optval L203, release
/// L242, tree_of L247, in_list L253, running L259, wait_gone L266, stop_tree L285,
/// on_signal L315, write_owner L332.
const QEMU_LOCK_FUNCTIONS: &[&str] = &[
    "qemu_pids",
    "owner_lines",
    "holder_state",
    "load1",
    "above",
    "is_stale",
    "optval",
    "release",
    "tree_of",
    "in_list",
    "running",
    "wait_gone",
    "stop_tree",
    "on_signal",
    "write_owner",
];

/// The fake gh's functions (`fake_gh::SCRIPT`): say, pick.
const FAKE_GH_FUNCTIONS: &[&str] = &["say", "pick"];

/// Words the extractor reports that are not host tools: `gh` is a fake (brief L154,
/// L166, L177, L197, L269, L283, L551-552; checkpoint L219), `.claude/hooks/aios` is
/// the scenario's committed stub (brief L103), and `sh` is always linked (brief L474).
const A2_REMOVED: &[&str] = &["gh", ".claude/hooks/aios", "sh"];

fn set(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

fn commands_of(src: &str, functions: &[&str]) -> BTreeSet<String> {
    jail::script_commands(src, &[jail::SH_WORDS, functions, A2_REMOVED].concat())
}

/// A fake root with a jail, and a place to run the fake gh.
struct Rig {
    dir: PathBuf,
    fake: FakeGh,
    jail_bin: PathBuf,
}

fn rig(label: &str) -> Rig {
    let dir = unique_dir(label)
        .canonicalize()
        .expect("canonicalise the rig directory");
    let fake = FakeGh::install(&dir.join("fake"));
    let jail_bin = jail::bin(&dir.join("jail"), jail::ORACLE_TOOLS);
    Rig {
        dir,
        fake,
        jail_bin,
    }
}

impl Rig {
    fn gh(&self, cwd: &Path, args: &[&str], tweak: impl FnOnce(&mut Command)) -> Output {
        let mut cmd = Command::new(self.fake.bin().join("gh"));
        tweak(&mut cmd);
        jail::hermetic(&mut cmd, &self.fake.bin(), &self.jail_bin);
        cmd.args(args).current_dir(cwd);
        cmd.output().expect("run the fake gh")
    }
}

/// Run `script` under `hermetic` with no fakes: an empty fake bin.
fn shell_in_jail(rig: &Rig, script: &str) -> Output {
    let no_fakes = rig.dir.join("no-fakes");
    std::fs::create_dir_all(&no_fakes).expect("create an empty fake bin");
    let mut cmd = Command::new(rig.jail_bin.join("sh"));
    jail::hermetic(&mut cmd, &no_fakes, &rig.jail_bin);
    cmd.args(["-c", script]);
    cmd.output().expect("run sh in the jail")
}

#[test]
fn jail_has_no_gh_or_claude() {
    let rig = rig("jail-none");
    for tool in ["gh", "claude"] {
        let out = shell_in_jail(&rig, &format!("command -v {tool}"));
        assert!(!out.status.success(), "{tool} is on the jail PATH");
    }
    assert!(shell_in_jail(&rig, "command -v git").status.success());
    // The jail refuses to link gh or claude at all.
    for tool in ["gh", "claude"] {
        let refused = std::panic::catch_unwind(|| jail::bin(&rig.dir.join("refused"), &[tool]));
        assert!(refused.is_err(), "jail::bin linked {tool}");
    }
    // hermetic strips a GH_*, GITHUB_*, AIOS_* and CLAUDE* variable, wherever it was set,
    // and leaves the PATH to the jail.
    let mut cmd = Command::new(rig.jail_bin.join("sh"));
    cmd.env("GH_TOKEN", "t")
        .env("AIOS_X", "1")
        .env("CLAUDE_X", "1");
    jail::hermetic(&mut cmd, &rig.fake.bin(), &rig.jail_bin);
    cmd.env("GITHUB_X", "kept-after");
    let out = cmd
        .args([
            "-c",
            "echo \"${GH_TOKEN-unset} ${AIOS_X-unset} ${CLAUDE_X-unset} $PATH $TZ $LC_ALL\"",
        ])
        .output()
        .expect("run sh");
    let printed = String::from_utf8_lossy(&out.stdout).into_owned();
    assert_eq!(
        printed.trim_end(),
        format!(
            "unset unset unset {}:{} UTC C",
            rig.fake.bin().display(),
            rig.jail_bin.display()
        )
    );
    // assert_no_command_not_found: a missing command fails, a missing file does not.
    for clean in [
        "",
        "cat: nofile: No such file or directory",
        "git: '/x/y': No such file or directory",
        "fatal: not a git repository",
    ] {
        jail::assert_no_command_not_found(clean.as_bytes());
    }
    for missing in [
        "sh: 1: foo: not found",
        "bash: line 3: foo: command not found",
        "env: foo: No such file or directory",
        "xargs: 'foo': No such file or directory",
    ] {
        let result = std::panic::catch_unwind(|| {
            jail::assert_no_command_not_found(missing.as_bytes());
        });
        assert!(result.is_err(), "{missing:?} was not flagged");
    }
}

#[test]
fn jail_covers_every_script_command() {
    // Each oracle's commands, less A2's removed words, are exactly its tool list.
    let cases: [(Which, &[&str], &[&str]); 3] = [
        (Which::Brief, BRIEF_FUNCTIONS, jail::BRIEF_TOOLS),
        (
            Which::Checkpoint,
            CHECKPOINT_FUNCTIONS,
            jail::CHECKPOINT_TOOLS,
        ),
        (Which::QemuLock, QEMU_LOCK_FUNCTIONS, jail::QEMU_LOCK_TOOLS),
    ];
    for (which, functions, tools) in cases {
        assert_eq!(
            commands_of(&agent::oracle_text(which), functions),
            set(tools),
            "{which:?}: the script's commands differ from its tool list"
        );
    }
    // The fakes.
    assert_eq!(
        commands_of(fake_gh::SCRIPT, FAKE_GH_FUNCTIONS),
        set(jail::FAKE_GH_TOOLS)
    );
    let docs = commands_of(agent::FAKE_DOCS, &[]);
    assert!(
        docs.is_subset(&set(jail::FAKE_GH_TOOLS)),
        "aios-fake-docs runs {docs:?}"
    );
    // The unions.
    let union: BTreeSet<String> = [
        jail::BRIEF_TOOLS,
        jail::CHECKPOINT_TOOLS,
        jail::QEMU_LOCK_TOOLS,
    ]
    .concat()
    .iter()
    .map(|s| s.to_string())
    .collect();
    assert_eq!(union, set(jail::ORACLE_TOOLS));
    for banned in [
        "flock", "lockf", "uniq", "xargs", "readlink", "gh", "claude",
    ] {
        assert!(
            !jail::ORACLE_TOOLS.contains(&banned),
            "{banned} in ORACLE_TOOLS"
        );
    }
    let fakes: BTreeSet<String> = [jail::FAKE_GH_TOOLS, &jail::FAKE_CLAUDE_TOOLS[..]]
        .concat()
        .iter()
        .map(|s| s.to_string())
        .collect();
    assert_eq!(fakes, set(jail::FAKE_TOOLS));
    assert_eq!(jail::FAKE_TOOLS.len(), 8);
    // Every tool resolves under hermetic.
    let rig = rig("jail-cover");
    let tools = [jail::ORACLE_TOOLS, jail::FAKE_TOOLS].concat().join(" ");
    let out = shell_in_jail(
        &rig,
        &format!("for t in {tools}; do command -v \"$t\" >/dev/null || echo \"missing $t\"; done"),
    );
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "");
}

#[test]
fn script_commands_scans_substitutions_and_compound_starts() {
    // (source, expected commands), with the shell keywords and builtins removed. Each
    // row cites the oracle line it is taken from.
    let rows: &[(&str, &str, &[&str])] = &[
        (
            "checkpoint L196: paste and sed inside a double-quoted $(...)",
            r#"override="Secret scan overridden with --allow (confirmed safe by the user): $(printf '%s\n' "$allowed" | cut -f1 | sort -u | paste -sd ',' - | sed 's/,/, /g')""#,
            &["cut", "sort", "paste", "sed"],
        ),
        (
            "checkpoint L205: head inside a double-quoted $(...)",
            r#"                wip="not committed (git commit failed: $(head -n 1 "$TMP/commit.err"))""#,
            &["head"],
        ),
        (
            "checkpoint L74: dirname and basename, one nested",
            r#"SELF="$(cd "$(dirname "$0")" && pwd -P)/$(basename "$0")" # before cd: $0 may be relative"#,
            &["dirname", "basename"],
        ),
        (
            "checkpoint L190: git after elif",
            "        elif git diff --cached --quiet; then",
            &["git"],
        ),
        (
            "checkpoint L198: git after if, with a continuation",
            "            if git commit -q -m \"wip: checkpoint $branch\" \\\n",
            &["git"],
        ),
        (
            "checkpoint L219: gh after elif, ! and an assignment prefix",
            "                elif ! prs=$(GH_PROMPT_DISABLED=1 gh pr list --head \"$branch\" --state open --json number,isDraft \\\n",
            &["gh"],
        ),
        (
            "checkpoint L146: git, wc and tr around a ${...[@]} expansion",
            r#"        d=$(git --no-optional-locks -C "$wt" status --porcelain --untracked-files=all -- . "${EXCLUDES[@]}" 2>/dev/null | wc -l | tr -d ' ')"#,
            &["git", "wc", "tr"],
        ),
        (
            "checkpoint L89: an array assignment runs nothing",
            r#"EXCLUDES=(':(exclude).remember' ':(exclude,glob)**/__pycache__/**')"#,
            &[],
        ),
        (
            "checkpoint L247-249: a pipeline into while ... done inside $(...)",
            "unused=$(printf '%s' \"$ALLOW\" | while IFS= read -r a; do\n    [ -n \"$a\" ] && ! printf '%s\\n' \"$allowed\" | cut -f1 | grep -Fqx -e \"$a\" && echo \"$a\"\ndone)\n",
            &["cut", "grep"],
        ),
        (
            "brief L111: date inside a double-quoted $(...)",
            r#"printf '# AIOS brief - %s\n' "$(date '+%Y-%m-%d %H:%M %Z')""#,
            &["date"],
        ),
        (
            "brief L123: escaped backticks yield nothing, $(...) does",
            r#"echo "- checkout: $(rel_path "$ROOT") on \`$branch\` @ $(git log -1 --format='%h %s' 2>/dev/null)""#,
            &["rel_path", "git"],
        ),
        (
            "brief L521: date inside $((...))",
            "    since=$(($(date +%s) - 7 * 86400))",
            &["date"],
        ),
        (
            "brief L64: a trap's single-quoted command runs",
            r#"trap 'rm -rf "$TMP"' EXIT"#,
            &["rm"],
        ),
        (
            "brief L142-144: a here-document body that holds no substitution",
            "done <<EOF\n$WORKTREES\nEOF\n",
            &[],
        ),
        (
            "qemu-lock L148: commands in a case arm",
            r#"case "$eta" in '' | *[!0-9]*) ;; *) [ "$(date +%s)" -gt "$eta" ] && echo "overdue=1" ;; esac"#,
            &["date"],
        ),
        (
            "qemu-lock L220: patterns are not commands",
            r#"case "$team" in team-build | team-fix | solo) ;; *) echo "qemu-lock: --team must be team-build, team-fix or solo" >&2; exit 2 ;; esac"#,
            &[],
        ),
        (
            "qemu-lock L384: \"$@\" is not a command",
            r#""$@" </dev/null &"#,
            &[],
        ),
        (
            "synthetic: an unquoted here-document delimiter scans substitutions",
            "cat <<EOF\n$(date)\nEOF\n",
            &["cat", "date"],
        ),
        (
            "synthetic: a quoted delimiter does not",
            "cat <<'EOF'\n$(date)\nEOF\n",
            &["cat"],
        ),
        (
            "negative, checkpoint L108-113: awk's program, in single quotes, runs only awk",
            "    awk '\n        /^diff --git / { hdr = 1; next }\n        hdr && /^\\+\\+\\+ / { f = substr($0, 5); sub(/^b\\//, \"\", f); next }\n        /^@@/ { hdr = 0; next }\n        !hdr && /^\\+/ { print f \"\\t\" substr($0, 2) }\n    '\n",
            &["awk"],
        ),
        (
            "negative, brief L543-547: jq's program, in single quotes, runs only jq",
            "        jq -r '\n          def clean: tostring | gsub(\"[[:cntrl:]]\"; \"?\");\n          if length == 0 then \"- none\" else\n            sort_by(.number) | .[] | \"- #\\(.number) \\(.title | clean)\" end\n        ' \"$TMP/issues.json\"\n",
            &["jq"],
        ),
    ];
    for (what, src, want) in rows {
        assert_eq!(
            jail::script_commands(src, jail::SH_WORDS),
            set(want),
            "{what}"
        );
    }
}

#[test]
fn fake_gh_unknown_call_exits_99() {
    let rig = rig("fakegh-99");
    rig.fake.fixture("pr-list", "^pr list ").out("[]\n");
    let out = rig.gh(&rig.dir, &["pr", "merge", "1"], |_| {});
    assert_eq!(out.status.code(), Some(99));
    assert!(out.stdout.is_empty());
    assert_eq!(
        String::from_utf8_lossy(&out.stderr),
        "fake gh: no fixture for: pr merge 1 \n"
    );
    let calls = rig.fake.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].fixture, "");
    assert_eq!(calls[0].line(), "pr merge 1 ");
    assert!(String::from_utf8_lossy(&rig.fake.fake_stderr()).contains("no fixture for: pr merge 1"));
    let unmatched = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.fake.assert_all_matched();
    }));
    assert!(
        unmatched.is_err(),
        "assert_all_matched passed an unknown call"
    );
    // A fixture serves a matching call, and the tripwire notices the stderr line.
    let ok = rig.gh(&rig.dir, &["pr", "list"], |_| {});
    assert_eq!(ok.status.code(), Some(0));
    assert_eq!(ok.stdout, b"[]\n");
    let tripped = std::panic::catch_unwind(|| {
        jail::assert_no_command_not_found(b"sh: 1: gh: not found\n");
    });
    assert!(tripped.is_err());
}

#[test]
fn fake_gh_ordinals_advance() {
    let rig = rig("fakegh-nth");
    // A specific fixture first, a general one after it.
    rig.fake
        .fixture("pr-list-head", "^pr list --head ")
        .out("head\n");
    rig.fake
        .fixture("pr-list", "^pr list ")
        .out("default\n")
        .nth(1, "default\n", "", 0)
        .nth(2, "second\n", "", 0)
        .nth(3, "", "boom\n", 4);
    let run = |args: &[&str]| rig.gh(&rig.dir, args, |_| {});
    let first = run(&["pr", "list"]);
    assert_eq!(
        (first.status.code(), first.stdout.as_slice()),
        (Some(0), &b"default\n"[..])
    );
    let second = run(&["pr", "list"]);
    assert_eq!(second.stdout, b"second\n");
    let third = run(&["pr", "list"]);
    assert_eq!(third.status.code(), Some(4));
    assert_eq!(third.stderr, b"boom\n");
    assert!(third.stdout.is_empty());
    // Past the last ordinal, the last one sticks.
    let fourth = run(&["pr", "list"]);
    assert_eq!(fourth.status.code(), Some(4));
    assert_eq!(fourth.stderr, b"boom\n");
    // The earlier fixture wins for a call both match, and has its own counter.
    let head = run(&["pr", "list", "--head", "b"]);
    assert_eq!(head.stdout, b"head\n");
    let labels: Vec<String> = rig.fake.calls().into_iter().map(|c| c.fixture).collect();
    assert_eq!(
        labels,
        ["pr-list", "pr-list", "pr-list", "pr-list", "pr-list-head"]
    );
    rig.fake.assert_all_matched();
    rig.fake.assert_calls_match(&["^pr list "]);
    rig.fake.assert_none_match(&["merge", "--admin"]);
    let denied = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.fake.assert_none_match(&["--head "]);
    }));
    assert!(denied.is_err());
    let outside = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        rig.fake.assert_calls_match(&["^run list "]);
    }));
    assert!(outside.is_err());
}

#[test]
fn fake_gh_ordinals_follow_the_contract_rule() {
    let first_rig = rig("fakegh-rule");
    let run = || first_rig.gh(&first_rig.dir, &["pr", "list"], |_| {});
    // Only `.2` files: call 1 serves them too, not the unsuffixed file.
    first_rig
        .fake
        .fixture("only-two", "^pr list ")
        .out("unsuffixed\n")
        .nth(2, "two\n", "", 0);
    let first = run();
    assert_eq!(first.stdout, b"two\n", "call 1 serves out.2, not out");
    let second = run();
    assert_eq!(second.stdout, b"two\n");

    // `.1` and `.3`: call 2 has no file of its own and serves the highest, `.3`.
    let gap_rig = rig("fakegh-rule-gap");
    let run = || gap_rig.gh(&gap_rig.dir, &["pr", "list"], |_| {});
    gap_rig
        .fake
        .fixture("gap", "^pr list ")
        .nth(1, "one\n", "", 0)
        .nth(3, "three\n", "", 0);
    assert_eq!(run().stdout, b"one\n");
    assert_eq!(run().stdout, b"three\n", "call 2 serves the highest, out.3");
    assert_eq!(run().stdout, b"three\n");
    assert_eq!(run().stdout, b"three\n");

    // Each stream is picked on its own: `err.2` alone leaves out and rc on the
    // unsuffixed files, and `err.2` serves from call 1.
    let streams_rig = rig("fakegh-rule-streams");
    let run = || streams_rig.gh(&streams_rig.dir, &["pr", "list"], |_| {});
    streams_rig
        .fake
        .fixture("streams", "^pr list ")
        .out("out\n")
        .rc(3);
    std::fs::write(
        streams_rig.fake.root().join("gh/00-streams/err.2"),
        "late\n",
    )
    .expect("write err.2");
    let first = run();
    assert_eq!(
        (
            first.status.code(),
            first.stdout.as_slice(),
            first.stderr.as_slice()
        ),
        (Some(3), &b"out\n"[..], &b"late\n"[..])
    );
}

#[test]
fn fake_gh_logs_cwd_env_and_body() {
    let rig = rig("fakegh-log");
    rig.fake.fixture("any", ".");
    let sub = rig.dir.join("sub dir");
    std::fs::create_dir_all(&sub).expect("create the subdirectory");
    let body = rig.dir.join("body.txt");
    std::fs::write(&body, "hello\nbody").expect("write the body");
    let body_arg = body.to_str().expect("a UTF-8 path");
    let query_arg = format!("query=@{body_arg}");
    let out = rig.gh(
        &sub,
        &["issue", "comment", "7", "--body-file", body_arg, "a\nb"],
        |cmd| {
            // GH_REPO is set before hermetic, which strips it; this one is set after.
            cmd.env("GH_REPO", "stripped/by-hermetic");
        },
    );
    assert_eq!(out.status.code(), Some(0));
    let tweak_after = |args: &[&str]| {
        let mut cmd = Command::new(rig.fake.bin().join("gh"));
        jail::hermetic(&mut cmd, &rig.fake.bin(), &rig.jail_bin);
        cmd.env("GH_PROMPT_DISABLED", "1").env("NO_COLOR", "1");
        cmd.args(args).current_dir(&sub);
        cmd.output().expect("run the fake gh")
    };
    assert_eq!(
        tweak_after(&["api", "graphql", "-F", &query_arg])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(
        tweak_after(&["api", "x", &format!("--input={body_arg}")])
            .status
            .code(),
        Some(0)
    );
    assert_eq!(tweak_after(&["pr", "view", "1"]).status.code(), Some(0));
    let calls = rig.fake.calls();
    assert_eq!(calls.len(), 4);
    let first = &calls[0];
    assert_eq!(
        first.argv,
        [
            &b"issue"[..],
            b"comment",
            b"7",
            b"--body-file",
            body_arg.as_bytes(),
            b"a\nb"
        ]
    );
    assert_eq!(
        first.line(),
        format!("issue comment 7 --body-file {body_arg} a b ")
    );
    assert_eq!(first.cwd, sub.canonicalize().expect("canonicalise"));
    assert_eq!(first.fixture, "any");
    assert_eq!(first.body.as_deref(), Some(&b"hello\nbody"[..]));
    assert_eq!(first.env["GH_REPO"], None, "hermetic must strip GH_REPO");
    assert_eq!(first.env["GH_PROMPT_DISABLED"], None);
    assert_eq!(first.env["LC_ALL"].as_deref(), Some("C"));
    assert_eq!(
        calls[1].body.as_deref(),
        Some(&b"hello\nbody"[..]),
        "-F k=@F"
    );
    assert_eq!(
        calls[2].body.as_deref(),
        Some(&b"hello\nbody"[..]),
        "--input=F"
    );
    assert_eq!(calls[2].env["GH_PROMPT_DISABLED"].as_deref(), Some("1"));
    assert_eq!(calls[2].env["NO_COLOR"].as_deref(), Some("1"));
    assert_eq!(calls[3].body, None);
}

#[test]
fn fakes_are_posix_sh() {
    let dir = unique_dir("posix-sh");
    let files = [
        ("fake-gh", fake_gh::SCRIPT),
        ("fake-docs", agent::FAKE_DOCS),
        ("twin-stub", agent::TWIN_STUB),
    ];
    let mut shells = vec![jail::host_which("sh").expect("sh on the host PATH")];
    shells.extend(jail::host_which("dash"));
    for (name, text) in files {
        let path = dir.join(name);
        std::fs::write(&path, text).expect("write a fake");
        for shell in &shells {
            let out = Command::new(shell)
                .arg("-n")
                .arg(&path)
                .output()
                .expect("run sh -n");
            assert!(
                out.status.success(),
                "{name} is not valid for {}: {}",
                shell.display(),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        assert!(text.starts_with("#!/bin/sh\n"), "{name}: shebang");
    }
    // Where dash is installed (Debian, Ubuntu), the fake gh also runs under it.
    if let Some(dash) = jail::host_which("dash") {
        let rig = rig("posix-dash");
        rig.fake.fixture("any", "^pr list ").out("ok\n");
        let run = |args: &[&str]| {
            let mut cmd = Command::new(&dash);
            jail::hermetic(&mut cmd, &rig.fake.bin(), &rig.jail_bin);
            cmd.arg(rig.fake.bin().join("gh"))
                .args(args)
                .current_dir(&rig.dir);
            cmd.output().expect("run the fake gh under dash")
        };
        let served = run(&["pr", "list", "--body-file", "x"]);
        assert_eq!(
            (served.status.code(), &served.stdout[..]),
            (Some(0), &b"ok\n"[..])
        );
        let unknown = run(&["pr", "merge", "1"]);
        assert_eq!(unknown.status.code(), Some(99));
        let calls = rig.fake.calls();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].argv.len(), 4, "printf '%s\\0' under dash");
    }
}

#[test]
fn fake_docs_logs_argv_and_cwd() {
    let rig = rig("fake-docs");
    let fake = rig.dir.join("fake");
    std::fs::write(fake.join("bin/aios-fake-docs"), agent::FAKE_DOCS).expect("write fake docs");
    let mut perms = std::fs::metadata(fake.join("bin/aios-fake-docs"))
        .expect("stat")
        .permissions();
    std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
    std::fs::set_permissions(fake.join("bin/aios-fake-docs"), perms).expect("chmod");
    std::fs::create_dir_all(fake.join("docs")).expect("create docs");
    std::fs::write(fake.join("docs/out"), "{\"x\":1}\n").expect("write out");
    std::fs::write(fake.join("docs/err"), "docs-check: boom\n").expect("write err");
    std::fs::write(fake.join("docs/rc"), "2\n").expect("write rc");
    let sub = rig.dir.join("deep/er");
    std::fs::create_dir_all(&sub).expect("create the run directory");
    let run = || {
        let mut cmd = Command::new(fake.join("bin/aios-fake-docs"));
        jail::hermetic(&mut cmd, &fake.join("bin"), &rig.jail_bin);
        cmd.args(["docs-check", "--json"]).current_dir(&sub);
        cmd.output().expect("run the fake docs-check")
    };
    let out = run();
    assert_eq!(out.status.code(), Some(2));
    assert_eq!(out.stdout, b"{\"x\":1}\n");
    assert_eq!(out.stderr, b"docs-check: boom\n");
    std::fs::write(fake.join("docs/stale"), "").expect("write stale");
    let stale = run();
    assert!(String::from_utf8_lossy(&stale.stderr).starts_with(
        "aios: rebuilding target/tools/installed/aios failed; running the stale binary"
    ));
    let calls = agent::docs_calls(&fake);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].argv, [&b"docs-check"[..], &b"--json"[..]]);
    assert_eq!(calls[0].cwd, sub.canonicalize().expect("canonicalise"));
}

#[test]
fn oracles_load_from_history() {
    agent::require_tools();
    for which in [Which::Brief, Which::Checkpoint, Which::QemuLock] {
        let text = agent::oracle_text(which);
        assert!(!text.is_empty(), "{which:?} is empty");
        let first = text.lines().next().expect("a first line");
        let want = match which {
            Which::QemuLock => "#!/bin/sh",
            _ => "#!/usr/bin/env bash",
        };
        assert_eq!(first, want, "{which:?}");
    }
    // The brief oracle holds #234's and #246's sections.
    let brief = agent::oracle_text(Which::Brief);
    for needle in ["handoff-", "Routing log", "print_interleaved", "PANIC-LOCK"] {
        assert!(brief.contains(needle), "the brief oracle lacks {needle}");
    }
    assert!(agent::oracle_text(Which::Checkpoint).contains("secret_scan"));
    assert!(agent::oracle_text(Which::QemuLock).contains("stop_tree"));
}

#[test]
fn twin_repos_have_identical_heads() {
    let scenario = Scenario::brief("twin-heads")
        .file("a.txt", "1\n")
        .commit("one")
        .branch("claude/t")
        .file("b.txt", "2\n")
        .commit("two")
        .worktree(".claude/worktrees/w1", "claude/w1")
        .file(".claude/worktrees/w1/c.txt", "3\n")
        .commit_in(".claude/worktrees/w1", "three")
        .file("dirty.txt", "d\n")
        .mtime("dirty.txt", 1_700_000_000);
    let a = scenario.prepare(Twin::Oracle);
    let b = scenario.prepare(Twin::Aios);
    assert_ne!(a.root(), b.root());
    assert_eq!(a.git(&["rev-parse", "HEAD"]), b.git(&["rev-parse", "HEAD"]));
    assert_eq!(a.git(&["status", "--porcelain"]), "?? dirty.txt");
    assert_eq!(
        a.git(&["status", "--porcelain"]),
        b.git(&["status", "--porcelain"])
    );
    assert_eq!(a.git(&["write-tree"]), b.git(&["write-tree"]));
    assert_eq!(
        a.git(&["rev-parse", "claude/w1"]),
        b.git(&["rev-parse", "claude/w1"])
    );
    assert_eq!(a.git(&["worktree", "list"]).lines().count(), 2);
    assert_eq!(
        a.git_in(a.origin(), &["rev-parse", "main"]),
        b.git_in(b.origin(), &["rev-parse", "main"])
    );
    // The committed stub and the oracle's qemu-lock.sh are in the base commit.
    assert_eq!(
        a.git(&[
            "ls-files",
            ".claude/hooks/aios",
            "scripts/agent/qemu-lock.sh"
        ]),
        ".claude/hooks/aios\nscripts/agent/qemu-lock.sh"
    );
}

#[test]
fn checkpoint_wip_identity_is_pinned() {
    let scenario = Scenario::checkpoint("wip-identity")
        .branch("claude/x")
        .file("notes.txt", "work in progress\n")
        .gh_fixture("pr-list-head", "")
        .expects_gh(true);
    let want_identity =
        "aios-test <aios-test@example.invalid>|aios-test <aios-test@example.invalid>";
    let mut heads = Vec::new();
    for twin in [Twin::Oracle, Twin::Aios] {
        let run = agent::run_oracle_in(&scenario, twin, &["--handoff", "not-saved"]);
        let stdout = run.stdout_str();
        let first = stdout.lines().next().expect("a first line").to_string();
        assert!(first.starts_with("checkpoint: claude/x @ "), "{first}");
        assert!(first.contains("wip: committed 1 file(s)"), "{first}");
        assert!(first.contains("pushed: yes"), "{first}");
        assert_eq!(
            run.twin()
                .git(&["log", "-1", "--format=%an <%ae>|%cn <%ce>"]),
            want_identity
        );
        heads.push(run.twin().git(&["rev-parse", "HEAD"]));
        agent::assert_not_vacuous(&run);
        agent::assert_origin_invariants(&run);
        assert_ne!(
            run.repo_state.head_before, run.repo_state.head_after,
            "the wip commit moved HEAD"
        );
        assert_eq!(run.gh_calls.len(), 1);
        assert_eq!(run.gh_calls[0].fixture, "pr-list-head");
    }
    assert_eq!(heads[0], heads[1], "the wip commit differs between twins");
}

#[test]
fn gh_allowlists_reject_writes() {
    let writes = [
        "pr merge 1 ",
        "pr merge 1 --auto ",
        "api graphql -f query=mutation { mergePullRequest(input: {}) { x } } ",
        "issue create ",
        "label create x ",
    ];
    for sub in [Sub::Brief, Sub::Checkpoint] {
        for line in writes {
            assert!(
                !agent::gh_line_allowed(sub, line),
                "{sub:?} allows {line:?}"
            );
        }
    }
    // What the oracles actually run is allowed, and refused by nothing in gh::is_read.
    let brief_reads: &[&[&str]] = &[
        &["api", "rate_limit", "--jq", ".resources.core.remaining"],
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--label",
            "needs-human",
            "--limit",
            "1000",
        ],
        &[
            "issue",
            "list",
            "--state",
            "open",
            "--label",
            "agent-ready",
            "--limit",
            "1000",
        ],
        &[
            "pr", "list", "--state", "open", "--limit", "30", "--json", "number",
        ],
        &[
            "run", "list", "--branch", "main", "--event", "push", "--limit", "40",
        ],
        &[
            "api",
            "graphql",
            "-F",
            "owner={owner}",
            "-f",
            "query=query { reviewThreads }",
        ],
        &[
            "api",
            "graphql",
            "-F",
            "owner={owner}",
            "-f",
            "query=query { checkSuites }",
        ],
    ];
    for argv in brief_reads {
        assert!(
            agent::gh_line_allowed(Sub::Brief, &fake_gh::render(argv)),
            "{argv:?}"
        );
        assert_eq!(aios_tools::gh::is_read(argv), Ok(()), "{argv:?}");
    }
    let head = [
        "pr",
        "list",
        "--head",
        "claude/x",
        "--state",
        "open",
        "--json",
        "number,isDraft",
    ];
    assert!(agent::gh_line_allowed(
        Sub::Checkpoint,
        &fake_gh::render(&head)
    ));
    assert_eq!(aios_tools::gh::is_read(&head), Ok(()));
    for argv in [
        &["pr", "merge", "1"][..],
        &["api", "graphql", "-f", "query=mutation { x }"],
    ] {
        assert!(aios_tools::gh::is_read(argv).is_err());
    }
}

const ALL_SECTIONS: [&str; 11] = [
    "Git",
    "Open pull requests",
    "Main CI",
    "Soak",
    "Host QEMU lock",
    "Routing log",
    "Handoffs",
    "Knowledge notes changed since last session",
    "Needs human",
    "Next phase-doc step",
    "Docs drift",
];

#[test]
fn oracle_smoke_brief_and_checkpoint() {
    let docs =
        br#"{"summary":{"new":0,"total":0,"baselined":0,"accepted":0,"resolved":0},"checks":{}}"#;
    let brief = Scenario::brief("smoke-brief")
        .gh_fixture("rate-limit", "5000\n")
        .gh_fixture("issues-needs-human", "[]\n")
        .gh_fixture("issues-agent-ready", "0\n")
        .gh_fixture("issues-agent-working", "0\n")
        .gh_fixture("graphql-threads", "{}\n")
        .gh_fixture("graphql-suites", "{}\n")
        .gh_fixture("pr-list", "[]\n")
        .gh_fixture("run-list", "[]\n")
        .docs(docs, "", 0);
    let run = agent::run_oracle(&brief, &["--no-fetch"]);
    assert_eq!(
        run.code,
        0,
        "stderr: {}\nstdout: {}",
        run.stderr_str(),
        run.stdout_str()
    );
    let stdout = run.stdout_str();
    for section in ALL_SECTIONS {
        assert!(
            stdout.contains(&format!("\n## {section}\n")),
            "no ## {section} in:\n{stdout}"
        );
    }
    assert_eq!(agent::section_count(&run.stdout), ALL_SECTIONS.len());
    agent::assert_not_vacuous(&run);
    run.twin()
        .fake_gh()
        .expect("a fake gh")
        .assert_calls_match(agent::BRIEF_GH_ALLOW);
    assert_eq!(
        run.gh_calls.len(),
        8,
        "{:?}",
        run.gh_calls.iter().map(|c| c.line()).collect::<Vec<_>>()
    );
    assert_eq!(run.docs_calls.len(), 1);
    assert_eq!(run.docs_calls[0].argv, [&b"docs-check"[..], &b"--json"[..]]);
    assert_eq!(
        run.docs_calls[0].cwd,
        run.twin().root().canonicalize().expect("canonicalise")
    );
    assert!(
        stdout.contains("- docs-check: 0 new vs baseline"),
        "{stdout}"
    );
    // normalize() leaves no twin path or date behind.
    let normalized = String::from_utf8(agent::normalize(&run.stdout, &run)).expect("UTF-8");
    assert!(
        normalized.starts_with("# AIOS brief - <DATE>\n"),
        "{normalized}"
    );
    assert!(!normalized.contains(run.twin().root().to_str().expect("UTF-8")));

    let checkpoint = Scenario::checkpoint("smoke-checkpoint");
    let run = agent::run_oracle(&checkpoint, &[]);
    assert_eq!(run.code, 0, "{}", run.stdout_str());
    assert!(
        run.stdout_str().starts_with("checkpoint: main @ "),
        "{}",
        run.stdout_str()
    );
    assert!(run.stdout_str().contains("Safe to /clear"));
    agent::assert_not_vacuous(&run);
    agent::assert_origin_invariants(&run);
    assert!(run.gh_calls.is_empty());
}

#[test]
fn outside_repo_scenarios_escape_the_checkout() {
    let checkout = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("canonicalise the checkout");
    for (sub, name, usage) in [
        (
            Sub::Brief,
            "outside-brief",
            "brief: not inside a git repository\n",
        ),
        (
            Sub::Checkpoint,
            "outside-checkpoint",
            "checkpoint: not inside a git repository\n",
        ),
    ] {
        let scenario = Scenario::outside_repo(sub, name);
        let dir = scenario.prepare(Twin::Oracle);
        assert!(
            !dir.run_dir().starts_with(&checkout),
            "{} is inside {}",
            dir.run_dir().display(),
            checkout.display()
        );
        // prepare() asserted that git finds no repository here under the runner's own
        // environment; the oracle agrees, and exits 2.
        drop(dir);
        let run = agent::run_oracle(&scenario, &[]);
        assert_eq!(run.code, 2, "{}", run.stderr_str());
        assert_eq!(
            String::from_utf8(agent::oracle_renames(&run.stderr, &run)).expect("UTF-8"),
            usage
        );
        agent::assert_not_vacuous(&run);
    }
}
