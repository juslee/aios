//! Run one `gh` READ. Writes never go through this module.
//!
//! Every `gh` call that `aios` makes for itself is a read: [`run_program`] calls
//! [`is_read`] first and returns [`GhOutcome::Refused`] without spawning anything
//! when the argv is not on the read allowlist. The module forces no environment:
//! the caller passes a [`GhEnv`], and [`GhEnv::apply`] is the only way a `GhEnv`
//! reaches a `Command`.
//!
//! The shape (an outcome enum, not a `Result`) keeps the difference between "no
//! `gh` on PATH" and "`gh` failed", which `aios brief` prints differently.

use std::ffi::OsStr;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// What one `gh` read returned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhOutcome {
    /// Spawn failed with `io::ErrorKind::NotFound`: no `gh` on PATH.
    Missing,
    /// `gh` exited non-zero (`status: Some(code)`), was killed by a signal, or
    /// failed to spawn for any reason other than NotFound (`status: None`, stderr
    /// holds "gh: <io error>").
    Failed {
        status: Option<i32>,
        stderr: Vec<u8>,
    },
    /// `is_read` rejected the argv; nothing was spawned.
    Refused { reason: &'static str },
    /// Exit 0: stdout exactly as gh wrote it (stderr dropped).
    Ok(Vec<u8>),
}

impl GhOutcome {
    /// `head -n 1` of stderr for `Failed` (without its newline); empty otherwise.
    pub fn first_stderr_line(&self) -> &[u8] {
        match self {
            GhOutcome::Failed { stderr, .. } => {
                let end = stderr
                    .iter()
                    .position(|&b| b == b'\n')
                    .unwrap_or(stderr.len());
                &stderr[..end]
            }
            _ => &[],
        }
    }
}

/// Changes to the inherited environment, applied in order: removals, then sets.
/// Every caller's values are literals, so each caller's env is a `const`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GhEnv {
    pub set: &'static [(&'static str, &'static str)],
    pub remove: &'static [&'static str],
}

impl GhEnv {
    /// Empty: the scripts' behaviour.
    pub const INHERIT: GhEnv = GhEnv {
        set: &[],
        remove: &[],
    };

    /// The one place a `GhEnv` reaches a `Command`: `env_remove` each of `remove`,
    /// then `env` each of `set`.
    pub fn apply<'c>(&self, cmd: &'c mut Command) -> &'c mut Command {
        for name in self.remove {
            cmd.env_remove(name);
        }
        for (name, value) in self.set {
            cmd.env(name, value);
        }
        cmd
    }
}

/// Anything that can run one `gh` read (the real runner, or a fake in a unit test).
pub trait GhRunner {
    fn run(&self, args: &[&str]) -> GhOutcome;
}

/// The real runner: `gh <args>` in `cwd` with `env`, stdin null, stdout and stderr
/// captured.
pub struct GhCli {
    pub cwd: PathBuf,
    pub env: GhEnv,
}

impl GhRunner for GhCli {
    fn run(&self, args: &[&str]) -> GhOutcome {
        run_program(OsStr::new("gh"), args, &self.cwd, &self.env)
    }
}

/// The seam: `GhCli::run` is exactly this with `program` = "gh". It calls
/// [`is_read`] first and returns `Refused` without spawning when that fails. Its
/// `Command` gets its environment only through [`GhEnv::apply`].
pub fn run_program(program: &OsStr, args: &[&str], cwd: &Path, env: &GhEnv) -> GhOutcome {
    if let Err(reason) = is_read(args) {
        return GhOutcome::Refused { reason };
    }
    let mut cmd = Command::new(program);
    cmd.args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    env.apply(&mut cmd);
    match cmd.output() {
        // A missing working directory also fails the spawn with NotFound; that is
        // not "gh is not installed".
        Err(e) if e.kind() == ErrorKind::NotFound && cwd.is_dir() => GhOutcome::Missing,
        Err(e) => GhOutcome::Failed {
            status: None,
            stderr: format!("gh: {e}").into_bytes(),
        },
        Ok(out) if out.status.success() => GhOutcome::Ok(out.stdout),
        Ok(out) => GhOutcome::Failed {
            status: out.status.code(),
            stderr: out.stderr,
        },
    }
}

/// `Ok(())` when `args` is a read; `Err(reason)` otherwise. An allowlist (fail
/// closed):
/// - `api <endpoint> ...`, where `<endpoint>` is `args[1]` and does not start with
///   `-`. It is refused when it has `-X`/`--method` (any spelling) with a value
///   other than GET; `--input`; any field flag (`-f`, `-F`, `--field`,
///   `--raw-field`, attached or `=` forms) unless the endpoint is `graphql`; for
///   `graphql`, any field value starting with `@`, or a `query=` value containing
///   the word `mutation` (ASCII case-insensitive). (A REST `gh api` with a field is
///   a POST.) A flag outside the read set (`--jq`, `--paginate`, `--slurp`,
///   `--cache`, `-H`, `-q`, ...) is refused too.
/// - `pr list|view|checks|diff|status`, `issue list|view|status`, `run list|view`,
///   `label list`, `repo view`, `search issues|prs`.
/// - Everything else is refused (`pr merge|edit|comment|ready|close|create`,
///   `issue create|edit|close|comment`, `label create|edit|delete`,
///   `run rerun|cancel|delete`, `repo edit`, `auth`, `secret`, `workflow run`, ...).
pub fn is_read(args: &[&str]) -> Result<(), &'static str> {
    let Some(&command) = args.first() else {
        return Err("empty gh argv");
    };
    if command == "api" {
        return api_is_read(&args[1..]);
    }
    let reads: &[&str] = match command {
        "pr" => &["list", "view", "checks", "diff", "status"],
        "issue" => &["list", "view", "status"],
        "run" => &["list", "view"],
        "label" => &["list"],
        "repo" => &["view"],
        "search" => &["issues", "prs"],
        _ => return Err("gh command is not on the read allowlist"),
    };
    match args.get(1) {
        Some(sub) if reads.contains(sub) => Ok(()),
        _ => Err("gh subcommand is not on the read allowlist"),
    }
}

/// `gh api` flags that take a value in the next word (or after `=`).
const API_VALUE_FLAGS: &[&str] = &[
    "--jq",
    "--cache",
    "--header",
    "--hostname",
    "--preview",
    "--template",
];
/// `gh api` flags that take no value.
const API_BOOL_FLAGS: &[&str] = &[
    "--paginate",
    "--slurp",
    "--include",
    "--silent",
    "--verbose",
];

/// `args` is everything after `api`.
fn api_is_read(args: &[&str]) -> Result<(), &'static str> {
    let Some(&endpoint) = args.first() else {
        return Err("gh api needs an endpoint");
    };
    if endpoint.starts_with('-') {
        return Err("gh api endpoint must come first");
    }
    let graphql = endpoint == "graphql";
    let mut rest = args[1..].iter().copied();
    while let Some(arg) = rest.next() {
        if let Some(long) = arg.strip_prefix("--") {
            let (name, inline) = match long.split_once('=') {
                Some((name, value)) => (name, Some(value)),
                None => (long, None),
            };
            let flag = format!("--{name}");
            let mut value = || inline.or_else(|| rest.next());
            match flag.as_str() {
                "--method" => method_is_get(value())?,
                "--field" | "--raw-field" => field_is_read(graphql, value())?,
                "--input" => return Err("gh api --input is not a read"),
                f if API_VALUE_FLAGS.contains(&f) => {
                    value().ok_or("gh api flag is missing its value")?;
                }
                f if API_BOOL_FLAGS.contains(&f) && inline.is_none() => {}
                _ => return Err("gh api flag is not on the read allowlist"),
            }
        } else if let Some(short) = arg.strip_prefix('-').filter(|s| !s.is_empty()) {
            // A short-flag cluster: boolean letters, then one letter that takes the
            // rest of the word (after an optional `=`) or the next word as its value.
            let bytes = short.as_bytes();
            let mut i = 0;
            while i < bytes.len() {
                let letter = bytes[i];
                if letter == b'i' {
                    i += 1;
                    continue;
                }
                if !matches!(letter, b'X' | b'f' | b'F' | b'H' | b'q' | b'p' | b't') {
                    return Err("gh api flag is not on the read allowlist");
                }
                let attached = &short[i + 1..];
                let attached = attached.strip_prefix('=').unwrap_or(attached);
                let value = if attached.is_empty() {
                    rest.next()
                } else {
                    Some(attached)
                };
                match letter {
                    b'X' => method_is_get(value)?,
                    b'f' | b'F' => field_is_read(graphql, value)?,
                    _ => {
                        value.ok_or("gh api flag is missing its value")?;
                    }
                }
                break;
            }
        } else {
            return Err("gh api takes one endpoint and no other word");
        }
    }
    Ok(())
}

fn method_is_get(value: Option<&str>) -> Result<(), &'static str> {
    match value {
        Some("GET") => Ok(()),
        _ => Err("gh api method is not GET"),
    }
}

fn field_is_read(graphql: bool, value: Option<&str>) -> Result<(), &'static str> {
    if !graphql {
        return Err("a gh api field makes a REST call a POST");
    }
    let Some((key, text)) = value.and_then(|v| v.split_once('=')) else {
        return Err("gh api field has no key=value");
    };
    if text.starts_with('@') {
        return Err("gh api field reads a file");
    }
    if key == "query" && text.to_ascii_lowercase().contains("mutation") {
        return Err("gh api graphql query is a mutation");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn scratch(label: &str) -> PathBuf {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!(
            "aios-gh-{label}-{}-{}",
            std::process::id(),
            N.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the scratch directory");
        dir
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("fake-gh");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).expect("write the script");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755))
            .expect("chmod the script");
        path
    }

    #[test]
    fn run_program_missing_is_missing() {
        let dir = scratch("missing");
        let program = dir.join("no-such-gh");
        let out = run_program(program.as_os_str(), &["pr", "list"], &dir, &GhEnv::INHERIT);
        assert_eq!(out, GhOutcome::Missing);
        assert!(out.first_stderr_line().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_cwd_is_failed_not_missing() {
        let dir = scratch("cwd");
        let program = script(&dir, "exit 0");
        let gone = dir.join("gone");
        let out = run_program(program.as_os_str(), &["pr", "list"], &gone, &GhEnv::INHERIT);
        assert!(
            matches!(out, GhOutcome::Failed { status: None, .. }),
            "{out:?}"
        );
        assert!(out.first_stderr_line().starts_with(b"gh: "));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn nonzero_exit_is_failed_with_stderr() {
        let dir = scratch("failed");
        let program = script(
            &dir,
            "echo 'first line' >&2; echo second >&2; echo out; exit 4",
        );
        let out = run_program(program.as_os_str(), &["pr", "list"], &dir, &GhEnv::INHERIT);
        assert_eq!(out.first_stderr_line(), b"first line");
        match out {
            GhOutcome::Failed { status, stderr } => {
                assert_eq!(status, Some(4));
                assert_eq!(stderr, b"first line\nsecond\n");
            }
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn success_returns_stdout_exactly() {
        let dir = scratch("ok");
        let program = script(&dir, "printf 'a\\n\\n'; echo ignored >&2");
        let out = run_program(program.as_os_str(), &["pr", "list"], &dir, &GhEnv::INHERIT);
        assert_eq!(out, GhOutcome::Ok(b"a\n\n".to_vec()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refused_argv_spawns_nothing() {
        let dir = scratch("refused");
        let marker = dir.join("ran");
        let program = script(&dir, &format!("touch '{}'", marker.display()));
        let out = run_program(
            program.as_os_str(),
            &["pr", "merge", "1"],
            &dir,
            &GhEnv::INHERIT,
        );
        assert!(matches!(out, GhOutcome::Refused { .. }), "{out:?}");
        assert!(
            !marker.exists(),
            "a refused argv must not spawn the program"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn env_set_and_remove_applied() {
        const ENV: GhEnv = GhEnv {
            set: &[("AIOS_GHENV_TEST_SET", "set-value")],
            remove: &["AIOS_GHENV_TEST_REMOVE"],
        };
        let dir = scratch("env");
        let program = script(
            &dir,
            "printf '%s|%s' \"${AIOS_GHENV_TEST_SET-unset}\" \"${AIOS_GHENV_TEST_REMOVE-unset}\"",
        );
        let mut cmd = Command::new(&program);
        cmd.env("AIOS_GHENV_TEST_REMOVE", "inherited");
        ENV.apply(&mut cmd);
        let out = cmd.output().expect("run the script");
        assert_eq!(out.stdout, b"set-value|unset");
        // Removals come before sets: a name in both ends up set.
        const BOTH: GhEnv = GhEnv {
            set: &[("AIOS_GHENV_TEST_REMOVE", "again")],
            remove: &["AIOS_GHENV_TEST_REMOVE"],
        };
        let mut cmd = Command::new(&program);
        BOTH.apply(&mut cmd);
        let out = cmd.output().expect("run the script");
        assert_eq!(out.stdout, b"unset|again");
        assert_eq!(GhEnv::INHERIT, GhEnv::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn gh_cli_runs_the_named_program_through_run_program() {
        let cli = GhCli {
            cwd: std::env::temp_dir(),
            env: GhEnv::INHERIT,
        };
        // `gh pr merge` is refused before any spawn, on whatever PATH this test has.
        assert!(matches!(
            cli.run(&["pr", "merge", "1"]),
            GhOutcome::Refused { .. }
        ));
    }

    #[test]
    fn is_read_table() {
        let reads: &[&[&str]] = &[
            // brief
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
                "--json",
                "number,title,body,updatedAt",
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
                "--json",
                "number",
                "--jq",
                "length",
            ],
            &[
                "api",
                "graphql",
                "-F",
                "owner={owner}",
                "-F",
                "name={repo}",
                "-f",
                "query=query { repository { pullRequests { nodes { number } } } }",
                "--jq",
                ".x",
            ],
            &[
                "api",
                "graphql",
                "-F",
                "owner={owner}",
                "-F",
                "name={repo}",
                "-F",
                "oid=abc",
                "-f",
                "query=query { x }",
            ],
            &[
                "pr",
                "list",
                "--state",
                "open",
                "--limit",
                "30",
                "--json",
                "number,title,headRefName",
            ],
            &[
                "run",
                "list",
                "--branch",
                "main",
                "--event",
                "push",
                "--limit",
                "40",
                "--json",
                "workflowName",
            ],
            // checkpoint
            &[
                "pr",
                "list",
                "--head",
                "claude/x",
                "--state",
                "open",
                "--json",
                "number,isDraft",
                "--jq",
                ".[]",
            ],
            // the other allowed reads
            &["pr", "view", "1"],
            &["pr", "checks", "1", "--watch"],
            &["pr", "diff", "1"],
            &["pr", "status"],
            &["issue", "view", "1"],
            &["issue", "status"],
            &["run", "view", "1", "--log-failed"],
            &["label", "list"],
            &["repo", "view"],
            &["search", "issues", "x"],
            &["search", "prs", "x"],
            &["api", "repos/{owner}/{repo}/pulls", "--paginate", "--slurp"],
            &["api", "repos/x/y", "-X", "GET"],
            &["api", "repos/x/y", "--method=GET"],
            &["api", "repos/x/y", "-XGET", "-H", "Accept: x"],
            &["api", "graphql", "--raw-field", "query=query { x }"],
        ];
        for argv in reads {
            assert_eq!(is_read(argv), Ok(()), "{argv:?} must be a read");
        }
        let writes: &[&[&str]] = &[
            &[],
            &["pr", "merge", "1"],
            &["pr", "merge", "1", "--auto"],
            &["pr", "edit", "1"],
            &["pr", "comment", "1"],
            &["pr", "ready", "1"],
            &["pr", "close", "1"],
            &["pr", "create"],
            &["pr"],
            &["issue", "create"],
            &["issue", "edit", "1"],
            &["issue", "close", "1"],
            &["issue", "comment", "1"],
            &["label", "create", "x"],
            &["label", "edit", "x"],
            &["label", "delete", "x"],
            &["run", "rerun", "1"],
            &["run", "cancel", "1"],
            &["run", "delete", "1"],
            &["repo", "edit"],
            &["auth", "login"],
            &["secret", "set", "x"],
            &["workflow", "run", "x"],
            &["api"],
            &["api", "-X", "GET", "repos/x/y"],
            &["api", "-X", "POST", "x"],
            &["api", "repos/x/y", "-X", "POST"],
            &["api", "repos/x/y", "-XPOST"],
            &["api", "repos/x/y", "-X=DELETE"],
            &["api", "repos/x/y", "--method", "PATCH"],
            &["api", "repos/x/y", "--method=PUT"],
            &["api", "repos/x/y", "-X"],
            &["api", "repos/x/y", "--input", "body.json"],
            &["api", "repos/x/y", "--input=body.json"],
            &["api", "repos/x/y", "-f", "a=b"],
            &["api", "repos/x/y", "-fa=b"],
            &["api", "repos/x/y", "-F", "a=b"],
            &["api", "repos/x/y", "-Fa=b"],
            &["api", "repos/x/y", "--field", "a=b"],
            &["api", "repos/x/y", "--field=a=b"],
            &["api", "repos/x/y", "--raw-field", "a=b"],
            &["api", "repos/x/y", "-iF", "a=b"],
            &["api", "graphql", "-f", "query=mutation{ x }"],
            &["api", "graphql", "-f", "query=query { x } MUTATION"],
            &["api", "graphql", "-F", "query=@q"],
            &["api", "graphql", "--field", "query=@q"],
            &["api", "graphql", "-f", "other=@q"],
            &["api", "graphql", "-f", "novalue"],
            &["api", "graphql", "--input", "q.json"],
            &["api", "graphql", "-X", "POST", "-f", "query=query { x }"],
            &["api", "rate_limit", "--unknown-flag"],
            &["api", "rate_limit", "-z"],
            &["api", "rate_limit", "extra"],
        ];
        for argv in writes {
            assert!(is_read(argv).is_err(), "{argv:?} must not be a read");
        }
    }
}
