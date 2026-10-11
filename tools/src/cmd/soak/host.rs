//! Host facts and helper commands for `aios soak`, ported from the former
//! `scripts/soak-qemu.sh` (blob at `212df62`: `loadavg`, `sha256_of`,
//! `host_cpus` at L499-517, and the inline probes in `run_soak`). Those ported
//! helpers run the same utilities as the script, so `summary.md` reads the
//! same on each host. Crash-fix step 1a adds the interleave preflight's probes
//! (toolchain channel, `rustc --version`, HEAD and arm-base checks, the QEMU
//! version line and full sha256), the harness checkout's build-input dirty
//! test ([`tools_inputs_dirty`]) and stamp check ([`tools_stamp`]),
//! the parent cargo config scan
//! ([`cargo_home`], [`parent_cargo_configs`]), the [`git`] wrapper every git
//! call goes through, and the debug-only `AIOS_SOAK_LOADAVG` override.
//! Every program `aios soak` runs starts from [`command`], in the C locale,
//! except the `kill` that `proc::Supervisor` signals QEMU's process group with
//! (its output is discarded).

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::awk::fields;

/// The first executable file called `name` on `PATH` (`command -v name`).
pub fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in path.as_bytes().split(|&b| b == b':') {
        let dir = if dir.is_empty() {
            Path::new(".")
        } else {
            Path::new(OsStr::from_bytes(dir))
        };
        let candidate = dir.join(name);
        if let Ok(meta) = std::fs::metadata(&candidate) {
            if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
                return Some(candidate);
            }
        }
    }
    None
}

/// `program`, set to run in the C locale. The script exported `LC_ALL=C`, so
/// every program it ran used it; without it, macOS `sysctl -n vm.loadavg`
/// prints comma decimals under a locale such as `de_DE.UTF-8`.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut cmd = Command::new(program);
    cmd.env("LC_ALL", "C");
    cmd
}

/// `s` without its trailing newlines, as `$(...)` gives command output.
pub fn chomp(s: &[u8]) -> &[u8] {
    let end = s.iter().rposition(|&b| b != b'\n').map_or(0, |p| p + 1);
    &s[..end]
}

/// `git`, from [`command`], with replace refs and the grafts file off, as the
/// justfile's `tools` recipe and the `.claude/hooks/aios` shim run it. A file
/// under `.git/refs/replace/` or `.git/info/grafts` changes the commits and
/// trees git sees, so it could otherwise make a staged edit read as committed
/// (a dirty harness as clean) or an arm without the base commit read as
/// containing it. A grafts file under `/dev/null` cannot exist, so git reads
/// none and gives no warning. Every git call `aios soak` makes starts here.
pub fn git() -> Command {
    let mut cmd = command("git");
    cmd.env("GIT_NO_REPLACE_OBJECTS", "1")
        .env("GIT_GRAFT_FILE", "/dev/null/no-grafts");
    cmd
}

/// `$(program args)` run in `cwd`, with stderr discarded: stdout without its
/// trailing newlines, or `None` when the program cannot start or exits non-zero.
pub fn output_of<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
    cwd: Option<&Path>,
) -> Option<Vec<u8>> {
    stdout_of(command(program), args, cwd)
}

/// [`output_of`] for `git`, run through [`git`].
pub fn git_output<S: AsRef<OsStr>>(args: &[S], cwd: Option<&Path>) -> Option<Vec<u8>> {
    stdout_of(git(), args, cwd)
}

/// `cmd` with `args` added, run as [`output_of`] runs its program.
fn stdout_of<S: AsRef<OsStr>>(mut cmd: Command, args: &[S], cwd: Option<&Path>) -> Option<Vec<u8>> {
    cmd.args(args).stdin(Stdio::null()).stderr(Stdio::null());
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    let out = cmd.output().ok()?;
    out.status.success().then(|| chomp(&out.stdout).to_vec())
}

/// The first line of `s`, without its newline (`head -n 1`).
pub fn first_line(s: &[u8]) -> &[u8] {
    s.split(|&b| b == b'\n').next().unwrap_or(&[])
}

/// The last `n` lines of `data` (`tail -n N`), a final unterminated line included.
pub fn tail_lines(data: &[u8], n: usize) -> &[u8] {
    if n == 0 {
        return &[];
    }
    let mut end = data.len();
    if end > 0 && data[end - 1] == b'\n' {
        end -= 1;
    }
    let mut seen = 0;
    for i in (0..end).rev() {
        if data[i] == b'\n' {
            seen += 1;
            if seen == n {
                return &data[i + 1..];
            }
        }
    }
    data
}

/// Replaces [`loadavg`] in builds with debug assertions (the profile
/// `cargo test -p aios-tools` uses), so the fake-QEMU tests can pin the load
/// that an interleaved soak checks and reports. Release builds (`just tools`,
/// so `just soak` and CI's soak jobs) never read it.
pub const LOADAVG_VAR: &str = "AIOS_SOAK_LOADAVG";

/// [`LOADAVG_VAR`]'s value, in a debug-assertion build where it is set.
pub fn loadavg_override() -> Option<String> {
    if cfg!(debug_assertions) {
        std::env::var(LOADAVG_VAR).ok()
    } else {
        None
    }
}

/// The note every `summary.md` load row carries while [`LOADAVG_VAR`] forges
/// the load (debug builds only); empty otherwise.
pub fn loadavg_override_note() -> String {
    loadavg_override()
        .map(|fixed| format!(" ({LOADAVG_VAR} override: every load reads '{fixed}')"))
        .unwrap_or_default()
}

/// `loadavg`: the 1, 5 and 15-minute load averages separated by spaces, from
/// `/proc/loadavg` or `sysctl -n vm.loadavg`; empty when neither works.
/// [`LOADAVG_VAR`] replaces them in debug builds.
pub fn loadavg() -> String {
    if let Some(fixed) = loadavg_override() {
        return fixed;
    }
    if let Ok(text) = std::fs::read("/proc/loadavg") {
        // cut -d' ' -f1-3: a line without a space is printed whole.
        let line = first_line(&text);
        let cut: Vec<&[u8]> = line.split(|&b| b == b' ').take(3).collect();
        return String::from_utf8_lossy(&cut.join(&b' ')).into_owned();
    }
    let raw = command("sysctl")
        .args(["-n", "vm.loadavg"])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .map(|o| o.stdout)
        .unwrap_or_default();
    // tr -d '{}' | awk '{ print $1, $2, $3 }'
    let cleaned: Vec<u8> = raw
        .into_iter()
        .filter(|&b| b != b'{' && b != b'}')
        .collect();
    let body = chomp(&cleaned);
    if cleaned.is_empty() {
        return String::new();
    }
    let lines: Vec<String> = body
        .split(|&b| b == b'\n')
        .map(|line| {
            let f: Vec<&[u8]> = fields(line).collect();
            let get =
                |i: usize| String::from_utf8_lossy(f.get(i).copied().unwrap_or(b"")).into_owned();
            format!("{} {} {}", get(0), get(1), get(2))
        })
        .collect();
    lines.join("\n")
}

/// The first field of [`loadavg`] (`loadavg | cut -d' ' -f1`).
pub fn load1() -> Vec<u8> {
    let all = loadavg();
    all.as_bytes()
        .split(|&b| b == b' ')
        .next()
        .unwrap_or(b"")
        .to_vec()
}

/// `host_cpus`: online CPUs from `getconf`, else `sysctl -n hw.ncpu`, else `?`.
pub fn host_cpus() -> Vec<u8> {
    output_of("getconf", &["_NPROCESSORS_ONLN"], None)
        .or_else(|| output_of("sysctl", &["-n", "hw.ncpu"], None))
        .unwrap_or_else(|| b"?".to_vec())
}

/// `uname -srm`, or empty when it fails.
pub fn uname() -> Vec<u8> {
    output_of("uname", &["-srm"], None).unwrap_or_default()
}

/// The first 16 hex digits of the SHA-256 of `path` (`sha256_of FILE | cut -c1-16`),
/// from `sha256sum`, or `shasum -a 256` where there is none.
pub fn sha256_16(path: &Path) -> Result<String> {
    let mut digest = sha256(path)?;
    digest.truncate(16);
    Ok(digest)
}

/// The SHA-256 of `path` in hex (the first field of `sha256sum`'s line), from
/// `sha256sum`, or `shasum -a 256` where there is none.
pub fn sha256(path: &Path) -> Result<String> {
    let mut cmd = if find_in_path("sha256sum").is_some() {
        command("sha256sum")
    } else {
        let mut c = command("shasum");
        c.args(["-a", "256"]);
        c
    };
    let out = cmd
        .arg(path)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("cannot compute the sha256 of {}", path.display()))?;
    if !out.status.success() {
        bail!("cannot compute the sha256 of {}", path.display());
    }
    let digest = first_line(&out.stdout)
        .split(|&b| b == b' ')
        .next()
        .unwrap_or(b"");
    Ok(String::from_utf8_lossy(digest).into_owned())
}

/// The first line of `qemu --version` (empty when it cannot run), with
/// QEMU's stderr passed through.
pub fn qemu_version(qemu: &OsStr) -> Vec<u8> {
    command(qemu)
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map(|o| first_line(&o.stdout).to_vec())
        .unwrap_or_default()
}

/// The toolchain channel `rust-toolchain.toml` in `root` pins: the quoted
/// `channel` value of its `[toolchain]` table.
pub fn toolchain_channel(root: &Path) -> Result<String> {
    let path = root.join("rust-toolchain.toml");
    let text = std::fs::read_to_string(&path)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let mut table = "";
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            table = line;
            continue;
        }
        if table != "[toolchain]" {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if key.trim() != "channel" {
            continue;
        }
        let value = value.trim();
        if let Some(channel) = value
            .strip_prefix('"')
            .and_then(|v| v.split_once('"'))
            .map(|(c, _)| c)
            .filter(|c| !c.is_empty())
        {
            return Ok(channel.to_string());
        }
    }
    bail!(
        "{} has no channel = \"...\" in its [toolchain] table",
        path.display()
    )
}

/// `rustc --version` run in `dir`, without the caller's `RUSTUP_TOOLCHAIN`, so
/// rustup picks the toolchain `dir`'s `rust-toolchain.toml` pins.
pub fn rustc_version(dir: &Path) -> Result<String> {
    let out = command("rustc")
        .arg("--version")
        .current_dir(dir)
        .env_remove("RUSTUP_TOOLCHAIN")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .with_context(|| format!("cannot run rustc --version in {}", dir.display()))?;
    if !out.status.success() {
        bail!(
            "rustc --version failed in {} (is the toolchain its rust-toolchain.toml pins installed?)",
            dir.display()
        );
    }
    Ok(String::from_utf8_lossy(first_line(&out.stdout)).into_owned())
}

/// The full commit id `HEAD` names in the checkout `root` (`git rev-parse
/// --verify HEAD`), or `None` when git cannot tell.
pub fn head_commit(root: &Path) -> Option<String> {
    let out = git_output(
        &[
            OsStr::new("-C"),
            root.as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new("--verify"),
            OsStr::new("HEAD"),
        ],
        None,
    )?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

/// Whether commit `base` is an ancestor of `commit` in the checkout `root`
/// (`git merge-base --is-ancestor`): `Err` when git cannot tell (an unknown
/// commit, or not a checkout).
pub fn contains_commit(root: &Path, base: &str, commit: &str) -> Result<bool> {
    let status = git()
        .arg("-C")
        .arg(root)
        .args(["merge-base", "--is-ancestor", base, commit])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("cannot run git merge-base")?;
    match status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => bail!(
            "git merge-base --is-ancestor {base} {commit} failed in {}",
            root.display()
        ),
    }
}

/// The commit under test: `git rev-parse --short HEAD` (or `unknown`), with a
/// `-dirty` suffix when tracked files have uncommitted changes.
pub fn git_rev(root: &Path) -> String {
    git_rev_of(root, "HEAD")
}

/// [`git_rev`] for `commit` (a full id pinned earlier, say) instead of
/// whatever `HEAD` names now; the `-dirty` test still reads the work tree.
pub fn git_rev_of(root: &Path, commit: &str) -> String {
    let root_arg = root.as_os_str();
    let mut rev = git_output(
        &[
            OsStr::new("-C"),
            root_arg,
            OsStr::new("rev-parse"),
            OsStr::new("--short"),
            OsStr::new(commit),
        ],
        None,
    )
    .map(|v| String::from_utf8_lossy(&v).into_owned())
    .unwrap_or_else(|| "unknown".to_string());
    let status = git_output(
        &[
            OsStr::new("-C"),
            root_arg,
            OsStr::new("status"),
            OsStr::new("--porcelain"),
            OsStr::new("--untracked-files=no"),
        ],
        None,
    )
    .unwrap_or_default();
    if !status.is_empty() {
        rev.push_str("-dirty");
    }
    rev
}

/// The host tools' build inputs, as the justfile's `tools` recipe and the
/// `.claude/hooks/aios` shim list them in `inputs` (a test checks all three
/// agree): cargo reads untracked and ignored files among them (a
/// `tools/build.rs`, a legacy `rust-toolchain`, a `.cargo/config`) as well as
/// tracked ones.
const TOOLS_INPUTS: [&str; 8] = [
    "tools",
    "shared",
    "Cargo.lock",
    "Cargo.toml",
    "rust-toolchain.toml",
    "rust-toolchain",
    ".cargo",
    "justfile",
];

/// The directory inputs the dirty test's exclude pathspecs are anchored
/// under, and the editor and Finder files they leave out, as the `tools`
/// recipe and the shim write them (`':(exclude,glob)tools/**/.DS_Store'` …).
const TOOLS_EXCLUDE_DIRS: [&str; 3] = ["tools", "shared", ".cargo"];
const TOOLS_JUNK: [&str; 5] = [".DS_Store", "*.swp", "*.swo", "*~", "*.rs.bk"];

/// Whether the checkout `root` holds host-tools build inputs that its `HEAD`
/// does not: the `tools` recipe's own test, so a harness the recipe stamps
/// `source dirty` for uncommitted, untracked or ignored inputs reads dirty
/// here too. `git status --untracked-files=all --ignored=matching` over
/// [`TOOLS_INPUTS`], less the editor and Finder files the recipe excludes,
/// plus any file `git ls-files -v` flags (assume-unchanged, skip-worktree),
/// which `git status` does not show. Both run as the recipe runs them: through
/// [`git`] (replace refs and grafts off), with the work tree pinned to `root`,
/// no optional locks, and the config that lets `git status` skip reading
/// files (fsmonitor, the untracked cache, a minimal `checkStat`, an ignored
/// ctime) turned off. `None` when git cannot tell.
pub fn tools_inputs_dirty(root: &Path) -> Option<bool> {
    let mut work_tree = OsString::from("--work-tree=");
    work_tree.push(root.as_os_str());
    let mut status: Vec<OsString> = [
        "--no-optional-locks",
        "-c",
        "core.fsmonitor=false",
        "-c",
        "core.untrackedCache=false",
        "-c",
        "core.checkStat=default",
        "-c",
        "core.trustctime=true",
    ]
    .iter()
    .map(OsString::from)
    .collect();
    status.push(work_tree.clone());
    status.extend(
        [
            "status",
            "--porcelain",
            "--untracked-files=all",
            "--ignored=matching",
            "--",
        ]
        .iter()
        .map(OsString::from),
    );
    status.extend(TOOLS_INPUTS.iter().map(OsString::from));
    for dir in TOOLS_EXCLUDE_DIRS {
        status.extend(
            TOOLS_JUNK
                .iter()
                .map(|junk| OsString::from(format!(":(exclude,glob){dir}/**/{junk}"))),
        );
    }
    let listed = git_output(&status, Some(root))?;
    if !listed.is_empty() {
        return Some(true);
    }
    let mut ls = vec![work_tree];
    ls.extend(["ls-files", "-v", "--"].iter().map(OsString::from));
    ls.extend(TOOLS_INPUTS.iter().map(OsString::from));
    let flags = git_output(&ls, Some(root))?;
    Some(
        flags
            .split(|&b| b == b'\n')
            .any(|l| !l.is_empty() && !l.starts_with(b"H ")),
    )
}

/// What the `.claude/hooks/aios` shim's freshness test would say about `exe`,
/// the running binary, in the checkout `root`, read as the shim's `verdict`
/// reads it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolsStamp {
    /// No input file is newer than `exe`, and the provenance stamp `just
    /// tools` writes beside it (`aios.stamp`) names `root`'s current `HEAD`
    /// tree entries for [`TOOLS_INPUTS`], `exe`'s git hash and `source
    /// clean`, or `source dirty` while a cause of that remains (the shim's
    /// `fresh` and `dirty-<cause>`).
    Fresh,
    /// An input file, editor and Finder files aside, is newer than `exe`: an
    /// edit since the build, or one reverted since (`git checkout` rewrites
    /// the file), so `exe` may hold source `HEAD` does not.
    InputNewer,
    /// The stamp names other `HEAD` entries or another binary, or is not in
    /// the recipe's format (a binary left from an older commit, or one the
    /// stamp does not describe).
    Mismatch,
    /// The stamp says `source dirty`, but no cause of that remains: the build
    /// may hold edits reverted since.
    DirtyGone,
}

/// [`ToolsStamp`] for the binary `exe` in the checkout `root` whose `HEAD`
/// was read as the commit `head` (a full id, so a commit or checkout in
/// `root` meanwhile cannot make the stamp be checked against a tree other
/// than the commit the caller names), in the shim's order: the mtime test,
/// then the stamp, then, for `source dirty`, the recipe's dirty causes
/// (uncommitted, untracked, ignored or index-flagged inputs by
/// [`tools_inputs_dirty`], a test git cannot run counting as one; no
/// `origin/main`; `head` input changes that `origin/main` lacks). `None`
/// when `exe` has no stamp, or its mtime or git's view of `head` and `exe`
/// cannot be read.
pub fn tools_stamp(root: &Path, exe: &Path, head: &str) -> Option<ToolsStamp> {
    let have = std::fs::read(exe.with_file_name("aios.stamp")).ok()?;
    let built = std::fs::metadata(exe).and_then(|m| m.modified()).ok()?;
    if TOOLS_INPUTS
        .iter()
        .any(|input| newer_file(&root.join(input), built))
    {
        return Some(ToolsStamp::InputNewer);
    }
    let mut ls: Vec<&OsStr> = ["ls-tree", head, "--"]
        .into_iter()
        .map(OsStr::new)
        .collect();
    ls.extend(TOOLS_INPUTS.iter().map(OsStr::new));
    let src = git_output(&ls, Some(root))?;
    let sum = git_output(
        &[
            OsStr::new("hash-object"),
            OsStr::new("--no-filters"),
            OsStr::new("--"),
            exe.as_os_str(),
        ],
        Some(root),
    )?;
    let want = [
        b"aios-tools-stamp 1\n".as_slice(),
        &src,
        b"\nbin ",
        &sum,
        b"\nsource ",
    ]
    .concat();
    let have = chomp(&have);
    let Some(state) = have.strip_prefix(want.as_slice()) else {
        return Some(ToolsStamp::Mismatch);
    };
    Some(match state {
        b"clean" => ToolsStamp::Fresh,
        b"dirty" if tools_dirty_cause(root, head) => ToolsStamp::Fresh,
        b"dirty" => ToolsStamp::DirtyGone,
        _ => ToolsStamp::Mismatch,
    })
}

/// Whether a file at or under `path`, other than a directory or an editor or
/// Finder file, has an mtime after `built`, as the shim's `find $inputs ! -type
/// d … -newer` finds one: symlinks are not followed, and an entry that cannot
/// be read is skipped.
fn newer_file(path: &Path, built: std::time::SystemTime) -> bool {
    let Ok(meta) = std::fs::symlink_metadata(path) else {
        return false;
    };
    if meta.is_dir() {
        return std::fs::read_dir(path).is_ok_and(|entries| {
            entries
                .flatten()
                .any(|entry| newer_file(&entry.path(), built))
        });
    }
    let name = path.file_name().map_or(&[][..], OsStr::as_bytes);
    let junk = name == b".DS_Store"
        || [&b".swp"[..], b".swo", b"~", b".rs.bk"]
            .iter()
            .any(|end| name.ends_with(end));
    !junk && meta.modified().is_ok_and(|m| m > built)
}

/// Whether the `tools` recipe would stamp a build in `root` `source dirty`
/// now, by the shim's `dirty_cause`: [`tools_inputs_dirty`] (or git cannot
/// tell), no `refs/remotes/origin/main`, or input changes in the commit
/// `head` (`HEAD`, read once by the caller) since its merge base with
/// `origin/main`.
fn tools_dirty_cause(root: &Path, head: &str) -> bool {
    if tools_inputs_dirty(root) != Some(false) {
        return true;
    }
    let origin = "refs/remotes/origin/main";
    if git_output(&["rev-parse", "--verify", "-q", origin], Some(root)).is_none() {
        return true;
    }
    let Some(base) = git_output(&["merge-base", head, origin], Some(root)) else {
        return true;
    };
    let mut diff: Vec<&OsStr> = ["diff-tree", "--quiet", "-r"]
        .iter()
        .map(OsStr::new)
        .collect();
    diff.extend([OsStr::from_bytes(&base), OsStr::new(head), OsStr::new("--")]);
    diff.extend(TOOLS_INPUTS.iter().map(OsStr::new));
    git_output(&diff, Some(root)).is_none()
}

/// The cargo home: `$CARGO_HOME`, else `$HOME/.cargo`, as cargo resolves it.
pub fn cargo_home() -> Option<PathBuf> {
    std::env::var_os("CARGO_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cargo")))
}

/// The cargo config files above the checkout `root` that a build in it reads
/// as well as its own: cargo merges `.cargo/config.toml` (or the legacy
/// `.cargo/config`) of every ancestor of the directory it runs in, joining
/// array values such as `rustflags`, so a worktree inside another checkout
/// builds with that checkout's flags too. `cargo_home`'s config is skipped,
/// since cargo reads it wherever the build runs.
pub fn parent_cargo_configs(root: &Path, cargo_home: Option<&Path>) -> Vec<PathBuf> {
    let home = cargo_home.map(|h| std::fs::canonicalize(h).unwrap_or_else(|_| h.to_path_buf()));
    root.ancestors()
        .skip(1)
        .filter_map(|dir| {
            let dot_cargo = dir.join(".cargo");
            let is_home = home
                .as_deref()
                .is_some_and(|h| std::fs::canonicalize(&dot_cargo).is_ok_and(|d| d == h));
            if is_home {
                return None;
            }
            ["config.toml", "config"]
                .iter()
                .map(|name| dot_cargo.join(name))
                .find(|f| f.is_file())
        })
        .collect()
}

/// `date +%Y%m%d-%H%M%S`, for the default output directory.
pub fn timestamp() -> Result<String> {
    match output_of("date", &["+%Y%m%d-%H%M%S"], None) {
        Some(v) => Ok(String::from_utf8_lossy(&v).into_owned()),
        None => bail!("cannot run date for the output directory's timestamp"),
    }
}

/// `just --evaluate VAR` in the repository root.
pub fn just_evaluate(root: &Path, var: &str) -> Result<Vec<u8>> {
    match output_of("just", &["--evaluate", var], Some(root)) {
        Some(v) => Ok(v),
        None => bail!("cannot read {var} from the justfile"),
    }
}

/// The repository `aios soak` boots: the git checkout containing `cwd`, resolved
/// to its physical path (the script's `cd .. && pwd -P`).
pub fn repo_root(cwd: &Path) -> Result<PathBuf> {
    let top = git_output(
        &[
            OsStr::new("-C"),
            cwd.as_os_str(),
            OsStr::new("rev-parse"),
            OsStr::new("--show-toplevel"),
        ],
        None,
    )
    .with_context(|| {
        format!(
            "{} is not inside a git checkout; aios soak boots the checkout it runs in",
            cwd.display()
        )
    })?;
    let top = PathBuf::from(OsStr::from_bytes(&top));
    std::fs::canonicalize(&top).with_context(|| format!("cannot resolve {}", top.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// [`TOOLS_INPUTS`], [`TOOLS_EXCLUDE_DIRS`] and [`TOOLS_JUNK`] are copies
    /// of the justfile `tools` recipe's and the `.claude/hooks/aios` shim's
    /// `inputs` list and exclude pathspecs: a list changed there and not here
    /// would make every Harness row read stale or miss dirty inputs.
    #[test]
    fn tools_inputs_match_the_recipe_and_the_shim() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let mut excludes: Vec<String> = TOOLS_EXCLUDE_DIRS
            .iter()
            .flat_map(|dir| {
                TOOLS_JUNK
                    .iter()
                    .map(move |junk| format!("{dir}/**/{junk}"))
            })
            .collect();
        excludes.sort();
        for file in ["justfile", ".claude/hooks/aios"] {
            let text = std::fs::read_to_string(root.join(file)).expect("read");
            let lists: Vec<&str> = text
                .lines()
                .filter_map(|l| l.trim_start().strip_prefix("inputs='"))
                .map(|rest| rest.split('\'').next().unwrap_or_default())
                .collect();
            assert_eq!(lists, [TOOLS_INPUTS.join(" ")], "{file}: inputs='...'");
            let mut found: Vec<String> = text
                .split("':(exclude,glob)")
                .skip(1)
                .map(|rest| rest.split('\'').next().unwrap_or_default().to_string())
                .collect();
            found.sort();
            assert_eq!(found, excludes, "{file}: exclude pathspecs");
        }
    }

    #[test]
    fn chomp_and_first_line_follow_the_shell() {
        assert_eq!(chomp(b"abc\n\n"), b"abc");
        assert_eq!(chomp(b"a\nb\n"), b"a\nb");
        assert_eq!(chomp(b"\n"), b"");
        assert_eq!(first_line(b"QEMU 10.1\nCopyright\n"), b"QEMU 10.1");
        assert_eq!(first_line(b""), b"");
    }

    #[test]
    fn tail_lines_matches_tail_n() {
        assert_eq!(tail_lines(b"a\nb\nc\n", 2), b"b\nc\n");
        assert_eq!(tail_lines(b"a\nb\nc", 2), b"b\nc");
        assert_eq!(tail_lines(b"a\nb\n", 5), b"a\nb\n");
        assert_eq!(tail_lines(b"a\n\n\n", 2), b"\n\n");
        assert_eq!(tail_lines(b"", 3), b"");
        assert_eq!(tail_lines(b"a\nb\n", 0), b"");
    }

    #[test]
    fn find_in_path_finds_sh_and_not_a_missing_program() {
        assert!(find_in_path("sh").is_some());
        assert!(find_in_path("aios-no-such-program").is_none());
    }

    #[test]
    fn output_of_is_none_on_failure() {
        assert_eq!(
            output_of("sh", &["-c", "printf 'x\\n\\n'"], None),
            Some(b"x".to_vec())
        );
        assert_eq!(output_of("sh", &["-c", "echo out; exit 1"], None), None);
        assert_eq!(
            output_of("aios-no-such-program", &[] as &[&str], None),
            None
        );
    }

    #[test]
    fn helpers_run_in_the_c_locale() {
        assert_eq!(
            output_of("sh", &["-c", "printf '%s' \"$LC_ALL\""], None),
            Some(b"C".to_vec())
        );
    }

    #[test]
    fn host_facts_have_the_script_shape() {
        let load = loadavg();
        let parts: Vec<&str> = load.split(' ').collect();
        assert_eq!(parts.len(), 3, "{load:?}");
        assert!(parts.iter().all(|p| p.parse::<f64>().is_ok()), "{load:?}");
        assert!(!load1().is_empty());
        let cpus = String::from_utf8(host_cpus()).expect("ASCII");
        assert!(cpus.parse::<u32>().is_ok_and(|n| n >= 1), "{cpus:?}");
        assert!(!uname().is_empty());
        let stamp = timestamp().expect("date runs");
        assert_eq!(stamp.len(), 15, "{stamp:?}");
        assert_eq!(stamp.as_bytes()[8], b'-');
    }

    #[test]
    fn sha256_16_is_the_digest_prefix() {
        let dir = std::env::temp_dir().join(format!("aios-host-sha-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("abc");
        std::fs::write(&file, b"abc").expect("write");
        // SHA-256("abc") = ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad
        assert_eq!(sha256_16(&file).expect("sha"), "ba7816bf8f01cfea");
        assert_eq!(
            sha256(&file).expect("sha"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert!(sha256_16(&dir.join("missing")).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn toolchain_channel_reads_the_toolchain_table() {
        let dir = std::env::temp_dir().join(format!("aios-host-tc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("rust-toolchain.toml");
        std::fs::write(
            &file,
            "[other]\nchannel = \"no\"\n\n[toolchain]\n# pinned\nchannel = \"nightly-2026-10-09\" # date\ntargets = []\n",
        )
        .expect("write");
        assert_eq!(
            toolchain_channel(&dir).expect("a channel"),
            "nightly-2026-10-09"
        );
        std::fs::write(&file, "[toolchain]\nchannel = nightly\n").expect("write");
        assert!(toolchain_channel(&dir).is_err());
        std::fs::remove_file(&file).expect("remove");
        assert!(toolchain_channel(&dir).is_err());
        // The workspace's own file.
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert!(toolchain_channel(&workspace)
            .expect("the workspace pins a channel")
            .starts_with("nightly-"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn git_rev_and_repo_root_in_a_scratch_repository() {
        let dir = std::env::temp_dir().join(format!("aios-host-git-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).expect("temp dir");
        let git = |args: &[&str]| {
            let ok = Command::new("git")
                .args([
                    "-c",
                    "user.name=t",
                    "-c",
                    "user.email=t@example.invalid",
                    "-c",
                    "commit.gpgsign=false",
                ])
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .expect("git runs");
            assert!(ok.status.success(), "git {args:?}");
        };
        git(&["init", "-q", "-b", "main"]);
        std::fs::write(dir.join("f"), "1").expect("write");
        git(&["add", "f"]);
        git(&["commit", "-q", "-m", "one"]);
        let first =
            String::from_utf8(git_output(&["rev-parse", "HEAD"], Some(&dir)).expect("rev-parse"))
                .expect("hex");
        std::fs::write(dir.join("g"), "1").expect("write");
        git(&["add", "g"]);
        git(&["commit", "-q", "-m", "two"]);
        let second =
            String::from_utf8(git_output(&["rev-parse", "HEAD"], Some(&dir)).expect("rev-parse"))
                .expect("hex");
        assert!(contains_commit(&dir, &first, "HEAD").expect("known"));
        assert!(contains_commit(&dir, &second, "HEAD").expect("known"));
        assert!(!contains_commit(&dir, &second, &first).expect("known"));
        assert_eq!(head_commit(&dir).as_deref(), Some(second.as_str()));
        git(&["checkout", "-q", "--detach", &first]);
        assert_eq!(head_commit(&dir).as_deref(), Some(first.as_str()));
        assert!(!contains_commit(&dir, &second, "HEAD").expect("known"));
        assert!(contains_commit(&dir, &first, &second).expect("known"));
        assert!(contains_commit(&dir, &"0".repeat(40), "HEAD").is_err());
        assert!(git_rev_of(&dir, &second).starts_with(&second[..7]));
        git(&["checkout", "-q", "main"]);
        let rev = git_rev(&dir);
        assert!(
            rev.len() >= 7 && rev.bytes().all(|b| b.is_ascii_hexdigit()),
            "{rev:?}"
        );
        std::fs::write(dir.join("f"), "2").expect("write");
        assert_eq!(git_rev(&dir), format!("{rev}-dirty"));
        let root = std::fs::canonicalize(&dir).expect("canonical");
        assert_eq!(repo_root(&dir.join("sub")).expect("inside"), root);
        assert!(git_rev(Path::new("/")).starts_with("unknown"));

        // The tools recipe's dirty test: untracked and ignored build inputs
        // count, editor files and files outside the inputs do not.
        git(&["checkout", "-q", "--", "f"]);
        std::fs::create_dir_all(dir.join("tools/src")).expect("tools dir");
        std::fs::write(dir.join("tools/src/main.rs"), "fn main() {}").expect("write");
        std::fs::write(dir.join(".gitignore"), "tools/build.rs\n").expect("write");
        git(&["add", "tools", ".gitignore"]);
        git(&["commit", "-q", "-m", "three"]);
        assert_eq!(tools_inputs_dirty(&dir), Some(false));
        std::fs::write(dir.join("tools/src/.DS_Store"), "x").expect("write");
        std::fs::write(dir.join("tools/src/main.rs.swp"), "x").expect("write");
        std::fs::write(dir.join("notes.txt"), "x").expect("write");
        assert_eq!(tools_inputs_dirty(&dir), Some(false));
        std::fs::write(dir.join("tools/build.rs"), "fn main() {}").expect("write");
        assert_eq!(tools_inputs_dirty(&dir), Some(true), "ignored input");
        std::fs::remove_file(dir.join("tools/build.rs")).expect("remove");
        std::fs::write(dir.join("rust-toolchain"), "nightly").expect("write");
        assert_eq!(tools_inputs_dirty(&dir), Some(true), "untracked input");
        std::fs::remove_file(dir.join("rust-toolchain")).expect("remove");
        git(&["update-index", "--assume-unchanged", "tools/src/main.rs"]);
        assert_eq!(tools_inputs_dirty(&dir), Some(true), "flagged input");
        git(&["update-index", "--no-assume-unchanged", "tools/src/main.rs"]);
        assert_eq!(tools_inputs_dirty(&dir), Some(false));
        assert_eq!(tools_inputs_dirty(Path::new("/")), None);

        // The installed binary's stamp, read as the shim reads it: fresh
        // while it names HEAD's input entries, the binary and a clean source
        // (or a dirty one whose cause remains) and no input file is newer
        // than the binary; absent beside a binary `just tools` did not
        // install.
        let exe = dir.join("installed/aios");
        std::fs::create_dir_all(dir.join("installed")).expect("mkdir");
        let now = std::time::SystemTime::now();
        let set_mtime = |path: &Path, secs: u64| {
            std::fs::File::options()
                .write(true)
                .open(path)
                .and_then(|f| f.set_modified(now + std::time::Duration::from_secs(secs)))
                .expect("set mtime");
        };
        let install = |bin: &str| {
            std::fs::write(&exe, bin).expect("write");
            set_mtime(&exe, 100);
        };
        let stamp_for = |state: &[u8]| -> Vec<u8> {
            let mut ls = vec!["ls-tree", "HEAD", "--"];
            ls.extend(TOOLS_INPUTS);
            let src = git_output(&ls, Some(&dir)).expect("ls-tree");
            let sum = git_output(
                &[
                    OsStr::new("hash-object"),
                    OsStr::new("--no-filters"),
                    OsStr::new("--"),
                    exe.as_os_str(),
                ],
                Some(&dir),
            )
            .expect("hash-object");
            [
                b"aios-tools-stamp 1\n".as_slice(),
                &src,
                b"\nbin ",
                &sum,
                b"\nsource ",
                state,
                b"\n",
            ]
            .concat()
        };
        let stamp_path = dir.join("installed/aios.stamp");
        let stamped = |stamp: &[u8]| {
            std::fs::write(&stamp_path, stamp).expect("write");
            tools_stamp(&dir, &exe, &head_commit(&dir).expect("head"))
        };
        install("binary");
        assert_eq!(
            tools_stamp(&dir, &exe, &head_commit(&dir).expect("head")),
            None,
            "no stamp"
        );
        let clean = stamp_for(b"clean");
        let dirty = stamp_for(b"dirty");
        assert_eq!(stamped(&clean), Some(ToolsStamp::Fresh));
        assert_eq!(stamped(&dirty), Some(ToolsStamp::Fresh), "no origin/main");
        assert_eq!(stamped(&stamp_for(b"cleanx")), Some(ToolsStamp::Mismatch));
        assert_eq!(stamped(b"aios-tools-stamp 1\n"), Some(ToolsStamp::Mismatch));
        install("rebuilt");
        assert_eq!(stamped(&clean), Some(ToolsStamp::Mismatch), "binary");
        install("binary");
        assert_eq!(stamped(&clean), Some(ToolsStamp::Fresh));
        git(&["update-ref", "refs/remotes/origin/main", "HEAD"]);
        assert_eq!(stamped(&dirty), Some(ToolsStamp::DirtyGone), "cause gone");
        std::fs::write(dir.join("tools/build.rs"), "fn main() {}").expect("write");
        assert_eq!(stamped(&dirty), Some(ToolsStamp::Fresh), "ignored input");
        std::fs::remove_file(dir.join("tools/build.rs")).expect("remove");
        // An edit reverted after the build rewrites the file, which the
        // mtime test catches; an editor file does not count.
        std::fs::write(dir.join("tools/src/.DS_Store"), "y").expect("write");
        set_mtime(&dir.join("tools/src/.DS_Store"), 200);
        assert_eq!(stamped(&clean), Some(ToolsStamp::Fresh), "editor file");
        std::fs::write(dir.join("tools/src/main.rs"), "fn main() {}").expect("write");
        set_mtime(&dir.join("tools/src/main.rs"), 200);
        assert_eq!(stamped(&clean), Some(ToolsStamp::InputNewer), "revert");
        set_mtime(&exe, 300);
        assert_eq!(stamped(&clean), Some(ToolsStamp::Fresh));
        let three = head_commit(&dir).expect("head");
        std::fs::write(dir.join("tools/src/lib.rs"), "").expect("write");
        git(&["add", "tools/src/lib.rs"]);
        git(&["commit", "-q", "-m", "four"]);
        set_mtime(&exe, 300);
        assert_eq!(stamped(&clean), Some(ToolsStamp::Mismatch), "new commit");
        // The stamp is checked against the commit the caller read, not
        // whatever HEAD names by the time the check runs.
        assert_eq!(
            tools_stamp(&dir, &exe, &three),
            Some(ToolsStamp::Fresh),
            "pinned head"
        );
        assert_eq!(
            stamped(&stamp_for(b"dirty")),
            Some(ToolsStamp::Fresh),
            "unmerged"
        );
        std::fs::remove_dir_all(dir.join("installed")).expect("remove");

        // Replace refs are off: one that swaps HEAD for a commit holding a
        // staged tools/ edit hides that edit from a plain git status, but not
        // from the dirty tests, and one that cuts HEAD's parents does not hide
        // an ancestor from the arm-base check.
        let plain = |args: &[&str]| -> Vec<u8> {
            let out = Command::new("git")
                .args(args)
                .current_dir(&dir)
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .env_remove("GIT_NO_REPLACE_OBJECTS")
                .env_remove("GIT_GRAFT_FILE")
                .output()
                .expect("git runs");
            assert!(out.status.success(), "git {args:?}");
            chomp(&out.stdout).to_vec()
        };
        let clean_rev = git_rev(&dir);
        std::fs::write(dir.join("tools/src/main.rs"), "fn main() { }").expect("write");
        git(&["add", "tools/src/main.rs"]);
        let tree = String::from_utf8(plain(&["write-tree"])).expect("hex");
        let fake = String::from_utf8(plain(&[
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.invalid",
            "commit-tree",
            &tree,
            "-p",
            "HEAD",
            "-m",
            "fake",
        ]))
        .expect("hex");
        git(&["replace", "HEAD", &fake]);
        assert!(
            plain(&[
                "status",
                "--porcelain",
                "--untracked-files=no",
                "--",
                "tools"
            ])
            .is_empty(),
            "the replace ref hides the staged edit from a plain git status"
        );
        assert_eq!(tools_inputs_dirty(&dir), Some(true), "replaced HEAD");
        assert_eq!(git_rev(&dir), format!("{clean_rev}-dirty"));
        git(&["replace", "-d", &head_commit(&dir).expect("head")]);
        git(&["reset", "-q", "--hard", "HEAD"]);
        assert_eq!(tools_inputs_dirty(&dir), Some(false));
        git(&["replace", "--graft", "HEAD"]);
        assert_eq!(
            plain(&["rev-list", "--count", "HEAD"]),
            b"1",
            "the replace ref cuts HEAD's parents for a plain git"
        );
        assert!(contains_commit(&dir, &first, "HEAD").expect("known"));
        git(&["replace", "-d", &head_commit(&dir).expect("head")]);

        // A checkout nested in another sees the outer one's cargo config
        // (the legacy name too), but not the cargo home's.
        let inner = dir.join("wt/arm");
        std::fs::create_dir_all(dir.join("wt/.cargo")).expect("mkdir");
        std::fs::create_dir_all(dir.join(".cargo")).expect("mkdir");
        std::fs::create_dir_all(inner.join(".cargo")).expect("mkdir");
        std::fs::write(inner.join(".cargo/config.toml"), "").expect("write");
        // Only those under the scratch repository: the temp directory's own
        // ancestors are the host's.
        let under = |home: Option<&Path>| -> Vec<PathBuf> {
            parent_cargo_configs(&root.join("wt/arm"), home)
                .into_iter()
                .filter(|p| p.starts_with(&root))
                .collect()
        };
        assert!(
            under(None).is_empty(),
            "the arm's own config is not a parent's"
        );
        std::fs::write(dir.join(".cargo/config.toml"), "").expect("write");
        std::fs::write(dir.join("wt/.cargo/config"), "").expect("write");
        assert_eq!(
            under(None),
            [
                root.join("wt/.cargo/config"),
                root.join(".cargo/config.toml")
            ]
        );
        assert_eq!(
            under(Some(&dir.join(".cargo"))),
            [root.join("wt/.cargo/config")]
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
