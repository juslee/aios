//! Host facts and helper commands for `aios soak`, ported from the former
//! `scripts/soak-qemu.sh` (blob at `212df62`: `loadavg`, `sha256_of`,
//! `host_cpus` at L499-517, and the inline probes in `run_soak`). They run the
//! same utilities as the script, so `summary.md` reads the same on each host.
//! Every program `aios soak` runs starts from [`command`], in the C locale.

use std::ffi::OsStr;
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

/// `$(program args)` run in `cwd`, with stderr discarded: stdout without its
/// trailing newlines, or `None` when the program cannot start or exits non-zero.
pub fn output_of<S: AsRef<OsStr>>(
    program: &str,
    args: &[S],
    cwd: Option<&Path>,
) -> Option<Vec<u8>> {
    let mut cmd = command(program);
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

/// `loadavg`: the 1, 5 and 15-minute load averages separated by spaces, from
/// `/proc/loadavg` or `sysctl -n vm.loadavg`; empty when neither works.
pub fn loadavg() -> String {
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
    Ok(String::from_utf8_lossy(&digest[..digest.len().min(16)]).into_owned())
}

/// The commit under test: `git rev-parse --short HEAD` (or `unknown`), with a
/// `-dirty` suffix when tracked files have uncommitted changes.
pub fn git_rev(root: &Path) -> String {
    let root_arg = root.as_os_str();
    let mut rev = output_of(
        "git",
        &[
            OsStr::new("-C"),
            root_arg,
            OsStr::new("rev-parse"),
            OsStr::new("--short"),
            OsStr::new("HEAD"),
        ],
        None,
    )
    .map(|v| String::from_utf8_lossy(&v).into_owned())
    .unwrap_or_else(|| "unknown".to_string());
    let status = output_of(
        "git",
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
    let top = output_of(
        "git",
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
        assert!(sha256_16(&dir.join("missing")).is_err());
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
        let _ = std::fs::remove_dir_all(&dir);
    }
}
