//! The docs-check fixture repository for R1 parity: the fixture bundle format and its
//! drift variants.
//!
//! The fixture lives in text bundles under `tests/fixtures/docs-check/`: `base.txt`
//! and one `variants/<name>.txt` per variant. A bundle line starting with `@@@ ` is a
//! directive `@@@ <kind> <arg>`; every other line is content of the preceding `file`,
//! `append` or `prepend` directive. Bundles are `.txt` so that the real repository's
//! docs-check, Cargo and Claude Code never see the fixture's Markdown, `CLAUDE.md`,
//! `.claude/` or `.rs` files. `materialize_fixture` turns base plus one variant into a
//! throwaway git repository (removed when the returned `TestRepo` drops).
//!
//! The parity half adds the real-repository snapshot at `SNAPSHOT_SHA`, the case list
//! replayed by `tests/docs_check_parity.rs`, the golden file paths, and the check.py
//! oracle: R1 deleted scripts/docs/check.py, so `check_py` materialises it from git
//! history (`git cat-file blob <SNAPSHOT_SHA>:scripts/docs/check.py`) into
//! `CARGO_TARGET_TMPDIR`, and `run_check_py` runs it against any repository.
//!
//! The oracle's `re` classes follow its interpreter's Unicode version, so `check_py`
//! accepts only a `python3` whose `unicodedata.unidata_version` is
//! `ORACLE_UNIDATA_VERSION` (16.0.0, CPython 3.14), the version `aios_tools::pyre`'s
//! classes match. CPython 3.12 and 3.13 (Unicode 15.x) lack 80 of the 760 code points
//! of `\d`, so a test using one of those digits would fail against them although aios
//! matches 3.14; `check_py` reports such an interpreter as unavailable instead.
//!
//! `python3` is often an asdf (or pyenv) shim: under the isolated test `HOME`
//! (`common::isolated`), such a shim exits 126 before it ever reaches CPython, which
//! would make the recorder fail outright and the differential silently skip without
//! comparing anything. So the interpreter's absolute path is resolved once from the
//! ambient environment (`python3 -c "import sys; print(sys.executable)"`, run outside
//! `isolated()`), cached, and only that resolved absolute path is ever run inside
//! `isolated()`.

use super::{git, isolated, unique_dir, Run, TestRepo};
use aios_tools::cmd::docs_check::model::CHECK_ORDER;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// One bundle directive with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `@@@ file <path>`: create or replace the file (parent directories created).
    File(String, String),
    /// `@@@ append <path>`: append to the file (created when missing; its parent
    /// directory must already exist, unlike `File`, which creates it).
    Append(String, String),
    /// `@@@ prepend <path>`: insert before the file's current content.
    Prepend(String, String),
    /// `@@@ delete <path>`: remove the file. Unused in R1; kept for the R4/R5 port fixtures.
    Delete(String),
    /// `@@@ commit <subject>` (base only): an empty commit after `Initial fixture`.
    Commit(String),
    /// `@@@ option <name>` (variants only); the only option is `no-phase-history`.
    Flag(String),
}

/// A fixture variant and the finding keys `aios docs-check --json` reports as new.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub name: &'static str,
    pub expect_new: &'static [&'static str],
}

/// Every variant, verified against scripts/docs/check.py: the base repository, one
/// single-drift variant per check (in CHECK_ORDER), then line-shift, skip and grown.
/// check.py read the project memory at the root `CLAUDE.md`; the bundles now place it
/// at `.claude/CLAUDE.md`, so these keys and the layout, harness-tables and repo-paths
/// goldens carry that path where check.py's carried `CLAUDE.md`.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "base",
        expect_new: &[],
    },
    Variant {
        name: "md-links",
        expect_new: &["md-links|docs/kernel/alpha.md|missing.md"],
    },
    Variant {
        name: "section-refs",
        expect_new: &["section-refs|docs/kernel/alpha.md|deadlock-prevention.md §9.9"],
    },
    Variant {
        name: "anchors",
        expect_new: &["anchors|docs/kernel/alpha.md|#no-such-heading"],
    },
    Variant {
        name: "wiki-links",
        expect_new: &["wiki-links|docs/kernel/alpha.md|nonexistent-note"],
    },
    Variant {
        name: "doc-map",
        expect_new: &["doc-map|docs/kernel/gamma.md|unlisted"],
    },
    Variant {
        name: "repo-paths",
        expect_new: &["repo-paths|.claude/CLAUDE.md|kernel/src/missing.rs"],
    },
    Variant {
        name: "just-recipes",
        expect_new: &["just-recipes|README.md|nope"],
    },
    Variant {
        name: "test-count",
        expect_new: &["test-count|docs/project/developer-guide.md|claimed:4"],
    },
    Variant {
        name: "lock-order",
        expect_new: &["lock-order|docs/kernel/deadlock-prevention.md|undocumented:GAMMA_LOCK"],
    },
    Variant {
        name: "milestone-status",
        expect_new: &["milestone-status|docs/phases/01-memory.md|M3:unchecked"],
    },
    Variant {
        name: "phase-count",
        expect_new: &["phase-count|README.md|claimed:4"],
    },
    Variant {
        name: "layout",
        expect_new: &["layout|.claude/CLAUDE.md|missing:kernel/src/extra.rs"],
    },
    Variant {
        name: "harness-tables",
        expect_new: &["harness-tables|.claude/CLAUDE.md|skills-table-stale:ghost"],
    },
    Variant {
        name: "pointer-doctor",
        expect_new: &["pointer-doctor|.claude/agents/worker.md|claude-md:Build Matrix"],
    },
    Variant {
        name: "knowledge-hygiene",
        expect_new: &["knowledge-hygiene|docs/knowledge/lessons/bad-name.md|name"],
    },
    Variant {
        name: "line-shift",
        expect_new: &[],
    },
    Variant {
        name: "skip",
        expect_new: &[],
    },
    Variant {
        name: "grown",
        expect_new: &["md-links|docs/kernel/alpha.md|old-spec.md"],
    },
];

/// `tools/tests/fixtures/docs-check`.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/docs-check")
}

/// Rejects a bundle path that could reach outside the fixture's working tree
/// (or otherwise confuse the line-based bundle format): empty, containing
/// `\r`, or with any path component that is not `Component::Normal` (no `..`,
/// no absolute root, no `.`, no Windows prefix).
fn validate_bundle_path(kind: &str, path: &str) {
    assert!(!path.is_empty(), "@@@ {kind} needs a path, found none");
    assert!(
        !path.contains('\r'),
        "@@@ {kind} {path:?}: a bundle path cannot contain '\\r'"
    );
    assert!(
        Path::new(path)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_))),
        "@@@ {kind} {path:?}: a bundle path must be a plain relative path \
         (no '..', absolute or root components)"
    );
}

/// Parse a bundle: strip one final `"\n"`, split on `"\n"`; content is the lines joined
/// with `"\n"` plus a final `"\n"`, or `""` when a directive has no content lines.
pub fn parse_bundle(text: &str) -> Vec<Op> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    assert!(!body.is_empty(), "bundle text is empty: no @@@ directives");
    let mut directives: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("@@@ ") {
            let (kind, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            directives.push((kind, arg, Vec::new()));
        } else if let Some((_, _, content)) = directives.last_mut() {
            content.push(line);
        } else {
            panic!("bundle line before the first @@@ directive: {line:?}");
        }
    }
    directives
        .into_iter()
        .map(|(kind, arg, lines)| {
            let content = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            let arg = arg.to_string();
            match kind {
                "file" => {
                    validate_bundle_path(kind, &arg);
                    Op::File(arg, content)
                }
                "append" => {
                    validate_bundle_path(kind, &arg);
                    Op::Append(arg, content)
                }
                "prepend" => {
                    validate_bundle_path(kind, &arg);
                    Op::Prepend(arg, content)
                }
                "delete" | "commit" | "option" => {
                    assert!(
                        lines.is_empty(),
                        "@@@ {kind} {arg} takes no content lines, found {}",
                        lines.len()
                    );
                    match kind {
                        "delete" => {
                            validate_bundle_path(kind, &arg);
                            Op::Delete(arg)
                        }
                        "commit" => Op::Commit(arg),
                        _ => Op::Flag(arg),
                    }
                }
                other => panic!("unknown bundle directive @@@ {other} {arg}"),
            }
        })
        .collect()
}

/// Parse `tests/fixtures/docs-check/<rel>`.
pub fn read_bundle(rel: &str) -> Vec<Op> {
    let path = fixtures_dir().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_bundle(&text)
}

/// Build the fixture repository for `variant` (`"base"` or a `variants/<name>.txt`):
/// write the base files, commit `Initial fixture`, add one empty commit per base
/// `@@@ commit` subject (unless the variant sets `no-phase-history`), apply the
/// variant's file operations and commit `Fixture variant: <variant>`.
pub fn materialize_fixture(variant: &str) -> TestRepo {
    let base = read_bundle("base.txt");
    let ops = if variant == "base" {
        Vec::new()
    } else {
        read_bundle(&format!("variants/{variant}.txt"))
    };
    let repo = TestRepo::new(&format!("fixture-{variant}"));
    let mut subjects = Vec::new();
    for op in &base {
        match op {
            Op::Commit(subject) => subjects.push(subject.as_str()),
            Op::Flag(name) => panic!("base.txt cannot set @@@ option {name}"),
            _ => apply(&repo, op),
        }
    }
    repo.commit("Initial fixture");
    let mut phase_history = true;
    for op in &ops {
        match op {
            Op::Flag(name) if name == "no-phase-history" => phase_history = false,
            Op::Flag(name) => panic!("variants/{variant}.txt: unknown @@@ option {name}"),
            Op::Commit(subject) => {
                panic!("variants/{variant}.txt: @@@ commit {subject} belongs in base.txt")
            }
            _ => {}
        }
    }
    if phase_history {
        for subject in subjects {
            repo.commit(subject);
        }
    }
    for op in ops.iter().filter(|op| !matches!(op, Op::Flag(_))) {
        apply(&repo, op);
    }
    repo.commit(&format!("Fixture variant: {variant}"));
    repo
}

/// Apply one file operation to the working tree (commits and options are handled by
/// `materialize_fixture`).
fn apply(repo: &TestRepo, op: &Op) {
    match op {
        Op::File(rel, content) => repo.write(rel, content),
        Op::Append(rel, content) => {
            let path = repo.path().join(rel);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
            file.write_all(content.as_bytes())
                .unwrap_or_else(|e| panic!("cannot append to {}: {e}", path.display()));
        }
        Op::Prepend(rel, content) => {
            let path = repo.path().join(rel);
            let old =
                fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let mut new = content.as_bytes().to_vec();
            new.extend_from_slice(&old);
            fs::write(&path, new)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        }
        Op::Delete(rel) => {
            let path = repo.path().join(rel);
            fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("cannot delete {}: {e}", path.display()));
        }
        Op::Commit(subject) => panic!("@@@ commit {subject} is applied by materialize_fixture"),
        Op::Flag(name) => panic!("@@@ option {name} is read by materialize_fixture"),
    }
}

/// main at the branch point of PR R1. Its scripts/docs/check.py and docs produced the
/// real-repository goldens; the snapshot replays this commit with `SNAPSHOT_MIGRATION`
/// applied.
pub const SNAPSHOT_SHA: &str = "33c6b3deabb36055d26d57fb2a60db233c4d3f6f";

/// `tests/fixtures/docs-check/<this>`: a patch against `SNAPSHOT_SHA` that moves its
/// root `CLAUDE.md` to `.claude/CLAUDE.md`, the only place docs-check reads it now, and
/// updates the links into and out of it and the one baseline entry keyed by its path.
/// The real-repository goldens were recorded from check.py on the unmigrated snapshot
/// and differ from its output only in that path (`CLAUDE.md` became
/// `.claude/CLAUDE.md`).
pub const SNAPSHOT_MIGRATION: &str = "snapshot-claude-md.patch";

/// The repository that contains `tools/` (the checkout or worktree under test).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools/ lives in the repository root")
        .to_path_buf()
}

/// `tools/tests/golden/docs-check`.
pub fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/docs-check")
}

/// Where a case's input repository comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A snapshot of this repository at `SNAPSHOT_SHA` with `SNAPSHOT_MIGRATION`
    /// applied (`snapshot_real`).
    Real,
    /// `materialize_fixture(<variant>)`.
    Fixture(&'static str),
}

impl Source {
    /// `real` or `fixture/<variant>`: the golden subdirectory and the sharing key.
    pub fn key(self) -> String {
        match self {
            Source::Real => "real".to_string(),
            Source::Fixture(variant) => format!("fixture/{variant}"),
        }
    }
}

/// One golden case: `aios docs-check <args>` (or `check.py <args>`) in a fresh copy of
/// `source`.
#[derive(Clone, Debug)]
pub struct Case {
    pub source: Source,
    pub name: String,
    /// docs-check flags only; `docs-check` itself is prepended for aios.
    pub args: Vec<String>,
    /// Where the tool runs, relative to the repository root; `None` is the root. Both
    /// tools find the same repository from anywhere inside it, but `--baseline` is
    /// resolved against this directory (check.py L1611-1613).
    pub cwd: Option<&'static str>,
    /// The baseline this case writes, relative to the repository root. Only read when
    /// `writes_baseline`.
    pub baseline_rel: &'static str,
    /// `--update-baseline`: the written baseline is a golden too.
    pub writes_baseline: bool,
}

impl Case {
    /// `real/<name>` or `fixture/<variant>/<name>`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.source.key(), self.name)
    }

    /// `exit <code>\n` followed by the exact stdout bytes.
    pub fn golden_path(&self) -> PathBuf {
        golden_root().join(format!("{}.golden", self.label()))
    }

    /// The bytes of `baseline_rel` after a `writes_baseline` case.
    pub fn baseline_golden_path(&self) -> PathBuf {
        golden_root().join(format!("{}.baseline.json", self.label()))
    }

    /// The working directory this case runs in, inside the materialized `repo`.
    pub fn run_dir(&self, repo: &Path) -> PathBuf {
        match self.cwd {
            Some(rel) => repo.join(rel),
            None => repo.to_path_buf(),
        }
    }

    /// The flags, for check.py.
    pub fn flag_args(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }

    /// `docs-check` plus the flags, for aios.
    pub fn aios_args(&self) -> Vec<&str> {
        std::iter::once("docs-check")
            .chain(self.args.iter().map(String::as_str))
            .collect()
    }
}

fn case(source: Source, name: &str, args: &[&str], writes_baseline: bool) -> Case {
    Case {
        source,
        name: name.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        baseline_rel: "scripts/docs/baseline.json",
        writes_baseline,
    }
}

/// `case`, run from `cwd` (relative to the repository root) and writing `baseline_rel`
/// instead of the default baseline.
fn case_at(
    source: Source,
    name: &str,
    args: &[&str],
    cwd: Option<&'static str>,
    baseline_rel: &'static str,
    writes_baseline: bool,
) -> Case {
    Case {
        cwd,
        baseline_rel,
        ..case(source, name, args, writes_baseline)
    }
}

/// Every golden case: 22 on the real snapshot and 48 on the fixture (70 in all, four
/// of which also write a baseline).
pub fn cases() -> Vec<Case> {
    let real = Source::Real;
    let mut out = vec![
        case(real, "default", &[], false),
        case(real, "all", &["--all"], false),
        case(real, "json", &["--json"], false),
        case(real, "json-all", &["--json", "--all"], false),
        case(real, "markdown", &["--markdown"], false),
        case(real, "list-checks", &["--list-checks"], false),
    ];
    for name in CHECK_ORDER {
        out.push(case(
            real,
            &format!("check-{name}"),
            &["--check", name],
            false,
        ));
    }
    out.push(case(real, "update-baseline", &["--update-baseline"], true));

    let base = Source::Fixture("base");
    out.extend([
        case(base, "default", &[], false),
        case(base, "all", &["--all"], false),
        case(base, "json-all", &["--json", "--all"], false),
        case(base, "markdown", &["--markdown"], false),
        case(base, "update-baseline", &["--update-baseline"], true),
        case(
            base,
            "check-subset",
            &["--check", "anchors,md-links"],
            false,
        ),
        case(base, "unknown-check", &["--check", "bogus"], false),
    ]);
    for name in CHECK_ORDER {
        let drift = Source::Fixture(name);
        out.push(case(drift, "default", &[], false));
        out.push(case(drift, "json", &["--json"], false));
    }
    let shift = Source::Fixture("line-shift");
    out.push(case(shift, "default", &[], false));
    out.push(case(shift, "all", &["--all"], false));
    let skip = Source::Fixture("skip");
    out.push(case(skip, "default", &[], false));
    out.push(case(skip, "markdown", &["--markdown"], false));
    out.push(case(skip, "json", &["--json"], false));
    out.push(case(skip, "update-baseline", &["--update-baseline"], true));
    let grown = Source::Fixture("grown");
    out.push(case(grown, "default", &[], false));
    out.push(case(grown, "markdown", &["--markdown"], false));
    // --baseline: the only place `run_with` reimplements CPython path semantics
    // (`os.path.relpath(baseline_path, root)` for the displayed path, the process
    // directory for the file that is opened and written; check.py L1611-1613,
    // L1618-1625). `docs/` and `docs/project/` exist in the fixture base, so no
    // fixture file changes for these.
    out.push(case(
        base,
        "baseline-alias",
        &["--baseline", "docs/../scripts/docs/baseline.json"],
        false,
    ));
    out.push(case_at(
        base,
        "baseline-subdir",
        &["--baseline", "../../scripts/docs/missing.json"],
        Some("docs/project"),
        "scripts/docs/baseline.json",
        false,
    ));
    out.push(case_at(
        base,
        "baseline-elsewhere-update",
        &[
            "--baseline",
            "scripts/docs/other-baseline.json",
            "--update-baseline",
        ],
        None,
        "scripts/docs/other-baseline.json",
        true,
    ));
    out
}

/// A clone of this repository at `SNAPSHOT_SHA` with `SNAPSHOT_MIGRATION` applied to the
/// index and working tree (not committed) and every ref deleted, so that docs-check's
/// history base (merge-base with origin/main or main) falls back to HEAD.
pub fn snapshot_real() -> TestRepo {
    let source = repo_root();
    let commit = format!("{SNAPSHOT_SHA}^{{commit}}");
    let present = isolated(Command::new("git").arg("-C").arg(&source).args([
        "cat-file",
        "-e",
        commit.as_str(),
    ]))
    .output()
    .is_ok_and(|out| out.status.success());
    assert!(
        present,
        "real-repository goldens need full history containing {SNAPSHOT_SHA} (CI: fetch-depth: 0)"
    );
    let repo = TestRepo::adopt(unique_dir("real"));
    let source_str = source.to_str().expect("repository path is UTF-8");
    git(
        &source,
        &[
            "clone",
            "-q",
            "--shared",
            "--no-checkout",
            source_str,
            repo.path_str(),
        ],
    );
    git(repo.path(), &["checkout", "-q", "--detach", SNAPSHOT_SHA]);
    let migration = fixtures_dir().join(SNAPSHOT_MIGRATION);
    let migration = migration.to_str().expect("fixture path is UTF-8");
    git(repo.path(), &["apply", "--index", migration]);
    // A clone does not inherit the source repository's config (see `TestRepo::new`).
    git(repo.path(), &["config", "core.excludesFile", "/dev/null"]);
    let deletions = git(repo.path(), &["for-each-ref", "--format=delete %(refname)"]);
    let mut child = isolated(Command::new("git").arg("-C").arg(repo.path()).args([
        "update-ref",
        "--no-deref",
        "--stdin",
    ]))
    .stdin(Stdio::piped())
    .spawn()
    .expect("git update-ref starts");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(deletions.as_bytes())
        .expect("write the ref deletions");
    let status = child.wait().expect("git update-ref finishes");
    assert!(
        status.success(),
        "git update-ref --stdin failed in {}",
        repo.path_str()
    );
    assert_eq!(
        git(repo.path(), &["for-each-ref"]),
        "",
        "the snapshot keeps no refs"
    );
    repo
}

/// A fresh input repository for `source`.
pub fn materialize(source: Source) -> TestRepo {
    match source {
        Source::Real => snapshot_real(),
        Source::Fixture(variant) => materialize_fixture(variant),
    }
}

/// The last `scripts/docs/check.py` on main, as a git object: `SNAPSHOT_SHA`'s copy.
/// faf6d20 (#166) added the file, no later commit on main changed it (the blob is
/// 6cea366 at faf6d20, at `SNAPSHOT_SHA` and at 201af49, the parent of the deletion),
/// and 212df62 (#207, R1) deleted it. The goldens were recorded against this version.
pub fn check_py_object() -> String {
    format!("{SNAPSHOT_SHA}:scripts/docs/check.py")
}

/// The Unicode version (`unicodedata.unidata_version`) the oracle's interpreter must
/// have: CPython 3.14's, the version `aios_tools::pyre`'s `\d` and `\s` classes were
/// pinned against (its `digit_class_is_pythons` and `space_class_is_pythons` tests).
/// A CPython on another Unicode version disagrees with aios on the digits one version
/// has and the other lacks, so it is not used as the oracle. When CPython and the
/// `regex` crate move to a newer Unicode version, update the pinned tables and this
/// together.
pub const ORACLE_UNIDATA_VERSION: &str = "16.0.0";

/// The flags every oracle interpreter run starts with. `-E` ignores every `PYTHON*`
/// variable, so the developer's `PYTHONPATH` (a `sitecustomize.py`, or a module
/// shadowing `re` or `unicodedata`), `PYTHONHOME`, `PYTHONSTARTUP` or
/// `PYTHONIOENCODING` cannot change what the pinned interpreter running the pinned
/// script prints. It also ignores the `PYTHONUTF8` and `PYTHONDONTWRITEBYTECODE` that
/// `isolated()` sets, so `-X utf8` and `-B` restate them. Not `-I`: it would also change
/// `sys.path[0]`, which `CHECK_PY_BOOTSTRAP` sets itself.
const ORACLE_FLAGS: [&str; 4] = ["-E", "-X", "utf8", "-B"];

/// Prints the interpreter's version and its `unicodedata` version, space-separated.
const VERSION_PROBE: &str =
    "import sys, unicodedata; print(sys.version.split()[0], unicodedata.unidata_version)";

/// The differential oracle: check.py materialised from git history, and the
/// interpreter that runs it.
pub struct CheckPy {
    /// The absolute path of the resolved `python3` (see `python3_interpreter`).
    pub interpreter: PathBuf,
    /// The interpreter's Python and Unicode versions, for the test log.
    pub version: String,
    /// check.py's bytes, written under `CARGO_TARGET_TMPDIR/check-py/`.
    pub script: PathBuf,
}

/// check.py materialised from `check_py_object()` and a working `python3`, resolved
/// once per test process. `Err` is the reason the oracle is unavailable, for the caller
/// to print when it skips: no usable `python3` on `PATH`, a `python3` whose Unicode
/// version is not `ORACLE_UNIDATA_VERSION`, or no such git object (a shallow clone;
/// CI's Tools (host) job checks out with `fetch-depth: 0` and installs CPython 3.14).
pub fn check_py() -> Result<&'static CheckPy, &'static str> {
    static ORACLE: OnceLock<Result<CheckPy, String>> = OnceLock::new();
    ORACLE
        .get_or_init(materialize_check_py)
        .as_ref()
        .map_err(String::as_str)
}

fn materialize_check_py() -> Result<CheckPy, String> {
    let interpreter = python3_interpreter()
        .ok_or("python3 is not available (not on PATH, or it does not run)")?
        .clone();
    let probe = isolated(
        Command::new(&interpreter)
            .args(ORACLE_FLAGS)
            .args(["-c", VERSION_PROBE]),
    )
    .output()
    .ok()
    .filter(|out| out.status.success())
    .map(|out| String::from_utf8_lossy(&out.stdout).trim().to_string())
    .ok_or_else(|| {
        format!(
            "{} cannot report its version in the isolated test environment",
            interpreter.display()
        )
    })?;
    let (python, unidata) = probe.split_once(' ').ok_or_else(|| {
        format!(
            "{} printed {probe:?} for its version",
            interpreter.display()
        )
    })?;
    if unidata != ORACLE_UNIDATA_VERSION {
        return Err(format!(
            "{} is Python {python} with Unicode {unidata}; the oracle needs Unicode \
             {ORACLE_UNIDATA_VERSION} (CPython 3.14), the version aios's \\d and \\s \
             classes match",
            interpreter.display()
        ));
    }
    let version = format!("Python {python}, Unicode {unidata}");
    let object = check_py_object();
    let out = isolated(Command::new("git").arg("-C").arg(repo_root()).args([
        "cat-file",
        "blob",
        object.as_str(),
    ]))
    .output()
    .map_err(|e| format!("cannot run git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git cannot read {object} (a shallow clone? the oracle needs history back to \
             {SNAPSHOT_SHA}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    // Written to a per-process temporary name, then renamed over the shared path, so
    // test processes that materialise it at the same time never read a partial file.
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("check-py");
    fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let script = dir.join("check.py");
    let partial = dir.join(format!("check.py.{}.tmp", std::process::id()));
    fs::write(&partial, &out.stdout)
        .and_then(|()| fs::rename(&partial, &script))
        .map_err(|e| format!("cannot write {}: {e}", script.display()))?;
    Ok(CheckPy {
        interpreter,
        version,
        script,
    })
}

/// Runs the script named by `argv[1]` as `__main__` with `__file__` = `argv[2]` and
/// `sys.argv` = `[argv[2]] + argv[3:]`, as `python3 <argv[2]> <args>` would if check.py
/// sat at `argv[2]`. check.py reads `__file__` only in `repo_root()` (L1576-1582), to
/// find its repository with `git rev-parse --show-toplevel` in the script's directory,
/// so `run_check_py` points it at `<root>/check.py`. `sys.path[0]` is the directory of
/// the materialised script (it holds nothing else that imports could pick up), as when
/// a script runs directly.
const CHECK_PY_BOOTSTRAP: &str = "\
import sys
def _run(script, fake, args):
    import __main__, os
    sys.argv[:] = [fake] + args
    sys.path[0] = os.path.dirname(os.path.abspath(script))
    with open(script, 'rb') as f:
        code = compile(f.read(), fake, 'exec', dont_inherit=True)
    namespace = __main__.__dict__
    del namespace['_run']
    namespace['__file__'] = fake
    exec(code, namespace)
_run(sys.argv[1], sys.argv[2], sys.argv[3:])
";

/// The absolute path of the ambient `python3` interpreter, resolved once outside
/// `isolated()` (see the module doc: a version-manager shim breaks under the isolated
/// `HOME`). `None` when `python3` is not on `PATH`, does not run, or does not print a
/// usable path.
fn python3_interpreter() -> Option<&'static PathBuf> {
    static INTERPRETER: OnceLock<Option<PathBuf>> = OnceLock::new();
    INTERPRETER
        .get_or_init(|| {
            // Deliberately NOT isolated(): resolving the shim to CPython's own
            // absolute path needs the developer's ambient HOME/PATH (asdf, pyenv,
            // etc.). Only the resolved absolute path is ever run under isolated().
            let out = Command::new("python3")
                .arg("-c")
                .arg("import sys; print(sys.executable)")
                .output()
                .ok()?;
            if !out.status.success() {
                return None;
            }
            let stdout = String::from_utf8(out.stdout).ok()?;
            let path = stdout.trim();
            if path.is_empty() {
                return None;
            }
            Some(PathBuf::from(path))
        })
        .as_ref()
}

/// check.py `<args>` on the repository at `root`, with `cwd` as the working directory:
/// the materialised script (`check_py`) run by the resolved interpreter (not the ambient
/// `python3` shim) in the isolated environment, through `CHECK_PY_BOOTSTRAP`, so that
/// check.py takes `root` as its repository without a copy of the script inside it.
/// `cwd` only changes the paths check.py resolves itself, such as a relative
/// `--baseline`. Panics when the oracle is unavailable; callers check `check_py` first.
pub fn run_check_py(root: &Path, cwd: &Path, args: &[&str]) -> Run {
    let oracle = check_py().unwrap_or_else(|reason| panic!("check.py is unavailable: {reason}"));
    let out = isolated(
        Command::new(&oracle.interpreter)
            .args(ORACLE_FLAGS)
            .arg("-c")
            .arg(CHECK_PY_BOOTSTRAP)
            .arg(&oracle.script)
            .arg(root.join("check.py"))
            .args(args)
            .current_dir(cwd),
    )
    .output()
    .unwrap_or_else(|e| {
        panic!(
            "cannot run {} in {}: {e}",
            oracle.interpreter.display(),
            cwd.display()
        )
    });
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}
