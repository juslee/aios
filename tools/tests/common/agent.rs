//! Support for the `aios brief` and `aios checkpoint` parity tests: the two oracles
//! read from git history, twin repositories, the runners and the invariants every
//! parity test asserts.
//!
//! The oracles are the deleted scripts' blobs (`git cat-file blob <sha>:<path>`, the R4
//! pattern in `soak.rs`), so the differentials keep running after the scripts are gone.
//! A [`Scenario`] describes a repository once; each run materialises its own twin of it
//! (a bare `origin`, a clone, optional linked worktrees and files, the fake `gh` and
//! the PATH jail). Both twins are deterministic: the identity and the dates of every
//! commit, in setup and in a run, are pinned, so a `wip` commit has the same sha in
//! both and on every host.
//!
//! The fake `gh` and the jail are the shared ones in `fake_gh.rs` and `jail.rs`.

use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, UNIX_EPOCH};

use regex::bytes::Regex;

use super::fake_gh::{self, FakeGh, GhCall};
use super::fixture::repo_root;
use super::{isolated, jail, unique_dir};

/// The commit on `origin/main` that last changed `scripts/agent/brief.sh` before the
/// R3 port: it holds #234's sections (`handoff-`, `Routing log`) and #246's
/// (`print_interleaved`, `PANIC-LOCK`). Its `scripts/agent/qemu-lock.sh` is the third
/// oracle.
pub const BRIEF_ORACLE: &str = "2d4a9f72ebb8cd98a6659d8c75ebec91be23658f";
/// #166, the last change of `scripts/agent/checkpoint.sh`.
pub const CHECKPOINT_ORACLE: &str = "faf6d20295136489d6ccdfa5a253480a9f757fcc";

/// An oracle script.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    Brief,
    Checkpoint,
    QemuLock,
}

impl Which {
    const ALL: [Which; 3] = [Which::Brief, Which::Checkpoint, Which::QemuLock];

    fn index(self) -> usize {
        self as usize
    }

    /// The commit and the repository path of the blob.
    fn blob(self) -> (&'static str, &'static str) {
        match self {
            Which::Brief => (BRIEF_ORACLE, "scripts/agent/brief.sh"),
            Which::Checkpoint => (CHECKPOINT_ORACLE, "scripts/agent/checkpoint.sh"),
            Which::QemuLock => (BRIEF_ORACLE, "scripts/agent/qemu-lock.sh"),
        }
    }
}

/// A directory under `CARGO_TARGET_TMPDIR` that lives as long as the test process.
fn process_dir(label: &str) -> PathBuf {
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{label}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("create the directory");
    dir
}

/// The oracle script, written once per test process under its own name (a script
/// prints its own path in places). Panics when git history lacks the commit: CI's
/// Tools (host) job checks out with `fetch-depth: 0`.
pub fn oracle_script(which: Which) -> PathBuf {
    static SCRIPTS: [OnceLock<PathBuf>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    SCRIPTS[which.index()]
        .get_or_init(|| {
            let (sha, path) = which.blob();
            let spec = format!("{sha}:{path}");
            let out = isolated(
                Command::new("git")
                    .arg("-C")
                    .arg(repo_root())
                    .args(["cat-file", "blob", &spec]),
            )
            .output()
            .expect("run git");
            assert!(
                out.status.success(),
                "cannot read {spec} from git history (Tools (host) needs fetch-depth 0): {}",
                String::from_utf8_lossy(&out.stderr)
            );
            let script = process_dir("agent-oracle").join(
                Path::new(path)
                    .file_name()
                    .expect("the blob has a file name"),
            );
            std::fs::write(&script, &out.stdout).expect("write the oracle");
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("make the oracle executable");
            script
        })
        .clone()
}

/// The oracle's text.
pub fn oracle_text(which: Which) -> String {
    String::from_utf8(std::fs::read(oracle_script(which)).expect("read the oracle"))
        .expect("the oracle is UTF-8")
}

/// The host tools the oracles need before any scenario runs.
pub struct HostTools {
    /// `bash` as resolved on the host PATH. The oracles run as `<bash> <script>`, by
    /// absolute path: no script line runs `bash`, so the jail has none.
    pub bash: PathBuf,
}

/// Resolve `bash`, `git`, `sh` and `jq` on the host PATH, and require jq 1.7 or newer.
/// Panics, never skips: a differential that did not run proves nothing.
pub fn require_tools() -> HostTools {
    static TOOLS: OnceLock<PathBuf> = OnceLock::new();
    let bash = TOOLS.get_or_init(|| {
        for tool in ["git", "sh", "jq"] {
            assert!(
                jail::host_which(tool).is_some(),
                "the agent-script tests need {tool} on PATH"
            );
        }
        let jq = jail::host_which("jq").expect("jq");
        let out = Command::new(&jq).arg("--version").output().expect("run jq");
        let version = String::from_utf8_lossy(&out.stdout).trim().to_string();
        let numbers: Vec<u32> = version
            .trim_start_matches("jq-")
            .split(|c: char| !c.is_ascii_digit())
            .filter_map(|n| n.parse().ok())
            .collect();
        assert!(
            numbers.len() >= 2 && (numbers[0], numbers[1]) >= (1, 7),
            "the brief oracle needs jq 1.7 or newer, found {version:?}"
        );
        jail::host_which("bash").expect("the agent-script tests need bash on PATH")
    });
    HostTools { bash: bash.clone() }
}

/// The fake docs-check child (`aios-fake-docs`, installed in the fake bin): the
/// oracle's `.claude/hooks/aios docs-check --json` and `aios brief`'s child
/// (`AIOS_BRIEF_DOCS_CHECK`) both run it. It finds its root from `$0`, logs its argv
/// and `pwd -P` under `<root>/docs-calls/<SEQ>/`, prints `<root>/docs/out` and
/// `<root>/docs/err` and exits with `<root>/docs/rc`. When `<root>/docs/stale` exists
/// it first prints the shim's stale-rebuild warning on stderr.
pub const FAKE_DOCS: &str = r#"#!/bin/sh
# aios-fake-docs: the docs-check child of the brief tests.
root=$(cd "$(dirname "$0")/.." && pwd -P)
n=0
for d in "$root"/docs-calls/*; do
    [ -d "$d" ] && n=$((n + 1))
done
seq=$(printf '%06d' $((n + 1)))
call=$root/docs-calls/$seq
mkdir -p "$call"
if [ $# -gt 0 ]; then
    printf '%s\0' "$@" >"$call/argv"
else
    : >"$call/argv"
fi
pwd -P >"$call/cwd"
if [ -f "$root/docs/stale" ]; then
    echo "aios: rebuilding target/tools/installed/aios failed; running the stale binary (run: just tools)" >&2
fi
if [ -f "$root/docs/out" ]; then
    cat "$root/docs/out"
fi
if [ -f "$root/docs/err" ]; then
    cat "$root/docs/err" >&2
fi
rc=0
if [ -f "$root/docs/rc" ]; then
    rc=$(cat "$root/docs/rc")
fi
exit "$rc"
"#;

/// The tracked `.claude/hooks/aios` of every twin: it runs the fake docs-check.
pub const TWIN_STUB: &str = "#!/bin/sh\nexec aios-fake-docs \"$@\"\n";

/// One logged docs-check call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DocsCall {
    pub argv: Vec<Vec<u8>>,
    pub cwd: PathBuf,
}

/// The docs-check calls logged under a fake root.
pub fn docs_calls(root: &Path) -> Vec<DocsCall> {
    let Ok(entries) = std::fs::read_dir(root.join("docs-calls")) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries.filter_map(Result::ok).map(|e| e.path()).collect();
    dirs.sort();
    dirs.iter()
        .map(|dir| {
            let raw = std::fs::read(dir.join("argv")).expect("read the docs call's argv");
            let mut argv: Vec<Vec<u8>> = raw.split(|&b| b == 0).map(<[u8]>::to_vec).collect();
            argv.pop();
            let cwd = std::fs::read_to_string(dir.join("cwd")).expect("read the docs call's cwd");
            DocsCall {
                argv,
                cwd: PathBuf::from(cwd.trim_end_matches('\n')),
            }
        })
        .collect()
}

/// label -> regex over the rendered argv line, for each `gh` call `brief.sh` makes.
pub const BRIEF_GH_FIXTURES: &[(&str, &str)] = &[
    ("rate-limit", "^api rate_limit "),
    (
        "issues-needs-human",
        "^issue list --state open --label needs-human ",
    ),
    (
        "issues-agent-ready",
        "^issue list --state open --label agent-ready ",
    ),
    (
        "issues-agent-working",
        "^issue list --state open --label agent-working ",
    ),
    ("graphql-threads", "^api graphql .*reviewThreads"),
    ("graphql-suites", "^api graphql .*checkSuites"),
    ("pr-list", "^pr list --state open --limit 30 "),
    ("run-list", "^run list --branch main "),
];

/// label -> regex for the one `gh` call `checkpoint.sh` makes.
pub const CHECKPOINT_GH_FIXTURES: &[(&str, &str)] =
    &[("pr-list-head", "^pr list --head [^ ]+ --state open ")];

/// Every `gh` call a brief scenario may make: `api rate_limit`, `api graphql` with
/// `reviewThreads` or `checkSuites`, the three `issue list` calls,
/// `pr list --state open --limit 30` and `run list --branch main`. No write.
pub const BRIEF_GH_ALLOW: &[&str] = &[
    "^api rate_limit ",
    "^api graphql .*(reviewThreads|checkSuites)",
    "^issue list --state open --label (needs-human|agent-ready|agent-working) ",
    "^pr list --state open --limit 30 ",
    "^run list --branch main ",
];

/// The one `gh` call a checkpoint scenario may make: `pr list --head <branch>`.
pub const CHECKPOINT_GH_ALLOW: &[&str] = &["^pr list --head [^ ]+ --state open "];

/// Scenarios whose golden is aios-only, so the oracle differential skips them:
/// scenario -> the divergence (D-number) it pins.
pub const DIVERGENT_SCENARIOS: &[(&str, &str)] = &[
    ("cli_help", "D1"),
    ("pr_labelled_needs_human", "D10"),
    ("scan_git_failure_fails_closed", "D9"),
    ("scan_git_failure_on_clean_tree", "D9"),
];

/// Scenarios whose oracle output depends on the host's tools, so the oracle
/// comparison runs on macOS only (the goldens were recorded there):
/// scenario -> the tool and the oracle line.
pub const PLATFORM_SCENARIOS: &[(&str, &str)] = &[(
    "soak_interleaved_no_boots",
    "awk: BSD awk stops at the missing boots.tsv, gawk and mawk run END (brief.sh L395-407)",
)];

/// Whether the oracle comparison of `scenario` runs on this host. On a host the
/// scenario is not for, it prints `oracle skipped on <os>: <scenario> (<reason>)`.
pub fn oracle_runs_here(scenario: &str) -> bool {
    match PLATFORM_SCENARIOS
        .iter()
        .find(|(name, _)| *name == scenario)
    {
        Some((_, reason)) if !cfg!(target_os = "macos") => {
            println!(
                "oracle skipped on {}: {scenario} ({reason})",
                std::env::consts::OS
            );
            false
        }
        _ => true,
    }
}

/// Whether a test that runs `aios brief` with the fake docs-check child must be skipped
/// on this build. `AIOS_BRIEF_DOCS_CHECK` (the variable that points the brief at
/// `aios-fake-docs`) is honoured only with debug assertions, so a release build would run
/// the real docs-check over the fixture. It then prints one line saying so.
pub fn skip_brief_in_release(test: &str) -> bool {
    if cfg!(debug_assertions) {
        return false;
    }
    println!(
        "{test}: skipped in release builds: the docs-check override (AIOS_BRIEF_DOCS_CHECK) is debug-only"
    );
    true
}

/// Which script a scenario exercises.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sub {
    Brief,
    Checkpoint,
}

impl Sub {
    fn oracle(self) -> Which {
        match self {
            Sub::Brief => Which::Brief,
            Sub::Checkpoint => Which::Checkpoint,
        }
    }

    fn fixtures(self) -> &'static [(&'static str, &'static str)] {
        match self {
            Sub::Brief => BRIEF_GH_FIXTURES,
            Sub::Checkpoint => CHECKPOINT_GH_FIXTURES,
        }
    }

    fn allow(self) -> &'static [&'static str] {
        match self {
            Sub::Brief => BRIEF_GH_ALLOW,
            Sub::Checkpoint => CHECKPOINT_GH_ALLOW,
        }
    }
}

/// The two copies of a scenario's repository: the oracle's and aios's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Twin {
    Oracle,
    Aios,
}

impl Twin {
    fn label(self) -> &'static str {
        match self {
            Twin::Oracle => "a",
            Twin::Aios => "b",
        }
    }
}

enum Step {
    Write {
        rel: String,
        bytes: Vec<u8>,
        mode: u32,
    },
    /// A file whose `{rev:<rev>}` and `{short:<rev>}` placeholders become the commit
    /// `git rev-parse` names at that point of the setup (40 and 7 hex digits).
    WriteRevs {
        rel: String,
        template: String,
    },
    Remove {
        rel: String,
    },
    Mtime {
        rel: String,
        secs: u64,
    },
    Git {
        dir: String,
        args: Vec<String>,
    },
    Commit {
        dir: String,
        message: String,
    },
}

struct GhSpec {
    label: String,
    regex: String,
    out: Vec<u8>,
    err: Vec<u8>,
    rc: Option<i32>,
    nth: Vec<(usize, Vec<u8>, Vec<u8>, i32)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StartState {
    head: Option<String>,
    branch: Option<String>,
    status: String,
    index_tree: Option<String>,
    origin_refs: BTreeMap<String, String>,
}

/// What a run is expected to show, for [`assert_not_vacuous`].
#[derive(Clone, Copy, Debug)]
pub struct Expect {
    pub sub: Sub,
    /// At least one `gh` call.
    pub gh: bool,
    /// Exactly this many docs-check calls (`None`: do not check).
    pub docs_calls: Option<usize>,
    /// At least this many `\n## ` sections on stdout.
    pub min_sections: usize,
}

/// A repository described once and materialised once per run.
pub struct Scenario {
    pub name: String,
    pub sub: Sub,
    steps: Vec<Step>,
    run_cwd: String,
    toplevel: String,
    outside: bool,
    qemu_lock: bool,
    origin_main: bool,
    reftable: bool,
    gh_missing: bool,
    gh: Vec<GhSpec>,
    docs: Option<(Vec<u8>, Vec<u8>, i32)>,
    docs_stale: bool,
    env: Vec<(String, String)>,
    aios_env: Vec<(String, String)>,
    fake_tools: Vec<(String, String)>,
    expect: Expect,
    start: OnceLock<StartState>,
}

impl Scenario {
    fn new(sub: Sub, name: &str) -> Scenario {
        Scenario {
            name: name.to_string(),
            sub,
            steps: Vec::new(),
            run_cwd: String::new(),
            toplevel: String::new(),
            outside: false,
            qemu_lock: true,
            origin_main: true,
            reftable: false,
            gh_missing: false,
            gh: Vec::new(),
            docs: None,
            docs_stale: false,
            env: Vec::new(),
            aios_env: Vec::new(),
            fake_tools: Vec::new(),
            expect: Expect {
                sub,
                gh: sub == Sub::Brief,
                docs_calls: Some(usize::from(sub == Sub::Brief)),
                min_sections: usize::from(sub == Sub::Brief),
            },
            start: OnceLock::new(),
        }
    }

    /// A brief scenario: a gh call and one docs-check call are expected.
    pub fn brief(name: &str) -> Scenario {
        Scenario::new(Sub::Brief, name)
    }

    /// A checkpoint scenario: no gh call and no docs-check call are expected until
    /// the scenario installs the `pr-list-head` fixture and calls [`expects_gh`].
    ///
    /// [`expects_gh`]: Scenario::expects_gh
    pub fn checkpoint(name: &str) -> Scenario {
        Scenario::new(Sub::Checkpoint, name)
    }

    /// Run in a fresh directory outside every checkout (and no repository at all).
    pub fn outside_repo(sub: Sub, name: &str) -> Scenario {
        let mut scenario = Scenario::new(sub, name);
        scenario.outside = true;
        scenario.expect.gh = false;
        scenario.expect.docs_calls = None;
        scenario.expect.min_sections = 0;
        scenario
    }

    /// A tracked-or-untracked file in the working tree (mode 644), created before the
    /// next commit. `rel` is relative to the twin's root.
    pub fn file(self, rel: &str, content: impl AsRef<[u8]>) -> Scenario {
        self.file_mode(rel, content, 0o644)
    }

    pub fn file_mode(mut self, rel: &str, content: impl AsRef<[u8]>, mode: u32) -> Scenario {
        self.steps.push(Step::Write {
            rel: rel.to_string(),
            bytes: content.as_ref().to_vec(),
            mode,
        });
        self
    }

    /// A file (mode 644) whose text holds `{rev:<rev>}` and `{short:<rev>}` placeholders,
    /// expanded when the step runs: the full sha, or its first 7 digits, of the commit
    /// `git rev-parse <rev>` names in the root at that point (after the commits and
    /// pushes written before this call). A soak summary naming a commit uses this.
    pub fn file_revs(mut self, rel: &str, template: &str) -> Scenario {
        self.steps.push(Step::WriteRevs {
            rel: rel.to_string(),
            template: template.to_string(),
        });
        self
    }

    pub fn remove(mut self, rel: &str) -> Scenario {
        self.steps.push(Step::Remove {
            rel: rel.to_string(),
        });
        self
    }

    /// Set a file's or directory's mtime (seconds since the epoch).
    pub fn mtime(mut self, rel: &str, secs: u64) -> Scenario {
        self.steps.push(Step::Mtime {
            rel: rel.to_string(),
            secs,
        });
        self
    }

    /// `git add -A && git commit` in the root, with the next pinned date.
    pub fn commit(self, message: &str) -> Scenario {
        self.commit_in("", message)
    }

    /// The same in the directory `dir` (relative to the root; a linked worktree).
    pub fn commit_in(mut self, dir: &str, message: &str) -> Scenario {
        self.steps.push(Step::Commit {
            dir: dir.to_string(),
            message: message.to_string(),
        });
        self
    }

    /// Any git command in the root.
    pub fn git(self, args: &[&str]) -> Scenario {
        self.git_in("", args)
    }

    pub fn git_in(mut self, dir: &str, args: &[&str]) -> Scenario {
        self.steps.push(Step::Git {
            dir: dir.to_string(),
            args: args.iter().map(|a| a.to_string()).collect(),
        });
        self
    }

    /// `git checkout -q -b <branch>`.
    pub fn branch(self, branch: &str) -> Scenario {
        self.git(&["checkout", "-q", "-b", branch])
    }

    /// A linked worktree at `rel` (under the root, normally `.claude/worktrees/<name>`,
    /// which the base `.gitignore` covers) on a new branch from `main`.
    pub fn worktree(self, rel: &str, branch: &str) -> Scenario {
        self.git(&["worktree", "add", "-q", "-b", branch, rel, "main"])
    }

    /// Do not push `main`: there is no `origin/main`.
    pub fn no_origin_main(mut self) -> Scenario {
        self.origin_main = false;
        self
    }

    /// Both repositories keep their refs in the reftable format, so a ref name that is
    /// not valid UTF-8 can exist on a host file system that refuses such file names
    /// (APFS): the default format stores each ref as a file of that name.
    pub fn reftable(mut self) -> Scenario {
        self.reftable = true;
        self
    }

    /// The base commit holds no `scripts/agent/qemu-lock.sh`.
    pub fn no_qemu_lock(mut self) -> Scenario {
        self.qemu_lock = false;
        self
    }

    /// Run from a subdirectory of the root (the toplevel stays the root).
    pub fn run_cwd(mut self, rel: &str) -> Scenario {
        self.run_cwd = rel.to_string();
        self
    }

    /// Run from the linked worktree at `rel`: it is the toplevel.
    pub fn run_in_worktree(mut self, rel: &str) -> Scenario {
        self.run_cwd = rel.to_string();
        self.toplevel = rel.to_string();
        self
    }

    /// An environment variable for both the oracle and the aios run (`{root}` in the
    /// value is the twin's root). The jail strips every `AIOS_*` variable, so a
    /// scenario that needs one sets it here.
    pub fn env(mut self, key: &str, value: &str) -> Scenario {
        self.env.push((key.to_string(), value.to_string()));
        self
    }

    /// The same, for the aios run only.
    pub fn aios_env(mut self, key: &str, value: &str) -> Scenario {
        self.aios_env.push((key.to_string(), value.to_string()));
        self
    }

    /// A fake tool `name` in front of the jail's: `script` is written as an executable
    /// file in the twin's fake bin directory, the first entry of the jail PATH. This is
    /// the one way a scenario puts a fake tool there; it does not depend on the twin's
    /// directory layout.
    ///
    /// A fake that calls through to the real tool finds the jail's `bin` directory in
    /// the variable [`REAL_BIN_ENV`] (`exec "$AIOS_TEST_REAL_BIN/git" "$@"`). The runner
    /// sets it for the oracle and the aios run alike.
    pub fn fake_tool(mut self, name: &str, script: &str) -> Scenario {
        assert!(
            !name.is_empty() && !name.contains('/') && name != "gh" && name != "aios-fake-docs",
            "fake_tool: {name:?} is not a plain tool name (gh and aios-fake-docs have their own fakes)"
        );
        self.fake_tools.push((name.to_string(), script.to_string()));
        self
    }

    /// Serve the fixture `label` (its regex comes from [`BRIEF_GH_FIXTURES`] or
    /// [`CHECKPOINT_GH_FIXTURES`]) with `out` on stdout and exit 0.
    pub fn gh_fixture(self, label: &str, out: impl AsRef<[u8]>) -> Scenario {
        self.gh_fixture_with(label, out, b"", 0)
    }

    /// The same with stderr and an exit code.
    pub fn gh_fixture_with(
        self,
        label: &str,
        out: impl AsRef<[u8]>,
        err: impl AsRef<[u8]>,
        rc: i32,
    ) -> Scenario {
        let regex = self
            .sub
            .fixtures()
            .iter()
            .find(|(name, _)| *name == label)
            .unwrap_or_else(|| panic!("no {:?} gh fixture named {label}", self.sub))
            .1;
        self.gh_custom(label, regex, out, err, rc)
    }

    /// A fixture with an explicit regex.
    pub fn gh_custom(
        mut self,
        label: &str,
        regex: &str,
        out: impl AsRef<[u8]>,
        err: impl AsRef<[u8]>,
        rc: i32,
    ) -> Scenario {
        self.gh.push(GhSpec {
            label: label.to_string(),
            regex: regex.to_string(),
            out: out.as_ref().to_vec(),
            err: err.as_ref().to_vec(),
            rc: Some(rc),
            nth: Vec::new(),
        });
        self
    }

    /// What the K-th call to the fixture `label` serves.
    pub fn gh_nth(
        mut self,
        label: &str,
        k: usize,
        out: impl AsRef<[u8]>,
        err: impl AsRef<[u8]>,
        rc: i32,
    ) -> Scenario {
        let spec = self
            .gh
            .iter_mut()
            .find(|spec| spec.label == label)
            .unwrap_or_else(|| panic!("gh_nth: install the fixture {label} first"));
        spec.nth
            .push((k, out.as_ref().to_vec(), err.as_ref().to_vec(), rc));
        self
    }

    /// Install every `<label>.out` / `<label>.err` / `<label>.rc` file of `dir` as the
    /// fixture `label` (the committed fixture data of a scenario).
    pub fn gh_fixtures_from_dir(mut self, dir: &Path) -> Scenario {
        for (label, regex) in self.sub.fixtures() {
            let read = |ext: &str| std::fs::read(dir.join(format!("{label}.{ext}"))).ok();
            let (out, err, rc) = (read("out"), read("err"), read("rc"));
            if out.is_none() && err.is_none() && rc.is_none() {
                continue;
            }
            let rc = rc.map_or(0, |bytes| {
                String::from_utf8_lossy(&bytes)
                    .trim()
                    .parse()
                    .unwrap_or_else(|_| panic!("{label}.rc is not a number"))
            });
            self = self.gh_custom(
                label,
                regex,
                out.unwrap_or_default(),
                err.unwrap_or_default(),
                rc,
            );
        }
        self
    }

    /// No `gh` on PATH at all.
    pub fn gh_missing(mut self) -> Scenario {
        self.gh_missing = true;
        self.expect.gh = false;
        self
    }

    /// Whether the scenario expects at least one `gh` call.
    pub fn expects_gh(mut self, yes: bool) -> Scenario {
        self.expect.gh = yes;
        self
    }

    /// The number of docs-check calls to expect (`None`: do not check).
    pub fn expects_docs_calls(mut self, n: Option<usize>) -> Scenario {
        self.expect.docs_calls = n;
        self
    }

    /// The least number of `\n## ` sections to expect on stdout.
    pub fn expects_sections(mut self, n: usize) -> Scenario {
        self.expect.min_sections = n;
        self
    }

    /// What the fake docs-check prints on stdout and stderr, and its exit code.
    pub fn docs(mut self, out: impl AsRef<[u8]>, err: impl AsRef<[u8]>, rc: i32) -> Scenario {
        self.docs = Some((out.as_ref().to_vec(), err.as_ref().to_vec(), rc));
        self
    }

    /// The docs-check child reports a stale binary: the fake prints the shim's warning
    /// (what the oracle greps), and the aios run gets `AIOS_SHIM_STALE=1`.
    pub fn docs_stale(mut self) -> Scenario {
        self.docs_stale = true;
        self.aios_env("AIOS_SHIM_STALE", "1")
    }
}

// --- scenario builders that brief_parity.rs or checkpoint_parity.rs and skill_contract.rs share

/// The sections of a full brief report.
pub const BRIEF_SECTIONS: usize = 11;

/// The committed fixtures of the brief scenarios: `tests/fixtures/brief/<scenario>/`.
pub fn brief_fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/brief")
}

/// A normal brief scenario: the committed `gh` answers of `tests/fixtures/brief/<name>/gh`
/// (one `<label>.{out,err,rc}` per call the oracle makes in it) and the docs-check answer
/// of `.../docs/{out,err,rc,stale}`, a full report of 11 sections and one docs-check call.
pub fn brief_base(name: &str) -> Scenario {
    let dir = brief_fixtures_dir().join(name);
    assert!(dir.is_dir(), "no fixture directory {}", dir.display());
    let docs = dir.join("docs");
    let read = |file: &str| std::fs::read(docs.join(file)).unwrap_or_default();
    let rc = std::fs::read_to_string(docs.join("rc")).map_or(0, |text| {
        text.trim()
            .parse()
            .unwrap_or_else(|_| panic!("{name}: docs/rc is not a number"))
    });
    let scenario = Scenario::brief(name)
        .gh_fixtures_from_dir(&dir.join("gh"))
        .docs(read("out"), read("err"), rc)
        .expects_sections(BRIEF_SECTIONS);
    if docs.join("stale").exists() {
        scenario.docs_stale()
    } else {
        scenario
    }
}

/// A `claude/x` branch checked out from `main`.
pub fn on_x(name: &str) -> Scenario {
    Scenario::checkpoint(name).branch("claude/x")
}

/// A new doc whose name looks like a secret, on `claude/x`.
pub fn sensitive_name_new_doc(name: &str) -> Scenario {
    on_x(name).file("docs/deploy-secret.md", "how to deploy\n")
}

/// On `claude/x` in the main checkout, a linked worktree `w1` with a commit not on origin
/// and an uncommitted file.
pub fn other_worktree_dirty_from_main(name: &str) -> Scenario {
    on_x(name)
        .worktree(".claude/worktrees/w1", "claude/w1")
        .file(".claude/worktrees/w1/work.txt", "work\n")
        .commit_in(".claude/worktrees/w1", "w1 work")
        .file(".claude/worktrees/w1/dirty.txt", "dirty\n")
}

/// One materialised twin. Removed when dropped (set `AIOS_KEEP_TWINS=1` to keep it).
pub struct TwinDir {
    base: PathBuf,
    root: PathBuf,
    origin: PathBuf,
    run_dir: PathBuf,
    fake: PathBuf,
    jail_bin: PathBuf,
    gh: Option<FakeGh>,
    outside_parent: Option<PathBuf>,
}

impl Drop for TwinDir {
    fn drop(&mut self) {
        if std::env::var_os("AIOS_KEEP_TWINS").is_none() {
            let _ = std::fs::remove_dir_all(&self.base);
        }
    }
}

impl TwinDir {
    /// The twin's checkout root (the toplevel for a run from the root).
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn origin(&self) -> &Path {
        &self.origin
    }

    /// Where the program runs.
    pub fn run_dir(&self) -> &Path {
        &self.run_dir
    }

    /// The fake root (`bin/gh`, `gh-calls`, `docs-calls`, ...).
    pub fn fake(&self) -> &Path {
        &self.fake
    }

    pub fn fake_gh(&self) -> Option<&FakeGh> {
        self.gh.as_ref()
    }

    /// `git args` in the root (host git, isolated); stdout without its final newline.
    pub fn git(&self, args: &[&str]) -> String {
        setup_git(&self.root, RUN_DATE, args)
            .trim_end_matches('\n')
            .to_string()
    }

    /// `git -C <dir> args` for a directory under the root, or the origin.
    pub fn git_in(&self, dir: &Path, args: &[&str]) -> String {
        setup_git(dir, RUN_DATE, args)
            .trim_end_matches('\n')
            .to_string()
    }

    fn fake_bin(&self) -> PathBuf {
        self.fake.join("bin")
    }
}

/// The variable holding the jail's `bin` directory (the real tools) for a fake tool
/// installed with [`Scenario::fake_tool`]. `twin_command` sets it after `hermetic`,
/// which strips every `AIOS_*` variable it inherits, so no allow list changes.
pub const REAL_BIN_ENV: &str = "AIOS_TEST_REAL_BIN";

/// 2026-01-01T00:00:00Z: the date of the base commits. A scenario's n-th commit is
/// dated `EPOCH_BASE + n`, so a fixed mtime in a scenario can be set between two commits.
pub const EPOCH_BASE: u64 = 1_767_225_600;
/// The commit date `n` seconds after it that a run (a checkpoint `wip` commit) uses.
const RUN_DATE: u64 = 100_000;

fn pin_identity(cmd: &mut Command, n: u64) -> &mut Command {
    let date = format!("{} +0000", EPOCH_BASE + n);
    cmd.env("GIT_AUTHOR_NAME", "aios-test")
        .env("GIT_COMMITTER_NAME", "aios-test")
        .env("GIT_AUTHOR_EMAIL", "aios-test@example.invalid")
        .env("GIT_COMMITTER_EMAIL", "aios-test@example.invalid")
        .env("GIT_AUTHOR_DATE", &date)
        .env("GIT_COMMITTER_DATE", &date)
}

/// Host git for setup and inspection: isolated, with the pinned identity and date `n`.
fn setup_git(dir: &Path, n: u64, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    isolated(&mut cmd);
    pin_identity(&mut cmd, n);
    cmd.env("GIT_TERMINAL_PROMPT", "0")
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null());
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} in {} failed: {}",
        dir.display(),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn write_file(path: &Path, bytes: &[u8], mode: u32) {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("create the parent directories");
    }
    std::fs::write(path, bytes).expect("write a scenario file");
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
        .expect("chmod a scenario file");
}

/// `template` with each `{rev:<rev>}` and `{short:<rev>}` replaced by the commit
/// `git rev-parse <rev>` prints in `root`. Any other brace is kept as it is.
fn expand_revs(root: &Path, n: u64, template: &str) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        let (before, tail) = rest.split_at(start);
        out.push_str(before);
        let placeholder = tail
            .find('}')
            .map(|end| (&tail[1..end], &tail[end + 1..]))
            .and_then(|(inner, after)| inner.split_once(':').map(|kv| (kv, after)));
        match placeholder {
            Some((("rev", rev), after)) => {
                out.push_str(setup_git(root, n, &["rev-parse", rev]).trim_end());
                rest = after;
            }
            Some((("short", rev), after)) => {
                let full = setup_git(root, n, &["rev-parse", rev]);
                out.push_str(&full.trim_end()[..7]);
                rest = after;
            }
            _ => {
                out.push('{');
                rest = &tail[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn origin_refs(origin: &Path) -> BTreeMap<String, String> {
    setup_git(
        origin,
        RUN_DATE,
        &["for-each-ref", "--format=%(refname) %(objectname)"],
    )
    .lines()
    .filter_map(|line| line.split_once(' '))
    .map(|(name, sha)| (name.to_string(), sha.to_string()))
    .collect()
}

fn state_of(twin: &TwinDir, run_toplevel: &Path) -> StartState {
    let probe = |args: &[&str]| {
        let mut cmd = Command::new("git");
        isolated(&mut cmd);
        let out = cmd
            .args(args)
            .current_dir(run_toplevel)
            .stdin(Stdio::null())
            .output()
            .expect("run git");
        out.status
            .success()
            .then(|| String::from_utf8_lossy(&out.stdout).trim_end().to_string())
    };
    StartState {
        head: probe(&["rev-parse", "HEAD"]),
        branch: probe(&["symbolic-ref", "--short", "-q", "HEAD"]),
        status: probe(&["status", "--porcelain"]).unwrap_or_default(),
        index_tree: probe(&["write-tree"]),
        origin_refs: origin_refs(&twin.origin),
    }
}

impl Scenario {
    /// Materialise `twin`: the repository, the fake root and the jail. Asserts the
    /// run directory resolves to the expected toplevel (never the enclosing aios
    /// checkout), and that the start state is identical to the other twin's.
    pub fn prepare(&self, twin: Twin) -> TwinDir {
        require_tools();
        let dir = if self.outside {
            self.build_outside(twin)
        } else {
            self.build_repo(twin)
        };
        self.assert_toplevel(&dir);
        if !self.outside {
            let expected = dir.root.join(&self.toplevel);
            let state = state_of(&dir, &expected);
            let first = self.start.get_or_init(|| state.clone());
            assert_eq!(
                *first, state,
                "scenario {}: the twins start from different repositories",
                self.name
            );
        }
        dir
    }

    fn fake_root(&self, base: &Path) -> (PathBuf, PathBuf, Option<FakeGh>) {
        let fake = base.join("fake");
        std::fs::create_dir_all(fake.join("bin")).expect("create the fake bin");
        std::fs::create_dir_all(fake.join("docs-calls")).expect("create docs-calls");
        write_file(
            &fake.join("bin/aios-fake-docs"),
            FAKE_DOCS.as_bytes(),
            0o755,
        );
        for (name, script) in &self.fake_tools {
            write_file(&fake.join("bin").join(name), script.as_bytes(), 0o755);
        }
        if let Some((out, err, rc)) = &self.docs {
            write_file(&fake.join("docs/out"), out, 0o644);
            write_file(&fake.join("docs/err"), err, 0o644);
            write_file(&fake.join("docs/rc"), format!("{rc}\n").as_bytes(), 0o644);
        }
        if self.docs_stale {
            write_file(&fake.join("docs/stale"), b"", 0o644);
        }
        let gh = (!self.gh_missing).then(|| {
            let gh = FakeGh::install(&fake);
            for spec in &self.gh {
                let fixture = gh.fixture(&spec.label, &spec.regex);
                fixture.out(&spec.out).err(&spec.err);
                if let Some(rc) = spec.rc {
                    fixture.rc(rc);
                }
                for (k, out, err, rc) in &spec.nth {
                    fixture.nth(*k, out, err, *rc);
                }
            }
            gh
        });
        let jail_bin = jail::bin(&base.join("jail"), jail::ORACLE_TOOLS);
        (fake, jail_bin, gh)
    }

    fn build_outside(&self, twin: Twin) -> TwinDir {
        static N: AtomicUsize = AtomicUsize::new(0);
        let parent = std::env::temp_dir()
            .canonicalize()
            .expect("canonicalise the system temp directory")
            .join(format!(
                "aios-outside-{}-{}-{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed),
                twin.label()
            ));
        let _ = std::fs::remove_dir_all(&parent);
        let run = parent.join("run");
        std::fs::create_dir_all(&run).expect("create the outside run directory");
        let (fake, jail_bin, gh) = self.fake_root(&parent);
        TwinDir {
            base: parent.clone(),
            root: run.clone(),
            origin: parent.join("no-origin"),
            run_dir: run,
            fake,
            jail_bin,
            gh,
            outside_parent: Some(parent),
        }
    }

    fn build_repo(&self, twin: Twin) -> TwinDir {
        let base = unique_dir(&format!("agent-{}-{}", self.name, twin.label()))
            .canonicalize()
            .expect("canonicalise the twin directory");
        let root = base.join("root");
        let origin = base.join("origin.git");
        std::fs::create_dir_all(&root).expect("create the twin root");
        std::fs::create_dir_all(&origin).expect("create the origin");
        let format: &[&str] = if self.reftable {
            &["--ref-format=reftable"]
        } else {
            &[]
        };
        setup_git(
            &origin,
            0,
            &[&["init", "-q", "--bare"], format, &["-b", "main"]].concat(),
        );
        setup_git(&origin, 0, &["config", "core.logAllRefUpdates", "always"]);
        setup_git(
            &root,
            0,
            &[&["init", "-q"], format, &["-b", "main"]].concat(),
        );
        let origin_str = origin.to_str().expect("a UTF-8 origin path");
        setup_git(&root, 0, &["remote", "add", "origin", origin_str]);

        write_file(
            &root.join(".gitignore"),
            b".remember/\n.claude/worktrees/\ntarget/\n",
            0o644,
        );
        write_file(
            &root.join(".claude/hooks/aios"),
            TWIN_STUB.as_bytes(),
            0o755,
        );
        if self.qemu_lock {
            let lock = std::fs::read(oracle_script(Which::QemuLock)).expect("read qemu-lock.sh");
            write_file(&root.join("scripts/agent/qemu-lock.sh"), &lock, 0o755);
        }
        setup_git(&root, 0, &["add", "-A"]);
        setup_git(&root, 0, &["commit", "-q", "-m", "Initial"]);
        if self.origin_main {
            setup_git(&root, 0, &["push", "-q", "-u", "origin", "main"]);
        }

        let mut n = 0u64;
        for step in &self.steps {
            match step {
                Step::Write { rel, bytes, mode } => write_file(&root.join(rel), bytes, *mode),
                Step::WriteRevs { rel, template } => {
                    let text = expand_revs(&root, n, template);
                    write_file(&root.join(rel), text.as_bytes(), 0o644);
                }
                Step::Remove { rel } => {
                    std::fs::remove_file(root.join(rel)).expect("remove a scenario file");
                }
                Step::Mtime { rel, secs } => {
                    let file = std::fs::File::open(root.join(rel)).expect("open for mtime");
                    file.set_modified(UNIX_EPOCH + Duration::from_secs(*secs))
                        .expect("set the mtime");
                }
                Step::Git { dir, args } => {
                    let args: Vec<&str> = args.iter().map(String::as_str).collect();
                    setup_git(&root.join(dir), n, &args);
                }
                Step::Commit { dir, message } => {
                    n += 1;
                    let at = root.join(dir);
                    setup_git(&at, n, &["add", "-A"]);
                    setup_git(&at, n, &["commit", "-q", "--allow-empty", "-m", message]);
                }
            }
        }

        let (fake, jail_bin, gh) = self.fake_root(&base);
        TwinDir {
            run_dir: root.join(&self.run_cwd),
            base,
            root,
            origin,
            fake,
            jail_bin,
            gh,
            outside_parent: None,
        }
    }

    /// `git rev-parse --show-toplevel` from the run directory, under the runner's own
    /// environment, must be the expected root: a scenario can never run against the
    /// enclosing aios checkout. An outside-repo run must find no repository at all.
    fn assert_toplevel(&self, twin: &TwinDir) {
        let mut cmd = Command::new(twin.jail_bin.join("git"));
        jail::hermetic(&mut cmd, &twin.fake_bin(), &twin.jail_bin);
        if let Some(parent) = &twin.outside_parent {
            cmd.env("GIT_CEILING_DIRECTORIES", parent);
        }
        let out = cmd
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(&twin.run_dir)
            .stdin(Stdio::null())
            .output()
            .expect("run git");
        if self.outside {
            assert!(
                !out.status.success(),
                "scenario {}: {} is inside a repository ({})",
                self.name,
                twin.run_dir.display(),
                String::from_utf8_lossy(&out.stdout)
            );
            return;
        }
        assert!(
            out.status.success(),
            "scenario {}: no repository at {}: {}",
            self.name,
            twin.run_dir.display(),
            String::from_utf8_lossy(&out.stderr)
        );
        let found = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim_end());
        let expected = twin.root.join(&self.toplevel);
        assert_eq!(
            found.canonicalize().expect("canonicalise the toplevel"),
            expected
                .canonicalize()
                .expect("canonicalise the expected root"),
            "scenario {}: the run directory resolves to the wrong toplevel",
            self.name
        );
    }

    /// The program's command inside `twin`: `hermetic`, then the pinned identity and
    /// dates, then the scenario's variables.
    ///
    /// The identity env comes after `hermetic` because `hermetic` keeps
    /// `isolated()`'s `GIT_CONFIG_GLOBAL=/dev/null`: with no config, checkpoint's
    /// `wip` commit would fail on a CI runner (no user.email) and record
    /// `<user>@<host>` on a developer machine.
    fn twin_command(&self, twin: &TwinDir, program: &OsStr, aios: bool) -> Command {
        let mut cmd = Command::new(program);
        jail::hermetic(&mut cmd, &twin.fake_bin(), &twin.jail_bin);
        pin_identity(&mut cmd, RUN_DATE);
        cmd.env(REAL_BIN_ENV, &twin.jail_bin);
        let root = twin.root.to_str().expect("a UTF-8 root path");
        let vars = self
            .env
            .iter()
            .chain(if aios { &self.aios_env[..] } else { &[] });
        for (key, value) in vars {
            cmd.env(key, value.replace("{root}", root));
        }
        if aios && self.sub == Sub::Brief {
            cmd.env("AIOS_BRIEF_DOCS_CHECK", "aios-fake-docs");
        }
        if let Some(parent) = &twin.outside_parent {
            cmd.env("GIT_CEILING_DIRECTORIES", parent);
        }
        cmd.current_dir(&twin.run_dir).stdin(Stdio::null());
        cmd
    }
}

/// What the runner saw of the repository around a run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RepoState {
    pub origin_refs_before: BTreeMap<String, String>,
    pub origin_refs_after: BTreeMap<String, String>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    /// `git write-tree` before and after: a checkpoint stop restores the index
    /// byte for byte.
    pub index_tree_before: Option<String>,
    pub index_tree_after: Option<String>,
    pub status_after: String,
}

/// One run: exit code, output, the fake gh and docs-check call logs, and the
/// repository state around it. It owns the twin, which lives as long as the outcome.
pub struct Outcome {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub gh_calls: Vec<GhCall>,
    pub docs_calls: Vec<DocsCall>,
    pub repo_state: RepoState,
    pub fake_stderr: Vec<u8>,
    pub expect: Expect,
    twin: TwinDir,
    twin_kind: Twin,
}

impl Outcome {
    pub fn twin(&self) -> &TwinDir {
        &self.twin
    }

    pub fn twin_kind(&self) -> Twin {
        self.twin_kind
    }

    pub fn stdout_str(&self) -> String {
        String::from_utf8_lossy(&self.stdout).into_owned()
    }

    pub fn stderr_str(&self) -> String {
        String::from_utf8_lossy(&self.stderr).into_owned()
    }
}

/// Run the oracle script in the oracle twin: `<bash> <script> args`.
pub fn run_oracle(scenario: &Scenario, args: &[&str]) -> Outcome {
    run_oracle_in(scenario, Twin::Oracle, args)
}

/// Run the oracle script in either twin (the identity test runs it in both).
pub fn run_oracle_in(scenario: &Scenario, twin: Twin, args: &[&str]) -> Outcome {
    let tools = require_tools();
    run_oracle_with(scenario, twin, &tools.bash, args)
}

/// The same with an explicit `bash` (the 3.2 differential runs the oracle under
/// `/bin/bash`).
pub fn run_oracle_with(scenario: &Scenario, twin: Twin, bash: &Path, args: &[&str]) -> Outcome {
    require_tools();
    let dir = scenario.prepare(twin);
    let mut full = vec![oracle_script(scenario.sub.oracle()).into_os_string()];
    full.extend(args.iter().map(OsString::from));
    execute(scenario, dir, twin, bash.as_os_str(), &full, false)
}

/// Run the aios binary under test in the aios twin: `aios <sub> args`.
pub fn run_aios(scenario: &Scenario, sub: &str, args: &[&str]) -> Outcome {
    let dir = scenario.prepare(Twin::Aios);
    let mut full = vec![OsString::from(sub)];
    full.extend(args.iter().map(OsString::from));
    execute(
        scenario,
        dir,
        Twin::Aios,
        OsStr::new(env!("CARGO_BIN_EXE_aios")),
        &full,
        true,
    )
}

fn execute(
    scenario: &Scenario,
    twin: TwinDir,
    kind: Twin,
    program: &OsStr,
    args: &[OsString],
    aios: bool,
) -> Outcome {
    let before = (!scenario.outside).then(|| state_of(&twin, &twin.root.join(&scenario.toplevel)));
    let mut cmd = scenario.twin_command(&twin, program, aios);
    cmd.args(args);
    let out = cmd.output().expect("run the program");
    let after = (!scenario.outside).then(|| state_of(&twin, &twin.root.join(&scenario.toplevel)));
    let repo_state = match (before, after) {
        (Some(b), Some(a)) => RepoState {
            origin_refs_before: b.origin_refs,
            origin_refs_after: a.origin_refs,
            head_before: b.head,
            head_after: a.head,
            index_tree_before: b.index_tree,
            index_tree_after: a.index_tree,
            status_after: a.status,
        },
        _ => RepoState::default(),
    };
    let (gh_calls, fake_stderr) = twin
        .gh
        .as_ref()
        .map(|gh| (gh.calls(), gh.fake_stderr()))
        .unwrap_or_default();
    let docs_calls = docs_calls(&twin.fake);
    Outcome {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
        gh_calls,
        docs_calls,
        repo_state,
        fake_stderr,
        expect: scenario.expect,
        twin,
        twin_kind: kind,
    }
}

/// The count of `\n## ` section headings on stdout.
pub fn section_count(stdout: &[u8]) -> usize {
    stdout.windows(4).filter(|w| *w == b"\n## ").count()
}

/// A run proves something only if the program ran for real: every fake `gh` call
/// matched a fixture, `gh` was called when the scenario expects it, no command was
/// missing from the jail, and (brief) the docs-check child was called and the report
/// has sections.
pub fn assert_not_vacuous(outcome: &Outcome) {
    let expect = outcome.expect;
    let gh_dir = outcome.twin.fake_gh();
    if let Some(gh) = gh_dir {
        gh.assert_all_matched();
    }
    if expect.gh {
        assert!(
            !outcome.gh_calls.is_empty(),
            "the scenario expects gh calls but the fake saw none\nstdout:\n{}\nstderr:\n{}",
            outcome.stdout_str(),
            outcome.stderr_str()
        );
    }
    jail::assert_no_command_not_found(&outcome.stderr);
    jail::assert_no_command_not_found(&outcome.fake_stderr);
    if let Some(n) = expect.docs_calls {
        assert_eq!(
            outcome.docs_calls.len(),
            n,
            "docs-check calls: {:?}",
            outcome.docs_calls
        );
    }
    if expect.sub == Sub::Brief {
        let sections = section_count(&outcome.stdout);
        assert!(
            sections >= expect.min_sections,
            "{sections} sections, expected at least {}:\n{}",
            expect.min_sections,
            outcome.stdout_str()
        );
    }
}

/// The oracle's and aios's reports have the same sections.
pub fn assert_sections_equal(oracle: &Outcome, aios: &Outcome) {
    assert_eq!(
        section_count(&oracle.stdout),
        section_count(&aios.stdout),
        "oracle and aios print a different number of sections"
    );
}

/// No scenario pushes anything that should not reach origin: `refs/heads/main` is
/// unchanged, every origin reflog entry is a fast-forward (its old sha is zero or an
/// ancestor of its new one), and every fake-gh call passes the sub's allowlist.
pub fn assert_origin_invariants(outcome: &Outcome) {
    let state = &outcome.repo_state;
    assert_eq!(
        state.origin_refs_before.get("refs/heads/main"),
        state.origin_refs_after.get("refs/heads/main"),
        "a run moved origin's refs/heads/main"
    );
    let origin = outcome.twin.origin();
    let mut logs = Vec::new();
    let mut pending = vec![origin.join("logs")];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                logs.push(path);
            }
        }
    }
    let zero = "0".repeat(40);
    for log in logs {
        let text = std::fs::read_to_string(&log).expect("read an origin reflog");
        for line in text.lines() {
            let mut fields = line.split(' ');
            let (Some(old), Some(new)) = (fields.next(), fields.next()) else {
                continue;
            };
            if old == zero {
                continue;
            }
            let mut cmd = Command::new("git");
            isolated(&mut cmd);
            let ancestor = cmd
                .args(["merge-base", "--is-ancestor", old, new])
                .current_dir(origin)
                .status()
                .expect("run git")
                .success();
            assert!(
                ancestor,
                "{}: {old} -> {new} is not a fast-forward",
                log.display()
            );
        }
    }
    if let Some(gh) = outcome.twin.fake_gh() {
        gh.assert_calls_match(outcome.expect.sub.allow());
    }
}

/// `normalize()`: replace exactly these named rules and compare everything else raw.
///
/// - `root`: the twin's root, as given and canonicalised, becomes `<ROOT>`;
/// - `origin`: the twin's origin becomes `<ORIGIN>`;
/// - `header_date`: the brief's `# AIOS brief - ...` heading;
/// - `git_cr`: `(%cr)`, git's relative commit date;
/// - `since_7_days`: the date after `since 7 days ago (no handoff or marker yet):`.
pub fn normalize(text: &[u8], outcome: &Outcome) -> Vec<u8> {
    let twin = &outcome.twin;
    let mut out = text.to_vec();
    for (path, label) in [(&twin.root, "<ROOT>"), (&twin.origin, "<ORIGIN>")] {
        let canonical = path.canonicalize().unwrap_or_else(|_| path.clone());
        for form in [path, &canonical] {
            out = replace_bytes(&out, form.to_string_lossy().as_bytes(), label.as_bytes());
        }
    }
    let unit = "(?:seconds?|minutes?|hours?|days?|weeks?|months?|years?)";
    let rules = [
        (
            r"(?m)^# AIOS brief - .*$".to_string(),
            "# AIOS brief - <DATE>".to_string(),
        ),
        (
            format!(r"\(\d+ {unit}(?:, \d+ {unit})? ago\)"),
            "(<AGO>)".to_string(),
        ),
        (
            r"(since 7 days ago \(no handoff or marker yet\): )\d{4}-\d{2}-\d{2} \d{2}:\d{2}"
                .to_string(),
            "${1}<DATE>".to_string(),
        ),
    ];
    for (pattern, replacement) in rules {
        let re = Regex::new(&pattern).expect("a valid normalisation rule");
        out = re.replace_all(&out, replacement.as_bytes()).into_owned();
    }
    out
}

fn replace_bytes(haystack: &[u8], needle: &[u8], with: &[u8]) -> Vec<u8> {
    if needle.is_empty() {
        return haystack.to_vec();
    }
    let mut out = Vec::with_capacity(haystack.len());
    let mut i = 0;
    while i < haystack.len() {
        if haystack[i..].starts_with(needle) {
            out.extend_from_slice(with);
            i += needle.len();
        } else {
            out.push(haystack[i]);
            i += 1;
        }
    }
    out
}

/// Apply the documented renames to the oracle's output, and nothing else:
/// D1, the message prefixes (`brief.sh:` and `checkpoint.sh:` become `brief:` and
/// `checkpoint:`), and D5, the `checkpoint it:` hint
/// (`bash '<script>'` becomes `just --justfile '<root>/justfile' --working-directory .
/// checkpoint`).
pub fn oracle_renames(text: &[u8], oracle: &Outcome) -> Vec<u8> {
    let root = oracle.twin.root.to_string_lossy().into_owned();
    let rules = [
        (r"(?m)^brief\.sh: ".to_string(), "brief: ".to_string()),
        (
            r"(?m)^checkpoint\.sh: ".to_string(),
            "checkpoint: ".to_string(),
        ),
        (
            r"(?m)^(\s*checkpoint it: \(cd '[^']*' && )bash '[^']*'\)".to_string(),
            format!(
                "${{1}}just --justfile '{}/justfile' --working-directory . checkpoint)",
                root.replace('$', "$$")
            ),
        ),
    ];
    let mut out = text.to_vec();
    for (pattern, replacement) in rules {
        let re = Regex::new(&pattern).expect("a valid rename rule");
        out = re.replace_all(&out, replacement.as_bytes()).into_owned();
    }
    out
}

/// `fake_gh::line_allowed` for a sub's allowlist.
pub fn gh_line_allowed(sub: Sub, line: &str) -> bool {
    fake_gh::line_allowed(sub.allow(), line)
}

// --- the golden helpers of brief_parity.rs and checkpoint_parity.rs ---------------------

/// Whether the scenario's golden is aios-only (no oracle run records it).
pub fn is_divergent(name: &str) -> bool {
    DIVERGENT_SCENARIOS
        .iter()
        .any(|(divergent, _)| *divergent == name)
}

/// `git <args>` in `dir`, for what a golden records about the repository after the run.
/// `None` when git fails (no repository, an unborn HEAD). The ceiling keeps a run
/// directory outside every checkout from finding one above it.
pub fn probe(dir: &Path, ceiling: &Path, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("git");
    isolated(&mut cmd)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args)
        .current_dir(dir)
        .stdin(Stdio::null());
    let out = cmd.output().expect("run git");
    out.status.success().then(|| {
        String::from_utf8_lossy(&out.stdout)
            .trim_end_matches('\n')
            .to_string()
    })
}

/// One `== <title> ==` section of a golden.
pub fn golden_section(out: &mut Vec<u8>, title: &str, body: &[u8]) {
    out.extend_from_slice(format!("== {title} ==\n").as_bytes());
    out.extend_from_slice(body);
    if !body.is_empty() && !body.ends_with(b"\n") {
        out.extend_from_slice(b"\n\\ no newline at end of output\n");
    }
}

/// The body of a golden's `gh calls` section: the rendered argv, the cwd and the
/// environment of every fake-gh call, or `(none)`.
pub fn gh_calls_text(outcome: &Outcome) -> String {
    let mut text = String::new();
    for (i, call) in outcome.gh_calls.iter().enumerate() {
        let env: Vec<String> = call
            .env
            .iter()
            .map(|(k, v)| format!("{k}={}", v.as_deref().unwrap_or("<unset>")))
            .collect();
        text.push_str(&format!(
            "{}: {}\n   cwd {}\n   env {}\n",
            i + 1,
            call.line().trim_end(),
            call.cwd.display(),
            env.join(" ")
        ));
    }
    if text.is_empty() {
        text.push_str("(none)\n");
    }
    text
}

/// The first line where two goldens differ.
pub fn first_difference(want: &str, got: &str) -> String {
    let want: Vec<&str> = want.split('\n').collect();
    let got: Vec<&str> = got.split('\n').collect();
    for i in 0..want.len().max(got.len()) {
        let (w, g) = (want.get(i).copied(), got.get(i).copied());
        if w != g {
            return format!(
                "line {}: expected {:?}, got {:?}",
                i + 1,
                w.unwrap_or("<end of output>"),
                g.unwrap_or("<end of output>")
            );
        }
    }
    "the goldens differ".to_string()
}
