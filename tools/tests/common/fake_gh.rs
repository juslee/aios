//! The one fake `gh` of the agent-script tests (contract §5), shared by the brief and
//! checkpoint parity tests and by the loop's scenarios.
//!
//! Layout of one fake root (made at run time under `CARGO_TARGET_TMPDIR`):
//!
//! ```text
//! <root>/bin/gh                    the #!/bin/sh fake (SCRIPT); finds <root> as "$(dirname "$0")/.."
//! <root>/gh/<NN>-<label>/match     one extended regex (grep -E -f), matched against the rendered argv line
//! <root>/gh/<NN>-<label>/out[.K]   stdout   (default empty)
//! <root>/gh/<NN>-<label>/err[.K]   stderr   (default empty)
//! <root>/gh/<NN>-<label>/rc[.K]    exit code (default 0)
//! <root>/gh-state/<label>.n        how many calls this fixture has served
//! <root>/gh-calls/<SEQ>/           one directory per call: argv (NUL-separated), cwd,
//!                                  env, fixture (the label, empty for no match), body
//! <root>/fake-stderr               everything the fake wrote to stderr
//! ```
//!
//! Fixtures are tried in `<NN>` order and the first whose regex matches the rendered
//! argv line serves the call. The K-th call to a fixture serves `out.K`, `err.K` and
//! `rc.K`. Each stream (out, err, rc) is picked on its own: the exact `.K` file if there
//! is one; else the highest `.K'` that stream has, for any K'; else the unsuffixed file;
//! else the default. So a changing state sticks at its last value, and a fixture with
//! only `out.2` serves `out.2` on call 1 as well. An unknown call
//! is logged with an empty fixture, prints `fake gh: no fixture for: <line>` on stderr
//! and exits 99, which is reserved for harness bugs: a gh failure is a fixture with a
//! non-zero `rc`. A missing `gh` is simply no `<root>/bin/gh`.
//!
//! This module uses only `super::{isolated, unique_dir}` and `jail` (contract A3).

use std::cell::Cell;
use std::collections::BTreeMap;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use regex::Regex;

/// The fake `gh`. Its external commands are `jail::FAKE_GH_TOOLS` (checked by
/// `agent_support::jail_covers_every_script_command`).
pub const SCRIPT: &str = r#"#!/bin/sh
# The fake gh of tools/tests/common/fake_gh.rs. The root is found from $0, never from
# the environment, because the tests strip every GH_* and AIOS_* variable.
root=$(cd "$(dirname "$0")/.." && pwd -P)

line=
if [ $# -gt 0 ]; then
    line=$(printf '%s ' "$@" | tr '\n' ' ')
fi

say() {
    printf '%s\n' "$1" >&2
    printf '%s\n' "$1" >>"$root/fake-stderr"
}

n=0
for d in "$root"/gh-calls/*; do
    [ -d "$d" ] && n=$((n + 1))
done
seq=$(printf '%06d' $((n + 1)))
call=$root/gh-calls/$seq
if ! mkdir "$call" 2>/dev/null; then
    say "fake gh: call directory $seq already exists (concurrent calls)"
    exit 98
fi

if [ $# -gt 0 ]; then
    printf '%s\0' "$@" >"$call/argv"
else
    : >"$call/argv"
fi
pwd -P >"$call/cwd"
{
    if [ "${GH_PROMPT_DISABLED+set}" = set ]; then printf 'GH_PROMPT_DISABLED=%s\n' "$GH_PROMPT_DISABLED"; else echo 'GH_PROMPT_DISABLED=<unset>'; fi
    if [ "${GH_NO_UPDATE_NOTIFIER+set}" = set ]; then printf 'GH_NO_UPDATE_NOTIFIER=%s\n' "$GH_NO_UPDATE_NOTIFIER"; else echo 'GH_NO_UPDATE_NOTIFIER=<unset>'; fi
    if [ "${NO_COLOR+set}" = set ]; then printf 'NO_COLOR=%s\n' "$NO_COLOR"; else echo 'NO_COLOR=<unset>'; fi
    if [ "${GH_REPO+set}" = set ]; then printf 'GH_REPO=%s\n' "$GH_REPO"; else echo 'GH_REPO=<unset>'; fi
    if [ "${GH_HOST+set}" = set ]; then printf 'GH_HOST=%s\n' "$GH_HOST"; else echo 'GH_HOST=<unset>'; fi
    if [ "${LC_ALL+set}" = set ]; then printf 'LC_ALL=%s\n' "$LC_ALL"; else echo 'LC_ALL=<unset>'; fi
} >"$call/env"

# The first file named by --body-file F, --input F or a field value @F (-F/--field k=@F).
src=
prev=
for a in "$@"; do
    if [ -z "$src" ]; then
        case "$prev" in
            --body-file | --input) src=$a ;;
            -F | --field)
                case "$a" in *=@*) src=${a#*=@} ;; esac
                ;;
        esac
        case "$a" in
            --body-file=* | --input=*) src=${a#*=} ;;
            --field=*=@* | -F*=@*) src=${a#*=@} ;;
        esac
    fi
    prev=$a
done
if [ -n "$src" ] && [ -f "$src" ]; then
    cp "$src" "$call/body"
fi

label=
dir=
for d in "$root"/gh/*/; do
    [ -f "${d}match" ] || continue
    if printf '%s\n' "$line" | grep -Eq -f "${d}match"; then
        dir=${d%/}
        name=${dir##*/}
        label=${name#*-}
        break
    fi
done
printf '%s' "$label" >"$call/fixture"

if [ -z "$label" ]; then
    say "fake gh: no fixture for: $line"
    exit 99
fi

k=0
if [ -f "$root/gh-state/$label.n" ]; then
    k=$(cat "$root/gh-state/$label.n")
fi
k=$((k + 1))
printf '%s\n' "$k" >"$root/gh-state/$label.n"

pick() {
    if [ -f "$dir/$1.$k" ]; then
        printf '%s\n' "$dir/$1.$k"
        return
    fi
    best=0
    for c in "$dir/$1".[0-9]*; do
        [ -f "$c" ] || continue
        j=${c##*.}
        case "$j" in *[!0-9]*) continue ;; esac
        if [ "$j" -gt "$best" ]; then
            best=$j
        fi
    done
    if [ "$best" -gt 0 ]; then
        printf '%s\n' "$dir/$1.$best"
    elif [ -f "$dir/$1" ]; then
        printf '%s\n' "$dir/$1"
    fi
}

f=$(pick out)
if [ -n "$f" ]; then
    cat "$f"
fi
f=$(pick err)
if [ -n "$f" ]; then
    cat "$f" >&2
    cat "$f" >>"$root/fake-stderr"
fi
rc=0
f=$(pick rc)
if [ -n "$f" ]; then
    rc=$(cat "$f")
fi
exit "$rc"
"#;

/// The rendered argv line the fake matches: every word followed by one space, each
/// newline in a word replaced by a space. Identical to the script's
/// `printf '%s ' "$@" | tr '\n' ' '`.
pub fn render<S: AsRef<[u8]>>(args: &[S]) -> String {
    let mut line = String::new();
    for arg in args {
        line.push_str(&String::from_utf8_lossy(arg.as_ref()).replace('\n', " "));
        line.push(' ');
    }
    line
}

/// Whether any of the regexes in `allow` matches the rendered line.
pub fn line_allowed(allow: &[&str], line: &str) -> bool {
    allow.iter().any(|pattern| {
        Regex::new(pattern)
            .expect("a valid allowlist regex")
            .is_match(line)
    })
}

/// One logged call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhCall {
    pub argv: Vec<Vec<u8>>,
    pub cwd: PathBuf,
    /// `GH_PROMPT_DISABLED`, `GH_NO_UPDATE_NOTIFIER`, `NO_COLOR`, `GH_REPO`, `GH_HOST`
    /// and `LC_ALL`: `None` when unset.
    pub env: BTreeMap<String, Option<String>>,
    /// The matched fixture's label; empty for an unknown call.
    pub fixture: String,
    /// A copy of the first file named by `--body-file`, `--input` or `-F k=@F`.
    pub body: Option<Vec<u8>>,
}

impl GhCall {
    /// The rendered argv line, as the fake matched it.
    pub fn line(&self) -> String {
        render(&self.argv)
    }
}

/// A fake `gh` installed under a root directory.
pub struct FakeGh {
    root: PathBuf,
    next: Cell<usize>,
}

impl FakeGh {
    /// Create `<root>/bin/gh` and the (empty) fixture, state and call directories.
    pub fn install(root: &Path) -> FakeGh {
        let bin = root.join("bin");
        for dir in [
            &bin,
            &root.join("gh"),
            &root.join("gh-state"),
            &root.join("gh-calls"),
        ] {
            std::fs::create_dir_all(dir).expect("create a fake gh directory");
        }
        let script = bin.join("gh");
        std::fs::write(&script, SCRIPT).expect("write the fake gh");
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
            .expect("make the fake gh executable");
        std::fs::write(root.join("fake-stderr"), b"").expect("create fake-stderr");
        FakeGh {
            root: root.to_path_buf(),
            next: Cell::new(0),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The directory holding the fake `gh`, for `jail::hermetic`'s `fake_bin`.
    pub fn bin(&self) -> PathBuf {
        self.root.join("bin")
    }

    /// Add a fixture. Fixtures are tried in the order they are added, so add a
    /// specific one (`pr list .*--head `) before a general one (`^pr list `).
    pub fn fixture(&self, label: &str, regex: &str) -> Fixture {
        assert!(
            !label.is_empty()
                && label
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"-._".contains(&b)),
            "a fixture label is [a-z0-9._-]+, not {label:?}"
        );
        let n = self.next.get();
        assert!(n < 100, "at most 99 fixtures per fake root");
        self.next.set(n + 1);
        let taken = std::fs::read_dir(self.root.join("gh"))
            .expect("read the fixture directory")
            .filter_map(Result::ok)
            .any(|e| {
                e.file_name()
                    .to_string_lossy()
                    .split_once('-')
                    .is_some_and(|(_, existing)| existing == label)
            });
        assert!(!taken, "fixture label {label:?} is already installed");
        Regex::new(regex).unwrap_or_else(|e| panic!("fixture {label}: bad regex {regex:?}: {e}"));
        let dir = self.root.join("gh").join(format!("{n:02}-{label}"));
        std::fs::create_dir_all(&dir).expect("create the fixture directory");
        std::fs::write(dir.join("match"), format!("{regex}\n")).expect("write the match file");
        Fixture { dir }
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<GhCall> {
        let mut dirs: Vec<PathBuf> = std::fs::read_dir(self.root.join("gh-calls"))
            .expect("read the call log")
            .filter_map(Result::ok)
            .map(|e| e.path())
            .collect();
        dirs.sort();
        dirs.iter().map(|dir| read_call(dir)).collect()
    }

    /// Everything the fake wrote to stderr.
    pub fn fake_stderr(&self) -> Vec<u8> {
        std::fs::read(self.root.join("fake-stderr")).unwrap_or_default()
    }

    /// Every call matched a fixture: no call fell through to exit 99.
    pub fn assert_all_matched(&self) {
        let unmatched: Vec<String> = self
            .calls()
            .iter()
            .filter(|call| call.fixture.is_empty())
            .map(GhCall::line)
            .collect();
        assert!(
            unmatched.is_empty(),
            "gh calls with no fixture (the fake exited 99): {unmatched:#?}"
        );
    }

    /// Every call's rendered line matches one of the `allow` regexes.
    pub fn assert_calls_match(&self, allow: &[&str]) {
        let outside: Vec<String> = self
            .calls()
            .iter()
            .map(GhCall::line)
            .filter(|line| !line_allowed(allow, line))
            .collect();
        assert!(
            outside.is_empty(),
            "gh calls outside the allowlist {allow:?}: {outside:#?}"
        );
    }

    /// No call's rendered line matches any of the `deny` regexes.
    pub fn assert_none_match(&self, deny: &[&str]) {
        let denied: Vec<String> = self
            .calls()
            .iter()
            .map(GhCall::line)
            .filter(|line| line_allowed(deny, line))
            .collect();
        assert!(
            denied.is_empty(),
            "gh calls that match the deny list {deny:?}: {denied:#?}"
        );
    }
}

fn read_call(dir: &Path) -> GhCall {
    let argv_raw = std::fs::read(dir.join("argv")).expect("read the call's argv");
    let mut argv: Vec<Vec<u8>> = argv_raw.split(|&b| b == 0).map(<[u8]>::to_vec).collect();
    // The file ends with the last word's NUL; an empty file is no words at all.
    argv.pop();
    let cwd = std::fs::read(dir.join("cwd")).expect("read the call's cwd");
    let cwd = PathBuf::from(std::ffi::OsStr::from_bytes(
        cwd.strip_suffix(b"\n").unwrap_or(&cwd),
    ));
    let env = String::from_utf8_lossy(&std::fs::read(dir.join("env")).expect("read the env"))
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(k, v)| (k.to_string(), (v != "<unset>").then(|| v.to_string())))
        .collect();
    let fixture = std::fs::read_to_string(dir.join("fixture")).expect("read the fixture label");
    GhCall {
        argv,
        cwd,
        env,
        fixture: fixture.trim().to_string(),
        body: std::fs::read(dir.join("body")).ok(),
    }
}

/// The files of one fixture. The setters return `&Self`, so they chain.
pub struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn put(&self, name: &str, data: &[u8]) -> &Fixture {
        std::fs::write(self.dir.join(name), data).expect("write a fixture file");
        self
    }

    /// stdout of every call without a more specific `out.K`.
    pub fn out(&self, data: impl AsRef<[u8]>) -> &Fixture {
        self.put("out", data.as_ref())
    }

    /// stderr of every call without a more specific `err.K`.
    pub fn err(&self, data: impl AsRef<[u8]>) -> &Fixture {
        self.put("err", data.as_ref())
    }

    /// The exit code of every call without a more specific `rc.K`.
    pub fn rc(&self, code: i32) -> &Fixture {
        self.put("rc", format!("{code}\n").as_bytes())
    }

    /// What the K-th call (counting from 1) serves.
    pub fn nth(&self, k: usize, out: impl AsRef<[u8]>, err: impl AsRef<[u8]>, rc: i32) -> &Fixture {
        assert!(k >= 1, "calls count from 1");
        self.put(&format!("out.{k}"), out.as_ref());
        self.put(&format!("err.{k}"), err.as_ref());
        self.put(&format!("rc.{k}"), format!("{rc}\n").as_bytes())
    }
}
