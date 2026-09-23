#![allow(dead_code)]
//! Shared helpers for the aios-tools integration tests: a temp directory per
//! test, an isolated git environment, and a throwaway repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh empty directory under `CARGO_TARGET_TMPDIR`.
pub fn unique_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

/// Strip the ambient git and Python environment so a test never depends on the
/// developer's configuration.
pub fn isolated(cmd: &mut Command) -> &mut Command {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("home");
    std::fs::create_dir_all(&home).expect("create the isolated HOME");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("AIOS_TOOLS_BIN")
}

/// Run git in `dir` with a fixed identity; panics with git's stderr on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    isolated(&mut cmd)
        .arg("-c")
        .arg("user.name=aios-test")
        .arg("-c")
        .arg("user.email=aios-test@example.invalid")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .args(args)
        .current_dir(dir);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git printed UTF-8")
}

/// A throwaway git repository, removed when dropped.
pub struct TestRepo {
    root: PathBuf,
}

impl TestRepo {
    pub fn new(label: &str) -> TestRepo {
        let root = unique_dir(label);
        git(&root, &["init", "-q", "-b", "main"]);
        // `isolated()` only reaches the commands these helpers spawn; the in-process
        // `Repo::open` of Tasks 5-12 runs git with the test process's environment.
        // Pin the excludes file in the repository's own config, which outranks the
        // developer's `~/.gitconfig` and `~/.config/git/ignore`, so a global ignore
        // rule can never hide a fixture file from
        // `git ls-files --cached --others --exclude-standard`.
        git(&root, &["config", "core.excludesFile", "/dev/null"]);
        TestRepo { root }
    }

    /// Take ownership of an existing directory (it need not be a repository).
    pub fn adopt(root: PathBuf) -> TestRepo {
        TestRepo { root }
    }

    pub fn with_files(label: &str, files: &[(&str, &str)]) -> TestRepo {
        let repo = TestRepo::new(label);
        for (rel, content) in files {
            repo.write(rel, content);
        }
        repo.commit("Initial");
        repo
    }

    pub fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the parent directories");
        }
        std::fs::write(&path, content).expect("write a test file");
    }

    pub fn commit(&self, subject: &str) {
        git(&self.root, &["add", "-A"]);
        git(
            &self.root,
            &["commit", "-q", "--allow-empty", "-m", subject],
        );
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn path_str(&self) -> &str {
        self.root.to_str().expect("a UTF-8 test path")
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
