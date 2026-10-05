#![allow(dead_code)]
//! Shared helpers for the `aios hook` integration tests: run the binary with
//! stdin, environment and working directory, and make a throwaway git repository.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Once;

static COUNTER: AtomicUsize = AtomicUsize::new(0);
static PROCESS_ISOLATED: Once = Once::new();

/// Ambient git variables that would make a git call act on another repository or
/// read another configuration; the same list as `tests/common::isolated`.
const GIT_AMBIENT: [&str; 8] = [
    "GIT_DIR",
    "GIT_WORK_TREE",
    "GIT_INDEX_FILE",
    "GIT_COMMON_DIR",
    "GIT_OBJECT_DIRECTORY",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_CEILING_DIRECTORIES",
];

fn isolated_home() -> PathBuf {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("home");
    std::fs::create_dir_all(&home).expect("create the isolated HOME");
    home
}

/// Isolate this test process itself, once, for tests that call library code which
/// runs `git` with the process environment (`Ctx::state_dir`). Only ever removes
/// the ambient git variables and points the configuration at an empty one.
fn isolate_process() {
    PROCESS_ISOLATED.call_once(|| {
        let home = isolated_home();
        for name in GIT_AMBIENT {
            std::env::remove_var(name);
        }
        std::env::set_var("GIT_CONFIG_GLOBAL", "/dev/null");
        std::env::set_var("GIT_CONFIG_NOSYSTEM", "1");
        std::env::set_var("HOME", &home);
        std::env::set_var("XDG_CONFIG_HOME", &home);
    });
}

/// What one run of the binary produced.
pub struct Run {
    pub code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
}

/// A fresh empty directory under `CARGO_TARGET_TMPDIR`, canonical so paths compare
/// equal on macOS, where the temp root sits behind a symlink.
pub fn unique_dir(label: &str) -> PathBuf {
    isolate_process();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR"))
        .join(format!("hook-{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    std::fs::canonicalize(&dir).expect("canonicalize the test directory")
}

/// Strip the ambient git configuration and repository variables so a test never
/// depends on the developer's shell, or on running inside a git hook.
pub fn isolated(cmd: &mut Command) -> &mut Command {
    let home = isolated_home();
    for name in GIT_AMBIENT {
        cmd.env_remove(name);
    }
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
}

/// Run `aios hook <args>` with `stdin` on its standard input, `env` added to a
/// clean hook environment and `cwd` as its working directory. The input is written
/// from a thread and write errors are ignored, because the binary may exit
/// without reading all of an oversized input.
pub fn run_hook(args: &[&str], stdin: &[u8], env: &[(&str, &str)], cwd: &Path) -> Run {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aios"));
    isolated(&mut cmd)
        .arg("hook")
        .args(args)
        .current_dir(cwd)
        .env_remove("AIOS_HOOK_STATE_DIR")
        .env_remove("AIOS_ROUTE_SHADOW")
        .env_remove("AIOS_JEV_URL")
        .env_remove("TYPESAFE_API_KEY")
        .envs(env.iter().copied())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("start the aios binary");
    let mut pipe = child.stdin.take().expect("stdin is piped");
    let input = stdin.to_vec();
    let writer = std::thread::spawn(move || {
        let _ = pipe.write_all(&input);
    });
    let out = child.wait_with_output().expect("wait for the aios binary");
    writer.join().expect("the stdin writer thread");
    Run {
        code: out.status.code(),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

/// Run `git args` in `cwd` in the isolated environment, panicking on failure.
pub fn git(cwd: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    let out = isolated(&mut cmd)
        .args(["-c", "user.name=t", "-c", "user.email=t@example.com"])
        .args([
            "-c",
            "commit.gpgsign=false",
            "-c",
            "init.defaultBranch=main",
        ])
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// A git repository with one commit, in a fresh directory.
pub fn make_git_repo(label: &str) -> PathBuf {
    let dir = unique_dir(label);
    git(&dir, &["init", "-q"]);
    std::fs::write(dir.join("README"), "x\n").expect("write the seed file");
    git(&dir, &["add", "README"]);
    git(&dir, &["commit", "-q", "-m", "seed"]);
    dir
}

/// The three subcommands with the arguments each needs to start.
pub const SUBCOMMANDS: [&[&str]; 3] = [
    &["repeat-error"],
    &["path-guard", "--deny", "kernel/"],
    &["route-shadow"],
];
