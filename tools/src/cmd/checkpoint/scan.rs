//! The secret scan of `aios checkpoint`: paths new to the repository that look like
//! secrets, and added lines that look like private keys or access tokens, over the
//! staged changes and over the local commits that are not on origin.
//!
//! This is the former script's scan (oracle L84-125, named in the parent module). The
//! regexes are copied from it; the path regex folds case for ASCII only (D8), as
//! `grep -i` does in the C locale, and both match raw bytes, so a path that is not
//! UTF-8 is scanned like any other.
//!
//! Divergence D9: the script ignored a failing git call inside the scan (a failed
//! `log` gave an empty result, so the scan said "clean"). Here any of the four scan
//! git calls that fails to spawn or exits non-zero is a [`ScanError`], and the caller
//! stops without committing or pushing. The one exception is an unborn HEAD, which has
//! no local commits to scan: the two `log` calls are not issued, as the oracle's empty
//! result there.

use std::io;
use std::path::Path;
use std::process::Output;
use std::sync::OnceLock;

use regex::bytes::Regex;

use crate::git;

/// Case-insensitive (ASCII only, D8), matched against repo-relative paths that are
/// new to the repo (oracle L85).
pub const SECRET_PATH_RE: &str = r"(?i-u)(^|/)(\.env(\.[^/]*)?|\.netrc|\.npmrc|\.pypirc|id_(rsa|dsa|ecdsa|ed25519)[^/]*|[^/]*\.(pem|key|p12|pfx|jks|keystore)|[^/]*(credential|secret)[^/]*)$";

/// Matched against added lines: PEM private keys, GitHub, Anthropic, AWS and Slack
/// tokens (oracle L87).
pub const SECRET_TEXT_RE: &str = r"(?-u)-----BEGIN ([A-Z0-9]+ )*PRIVATE KEY-----|gh[pousr]_[A-Za-z0-9]{36}|github_pat_[A-Za-z0-9_]{22,}|sk-ant-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|xox[abposr]-[A-Za-z0-9-]{10,}";

/// Never committed, even where `.gitignore` does not list them yet (L89).
pub const EXCLUDES: [&str; 2] = [":(exclude).remember", ":(exclude,glob)**/__pycache__/**"];

/// The reason printed for a path that looks like a secret.
pub const REASON_FILE_NAME: &str = "file name";
/// The reason printed for a path whose added line looks like a key or a token.
pub const REASON_TEXT: &str = "key or token in an added line";

/// One suspicious path and why.
pub type Hit = (Vec<u8>, &'static str);

/// A git call of the scan failed (D9).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScanError {
    /// Which of the four scan git calls, 1 to 4.
    pub call: usize,
    /// The first line of its stderr (the spawn error when it did not start).
    pub line: Vec<u8>,
}

fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(SECRET_PATH_RE).expect("SECRET_PATH_RE is a valid regex"))
}

fn text_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(SECRET_TEXT_RE).expect("SECRET_TEXT_RE is a valid regex"))
}

/// The lines of `data`, without their newlines; a final line without a newline counts.
pub fn lines(data: &[u8]) -> impl Iterator<Item = &[u8]> {
    let mut pieces: Vec<&[u8]> = data.split(|&b| b == b'\n').collect();
    if pieces.last().is_some_and(|last| last.is_empty()) {
        pieces.pop();
    }
    pieces.into_iter()
}

/// `added_lines`, the script's awk (L107-114), line for line: for each added line of a
/// patch, the file it was added to and its text. A line after `diff --git` is a header
/// line until the first `@@`; `+++ ` in a header names the file (a leading `b/` is
/// dropped; `/dev/null` stays).
pub fn added_lines(patch: &[u8]) -> Vec<(Vec<u8>, Vec<u8>)> {
    let mut found = Vec::new();
    let mut in_header = false;
    let mut file: &[u8] = b"";
    for line in lines(patch) {
        if line.starts_with(b"diff --git ") {
            in_header = true;
        } else if in_header && line.starts_with(b"+++ ") {
            let name = &line[4..];
            file = name.strip_prefix(b"b/").unwrap_or(name);
        } else if line.starts_with(b"@@") {
            in_header = false;
        } else if !in_header && line.starts_with(b"+") {
            found.push((file.to_vec(), line[1..].to_vec()));
        }
    }
    found
}

/// `head -n 1` of `stderr`.
pub fn first_line(stderr: &[u8]) -> &[u8] {
    let end = stderr
        .iter()
        .position(|&b| b == b'\n')
        .unwrap_or(stderr.len());
    &stderr[..end]
}

/// One scan call: its stdout, or the [`ScanError`] of a call that did not run or failed.
fn call(
    run: &mut dyn FnMut(&[&str]) -> io::Result<Output>,
    number: usize,
    args: &[&str],
) -> Result<Vec<u8>, ScanError> {
    match run(args) {
        Ok(output) if output.status.success() => Ok(output.stdout),
        Ok(output) => {
            let line = first_line(&output.stderr);
            Err(ScanError {
                call: number,
                line: if line.is_empty() {
                    format!("git exited with {} and no message", output.status).into_bytes()
                } else {
                    line.to_vec()
                },
            })
        }
        Err(e) => Err(ScanError {
            call: number,
            line: e.to_string().into_bytes(),
        }),
    }
}

/// The scan in `repo`: `(path, reason)` pairs, sorted and unique bytewise (D3), as
/// `sort -u` over the `<path>\t<reason>` lines in the C locale.
pub fn scan(repo: &Path) -> Result<Vec<Hit>, ScanError> {
    scan_with(&mut |args| git::output(repo, args))
}

/// [`scan`] over any way of running `git <args>` (the unit-test seam).
///
/// The four git calls, in order: (1) the staged new paths, (2) the new paths of the
/// local commits, (3) the staged patch, (4) the patch of the local commits. Calls 2
/// and 4 are not issued when `rev-parse -q --verify HEAD` fails (an unborn HEAD).
pub fn scan_with(
    run: &mut dyn FnMut(&[&str]) -> io::Result<Output>,
) -> Result<Vec<Hit>, ScanError> {
    let has_head = run(&["rev-parse", "-q", "--verify", "HEAD"])
        .map(|output| output.status.success())
        .unwrap_or(false);

    let mut names = call(
        run,
        1,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--cached",
            "--name-only",
            "--no-renames",
            "--diff-filter=A",
        ],
    )?;
    if has_head {
        names.extend(call(
            run,
            2,
            &[
                "-c",
                "core.quotePath=false",
                "log",
                "--format=",
                "--name-only",
                "--no-renames",
                "--diff-filter=A",
                "HEAD",
                "--not",
                "--remotes=origin",
            ],
        )?);
    }
    let mut patch = call(
        run,
        3,
        &[
            "-c",
            "core.quotePath=false",
            "diff",
            "--cached",
            "--no-color",
            "--no-ext-diff",
            "--no-textconv",
            "-U0",
        ],
    )?;
    if has_head {
        patch.extend(call(
            run,
            4,
            &[
                "-c",
                "core.quotePath=false",
                "log",
                "-p",
                "--format=",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "-U0",
                "HEAD",
                "--not",
                "--remotes=origin",
            ],
        )?);
    }
    Ok(hits(&names, &patch))
}

/// The suspicious paths of a list of new paths (one per line) and a patch.
fn hits(names: &[u8], patch: &[u8]) -> Vec<Hit> {
    let mut found: Vec<Hit> = Vec::new();
    for name in lines(names).filter(|name| !name.is_empty()) {
        if path_re().is_match(name) {
            found.push((name.to_vec(), REASON_FILE_NAME));
        }
    }
    for (file, text) in added_lines(patch) {
        // The script greps the whole `path<TAB>text` line, so a token in the path
        // matches too; `cut -f1` then keeps the first tab-separated field.
        let mut line = file.clone();
        line.push(b'\t');
        line.extend_from_slice(&text);
        if text_re().is_match(&line) {
            let path = file.split(|&b| b == b'\t').next().unwrap_or_default();
            found.push((path.to_vec(), REASON_TEXT));
        }
    }
    // `sort -u` over `<path>\t<reason>`: the whole line is the key.
    found.sort_by_cached_key(|(path, reason)| sort_line(path, reason));
    found.dedup_by(|a, b| sort_line(&a.0, a.1) == sort_line(&b.0, b.1));
    found
}

fn sort_line(path: &[u8], reason: &str) -> Vec<u8> {
    let mut line = path.to_vec();
    line.push(b'\t');
    line.extend_from_slice(reason.as_bytes());
    line
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::process::ExitStatusExt;
    use std::process::ExitStatus;

    fn path_hit(path: &str) -> bool {
        path_re().is_match(path.as_bytes())
    }

    fn text_hit(line: &str) -> bool {
        text_re().is_match(line.as_bytes())
    }

    /// Token-shaped strings are built from fragments: a literal would stop every
    /// `/justin:pause` on this branch (contract §10).
    fn github_token(kind: &str, len: usize) -> String {
        format!(
            "gh{kind}_{}",
            "A1".repeat(len).get(..len).expect("long enough")
        )
    }

    fn pem(kind: &str) -> String {
        ["-----", "BEGIN ", kind, "PRIVATE KEY", "-----"].concat()
    }

    #[test]
    fn path_regex_hits_and_near_misses() {
        for hit in [
            ".env",
            "config/.env",
            ".env.local",
            "a/.env.production",
            "a/.netrc",
            ".npmrc",
            "x/.pypirc",
            "id_rsa",
            "ssh/id_ed25519.pub",
            "id_ecdsa",
            "id_dsa_old",
            "server.pem",
            "A/SERVER.PEM",
            "k.key",
            "a.p12",
            "a.pfx",
            "a.jks",
            "a.keystore",
            "my-credentials.yml",
            "docs/CREDENTIAL",
            "docs/deploy-secret.md",
            "SECRETS",
        ] {
            assert!(path_hit(hit), "{hit} should be flagged");
        }
        for miss in [
            "env",
            ".environment",
            "a/.env/readme",
            "monkey.txt",
            "key",
            "pem",
            "keys/readme.md",
            "id_rsa/readme",
            "id_ed.pub",
            "notes.txt",
            "a.keystore.md",
            "docs/secrets/readme.md",
        ] {
            assert!(!path_hit(miss), "{miss} should not be flagged");
        }
    }

    #[test]
    fn path_regex_folds_ascii_only() {
        // U+017F (long s) and U+212A (Kelvin sign) fold to s and k under Unicode
        // case folding, never under grep -i in the C locale (D8).
        assert!(!path_hit("\u{17f}ecret.txt"));
        assert!(!path_hit("a.\u{212a}ey"));
        assert!(path_hit("SeCrEt.txt"));
        // A path that is not UTF-8 is matched like any other.
        assert!(path_re().is_match(b"caf\xe9/server.PEM"));
        assert!(!path_re().is_match(b"caf\xe9/readme.md"));
    }

    #[test]
    fn text_regex_hits_and_near_misses() {
        assert!(text_hit(&pem("")));
        assert!(text_hit(&pem("RSA ")));
        assert!(text_hit(&pem("OPENSSH ")));
        assert!(text_hit(&pem("EC 2 ")));
        assert!(!text_hit(
            &["-----", "BEGIN ", "PUBLIC KEY", "-----"].concat()
        ));
        assert!(!text_hit(
            &["-----", "BEGIN ", "rsa ", "PRIVATE KEY", "-----"].concat()
        ));

        for kind in ["p", "o", "u", "s", "r"] {
            assert!(text_hit(&github_token(kind, 36)), "gh{kind}_ + 36");
            assert!(!text_hit(&github_token(kind, 35)), "gh{kind}_ + 35");
        }
        assert!(!text_hit(&github_token("x", 36)));
        assert!(text_hit(&["github", "_pat_", &"a_".repeat(11)].concat()));
        assert!(!text_hit(&["github", "_pat_", &"a".repeat(21)].concat()));
        assert!(text_hit(&["sk", "-ant-", &"b-".repeat(10)].concat()));
        assert!(!text_hit(&["sk", "-ant-", &"b".repeat(19)].concat()));
        assert!(text_hit(&["AK", "IA", &"7".repeat(16)].concat()));
        assert!(!text_hit(&["AK", "IA", &"7".repeat(15)].concat()));
        assert!(!text_hit(&["AK", "IA", &"a".repeat(16)].concat()));
        for kind in ["a", "b", "p", "o", "s", "r"] {
            assert!(text_hit(&["xo", "x", kind, "-", &"1".repeat(10)].concat()));
            assert!(!text_hit(&["xo", "x", kind, "-", &"1".repeat(9)].concat()));
        }
        assert!(!text_hit(&["xo", "xz-", &"1".repeat(10)].concat()));
        assert!(!text_hit("an ordinary line of text"));
    }

    #[test]
    fn added_lines_follows_the_awk() {
        let patch = b"\
diff --git a/old.txt b/old.txt
deleted file mode 100644
--- a/old.txt
+++ /dev/null
@@ -1 +0,0 @@
-gone
diff --git a/dir/new file.txt b/dir/new file.txt
new file mode 100644
--- /dev/null
+++ b/dir/new file.txt\t
@@ -0,0 +1,2 @@
+first
++ not a header
diff --git a/q.txt b/q.txt
--- a/q.txt
+++ \"b/caf\\303\\251.txt\"
@@ -1 +1,2 @@
+quoted
\\ No newline at end of file
+last, no newline";
        let got = added_lines(patch);
        let as_text: Vec<(String, String)> = got
            .iter()
            .map(|(f, t)| {
                (
                    String::from_utf8_lossy(f).into_owned(),
                    String::from_utf8_lossy(t).into_owned(),
                )
            })
            .collect();
        assert_eq!(
            as_text,
            vec![
                ("dir/new file.txt\t".to_string(), "first".to_string()),
                (
                    "dir/new file.txt\t".to_string(),
                    "+ not a header".to_string()
                ),
                ("\"b/caf\\303\\251.txt\"".to_string(), "quoted".to_string()),
                (
                    "\"b/caf\\303\\251.txt\"".to_string(),
                    "last, no newline".to_string()
                ),
            ]
        );
        // `+++ /dev/null` names /dev/null, and its (impossible) added lines would
        // carry that name.
        let null = added_lines(b"diff --git a/x b/x\n+++ /dev/null\n@@ -0,0 +1 @@\n+text\n");
        assert_eq!(null, vec![(b"/dev/null".to_vec(), b"text".to_vec())]);
        assert!(added_lines(b"").is_empty());
    }

    #[test]
    fn hits_sort_and_dedup_bytewise_and_cut_the_tab() {
        let token = github_token("p", 36);
        let patch = format!(
            "diff --git a/b b/b\n+++ b/dir/a b.txt\t\n@@ -0,0 +1 @@\n+{token}\n\
             diff --git a/z b/z\n+++ b/z.txt\n@@ -0,0 +2 @@\n+{token}\n+{token}\n"
        );
        let names = "z.pem\nA.pem\nz.pem\nplain.txt\n\n";
        let got = hits(names.as_bytes(), patch.as_bytes());
        let as_text: Vec<(String, &str)> = got
            .into_iter()
            .map(|(p, r)| (String::from_utf8(p).expect("UTF-8"), r))
            .collect();
        assert_eq!(
            as_text,
            vec![
                ("A.pem".to_string(), REASON_FILE_NAME),
                ("dir/a b.txt".to_string(), REASON_TEXT),
                ("z.pem".to_string(), REASON_FILE_NAME),
                ("z.txt".to_string(), REASON_TEXT),
            ]
        );
        // The whole line is the sort key: 0x01 sorts before the tab that ends "a".
        let got = hits(b"a.pem\na\x01.pem\n", b"");
        assert_eq!(got[0].0, b"a\x01.pem");
    }

    /// A synthetic `Output` that exits `code` with `stdout` and `stderr`.
    fn output(code: i32, stdout: &str, stderr: &str) -> Output {
        Output {
            status: ExitStatus::from_raw(code << 8),
            stdout: stdout.as_bytes().to_vec(),
            stderr: stderr.as_bytes().to_vec(),
        }
    }

    /// Which scan call (1 to 4) the argv is, or 0 for `rev-parse`.
    fn which(args: &[&str]) -> usize {
        match args {
            ["rev-parse", ..] => 0,
            ["-c", _, "diff", "--cached", "--name-only", ..] => 1,
            ["-c", _, "log", "--format=", ..] => 2,
            ["-c", _, "diff", "--cached", "--no-color", ..] => 3,
            ["-c", _, "log", "-p", ..] => 4,
            other => panic!("an unexpected git call: {other:?}"),
        }
    }

    #[test]
    fn scan_fails_closed_on_each_git_call() {
        for failing in 1..=4 {
            let mut issued = Vec::new();
            let mut run = |args: &[&str]| {
                let n = which(args);
                issued.push(n);
                Ok(if n == failing {
                    output(128, "", "fatal: broken call\nsecond line\n")
                } else {
                    output(0, "", "")
                })
            };
            assert_eq!(
                scan_with(&mut run),
                Err(ScanError {
                    call: failing,
                    line: b"fatal: broken call".to_vec()
                }),
                "call {failing} fails"
            );
        }
        // A call that exits non-zero with no stderr still fails, with a fallback line.
        let mut run = |args: &[&str]| Ok(output(if which(args) == 3 { 1 } else { 0 }, "", ""));
        let err = scan_with(&mut run).expect_err("call 3 failed");
        assert_eq!(err.call, 3);
        assert!(!err.line.is_empty());

        // The fifth row: call 1 does not even spawn.
        let mut run = |args: &[&str]| {
            if which(args) == 1 {
                Err(io::Error::new(io::ErrorKind::NotFound, "no git here"))
            } else {
                Ok(output(0, "", ""))
            }
        };
        assert_eq!(
            scan_with(&mut run),
            Err(ScanError {
                call: 1,
                line: b"no git here".to_vec()
            })
        );

        // The sixth row: an unborn HEAD issues neither `log` call, and the scan is Ok.
        let mut issued = Vec::new();
        let mut run = |args: &[&str]| {
            let n = which(args);
            issued.push(n);
            Ok(if n == 0 {
                output(1, "", "")
            } else {
                output(0, "", "")
            })
        };
        assert_eq!(scan_with(&mut run), Ok(Vec::new()));
        assert_eq!(issued, vec![0, 1, 3]);
    }

    #[test]
    fn scan_with_reads_all_four_calls() {
        let token = github_token("s", 36);
        let mut run = |args: &[&str]| {
            Ok(match which(args) {
                0 => output(0, "", ""),
                1 => output(0, "staged/server.pem\n", ""),
                2 => output(0, "earlier/.env\n", ""),
                3 => output(
                    0,
                    &format!("diff --git a/s b/s\n+++ b/s.txt\n@@ -0,0 +1 @@\n+{token}\n"),
                    "",
                ),
                _ => output(
                    0,
                    "diff --git a/t b/t\n+++ b/t.txt\n@@ -0,0 +1 @@\n+nothing here\n",
                    "",
                ),
            })
        };
        let got = scan_with(&mut run).expect("the scan runs");
        let as_text: Vec<(String, &str)> = got
            .into_iter()
            .map(|(p, r)| (String::from_utf8(p).expect("UTF-8"), r))
            .collect();
        assert_eq!(
            as_text,
            vec![
                ("earlier/.env".to_string(), REASON_FILE_NAME),
                ("s.txt".to_string(), REASON_TEXT),
                ("staged/server.pem".to_string(), REASON_FILE_NAME),
            ]
        );
    }
}
