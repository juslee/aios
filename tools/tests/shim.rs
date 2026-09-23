//! `.claude/hooks/aios`: freshness, the guard branch, foreground and background
//! builds, `AIOS_TOOLS_BIN` and linked worktrees, exercised with a fake `just`.

mod common;

use common::{isolated, unique_dir, TestRepo};

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The PreToolUse decision the shim prints when the binary is missing.
const ASK_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"aios tools not built; run just tools"}}"#;

/// A `just` that only knows `tools`: it logs the build and writes a binary that
/// echoes its arguments. `FAKE_JUST_FAIL` makes it fail, `FAKE_JUST_DELAY`
/// slows it down, `FAKE_EXIT` sets the exit status of the binary it writes.
const FAKE_JUST: &str = r#"#!/bin/sh
set -u
if [ "${1:-}" != "tools" ]; then
    echo "fake just: unexpected recipe ${*}" >&2
    exit 2
fi
echo "fake just: building the aios binary"
printf 'build\n' >> just.log
if [ -n "${FAKE_JUST_FAIL:-}" ]; then
    echo "fake just: the build failed" >&2
    exit 1
fi
sleep "${FAKE_JUST_DELAY:-0}"
mkdir -p target/tools/release
# Install atomically: the stale-guard branch execs this file while this build
# runs, and truncating a script in place can leave /bin/sh reading a half-written
# file. mv within one directory is rename(2), so the running shim keeps the old
# inode.
cat > target/tools/release/aios.new <<'BIN'
#!/bin/sh
printf 'fake:%s\n' "$*"
exit ${FAKE_EXIT:-0}
BIN
chmod 755 target/tools/release/aios.new
mv -f target/tools/release/aios.new target/tools/release/aios
"#;

/// The same binary the fake `just` writes, installed directly by a test.
const FAKE_BIN: &str = r#"#!/bin/sh
printf 'fake:%s\n' "$*"
exit ${FAKE_EXIT:-0}
"#;

fn repo_shim() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tools crate has a parent directory")
        .join(".claude/hooks/aios")
}

fn shim_source() -> String {
    let path = repo_shim();
    std::fs::read_to_string(&path).unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn make_executable(path: &Path) {
    let mut perms = std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("stat {}: {err}", path.display()))
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod 755");
}

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

/// A main checkout holding the shim, with a fake `just` first on `PATH`.
struct Sandbox {
    repo: TestRepo,
    bin_dir: TestRepo,
}

impl Sandbox {
    fn new(label: &str) -> Sandbox {
        let repo = TestRepo::new(label);
        repo.write(".claude/hooks/aios", &shim_source());
        repo.write(
            "tools/src/lib.rs",
            "// the shim only needs tools/ to exist\n",
        );
        repo.write("Cargo.lock", "# the shim only needs Cargo.lock to exist\n");
        make_executable(&repo.path().join(".claude/hooks/aios"));
        repo.commit("Initial");

        let bin_dir = TestRepo::adopt(unique_dir(&format!("{label}-path")));
        let just = bin_dir.path().join("just");
        std::fs::write(&just, FAKE_JUST).expect("write the fake just");
        make_executable(&just);

        Sandbox { repo, bin_dir }
    }

    fn shim(&self) -> PathBuf {
        self.repo.path().join(".claude/hooks/aios")
    }

    fn bin(&self) -> PathBuf {
        self.repo.path().join("target/tools/release/aios")
    }

    fn lock(&self) -> PathBuf {
        self.repo.path().join("target/tools/.building")
    }

    fn just_log(&self) -> PathBuf {
        self.repo.path().join("just.log")
    }

    fn install_bin(&self, fresh: bool) {
        let bin = self.bin();
        std::fs::create_dir_all(bin.parent().expect("the binary has a parent"))
            .expect("create target/tools/release");
        std::fs::write(&bin, FAKE_BIN).expect("write the fake binary");
        make_executable(&bin);
        let stamp = if fresh {
            "209901010000"
        } else {
            "200001010000"
        };
        let status = Command::new("touch")
            .arg("-t")
            .arg(stamp)
            .arg(&bin)
            .status()
            .expect("run touch");
        assert!(status.success(), "touch -t {stamp} failed");
    }

    fn run_at(&self, shim: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(shim);
        isolated(&mut cmd);
        let path = format!(
            "{}:{}",
            self.bin_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("PATH", path)
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
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), format!("{ASK_JSON}\n"));
    assert!(
        !sandbox.just_log().exists(),
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
        stderr(&out).contains("fake just: building the aios binary"),
        "build output belongs on stderr: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
}

#[test]
fn a_missing_binary_with_a_failing_build_exits_3() {
    let sandbox = Sandbox::new("shim-build-fails");
    let out = sandbox.run_env(&["docs-check"], &[("FAKE_JUST_FAIL", "1")]);
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
    assert!(
        !sandbox.just_log().exists(),
        "a fresh binary must not build"
    );

    let out = sandbox.run_env(&["docs-check"], &[("FAKE_EXIT", "7")]);
    assert_eq!(code(&out), 7);
    assert!(!sandbox.just_log().exists());
}

#[test]
fn a_stale_binary_is_rebuilt_in_the_foreground() {
    let sandbox = Sandbox::new("shim-stale");
    sandbox.install_bin(false);
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
    assert!(!sandbox.lock().exists(), "a foreground build takes no lock");
}

#[test]
fn a_stale_guard_runs_at_once_and_rebuilds_in_the_background() {
    let sandbox = Sandbox::new("shim-stale-guard");
    sandbox.install_bin(false);
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");
    wait_for("the background build to log a line", || {
        sandbox.just_log().exists()
    });
    wait_for("the build lock to be released", || !sandbox.lock().exists());
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
}

#[test]
fn prebuild_returns_at_once_and_builds_in_the_background() {
    let sandbox = Sandbox::new("shim-prebuild");
    let started = Instant::now();
    let out = sandbox.run_env(&["--prebuild"], &[("FAKE_JUST_DELAY", "1")]);
    assert_eq!(code(&out), 0);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "--prebuild must not wait for the build"
    );
    wait_for("the background build to produce the binary", || {
        sandbox.bin().exists()
    });
    wait_for("the build lock to be released", || !sandbox.lock().exists());
}

#[test]
fn an_override_replaces_the_main_checkouts_binary() {
    let sandbox = Sandbox::new("shim-override");
    let other = sandbox.bin_dir.path().join("other-aios");
    std::fs::write(&other, "#!/bin/sh\nprintf 'override:%s\\n' \"$*\"\n")
        .expect("write the override binary");
    make_executable(&other);
    let out = sandbox.run_env(
        &["docs-check"],
        &[("AIOS_TOOLS_BIN", other.to_str().expect("a UTF-8 path"))],
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "override:docs-check\n");
    assert!(!sandbox.just_log().exists(), "an override must not build");
}

#[test]
fn a_missing_override_fails_closed() {
    let sandbox = Sandbox::new("shim-override-missing");
    let missing = sandbox.bin_dir.path().join("not-built");
    let missing = missing.to_str().expect("a UTF-8 path");

    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("is not an executable file"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), format!("{ASK_JSON}\n"));
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
    let other = worktree.path().join("target/tools/release/aios");
    std::fs::create_dir_all(other.parent().expect("the binary has a parent"))
        .expect("create the worktree target directory");
    std::fs::write(&other, "#!/bin/sh\nprintf 'worktree:%s\\n' \"$*\"\n")
        .expect("write the worktree binary");
    make_executable(&other);

    let out = sandbox.run_at(
        &worktree.path().join(".claude/hooks/aios"),
        &["docs-check"],
        &[],
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert!(!sandbox.just_log().exists(), "the main binary is fresh");
}
