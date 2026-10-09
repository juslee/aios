//! `.claude/hooks/aios` and the justfile's `tools` recipe: freshness and the
//! provenance stamp, the guard branch's fail-closed paths (#203), foreground and
//! background builds, `AIOS_TOOLS_BIN` and linked worktrees (with and without a
//! working git). Builds run the real `tools` recipe through `just` with a fake
//! `cargo`, so the stamp the recipe writes is the one the shim checks.

mod common;

use common::{isolated, unique_dir, TestRepo};

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant, SystemTime};

/// The PreToolUse decision the shim prints when the binary is missing, as R1
/// shipped it.
const ASK_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"aios tools not built; run just tools"}}"#;

/// The guard branch's ask reasons, one per path.
const NOT_BUILT: &str = "aios tools not built; run just tools";
const STALE: &str = "aios tools are stale or unverified; rebuilding in the background (just tools)";
const STALE_RUNNING: &str =
    "aios tools are stale or unverified; a background build (just tools) is already running";
const STALE_NO_BUILD: &str =
    "aios tools are stale or unverified, and a background build could not start; run just tools";
const UNCOMMITTED: &str = "aios tools were built from uncommitted, untracked or gitignored input files in the main checkout; revert or remove them (merge any you need through a PR first), then run just tools";
const HIDDEN: &str = "aios tools were built while assume-unchanged or skip-worktree flags in the main checkout's index hid input files from git status; clear the flags (git ls-files -v shows them as lowercase tags or S), then run just tools";
const DIRTY: &str = "aios tools were built from input commits in the main checkout that are not on origin/main; once they have merged through a PR, or if they are not wanted, reset the main checkout onto origin/main (git reset --keep origin/main), then run just tools";
const BAD_OVERRIDE: &str = "AIOS_TOOLS_BIN is not a non-empty regular executable file";
const NO_ORIGIN: &str = "aios tools were built with no origin/main to check their inputs against; fetch main from origin into refs/remotes/origin/main, then run just tools";
const NO_OWN_DIR: &str =
    "aios shim cannot find its own directory; check that .claude/hooks exists and is accessible";
const NO_MAIN: &str =
    "aios shim cannot find the main checkout (git failed); fix git or set AIOS_TOOLS_BIN";
const NO_MAIN_DIRNAME: &str =
    "aios shim cannot find the main checkout (dirname failed on the git common dir); retry";
const NO_MAIN_ROOT: &str = "aios shim cannot find the main checkout (git failed and the checkout root is not accessible); fix git or the checkout's permissions";

fn failed(status: i32) -> String {
    format!("aios guard failed (exit {status}); run just tools")
}

fn override_failed(status: i32) -> String {
    format!("AIOS_TOOLS_BIN guard failed (exit {status}); fix or unset AIOS_TOOLS_BIN")
}

fn ask_json(reason: &str) -> String {
    format!(
        "{{\"hookSpecificOutput\":{{\"hookEventName\":\"PreToolUse\",\"permissionDecision\":\"ask\",\"permissionDecisionReason\":\"{reason}\"}}}}\n"
    )
}

/// A `cargo` that only knows the `tools` recipe's build: it logs the build and
/// writes target/tools/release/aios, by default a binary that echoes its
/// arguments and exits `FAKE_EXIT`. Its progress line goes to stdout, so the
/// tests see whether the shim keeps build output off its own stdout.
/// `FAKE_CARGO_FAIL` makes it fail, `FAKE_CARGO_DELAY` slows it down,
/// `FAKE_CARGO_NOOP` leaves release/aios alone (a no-op build on Linux) and
/// `FAKE_CARGO_SOURCE` names a file to build instead. `FAKE_CARGO_WRITE_DELAY`
/// leaves release/aios half-written for that many seconds, like the uplift of a
/// large binary, and creates `FAKE_CARGO_MARK` once the half is written.
/// `FAKE_CARGO_PIDS` names a file to write the recipe's pid and its own to.
/// `FAKE_CARGO_EXCLUSIVE` logs `overlap` to overlap.log when another build is
/// running at the same time.
const FAKE_CARGO: &str = r#"#!/bin/sh
set -u
if [ "$*" != "build --release -p aios-tools --target-dir target/tools" ]; then
    echo "fake cargo: unexpected arguments: $*" >&2
    exit 2
fi
echo "fake cargo: building the aios binary"
printf 'build\n' >> cargo.log
[ -z "${FAKE_CARGO_PIDS:-}" ] || echo "$PPID $$" > "$FAKE_CARGO_PIDS"
if [ -n "${FAKE_CARGO_EXCLUSIVE:-}" ]; then
    mkdir target/tools/.fake-cargo-busy 2>/dev/null || printf 'overlap\n' >> overlap.log
    trap 'rmdir target/tools/.fake-cargo-busy 2>/dev/null' EXIT
fi
if [ -n "${FAKE_CARGO_FAIL:-}" ]; then
    echo "fake cargo: the build failed" >&2
    exit 1
fi
sleep "${FAKE_CARGO_DELAY:-0}"
[ -z "${FAKE_CARGO_NOOP:-}" ] || exit 0
mkdir -p target/tools/release
if [ -n "${FAKE_CARGO_SOURCE:-}" ]; then
    cat "$FAKE_CARGO_SOURCE" > target/tools/aios.next
else
    cat > target/tools/aios.next <<'BIN'
#!/bin/sh
printf 'fake:%s\n' "$*"
exit ${FAKE_EXIT:-0}
BIN
fi
# Like cargo's uplift: remove the old file, then write the new one.
rm -f target/tools/release/aios
if [ -n "${FAKE_CARGO_WRITE_DELAY:-}" ]; then
    head -c 16 target/tools/aios.next > target/tools/release/aios
    chmod 755 target/tools/release/aios
    : > "$FAKE_CARGO_MARK"
    sleep "$FAKE_CARGO_WRITE_DELAY"
fi
cat target/tools/aios.next > target/tools/release/aios
rm -f target/tools/aios.next
chmod 755 target/tools/release/aios
"#;

/// A `cp` that leaves the destination half-written for `FAKE_CP_DELAY` seconds,
/// like a large binary in the middle of a copy, and creates `FAKE_CP_MARK` once
/// the half is written.
const SLOW_CP: &str = r#"#!/bin/sh
if [ -n "${FAKE_CP_DELAY:-}" ] && [ "$#" -eq 2 ]; then
    head -c 16 "$1" > "$2"
    : > "$FAKE_CP_MARK"
    sleep "$FAKE_CP_DELAY"
    cat "$1" > "$2"
    exit
fi
exec /bin/cp "$@"
"#;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tools crate has a parent directory")
}

fn repo_shim() -> PathBuf {
    repo_root().join(".claude/hooks/aios")
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn write_executable(path: &Path, content: &str) {
    std::fs::write(path, content).unwrap_or_else(|err| panic!("write {}: {err}", path.display()));
    make_executable(path);
}

fn make_executable(path: &Path) {
    let mut perms = std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("stat {}: {err}", path.display()))
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod 755");
}

/// `input` with an edit marked by `note`. The justfile keeps its recipes, since
/// the next build runs them.
fn edited(input: &str, note: &str) -> String {
    let edit = format!("# {note}: {input}\n");
    if input == "justfile" {
        read(&repo_root().join("justfile")) + &edit
    } else {
        edit
    }
}

/// A `touch -t` stamp newer than every file a test repository holds.
const FRESH_STAMP: &str = "209901010000";
/// A `touch -t` stamp older than every file a test repository holds.
const STALE_STAMP: &str = "200001010000";
/// A `touch -t` stamp newer than `FRESH_STAMP`.
const NEWER_STAMP: &str = "209912310000";

fn set_mtime(path: &Path, stamp: &str) {
    let status = Command::new("touch")
        .arg("-t")
        .arg(stamp)
        .arg(path)
        .status()
        .expect("run touch");
    assert!(
        status.success(),
        "touch -t {stamp} {} failed",
        path.display()
    );
}

/// Dates `path`, a file or a directory, `age` before now.
fn set_age(path: &Path, age: Duration) {
    std::fs::File::open(path)
        .and_then(|file| file.set_modified(SystemTime::now() - age))
        .unwrap_or_else(|err| panic!("set the mtime of {}: {err}", path.display()));
}

/// Older than the minute or two (`find -mmin +1`) after which the shim takes
/// a lock directory over, and young enough that a takeover wait of several
/// minutes fails the test.
const DEAD_LOCK_AGE: Duration = Duration::from_secs(180);

fn wait_for(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("timed out after 10 s waiting for {label}");
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the shim exited normally")
}

/// The shim exited 0 with exactly one PreToolUse "ask" decision for `reason`.
fn assert_asks(out: &Output, reason: &str) {
    assert_eq!(code(out), 0, "stderr: {}", stderr(out));
    assert_eq!(stdout(out), ask_json(reason));
    let decision: serde_json::Value =
        serde_json::from_str(&stdout(out)).expect("the ask decision is JSON");
    assert_eq!(
        decision["hookSpecificOutput"]["permissionDecision"], "ask",
        "{decision}"
    );
}

/// A main checkout holding the shim, the real justfile and the binary's build
/// inputs, with a fake `cargo` first on `PATH`.
struct Sandbox {
    repo: TestRepo,
    bin_dir: TestRepo,
}

impl Sandbox {
    fn new(label: &str) -> Sandbox {
        let repo = TestRepo::new(label);
        repo.write(".claude/hooks/aios", &read(&repo_shim()));
        make_executable(&repo.path().join(".claude/hooks/aios"));
        repo.write("justfile", &read(&repo_root().join("justfile")));
        repo.write(".gitignore", "target/\n*.log\n");
        repo.write(
            "tools/src/lib.rs",
            "// the shim only needs tools/ to exist\n",
        );
        repo.write("Cargo.lock", "# a build input of the aios binary\n");
        repo.write("Cargo.toml", "# a build input of the aios binary\n");
        repo.write(
            "rust-toolchain.toml",
            "# a build input of the aios binary\n",
        );
        repo.write(".cargo/config.toml", "# a build input of the aios binary\n");
        repo.commit("Initial");
        // The initial commit is merged: origin/main points at it.
        common::git(
            repo.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );

        let bin_dir = TestRepo::adopt(unique_dir(&format!("{label}-path")));
        write_executable(&bin_dir.path().join("cargo"), FAKE_CARGO);

        Sandbox { repo, bin_dir }
    }

    fn shim(&self) -> PathBuf {
        self.repo.path().join(".claude/hooks/aios")
    }

    fn bin(&self) -> PathBuf {
        self.repo.path().join("target/tools/installed/aios")
    }

    fn stamp(&self) -> PathBuf {
        self.repo.path().join("target/tools/installed/aios.stamp")
    }

    fn lock(&self) -> PathBuf {
        self.repo.path().join("target/tools/.building")
    }

    /// Fast-forwards origin/main to HEAD. This stands in for HEAD's changes
    /// merging through a PR and the main checkout then being reset onto the
    /// merged origin/main: a squash merge alone leaves HEAD's own commits off
    /// origin/main, so the dirty test keeps finding them.
    fn merge(&self) {
        common::git(
            self.repo.path(),
            &["update-ref", "refs/remotes/origin/main", "HEAD"],
        );
    }

    fn cargo_log(&self) -> PathBuf {
        self.repo.path().join("cargo.log")
    }

    fn built(&self) -> bool {
        self.cargo_log().exists()
    }

    /// No build ran and none is starting. `build_bg` takes the lock before the
    /// shim returns, but the fake cargo writes cargo.log only once `just` has
    /// started the recipe, so a background build shows in the lock first.
    fn no_build_started(&self) -> bool {
        !self.lock().exists() && !self.built()
    }

    fn path_env(&self) -> String {
        format!(
            "{}:{}",
            self.bin_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }

    /// Runs the real `tools` recipe through `just`, as the shim does.
    fn just_tools(&self, envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new("just");
        isolated(&mut cmd);
        cmd.env("PATH", self.path_env())
            .current_dir(self.repo.path())
            .arg("tools");
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.output()
            .expect("run just (the tests build through the real justfile recipe)")
    }

    /// Builds `source` (the default fake binary when `None`) through the recipe,
    /// forgets the build, and moves the binary's mtime after every input
    /// (fresh) or before them (stale by mtime; the stamp still matches).
    fn install(&self, source: Option<&str>, fresh: bool) {
        let source_file = self.bin_dir.path().join("aios-source");
        let mut envs = Vec::new();
        if let Some(source) = source {
            std::fs::write(&source_file, source).expect("write the binary source");
            envs.push((
                "FAKE_CARGO_SOURCE",
                source_file.to_str().expect("a UTF-8 path"),
            ));
        }
        let out = self.just_tools(&envs);
        assert!(
            out.status.success(),
            "just tools failed: {}{}",
            stdout(&out),
            stderr(&out)
        );
        std::fs::remove_file(self.cargo_log()).expect("remove cargo.log");
        set_mtime(&self.bin(), if fresh { FRESH_STAMP } else { STALE_STAMP });
    }

    fn install_bin(&self, fresh: bool) {
        self.install(None, fresh);
    }

    fn run_at(&self, shim: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(shim);
        isolated(&mut cmd);
        cmd.env("PATH", self.path_env())
            .current_dir(self.repo.path())
            .args(args);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.output().expect("run the shim")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_at(&self.shim(), args, &[])
    }

    fn run_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        self.run_at(&self.shim(), args, envs)
    }

    fn wait_for_background_build(&self) {
        wait_for("the background build to log a line", || self.built());
        wait_for("the build lock to be released", || !self.lock().exists());
    }
}

#[test]
fn the_shim_is_posix_sh() {
    let out = Command::new("sh")
        .arg("-n")
        .arg(repo_shim())
        .output()
        .expect("run sh -n");
    assert!(
        out.status.success(),
        "sh -n rejected the shim: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn guard_without_a_binary_asks_and_does_not_build() {
    let sandbox = Sandbox::new("shim-guard-missing");
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_asks(&out, NOT_BUILT);
    assert_eq!(stdout(&out), format!("{ASK_JSON}\n"), "unchanged since R1");
    assert!(
        sandbox.no_build_started(),
        "the guard branch must not build"
    );
    assert!(!sandbox.bin().exists());
}

#[test]
fn a_missing_binary_is_built_then_run() {
    let sandbox = Sandbox::new("shim-missing");
    let out = sandbox.run(&["docs-check", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check --json\n");
    assert!(
        stderr(&out).contains("fake cargo: building the aios binary"),
        "build output belongs on stderr: {}",
        stderr(&out)
    );
    assert_eq!(read(&sandbox.cargo_log()), "build\n");
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
}

#[test]
fn a_missing_binary_with_a_failing_build_exits_3() {
    let sandbox = Sandbox::new("shim-build-fails");
    let out = sandbox.run_env(&["docs-check"], &[("FAKE_CARGO_FAIL", "1")]);
    assert_eq!(code(&out), 3);
    assert!(stderr(&out).contains("run: just tools"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

#[test]
fn a_fresh_binary_runs_without_building_and_passes_its_status_on() {
    let sandbox = Sandbox::new("shim-fresh");
    sandbox.install_bin(true);

    let out = sandbox.run(&["docs-check", "--all"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check --all\n");
    assert!(!sandbox.built(), "a fresh binary must not build");

    let out = sandbox.run_env(&["docs-check"], &[("FAKE_EXIT", "7")]);
    assert_eq!(code(&out), 7);
    assert!(!sandbox.built());

    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");
    assert!(sandbox.no_build_started());
}

#[test]
fn a_stale_binary_is_rebuilt_in_the_foreground() {
    let sandbox = Sandbox::new("shim-stale");
    sandbox.install_bin(false);
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert_eq!(read(&sandbox.cargo_log()), "build\n");
    assert!(!sandbox.lock().exists(), "a foreground build takes no lock");
}

/// The fallback spec §2 records for other subcommands, and the warning
/// `scripts/agent/brief.sh` greps for (`^aios: rebuilding .* failed; running
/// the stale binary`) to flag a drift summary from the stale binary.
#[test]
fn a_stale_binary_whose_rebuild_fails_warns_and_runs_it() {
    let sandbox = Sandbox::new("shim-stale-build-fails");
    sandbox.install_bin(false);
    let out = sandbox.run_env(
        &["docs-check", "--json"],
        &[("FAKE_CARGO_FAIL", "1"), ("FAKE_EXIT", "1")],
    );
    assert_eq!(code(&out), 1, "the stale binary's own status passes on");
    assert_eq!(stdout(&out), "fake:docs-check --json\n");
    let err = stderr(&out);
    assert!(
        err.lines().any(|line| line.starts_with("aios: rebuilding ")
            && line.ends_with(
                "/target/tools/installed/aios failed; running the stale binary (run: just tools)"
            )),
        "missing the stale-binary warning: {err}"
    );
    assert_eq!(read(&sandbox.cargo_log()), "build\n");
}

#[test]
fn a_newer_toolchain_pin_manifest_cargo_config_or_justfile_makes_the_binary_stale() {
    // A pull that only bumps the pinned nightly (or the workspace manifest, the
    // cargo config, or the justfile whose recipe builds and stamps the binary)
    // touches nothing under tools/ and not Cargo.lock, yet changes the build. A legacy rust-toolchain file, which rustup prefers
    // to rust-toolchain.toml, changes it too, even untracked.
    for (label, input) in [
        ("shim-stale-toolchain", "rust-toolchain.toml"),
        ("shim-stale-legacy-toolchain", "rust-toolchain"),
        ("shim-stale-manifest", "Cargo.toml"),
        ("shim-stale-cargo-config", ".cargo/config.toml"),
        ("shim-stale-justfile", "justfile"),
    ] {
        let sandbox = Sandbox::new(label);
        sandbox.install_bin(true);
        sandbox
            .repo
            .write(input, &edited(input, "an edited build input"));

        let out = sandbox.run(&["docs-check"]);
        assert_eq!(code(&out), 0);
        assert!(!sandbox.built(), "an older {input} must not rebuild");

        set_mtime(&sandbox.repo.path().join(input), NEWER_STAMP);
        let out = sandbox.run(&["docs-check"]);
        assert_eq!(code(&out), 0);
        assert_eq!(stdout(&out), "fake:docs-check\n");
        assert_eq!(
            read(&sandbox.cargo_log()),
            "build\n",
            "a {input} newer than the binary must rebuild it"
        );
        assert!(
            read(&sandbox.stamp()).ends_with("\nsource dirty\n"),
            "the uncommitted {input} makes the build dirty"
        );
    }
}

#[test]
fn a_stale_guard_asks_at_once_and_rebuilds_in_the_background() {
    let sandbox = Sandbox::new("shim-stale-guard");
    sandbox.install_bin(false);
    let started = Instant::now();
    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("FAKE_CARGO_DELAY", "3")]);
    assert_asks(&out, STALE);
    assert!(stderr(&out).is_empty(), "{}", stderr(&out));
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "guard must not wait for the background build"
    );
    assert!(
        sandbox.lock().exists(),
        "a background build takes the lock before the shim returns"
    );
    sandbox.wait_for_background_build();
    assert_eq!(read(&sandbox.cargo_log()), "build\n");

    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n", "rebuilt: fresh");
}

// A background build killed with its subshell leaves the lock directory
// behind. guard takes it over once it is a minute or two old and no recipe
// holds the recipe's lock; until then, and while a recipe holds it, the ask
// says that a build is already running rather than that one started.
#[test]
fn a_dead_background_build_lock_is_taken_over() {
    let sandbox = Sandbox::new("shim-dead-lock");
    sandbox.install_bin(false);
    std::fs::create_dir(sandbox.lock()).expect("create the lock directory");

    // A young lock: its build may still be starting.
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE_RUNNING);
    assert!(sandbox.lock().exists());
    assert!(!sandbox.built(), "no second build");

    // An old lock while a recipe holds the recipe's lock: still running.
    set_age(&sandbox.lock(), DEAD_LOCK_AGE);
    let mut cmd = Command::new("just");
    isolated(&mut cmd);
    let mut recipe = cmd
        .env("PATH", sandbox.path_env())
        .env("FAKE_CARGO_DELAY", "3")
        .current_dir(sandbox.repo.path())
        .arg("tools")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run just tools");
    wait_for("the recipe's cargo to start", || sandbox.built());
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE_RUNNING);
    assert!(sandbox.lock().exists(), "a running recipe's build keeps it");
    assert!(recipe.wait().expect("wait for just tools").success());

    // An old lock and no recipe: dead, so guard takes it over and builds.
    std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
    set_mtime(&sandbox.bin(), STALE_STAMP);
    set_age(&sandbox.lock(), DEAD_LOCK_AGE);
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert_eq!(read(&sandbox.cargo_log()), "build\n");
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
}

#[test]
fn a_stale_guard_says_when_no_background_build_can_start() {
    let sandbox = Sandbox::new("shim-no-background-build");
    sandbox.install_bin(false);
    // A file at the lock path, which neither rmdir nor mkdir replaces: no
    // background build can start.
    std::fs::write(sandbox.lock(), "").expect("write a file at the lock path");
    set_age(&sandbox.lock(), DEAD_LOCK_AGE);
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE_NO_BUILD);
    assert!(!sandbox.built());
    let out = sandbox.run(&["--prebuild"]);
    assert_eq!(code(&out), 0);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}

/// A directory of links to the named commands as the test's `PATH` finds
/// them, for a `PATH` that holds only those. A name it does not find is left
/// out.
fn only_commands(label: &str, names: &[&str]) -> TestRepo {
    let dir = TestRepo::adopt(unique_dir(label));
    for name in names {
        let out = Command::new("sh")
            .arg("-c")
            .arg("command -v \"$1\"")
            .arg("sh")
            .arg(name)
            .output()
            .expect("run sh");
        let found = String::from_utf8_lossy(&out.stdout).trim().to_owned();
        if out.status.success() && found.starts_with('/') {
            std::os::unix::fs::symlink(&found, dir.path().join(name))
                .unwrap_or_else(|err| panic!("link {found}: {err}"));
        }
    }
    dir
}

// The recipe needs `just` to run it and flock(1) or lockf(1) for its lock.
// Without either, a background build would fail at once, so guard says that
// none could start rather than that one is rebuilding, and starts none.
#[test]
fn a_stale_guard_without_just_or_a_lock_tool_starts_no_build() {
    let sandbox = Sandbox::new("shim-no-just");
    sandbox.install_bin(false);
    let shim_needs = [
        "dirname", "git", "find", "head", "cat", "sed", "mkdir", "rmdir",
    ];
    let no_just = only_commands(
        "shim-no-just-path",
        &[&shim_needs[..], &["flock", "lockf"]].concat(),
    );
    let no_lock_tool = only_commands(
        "shim-no-lock-tool-path",
        &[&shim_needs[..], &["just"]].concat(),
    );
    assert!(
        no_lock_tool.path().join("just").exists(),
        "the tests need just on PATH"
    );
    for dir in [&no_just, &no_lock_tool] {
        let path = format!(
            "{}:{}",
            sandbox.bin_dir.path().display(),
            dir.path().display()
        );
        assert_asks(
            &sandbox.run_env(&["guard", "PreToolUse"], &[("PATH", &path)]),
            STALE_NO_BUILD,
        );
        assert!(sandbox.no_build_started());
        let out = sandbox.run_env(&["--prebuild"], &[("PATH", &path)]);
        assert_eq!(code(&out), 0);
        assert!(stdout(&out).is_empty(), "{}", stdout(&out));
        assert!(sandbox.no_build_started());
    }
}

#[test]
fn prebuild_returns_at_once_and_builds_in_the_background() {
    let sandbox = Sandbox::new("shim-prebuild");
    let started = Instant::now();
    let out = sandbox.run_env(&["--prebuild"], &[("FAKE_CARGO_DELAY", "3")]);
    assert_eq!(code(&out), 0);
    assert!(
        started.elapsed() < Duration::from_secs(3),
        "--prebuild must not wait for the build"
    );
    assert!(
        sandbox.lock().exists(),
        "a background build takes the lock before the shim returns"
    );
    sandbox.wait_for_background_build();
    assert!(sandbox.bin().exists() && sandbox.stamp().exists());
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");
}

// #203 path 1: `[ -x ]` alone is true for a directory, which exec cannot run.
#[test]
fn a_directory_or_an_empty_file_is_not_a_binary() {
    let sandbox = Sandbox::new("shim-directory");
    let dir = sandbox.bin_dir.path().join("a-directory");
    std::fs::create_dir(&dir).expect("create the directory");
    let dir = dir.to_str().expect("a UTF-8 path");
    let empty = sandbox.bin_dir.path().join("empty-aios");
    write_executable(&empty, "");
    let empty = empty.to_str().expect("a UTF-8 path");

    for over in [dir, empty] {
        let out = sandbox.run_env(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", over)]);
        assert_asks(&out, BAD_OVERRIDE);

        let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", over)]);
        assert_eq!(code(&out), 3);
        assert!(stdout(&out).is_empty(), "{}", stdout(&out));
        assert!(
            stderr(&out).contains("is not a non-empty regular executable file"),
            "{}",
            stderr(&out)
        );
    }

    // The same test for the main checkout's binary.
    std::fs::create_dir_all(sandbox.bin()).expect("create a directory at the binary path");
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_asks(&out, NOT_BUILT);
    assert!(sandbox.no_build_started());
}

// #203 path 2, shim side: a guard binary that fails to start (as a vanished or
// half-written file does) or is killed still ends in a decision.
#[test]
fn a_guard_binary_that_cannot_start_or_is_killed_asks() {
    let sandbox = Sandbox::new("shim-guard-start-fails");
    // A missing interpreter fails at exec like a missing file: 126 under bash
    // (macOS /bin/sh), 127 under dash.
    sandbox.install(Some("#!/nonexistent/interpreter\n"), true);
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert!(
        [126, 127]
            .iter()
            .any(|n| stdout(&out) == ask_json(&failed(*n))),
        "{}",
        stdout(&out)
    );

    sandbox.install(Some("#!/bin/sh\nprintf 'partial'\nkill -9 $$\n"), true);
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_asks(&out, &failed(137));
    assert!(sandbox.no_build_started(), "both binaries were fresh");
}

// #203 path 2: the binary's own exits other than 0 and 2 fail closed too.
#[test]
fn guard_passes_on_only_the_binarys_own_0_and_2() {
    let sandbox = Sandbox::new("shim-guard-exits");
    sandbox.install_bin(true);

    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("FAKE_EXIT", "2")]);
    assert_eq!(code(&out), 2, "exit 2 blocks the tool");
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");

    for status in [1, 3, 101, 126] {
        let value = status.to_string();
        let out = sandbox.run_env(&["guard", "PreToolUse"], &[("FAKE_EXIT", &value)]);
        // The binary's own "fake:" line is dropped: only the ask remains.
        assert_asks(&out, &failed(status));
    }

    // The same for an override, which also gets the hook payload on stdin.
    let echo = sandbox.bin_dir.path().join("echo-aios");
    write_executable(&echo, "#!/bin/sh\ncat\nexit ${FAKE_EXIT:-0}\n");
    let payload = r#"{"tool_name":"Bash","tool_input":{"command":"ls"}}"#;
    for (status, want) in [
        ("0", format!("{payload}\n")),
        ("1", ask_json(&override_failed(1))),
    ] {
        let mut cmd = Command::new(sandbox.shim());
        isolated(&mut cmd);
        let mut child = cmd
            .env("PATH", sandbox.path_env())
            .env("AIOS_TOOLS_BIN", &echo)
            .env("FAKE_EXIT", status)
            .current_dir(sandbox.repo.path())
            .args(["guard", "PreToolUse"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run the shim");
        child
            .stdin
            .take()
            .expect("a piped stdin")
            .write_all(payload.as_bytes())
            .expect("write the hook payload");
        let out = child.wait_with_output().expect("wait for the shim");
        assert_eq!(code(&out), 0);
        assert_eq!(stdout(&out), want, "binary exit {status}");
    }
}

// #203 path 2, build side: `just tools` never leaves the binary missing or
// half-written, and does not depend on the file cargo rewrites.
#[test]
fn just_tools_installs_by_rename_at_a_path_cargo_never_writes() {
    let sandbox = Sandbox::new("shim-install");
    sandbox.install_bin(true);
    let old = read(&sandbox.bin());
    let old_stamp = read(&sandbox.stamp());

    // cargo's uplift removes release/aios before it writes the new one.
    std::fs::remove_file(sandbox.repo.path().join("target/tools/release/aios"))
        .expect("remove release/aios");
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");

    // A failed build leaves the installed binary and its stamp alone.
    let out = sandbox.just_tools(&[("FAKE_CARGO_FAIL", "1")]);
    assert!(!out.status.success());
    assert_eq!(read(&sandbox.bin()), old);
    assert_eq!(read(&sandbox.stamp()), old_stamp);

    // A new version, copied slowly: while the copy is half-written, the shim
    // still runs the complete old binary.
    let slow = TestRepo::adopt(unique_dir("shim-install-cp"));
    write_executable(&slow.path().join("cp"), SLOW_CP);
    let mark = slow.path().join("half-copied");
    let v2 = sandbox.bin_dir.path().join("aios-v2");
    std::fs::write(&v2, "#!/bin/sh\nprintf 'v2:%s\\n' \"$*\"\n").expect("write v2");
    let mut cmd = Command::new("just");
    isolated(&mut cmd);
    let mut build = cmd
        .env(
            "PATH",
            format!("{}:{}", slow.path().display(), sandbox.path_env()),
        )
        .env("FAKE_CARGO_SOURCE", &v2)
        .env("FAKE_CP_DELAY", "2")
        .env("FAKE_CP_MARK", &mark)
        .current_dir(sandbox.repo.path())
        .arg("tools")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run just tools");
    wait_for("the recipe's copy to be half-written", || mark.exists());
    assert_eq!(
        read(&sandbox.bin()),
        old,
        "the installed binary must never be the half-written copy"
    );
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(code(&out), 0, "{}", stderr(&out));
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert!(build.wait().expect("wait for just tools").success());

    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(stdout(&out), "v2:guard PreToolUse\n");
    let installed = sandbox.bin().parent().expect("a parent").to_path_buf();
    assert!(
        std::fs::read_dir(&installed)
            .expect("list installed/")
            .filter_map(Result::ok)
            .all(|entry| !entry.file_name().to_string_lossy().starts_with(".aios.")),
        "the recipe leaves no temporary files"
    );
    assert!(
        std::fs::read_dir(sandbox.repo.path().join("target/tools"))
            .expect("list target/tools")
            .filter_map(Result::ok)
            .all(|entry| !entry
                .file_name()
                .to_string_lossy()
                .starts_with(".build-start.")),
        "the recipe removes its start file"
    );
}

// Overlapping recipes run one at a time: a recipe must not copy release/aios
// while another recipe's cargo build is rewriting it.
#[test]
fn overlapping_just_tools_runs_never_install_a_half_written_binary() {
    let sandbox = Sandbox::new("shim-install-overlap");
    sandbox.install_bin(true);

    // The first recipe's cargo leaves release/aios half-written for 2 s.
    let mark = sandbox.bin_dir.path().join("half-uplifted");
    let v2 = sandbox.bin_dir.path().join("aios-v2");
    std::fs::write(&v2, "#!/bin/sh\nprintf 'v2:%s\\n' \"$*\"\n").expect("write v2");
    let mut cmd = Command::new("just");
    isolated(&mut cmd);
    let mut first = cmd
        .env("PATH", sandbox.path_env())
        .env("FAKE_CARGO_SOURCE", &v2)
        .env("FAKE_CARGO_WRITE_DELAY", "2")
        .env("FAKE_CARGO_MARK", &mark)
        .current_dir(sandbox.repo.path())
        .arg("tools")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run just tools");
    wait_for("the first recipe's uplift to be half-written", || {
        mark.exists()
    });

    // The second recipe's build is a no-op, so without the lock it would copy
    // the half-written file at once and install it with a matching stamp. It
    // waits for the first recipe instead, so when it returns, the complete
    // binary is in place.
    let out = sandbox.just_tools(&[("FAKE_CARGO_NOOP", "1")]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert_eq!(read(&sandbox.bin()), read(&v2), "the complete binary");
    assert!(
        stderr(&out).contains("waiting for another just tools to finish"),
        "{}",
        stderr(&out)
    );
    assert!(first.wait().expect("wait for just tools").success());
    assert_eq!(read(&sandbox.bin()), read(&v2));
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "v2:guard PreToolUse\n"
    );
}

// The lock is a kernel file lock, so recipes never take one over: three
// recipes started together each build alone, and a lock file a recipe killed
// with SIGKILL leaves behind does not hold up the next one.
#[test]
fn concurrent_and_killed_just_tools_runs_never_overlap() {
    let sandbox = Sandbox::new("shim-install-race");
    sandbox.install_bin(true);

    // A recipe killed mid-build, with its cargo, leaves the lock file behind.
    let pids = sandbox.bin_dir.path().join("pids");
    let mut cmd = Command::new("just");
    isolated(&mut cmd);
    let mut killed = cmd
        .env("PATH", sandbox.path_env())
        .env("FAKE_CARGO_DELAY", "30")
        .env("FAKE_CARGO_PIDS", &pids)
        .current_dir(sandbox.repo.path())
        .arg("tools")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("run just tools");
    wait_for("the recipe's cargo to start", || {
        std::fs::read_to_string(&pids).is_ok_and(|text| text.ends_with('\n'))
    });
    for pid in read(&pids).split_whitespace() {
        let status = Command::new("kill")
            .args(["-KILL", pid])
            .status()
            .expect("run kill");
        assert!(status.success(), "kill -KILL {pid}");
    }
    killed.wait().expect("wait for the killed just tools");
    assert!(sandbox
        .repo
        .path()
        .join("target/tools/.install.lock")
        .exists());

    // Three recipes at once: none waits on the dead recipe's lock for long,
    // and their builds never overlap.
    let spawn = || {
        let mut cmd = Command::new("just");
        isolated(&mut cmd);
        cmd.env("PATH", sandbox.path_env())
            .env("FAKE_CARGO_DELAY", "1")
            .env("FAKE_CARGO_EXCLUSIVE", "1")
            .current_dir(sandbox.repo.path())
            .arg("tools")
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .expect("run just tools")
    };
    let started = Instant::now();
    let recipes: Vec<_> = (0..3).map(|_| spawn()).collect();
    let outputs: Vec<Output> = recipes
        .into_iter()
        .map(|child| child.wait_with_output().expect("wait for just tools"))
        .collect();
    for out in &outputs {
        assert!(out.status.success(), "{}", stderr(out));
    }
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "the killed recipe's lock held the others up"
    );
    assert_eq!(
        read(&sandbox.cargo_log()).lines().count(),
        4,
        "four builds ran"
    );
    assert!(
        !sandbox.repo.path().join("overlap.log").exists(),
        "two builds ran at the same time"
    );
    let waited = outputs
        .iter()
        .filter(|out| stderr(out).contains("waiting for another just tools to finish"))
        .count();
    assert!(waited >= 1, "at least one recipe waited for another");
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
}

// #203 path 3: exits before the shim reaches the guard branch.
#[test]
fn guard_asks_when_the_shim_cannot_find_its_own_directory() {
    let sandbox = Sandbox::new("shim-no-own-dir");
    sandbox.install_bin(true);
    // `sh -c` sets $0, the path the shim resolves its directory from.
    let run = |args: &[&str]| {
        let mut cmd = Command::new("sh");
        isolated(&mut cmd);
        cmd.env("PATH", sandbox.path_env())
            .current_dir(sandbox.repo.path())
            .arg("-c")
            .arg(read(&repo_shim()))
            .arg("/nonexistent/.claude/hooks/aios")
            .args(args)
            .output()
            .expect("run the shim")
    };

    assert_asks(&run(&["guard", "PreToolUse"]), NO_OWN_DIR);

    let out = run(&["docs-check"]);
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("cannot find the directory of /nonexistent/.claude/hooks/aios"),
        "{}",
        stderr(&out)
    );

    let out = run(&["--prebuild"]);
    assert_eq!(code(&out), 0);
    assert!(sandbox.no_build_started());
}

// #203 path 3, continued: the two exits for a main checkout the shim cannot
// name. Both come before the AIOS_TOOLS_BIN override, so it cannot skip them.
#[test]
fn guard_asks_when_the_shim_cannot_find_the_main_checkout() {
    let sandbox = Sandbox::new("shim-no-main");
    sandbox.install_bin(true);
    let fakes = TestRepo::adopt(unique_dir("shim-no-main-path"));
    let path = format!("{}:{}", fakes.path().display(), sandbox.path_env());
    let over = sandbox.bin_dir.path().join("other-aios");
    write_executable(&over, "#!/bin/sh\nprintf 'override:%s\\n' \"$*\"\n");
    let over = over.to_str().expect("a UTF-8 path");

    // git names the common dir, but taking its parent fails: a `dirname` that
    // fails on a path ending in /.git and runs the real one otherwise.
    write_executable(
        &fakes.path().join("dirname"),
        "#!/bin/sh\ncase ${2:-} in */.git) exit 1 ;; esac\nexec /usr/bin/dirname \"$@\"\n",
    );
    let run = |args: &[&str], envs: &[(&str, &str)]| {
        let mut all = vec![("PATH", path.as_str())];
        all.extend_from_slice(envs);
        sandbox.run_at(&sandbox.shim(), args, &all)
    };
    assert_asks(&run(&["guard", "PreToolUse"], &[]), NO_MAIN_DIRNAME);
    assert_asks(
        &run(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", over)]),
        NO_MAIN_DIRNAME,
    );
    let out = run(&["docs-check"], &[]);
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("(dirname failed)"),
        "{}",
        stderr(&out)
    );
    let out = run(&["--prebuild"], &[]);
    assert_eq!(code(&out), 0);
    assert!(sandbox.no_build_started());

    // git fails, and the checkout root two levels above the hooks directory
    // cannot be entered. The wrapper enters the hooks directory first, then
    // takes the search permission off the root, and runs the shim by a
    // relative path. It runs the shim under bash, whose `cd .` still succeeds
    // there: outside POSIX mode bash retries the relative path when the
    // absolute one is blocked, and on macOS, where getcwd fails under the
    // locked root, it uses the relative path from the start. Linux dash's
    // `cd .` tries only the absolute path, so under dash the shim stops
    // earlier, at its own directory, which the test above covers.
    std::fs::remove_file(fakes.path().join("dirname")).expect("remove the fake dirname");
    write_executable(&fakes.path().join("git"), "#!/bin/sh\nexit 128\n");
    const LOCKED: &str = r#"root=$1
shift
chmod 600 "$root" || exit 99
if [ -d "$root/.claude" ]; then
    chmod 755 "$root"
    exit 98
fi
bash ./aios "$@"
status=$?
chmod 755 "$root"
exit "$status"
"#;
    let root = sandbox.repo.path();
    let mode = std::fs::metadata(root)
        .expect("stat the checkout root")
        .permissions();
    let run_locked = |args: &[&str], envs: &[(&str, &str)]| {
        let mut cmd = Command::new("sh");
        isolated(&mut cmd);
        cmd.env("PATH", &path)
            .current_dir(root.join(".claude/hooks"))
            .arg("-c")
            .arg(LOCKED)
            .arg("sh")
            .arg(root)
            .args(args);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        let out = cmd.output().expect("run the shim");
        std::fs::set_permissions(root, mode.clone()).expect("restore the checkout root");
        assert_ne!(
            code(&out),
            98,
            "chmod 600 did not lock the checkout root: run the shim tests as a user that \
             directory permissions apply to (not root)"
        );
        out
    };
    assert_asks(&run_locked(&["guard", "PreToolUse"], &[]), NO_MAIN_ROOT);
    assert_asks(
        &run_locked(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", over)]),
        NO_MAIN_ROOT,
    );
    let out = run_locked(&["docs-check"], &[]);
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("git cannot name the main checkout and ")
            && stderr(&out).contains(" is not accessible; fix git or the checkout's permissions"),
        "{}",
        stderr(&out)
    );
    let out = run_locked(&["--prebuild"], &[]);
    assert_eq!(code(&out), 0);
    assert!(sandbox.no_build_started());
}

// #203 path 4: the stamp ties the binary to main's committed inputs.
#[test]
fn a_replaced_binary_is_stale_even_when_future_dated() {
    let sandbox = Sandbox::new("shim-replaced");
    sandbox.install_bin(true);
    let replace = || {
        write_executable(
            &sandbox.bin(),
            "#!/bin/sh\nprintf 'replaced:%s\\n' \"$*\"\n",
        );
        set_mtime(&sandbox.bin(), NEWER_STAMP);
    };

    replace();
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_asks(&out, STALE);
    sandbox.wait_for_background_build();
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n", "rebuilt");

    std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
    replace();
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(
        stdout(&out),
        "fake:docs-check\n",
        "rebuilt in the foreground"
    );
    assert_eq!(read(&sandbox.cargo_log()), "build\n");
}

#[test]
fn a_missing_or_foreign_stamp_is_stale() {
    let sandbox = Sandbox::new("shim-stamp");
    sandbox.install_bin(true);

    std::fs::remove_file(sandbox.stamp()).expect("remove the stamp");
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );

    // Committed changes to each input, with the binary dated after them so
    // that only the stamp can tell.
    for input in [
        "tools/src/lib.rs",
        "Cargo.lock",
        "Cargo.toml",
        "rust-toolchain.toml",
        "rust-toolchain",
        ".cargo/config.toml",
        "justfile",
    ] {
        std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
        sandbox.repo.write(input, &edited(input, "changed"));
        sandbox.repo.commit(&format!("Change {input}"));
        sandbox.merge();
        set_mtime(&sandbox.bin(), FRESH_STAMP);

        assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
        sandbox.wait_for_background_build();
        assert_eq!(
            stdout(&sandbox.run(&["guard", "PreToolUse"])),
            "fake:guard PreToolUse\n",
            "rebuilt after {input} changed"
        );
    }
}

#[test]
fn a_build_from_uncommitted_changes_is_dirty() {
    let sandbox = Sandbox::new("shim-dirty");
    sandbox
        .repo
        .write("tools/src/lib.rs", "// an uncommitted edit\n");
    sandbox
        .repo
        .write("tools/src/new.rs", "// an untracked file\n");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));

    // guard asks without a rebuild, which would stamp it dirty again.
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), UNCOMMITTED);
    assert!(sandbox.no_build_started());
    // Other subcommands run it, as they would any build of the working tree.
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert!(!sandbox.built());

    // Committed, the inputs no longer match the stamp's HEAD: a rebuild, but
    // still dirty, since origin/main lacks the commit.
    sandbox.repo.commit("Commit the edits");
    set_mtime(&sandbox.bin(), FRESH_STAMP);
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), DIRTY);
    assert!(sandbox.no_build_started());

    // origin/main fast-forwarded to HEAD (standing in for a merge through a
    // PR and a reset of the main checkout onto the merged origin/main), which
    // rewrites no input file: the dirty stamp's cause is gone, so guard treats
    // the binary as stale and rebuilds it, and the rebuild is clean.
    sandbox.merge();
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
}

// Gitignored files under the inputs change the build as much as untracked
// ones: cargo runs an ignored tools/build.rs, and rustup prefers an excluded
// rust-toolchain to rust-toolchain.toml.
#[test]
fn a_build_with_ignored_input_files_is_dirty() {
    let sandbox = Sandbox::new("shim-ignored-build-script");
    sandbox
        .repo
        .write("tools/build.rs", "fn main() { /* any code */ }\n");
    sandbox
        .repo
        .write("tools/.gitignore", "/build.rs\n/.gitignore\n");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), UNCOMMITTED);
    assert!(sandbox.no_build_started());

    let sandbox = Sandbox::new("shim-excluded-toolchain");
    sandbox.repo.write("rust-toolchain", "nightly-2000-01-01\n");
    let exclude = sandbox.repo.path().join(".git/info/exclude");
    let mut excluded = std::fs::read_to_string(&exclude).unwrap_or_default();
    excluded.push_str("rust-toolchain\n");
    std::fs::write(&exclude, excluded).expect("write .git/info/exclude");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), UNCOMMITTED);
    assert!(sandbox.no_build_started());
}

// OS and editor files that no build reads (Finder's .DS_Store, editor swap and
// backup files) do not make a build dirty, ignored or untracked, nor stale
// when written after it (nor do the directories they change), so one left
// in the main checkout never holds guard at a dirty ask that starts no build,
// and one an editor keeps rewriting never starts a rebuild.
#[test]
fn os_and_editor_files_under_the_inputs_keep_a_build_clean() {
    let sandbox = Sandbox::new("shim-inert-files");
    sandbox
        .repo
        .write(".gitignore", "target/\n*.log\n.DS_Store\n*.swp\n");
    sandbox.repo.commit("Ignore OS and editor files");
    sandbox.merge();
    let inert = [
        "tools/.DS_Store",
        "tools/src/.DS_Store",
        "tools/src/.lib.rs.swp",
        "tools/src/lib.rs~",
        "tools/src/lib.rs.bk",
        ".cargo/.config.toml.swo",
    ];
    for file in inert {
        sandbox.repo.write(file, "not a build input\n");
    }
    // The binary keeps its build's start time; the files, and the directories
    // that hold them, are rewritten after the build.
    let out = sandbox.just_tools(&[]);
    assert!(out.status.success(), "just tools failed: {}", stderr(&out));
    std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
    for file in inert {
        sandbox.repo.write(file, "rewritten after the build\n");
        set_mtime(&sandbox.repo.path().join(file), NEWER_STAMP);
    }
    for dir in ["tools", "tools/src", ".cargo"] {
        set_mtime(&sandbox.repo.path().join(dir), NEWER_STAMP);
    }
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
    assert!(sandbox.no_build_started());

    // The shim's repeat of the dirty test leaves them out too: once a local
    // commit's cause is gone, the dirty stamp is stale and the rebuild clean.
    sandbox
        .repo
        .write("tools/src/lib.rs", "// a local commit\n");
    common::git(sandbox.repo.path(), &["commit", "-qam", "Local change"]);
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), DIRTY);
    sandbox.merge();
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
}

// The dirty test reads the main checkout's git config and index. Config that
// moves the work tree or lets git status trust a file system monitor, and
// index flags that make git status skip a file, must not hide a changed input
// from the recipe or from the shim's repeat of its test.
#[test]
fn git_config_and_index_flags_do_not_hide_changed_inputs() {
    let edit = "// an edit hidden from a plain git status\n";

    // core.worktree pointing at a clean copy of HEAD.
    let sandbox = Sandbox::new("shim-hidden-worktree");
    let copy = TestRepo::adopt(unique_dir("shim-hidden-worktree-copy"));
    let mut cmd = Command::new("sh");
    isolated(&mut cmd)
        .current_dir(sandbox.repo.path())
        .arg("-c")
        .arg("git archive HEAD | tar -xf - -C \"$1\"")
        .arg("sh")
        .arg(copy.path());
    assert!(cmd.status().expect("run git archive").success());
    common::git(
        sandbox.repo.path(),
        &["config", "core.worktree", copy.path_str()],
    );
    sandbox.repo.write("tools/src/lib.rs", edit);
    sandbox.repo.write("tools/build.rs", "fn main() {}\n");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), UNCOMMITTED);
    assert!(sandbox.no_build_started());

    // A file system monitor that never reports a change, queried once before
    // the edit so that the index records its token.
    let sandbox = Sandbox::new("shim-hidden-fsmonitor");
    let monitor = sandbox.bin_dir.path().join("fsmonitor");
    write_executable(&monitor, "#!/bin/sh\nprintf 'tok\\0'\n");
    common::git(
        sandbox.repo.path(),
        &[
            "config",
            "core.fsmonitor",
            monitor.to_str().expect("a UTF-8 path"),
        ],
    );
    common::git(sandbox.repo.path(), &["status", "--porcelain"]);
    sandbox.repo.write("tools/src/lib.rs", edit);
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), UNCOMMITTED);
    assert!(sandbox.no_build_started());

    // Index flags: git status skips a file marked assume-unchanged or
    // skip-worktree, so the test reads the flags from git ls-files -v.
    for (label, flag) in [
        ("shim-hidden-assume-unchanged", "--assume-unchanged"),
        ("shim-hidden-skip-worktree", "--skip-worktree"),
    ] {
        let sandbox = Sandbox::new(label);
        common::git(
            sandbox.repo.path(),
            &["update-index", flag, "tools/src/lib.rs"],
        );
        sandbox.repo.write("tools/src/lib.rs", edit);
        sandbox.install_bin(true);
        assert!(
            read(&sandbox.stamp()).ends_with("\nsource dirty\n"),
            "{flag}"
        );
        assert_asks(&sandbox.run(&["guard", "PreToolUse"]), HIDDEN);
        assert!(sandbox.no_build_started());
    }
}

// A directory at either install path fails the recipe with a message, rather
// than `mv -f` moving the new file into it and reporting success.
#[test]
fn a_directory_at_an_install_path_fails_just_tools() {
    for (label, path) in [
        ("shim-install-dir-bin", "target/tools/installed/aios"),
        (
            "shim-install-dir-stamp",
            "target/tools/installed/aios.stamp",
        ),
    ] {
        let sandbox = Sandbox::new(label);
        std::fs::create_dir_all(sandbox.repo.path().join(path)).expect("create the directory");
        let out = sandbox.just_tools(&[]);
        assert!(!out.status.success(), "{path}: {}", stderr(&out));
        assert!(
            stderr(&out).contains(&format!("{path} is a directory")),
            "{}",
            stderr(&out)
        );
    }
}

// A commit only the main checkout has (not merged through a PR) must not turn
// a build of it fresh: the stamp is clean only for inputs on origin/main.
#[test]
fn a_build_from_inputs_not_on_origin_main_is_dirty() {
    let sandbox = Sandbox::new("shim-unmerged");

    // A local commit to an input, then a build: dirty, with no rebuild.
    sandbox
        .repo
        .write("tools/src/lib.rs", "// a local, unmerged commit\n");
    sandbox.repo.commit("Unmerged change");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), DIRTY);
    assert!(sandbox.no_build_started());

    // A later commit that touches no input keeps the earlier one dirty.
    sandbox.repo.write("README.md", "# not a build input\n");
    sandbox.repo.commit("Docs only");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));

    // HEAD behind origin/main, or ahead of it only in other files, is clean.
    sandbox.merge();
    sandbox.repo.write("tools/src/lib.rs", "// merged later\n");
    sandbox.repo.commit("Merged later");
    sandbox.merge();
    common::git(sandbox.repo.path(), &["reset", "-q", "--hard", "HEAD~1"]);
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    sandbox
        .repo
        .write("README.md", "# still not a build input\n");
    sandbox.repo.commit("Local docs commit");
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );

    // Without origin/main nothing proves the inputs were merged: dirty, with
    // its own reason, since no reset onto origin/main can fix that.
    common::git(
        sandbox.repo.path(),
        &["update-ref", "-d", "refs/remotes/origin/main"],
    );
    sandbox.install_bin(true);
    assert!(read(&sandbox.stamp()).ends_with("\nsource dirty\n"));
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), NO_ORIGIN);
    assert!(sandbox.no_build_started());

    // Fetched again, the dirty stamp's cause is gone: guard treats the binary
    // as stale, and the background rebuild is clean.
    sandbox.merge();
    assert_asks(&sandbox.run(&["guard", "PreToolUse"]), STALE);
    sandbox.wait_for_background_build();
    assert!(read(&sandbox.stamp()).ends_with("\nsource clean\n"));
    assert_eq!(
        stdout(&sandbox.run(&["guard", "PreToolUse"])),
        "fake:guard PreToolUse\n"
    );
}

#[test]
fn an_override_replaces_the_main_checkouts_binary() {
    let sandbox = Sandbox::new("shim-override");
    let other = sandbox.bin_dir.path().join("other-aios");
    write_executable(&other, "#!/bin/sh\nprintf 'override:%s\\n' \"$*\"\n");
    let other = other.to_str().expect("a UTF-8 path");
    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", other)]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "override:docs-check\n");
    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", other)]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "override:guard PreToolUse\n");
    assert!(sandbox.no_build_started(), "an override must not build");
}

#[test]
fn a_missing_override_fails_closed() {
    let sandbox = Sandbox::new("shim-override-missing");
    let missing = sandbox.bin_dir.path().join("not-built");
    let missing = missing.to_str().expect("a UTF-8 path");

    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("is not a non-empty regular executable file"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_asks(&out, BAD_OVERRIDE);
}

#[test]
fn a_linked_worktree_runs_the_main_checkouts_binary() {
    let sandbox = Sandbox::new("shim-worktree");
    sandbox.install_bin(true);

    let worktree = TestRepo::adopt(unique_dir("shim-worktree-linked"));
    common::git(
        sandbox.repo.path(),
        &["worktree", "add", "-q", "--detach", worktree.path_str()],
    );

    // A different binary inside the worktree must be ignored.
    let other = worktree.path().join("target/tools/installed/aios");
    std::fs::create_dir_all(other.parent().expect("the binary has a parent"))
        .expect("create the worktree target directory");
    write_executable(&other, "#!/bin/sh\nprintf 'worktree:%s\\n' \"$*\"\n");

    let shim = worktree.path().join(".claude/hooks/aios");
    let out = sandbox.run_at(&shim, &["docs-check"], &[]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    let out = sandbox.run_at(&shim, &["guard", "PreToolUse"], &[]);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");
    assert!(sandbox.no_build_started(), "the main binary is fresh");
}

#[test]
fn a_linked_worktree_fails_closed_when_git_cannot_name_the_main_checkout() {
    let sandbox = Sandbox::new("shim-worktree-nogit");
    sandbox.install_bin(true);

    let worktree = TestRepo::adopt(unique_dir("shim-worktree-nogit-linked"));
    common::git(
        sandbox.repo.path(),
        &["worktree", "add", "-q", "--detach", worktree.path_str()],
    );
    let other = worktree.path().join("target/tools/installed/aios");
    std::fs::create_dir_all(other.parent().expect("the binary has a parent"))
        .expect("create the worktree target directory");
    write_executable(&other, "#!/bin/sh\nprintf 'worktree:%s\\n' \"$*\"\n");

    // A `git` that always fails, first on PATH, ahead of the fake `cargo`.
    let git_dir = TestRepo::adopt(unique_dir("shim-worktree-nogit-git"));
    write_executable(&git_dir.path().join("git"), "#!/bin/sh\nexit 128\n");
    let path = format!("{}:{}", git_dir.path().display(), sandbox.path_env());
    let shim = worktree.path().join(".claude/hooks/aios");

    let out = sandbox.run_at(&shim, &["docs-check"], &[("PATH", &path)]);
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
    assert!(
        stderr(&out).contains("git cannot name the main checkout"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run_at(&shim, &["guard", "PreToolUse"], &[("PATH", &path)]);
    assert_asks(&out, NO_MAIN);

    // A stale worktree binary and a slow build: a shim that took the worktree
    // for the main checkout would hold the worktree's build lock on return.
    set_mtime(&other, STALE_STAMP);
    let out = sandbox.run_at(
        &shim,
        &["--prebuild"],
        &[("PATH", &path), ("FAKE_CARGO_DELAY", "3")],
    );
    assert_eq!(code(&out), 0);
    assert!(!sandbox.lock().exists());
    assert!(
        !worktree.path().join("target/tools/.building").exists(),
        "--prebuild must not build the worktree's binary"
    );

    // An explicit override still works.
    let out = sandbox.run_at(
        &shim,
        &["docs-check"],
        &[
            ("PATH", &path),
            ("AIOS_TOOLS_BIN", other.to_str().expect("a UTF-8 path")),
        ],
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "worktree:docs-check\n");

    // The main checkout's own shim keeps the fallback, since its .git is a
    // directory, but without git it cannot check the stamp: guard asks, and
    // the other subcommands' rebuild fails (the recipe needs git too), so they
    // warn and run the unverified binary.
    let out = sandbox.run_at(
        &sandbox.shim(),
        &["guard", "PreToolUse"],
        &[("PATH", &path)],
    );
    assert_asks(&out, STALE);
    sandbox.wait_for_background_build();
    std::fs::remove_file(sandbox.cargo_log()).expect("remove cargo.log");
    let out = sandbox.run_at(&sandbox.shim(), &["docs-check"], &[("PATH", &path)]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert!(
        stderr(&out).contains("failed; running the stale binary"),
        "{}",
        stderr(&out)
    );
}

#[test]
fn a_bare_override_names_a_file_in_the_current_directory() {
    let sandbox = Sandbox::new("shim-override-bare");
    // The same name in the current directory and on PATH: the shim must run
    // the file its `-x` test checked, not the one exec would find on PATH.
    write_executable(
        &sandbox.repo.path().join("bare-aios"),
        "#!/bin/sh\nprintf 'cwd:%s\\n' \"$*\"\n",
    );
    write_executable(
        &sandbox.bin_dir.path().join("bare-aios"),
        "#!/bin/sh\nprintf 'path:%s\\n' \"$*\"\n",
    );

    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", "bare-aios")]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "cwd:docs-check\n");

    // A bare name found only on PATH is not a runnable file here.
    write_executable(
        &sandbox.bin_dir.path().join("path-only-aios"),
        "#!/bin/sh\nprintf 'path:%s\\n' \"$*\"\n",
    );
    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", "path-only-aios")]);
    assert_eq!(code(&out), 3);
    assert!(stdout(&out).is_empty(), "{}", stdout(&out));
}
