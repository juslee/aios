//! Read-only git queries for one directory, cached per hook run (`Repo`,
//! git-push-guard.py L984-1099).
//!
//! git runs as Python runs it: the hook's environment with `GIT_OPTIONAL_LOCKS=0`
//! and `LC_ALL=C`, without `GIT_DIR`, `GIT_WORK_TREE`, `GIT_CONFIG_PARAMETERS`,
//! `GIT_CONFIG_COUNT` and `GIT_INDEX_FILE`, stdin closed, a 4 s limit, and its
//! output decoded as strict UTF-8 text (a bad byte is Python's
//! `UnicodeDecodeError`, which nothing in the guard catches).

use std::cell::RefCell;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Read;
use std::process::{Command, Stdio};
use std::rc::Rc;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use regex::Regex;

use super::pattern::re;
use super::pyos::{self, Fault};
use crate::pystr;

/// Git config as `git config --list -z` lists it: key -> values in order.
pub type Config = BTreeMap<String, Vec<String>>;

/// `Repo.get`: one [`Repo`] per (directory, extra global options) for a run.
/// (directory, extra global options) -> its [`Repo`].
type Cache = HashMap<(String, Vec<String>), Rc<Repo>>;

#[derive(Default)]
pub struct Repos {
    cache: RefCell<Cache>,
}

impl Repos {
    pub fn get(&self, cwd: &str, extra: &[String]) -> Rc<Repo> {
        let key = (cwd.to_string(), extra.to_vec());
        Rc::clone(
            self.cache
                .borrow_mut()
                .entry(key)
                .or_insert_with(|| Rc::new(Repo::new(cwd, extra))),
        )
    }
}

#[derive(Default)]
struct Memo {
    is_repo: Option<bool>,
    roots: Option<Vec<String>>,
    git_dirs: Option<(Option<String>, Option<String>)>,
    config: Option<Rc<Config>>,
    branch: Option<Option<String>>,
}

pub struct Repo {
    pub cwd: String,
    extra: Vec<String>,
    memo: RefCell<Memo>,
}

/// How long one git query may run (`timeout=4`).
const GIT_TIMEOUT: Duration = Duration::from_secs(4);

impl Repo {
    fn new(cwd: &str, extra: &[String]) -> Repo {
        Repo {
            cwd: cwd.to_string(),
            extra: extra.to_vec(),
            memo: RefCell::new(Memo::default()),
        }
    }

    /// `Repo.run`: git's stdout, or `None` when the directory is missing, git
    /// cannot start, runs out of time or exits non-zero.
    pub fn run(&self, args: &[&str]) -> Result<Option<String>, Fault> {
        if !pyos::isdir(&self.cwd) {
            return Ok(None);
        }
        let all: Vec<&str> = self
            .extra
            .iter()
            .map(String::as_str)
            .chain(args.iter().copied())
            .collect();
        if all.iter().any(|a| a.contains('\0')) {
            return Err(Fault::null_byte());
        }
        let mut cmd = Command::new("git");
        cmd.args(&all)
            .current_dir(&self.cwd)
            .env("GIT_OPTIONAL_LOCKS", "0")
            .env("LC_ALL", "C")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_INDEX_FILE",
        ] {
            cmd.env_remove(var);
        }
        let Some((code, stdout, stderr)) = run_with_limit(cmd, GIT_TIMEOUT) else {
            return Ok(None);
        };
        let stdout = pyos::decode_strict(&stdout)?;
        pyos::decode_strict(&stderr)?;
        Ok((code == Some(0)).then_some(stdout))
    }

    pub fn is_repo(&self) -> Result<bool, Fault> {
        if let Some(v) = self.memo.borrow().is_repo {
            return Ok(v);
        }
        let v = self.run(&["rev-parse", "--git-dir"])?.is_some();
        self.memo.borrow_mut().is_repo = Some(v);
        Ok(v)
    }

    /// The working tree root and the main worktree root, resolved.
    pub fn roots(&self) -> Result<Vec<String>, Fault> {
        if let Some(v) = &self.memo.borrow().roots {
            return Ok(v.clone());
        }
        let out = self
            .run(&[
                "rev-parse",
                "--path-format=absolute",
                "--show-toplevel",
                "--git-common-dir",
            ])?
            .unwrap_or_default();
        let lines: Vec<&str> = pystr::splitlines(&out)
            .into_iter()
            .filter(|l| !l.is_empty())
            .collect();
        let mut roots = Vec::new();
        if let Some(first) = lines.first() {
            roots.push(pyos::realpath(first)?);
        }
        if lines.len() > 1 && pyos::basename(lines[1]) == ".git" {
            roots.push(pyos::realpath(pyos::dirname(lines[1]))?);
        }
        self.memo.borrow_mut().roots = Some(roots.clone());
        Ok(roots)
    }

    /// (git dir, common dir), resolved: for a linked worktree the git dir is its
    /// admin directory `<common>/worktrees/<name>`.
    pub fn git_dirs(&self) -> Result<(Option<String>, Option<String>), Fault> {
        if let Some(v) = &self.memo.borrow().git_dirs {
            return Ok(v.clone());
        }
        let out = self
            .run(&[
                "rev-parse",
                "--path-format=absolute",
                "--absolute-git-dir",
                "--git-common-dir",
            ])?
            .unwrap_or_default();
        let lines: Vec<String> = pystr::splitlines(&out)
            .into_iter()
            .filter(|l| !l.is_empty())
            .map(pyos::realpath)
            .collect::<Result<_, _>>()?;
        let v = if lines.len() == 2 {
            (Some(lines[0].clone()), Some(lines[1].clone()))
        } else {
            (None, None)
        };
        self.memo.borrow_mut().git_dirs = Some(v.clone());
        Ok(v)
    }

    pub fn config(&self) -> Result<Rc<Config>, Fault> {
        if let Some(v) = &self.memo.borrow().config {
            return Ok(Rc::clone(v));
        }
        let raw = self.run(&["config", "--list", "-z"])?.unwrap_or_default();
        let mut entries = Config::new();
        for item in raw.split('\0') {
            if item.is_empty() {
                continue;
            }
            let (key, value) = item.split_once('\n').unwrap_or((item, ""));
            entries
                .entry(key.to_string())
                .or_default()
                .push(value.to_string());
        }
        let entries = Rc::new(entries);
        self.memo.borrow_mut().config = Some(Rc::clone(&entries));
        Ok(entries)
    }

    pub fn config_last(&self, key: &str) -> Result<Option<String>, Fault> {
        Ok(self
            .config()?
            .get(key)
            .and_then(|values| values.last().cloned()))
    }

    /// owner/repo (lowercased) for every remote URL that points at github.com.
    pub fn github_repos(&self) -> Result<BTreeSet<String>, Fault> {
        static KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^remote\..+\.(?:url|pushurl)$"));
        static URL: LazyLock<Regex> =
            LazyLock::new(|| re(r"github\.com[:/]+([^/]+/[^/]+?)(?:\.git)?/?\n?$"));
        let mut found = BTreeSet::new();
        for (key, values) in self.config()?.iter() {
            if !KEY.is_match(key) {
                continue;
            }
            for url in values {
                if let Some(m) = URL.captures(url).and_then(|c| c.get(1)) {
                    found.insert(m.as_str().to_lowercase());
                }
            }
        }
        Ok(found)
    }

    pub fn current_branch(&self) -> Result<Option<String>, Fault> {
        if let Some(v) = &self.memo.borrow().branch {
            return Ok(v.clone());
        }
        let out = self
            .run(&["symbolic-ref", "-q", "HEAD"])?
            .unwrap_or_default();
        let r = pystr::strip(&out);
        let v = r.strip_prefix("refs/heads/").map(str::to_string);
        self.memo.borrow_mut().branch = Some(v.clone());
        Ok(v)
    }

    /// True when commits not on any remote touch `.github/`. `wide` also counts
    /// uncommitted and untracked files there, for a push that follows an add or
    /// commit in the same command.
    pub fn unpushed_github(&self, revs: &[String], wide: bool) -> Result<bool, Fault> {
        static SHA: LazyLock<Regex> = LazyLock::new(|| re(r"^[0-9a-f]{40,64}$"));
        let mut shas = Vec::new();
        for rev in revs {
            let spec = format!("{rev}^{{commit}}");
            let out = self
                .run(&[
                    "rev-parse",
                    "--verify",
                    "--quiet",
                    "--end-of-options",
                    &spec,
                ])?
                .unwrap_or_default();
            let sha = pystr::strip(&out);
            if SHA.is_match(sha) {
                shas.push(sha.to_string());
            }
        }
        if !shas.is_empty() {
            let mut args = vec!["log", "--format=", "--name-only", "--max-count=500"];
            args.extend(shas.iter().map(String::as_str));
            args.extend(["--not", "--remotes", "--", ":(top).github"]);
            if let Some(out) = self.run(&args)? {
                if !pystr::strip(&out).is_empty() {
                    return Ok(true);
                }
            }
        }
        if wide {
            let status = self.run(&[
                "status",
                "--porcelain",
                "--untracked-files=all",
                "--",
                ":(top).github",
            ])?;
            if let Some(out) = status {
                if !pystr::strip(&out).is_empty() {
                    return Ok(true);
                }
            }
        }
        Ok(false)
    }
}

/// Run `cmd` with its output captured: (exit code, stdout, stderr), or `None`
/// when it cannot start or outlives `limit` (then it is killed, as
/// `subprocess.run(timeout=...)` kills it).
pub fn run_with_limit(
    mut cmd: Command,
    limit: Duration,
) -> Option<(Option<i32>, Vec<u8>, Vec<u8>)> {
    let mut child = cmd.spawn().ok()?;
    let mut out_pipe = child.stdout.take()?;
    let mut err_pipe = child.stderr.take()?;
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out_pipe.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err_pipe.read_to_end(&mut buf);
        buf
    });
    let deadline = Instant::now() + limit;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(2)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
        }
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    status.map(|s| (s.code(), stdout, stderr))
}
