//! The Soak section: the newest soak of a main commit, the newest other soak and the
//! newest interleaved (`aios soak --arm`) soak, found under `target/soak/` of every
//! worktree.
//!
//! A run counts as a main soak only when it finished and its commit is on origin/main
//! with no uncommitted changes; everything else is reported separately, so it is never
//! read as main's state. An interleaved soak (a directory holding `arms.tsv`) is one run
//! of its own, listed with the status line of its `summary.md` and each arm's CLEAN
//! count, and never as the main soak; its `arm-X/` directories are not listed.
//!
//! Order is bytewise (divergence D3, D7): the run directories of a worktree sort as bytes,
//! the worktrees come in `git worktree list` order, and a tie on the modification time
//! keeps the first.
//!
//! The classes and the columns of `summary.tsv` come from the soak report
//! ([`SCRIPT_COLUMNS`], [`Class`]).

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::io::Write;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::clock::{fmt_epoch, mtime};
use super::git_section::{branch_or_detached, out_os};
use super::phase::status_of;
use super::{line, Ctx};
use crate::cmd::checkpoint::scan::lines;
use crate::cmd::soak::classify::Class;
use crate::cmd::soak::report::SCRIPT_COLUMNS;
use crate::git;

/// A run found in a worktree.
struct Run {
    dir: PathBuf,
    worktree: PathBuf,
    mtime: i64,
}

impl Run {
    /// Keep `candidate` when it is strictly newer than `slot` (a tie keeps the first).
    fn take_if_newer(slot: &mut Option<Run>, candidate: Run) {
        if slot.as_ref().map_or(0, |run| run.mtime) < candidate.mtime {
            *slot = Some(candidate);
        }
    }
}

/// The directories under `base` that hold a `summary.md`, `summary.tsv` or `arms.tsv`
/// regular file, sorted bytewise without duplicates. Symbolic links are not followed
/// (`find -type f`).
fn run_dirs(base: &Path) -> Vec<PathBuf> {
    let mut found: BTreeSet<Vec<u8>> = BTreeSet::new();
    let mut pending = vec![base.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file()
                && ["summary.md", "summary.tsv", "arms.tsv"]
                    .iter()
                    .any(|name| entry.file_name() == OsStr::new(name))
            {
                found.insert(dir.as_os_str().as_bytes().to_vec());
            }
        }
    }
    found
        .into_iter()
        .map(|bytes| PathBuf::from(std::ffi::OsString::from_vec(bytes)))
        .collect()
}

/// The first line of `text` that starts with `prefix`.
fn first_with_prefix<'a>(text: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    lines(text).find(|line| line.starts_with(prefix))
}

/// The commit a run recorded: the backticked word of the first `| Commit |` row of its
/// `summary.md`, empty when that row has none.
fn soak_commit(dir: &Path) -> Vec<u8> {
    let md = std::fs::read(dir.join("summary.md")).unwrap_or_default();
    first_with_prefix(&md, b"| Commit |")
        .and_then(|row| row.strip_prefix(b"| Commit | `"))
        .and_then(|rest| {
            rest.iter()
                .position(|&b| b == b'`')
                .map(|end| rest[..end].to_vec())
        })
        .unwrap_or_default()
}

/// A recorded commit's relation to origin/main, and whether it is a clean main commit.
fn soak_relation(root: &Path, tip: Option<&[u8]>, raw: &[u8]) -> (Vec<u8>, bool) {
    if raw.is_empty() {
        return (b"commit not recorded".to_vec(), false);
    }
    let commit = raw.strip_suffix(b"-dirty").unwrap_or(raw);
    // The recorded text is arbitrary bytes and may name a ref, so it reaches git as it is.
    let spec = [commit, b"^{commit}"].concat();
    let Some(full) = out_os(
        root,
        &[
            OsStr::new("rev-parse"),
            OsStr::new("-q"),
            OsStr::new("--verify"),
            OsStr::from_bytes(&spec),
        ],
    ) else {
        return (b"commit not in this repository".to_vec(), false);
    };
    let Some(tip) = tip else {
        return (b"origin/main unknown".to_vec(), false);
    };
    let (full_text, tip_text) = (
        String::from_utf8_lossy(&full).into_owned(),
        String::from_utf8_lossy(tip).into_owned(),
    );
    let relation: Vec<u8> = if full == tip {
        b"origin/main tip".to_vec()
    } else if git::ok(
        root,
        &["merge-base", "--is-ancestor", &full_text, &tip_text],
    ) {
        let behind = git::out(
            root,
            &["rev-list", "--count", &format!("{full_text}..{tip_text}")],
        )
        .unwrap_or_default();
        [&b"on main, "[..], &behind, b" commit(s) behind origin/main"].concat()
    } else {
        b"not on main".to_vec()
    };
    if commit != raw {
        return (
            [&relation[..], b", tree had uncommitted changes"].concat(),
            false,
        );
    }
    let on_main = relation != b"not on main";
    (relation, on_main)
}

/// The `| Boots |` row of a summary, with the table cell markers removed when the row
/// has the closing ` |`; a row without it is printed whole.
fn boots_cell(md: &[u8]) -> Vec<u8> {
    const OPEN: &[u8] = b"| Boots | ";
    const CLOSE: &[u8] = b" |";
    let Some(row) = first_with_prefix(md, b"| Boots |") else {
        return Vec::new();
    };
    if row.len() >= OPEN.len() + CLOSE.len() && row.starts_with(OPEN) && row.ends_with(CLOSE) {
        row[OPEN.len()..row.len() - CLOSE.len()].to_vec()
    } else {
        row.to_vec()
    }
}

/// The fields of a tab-separated line, as awk's `-F'\t'` splits them (an empty line has
/// none).
fn tab_fields(row: &[u8]) -> Vec<&[u8]> {
    if row.is_empty() {
        Vec::new()
    } else {
        row.split(|&b| b == b'\t').collect()
    }
}

/// The class counts and the mode (the last row's) of a `summary.tsv`: every line after the
/// header is a row, whatever it holds.
fn count_rows(tsv: &[u8]) -> (usize, HashMap<Vec<u8>, usize>, Vec<u8>) {
    let position = |name: &str| {
        SCRIPT_COLUMNS
            .iter()
            .position(|column| *column == name)
            .expect("the summary has this column")
    };
    let (mode_at, class_at) = (position("mode"), position("class"));
    let mut counts: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut mode: &[u8] = b"";
    let mut rows = 0;
    for row in lines(tsv).skip(1) {
        let fields = tab_fields(row);
        rows += 1;
        *counts
            .entry(fields.get(class_at).copied().unwrap_or_default().to_vec())
            .or_default() += 1;
        mode = fields.get(mode_at).copied().unwrap_or_default();
    }
    (rows, counts, mode.to_vec())
}

/// The class line of a run's `summary.tsv`. `partial` is ` so far` while the run has no
/// `summary.md` yet.
fn class_line(tsv: &[u8], partial: &str) -> Vec<u8> {
    let (rows, counts, mode) = count_rows(tsv);
    if rows == 0 {
        return b"  - summary.tsv has no rows yet".to_vec();
    }
    let count = |name: &str| counts.get(name.as_bytes()).copied().unwrap_or(0);
    let mut shown = vec![format!(
        "{} {}/{rows}",
        Class::Clean.name(),
        count(Class::Clean.name())
    )];
    for class in [
        Class::PcZero,
        Class::PanicLock,
        Class::Panic,
        Class::Exception,
        Class::WedgeStuck,
        Class::WedgeAlive,
        Class::Degraded,
    ] {
        shown.push(format!("{} {}", class.name(), count(class.name())));
    }
    // A run from before crash-fix step 1a records WEDGE, not WEDGE-STUCK or WEDGE-ALIVE.
    let old = match count("WEDGE") {
        0 => String::new(),
        n => format!(", WEDGE {n}"),
    };
    [
        &b"  - "[..],
        &mode,
        format!(" mode{partial}: {}{old}", shown.join(", ")).as_bytes(),
    ]
    .concat()
}

fn print_soak(
    ctx: &Ctx,
    tip: Option<&[u8]>,
    label: &str,
    run: &Run,
    out: &mut dyn Write,
) -> Result<()> {
    let branch = branch_or_detached(&run.worktree);
    let when = fmt_epoch(run.mtime, "+%Y-%m-%d %H:%M");
    let md = std::fs::read(run.dir.join("summary.md")).ok();
    let head = [
        format!("- {label}: ").as_bytes(),
        &ctx.rel_path(&run.dir),
        b" (worktree on `",
        &branch,
        b"`, ",
        &when,
        b")",
    ]
    .concat();
    line(out, &[&head])?;
    match &md {
        Some(text) => {
            let commit = soak_commit(&run.dir);
            let (relation, _) = soak_relation(&ctx.root, tip, &commit);
            let boots = boots_cell(text);
            let commit_shown: &[u8] = if commit.is_empty() { b"?" } else { &commit };
            let boots_shown: &[u8] = if boots.is_empty() {
                b"boots not recorded"
            } else {
                &boots
            };
            line(
                out,
                &[
                    b"  - commit ",
                    commit_shown,
                    b" (",
                    &relation,
                    b"); ",
                    boots_shown,
                ],
            )?;
        }
        None => line(
            out,
            &[b"  - partial: no summary.md yet (run in progress or aborted); commit not recorded"],
        )?,
    }
    if let Ok(tsv) = std::fs::read(run.dir.join("summary.tsv")) {
        let partial = if md.is_some() { "" } else { " so far" };
        line(out, &[&class_line(&tsv, partial)])?;
    } else if let Some(text) = &md {
        if let Some(rate) = first_with_prefix(text, b"CLEAN rate:") {
            line(out, &[b"  - ", rate])?;
        }
    }
    Ok(())
}

/// awk's `$col[file, "name"]`: the field a header named, or the whole line when the
/// header has no such name (an empty subscript is field 0).
fn field<'a>(row: &'a [u8], fields: &[&'a [u8]], at: Option<usize>) -> &'a [u8] {
    match at {
        Some(n) if n > 0 => fields.get(n - 1).copied().unwrap_or_default(),
        _ => row,
    }
}

/// The CLEAN count per arm: the awk over `arms.tsv` and `boots.tsv`, columns by header
/// name. `None` when a file cannot be read: the awk of the macOS recording stops before
/// its END block there, and prints nothing.
fn per_arm_line(arms: &Path, boots: &Path) -> Option<Vec<u8>> {
    let files = [std::fs::read(arms).ok()?, std::fs::read(boots).ok()?];
    let mut file = 0usize;
    let mut col: HashMap<(usize, Vec<u8>), usize> = HashMap::new();
    let mut order: Vec<Vec<u8>> = Vec::new();
    let mut commit: HashMap<Vec<u8>, Vec<u8>> = HashMap::new();
    let mut boot_count: HashMap<Vec<u8>, usize> = HashMap::new();
    let mut clean: HashMap<Vec<u8>, usize> = HashMap::new();
    for data in &files {
        for (index, row) in lines(data).enumerate() {
            let fields = tab_fields(row);
            if index == 0 {
                file += 1;
                for (i, name) in fields.iter().enumerate() {
                    col.insert((file, name.to_vec()), i + 1);
                }
                continue;
            }
            let at = |name: &str| col.get(&(file, name.as_bytes().to_vec())).copied();
            if file == 1 {
                let arm = field(row, &fields, at("arm")).to_vec();
                commit.insert(arm.clone(), field(row, &fields, at("commit")).to_vec());
                order.push(arm);
                continue;
            }
            let arm = field(row, &fields, at("arm")).to_vec();
            *boot_count.entry(arm.clone()).or_default() += 1;
            if field(row, &fields, at("class")) == b"CLEAN" {
                *clean.entry(arm).or_default() += 1;
            }
        }
    }
    if order.is_empty() {
        return Some(b"  - arms.tsv lists no arms".to_vec());
    }
    let mut text = b"  - CLEAN per arm:".to_vec();
    for (i, arm) in order.iter().enumerate() {
        if i > 0 {
            text.push(b';');
        }
        text.extend_from_slice(b" arm ");
        text.extend_from_slice(arm);
        text.extend_from_slice(b" `");
        text.extend_from_slice(commit.get(arm).map_or(&[][..], Vec::as_slice));
        text.extend_from_slice(
            format!(
                "` {}/{}",
                clean.get(arm).copied().unwrap_or(0),
                boot_count.get(arm).copied().unwrap_or(0)
            )
            .as_bytes(),
        );
    }
    Some(text)
}

fn print_interleaved(ctx: &Ctx, run: &Run, out: &mut dyn Write) -> Result<()> {
    let branch = branch_or_detached(&run.worktree);
    let md = std::fs::read(run.dir.join("summary.md")).unwrap_or_default();
    let status = status_of(&md);
    line(
        out,
        &[
            b"- newest interleaved soak (an A/B experiment, not main's state): ",
            &ctx.rel_path(&run.dir),
            b" (worktree on `",
            &branch,
            b"`, ",
            &fmt_epoch(run.mtime, "+%Y-%m-%d %H:%M"),
            b")",
        ],
    )?;
    let shown: &[u8] = if status.is_empty() {
        b"not recorded (no summary.md)"
    } else {
        status
    };
    line(out, &[b"  - status: ", shown])?;
    if let Some(per_arm) = per_arm_line(&run.dir.join("arms.tsv"), &run.dir.join("boots.tsv")) {
        line(out, &[&per_arm])?;
    }
    Ok(())
}

/// Print the section body.
pub fn print(ctx: &Ctx, out: &mut dyn Write) -> Result<()> {
    let tip = git::out(
        &ctx.root,
        &["rev-parse", "-q", "--verify", "refs/remotes/origin/main"],
    )
    .filter(|tip| !tip.is_empty());
    let (mut main, mut other, mut interleaved): (Option<Run>, Option<Run>, Option<Run>) =
        (None, None, None);
    for worktree in &ctx.worktrees {
        let base = worktree.join("target/soak");
        if worktree.as_os_str().is_empty() || !base.is_dir() {
            continue;
        }
        for dir in run_dirs(&base) {
            // An arm-X/ directory of an interleaved soak is part of that run.
            if dir
                .parent()
                .is_some_and(|parent| parent.join("arms.tsv").is_file())
            {
                continue;
            }
            if dir.join("arms.tsv").is_file() {
                let newest = ["summary.md", "boots.tsv"]
                    .iter()
                    .map(|name| dir.join(name))
                    .filter(|path| path.is_file())
                    .map(|path| mtime(&path))
                    .fold(mtime(&dir.join("arms.tsv")), i64::max);
                Run::take_if_newer(
                    &mut interleaved,
                    Run {
                        dir,
                        worktree: worktree.clone(),
                        mtime: newest,
                    },
                );
                continue;
            }
            let md = dir.join("summary.md");
            let stamp = if md.is_file() {
                mtime(&md)
            } else if dir.join("summary.tsv").is_file() {
                mtime(&dir.join("summary.tsv"))
            } else {
                continue;
            };
            let run = Run {
                dir,
                worktree: worktree.clone(),
                mtime: stamp,
            };
            let on_main =
                md.is_file() && soak_relation(&ctx.root, tip.as_deref(), &soak_commit(&run.dir)).1;
            Run::take_if_newer(if on_main { &mut main } else { &mut other }, run);
        }
    }
    if main.is_none() && other.is_none() && interleaved.is_none() {
        return line(
            out,
            &[b"- no soak results (no summary.* under target/soak/ in any worktree); run `just soak` on main"],
        );
    }
    match &main {
        Some(run) => print_soak(ctx, tip.as_deref(), "main soak", run, out)?,
        None => line(
            out,
            &[b"- main soak: none (no finished run of a commit on origin/main without local changes)"],
        )?,
    }
    if let Some(run) = &other {
        print_soak(
            ctx,
            tip.as_deref(),
            "newest other soak (not main's state)",
            run,
            out,
        )?;
    }
    if let Some(run) = &interleaved {
        print_interleaved(ctx, run, out)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn soak_commit_reads_the_first_commit_row() {
        let dir = std::env::temp_dir().join(format!("aios-brief-soak-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the directory");
        std::fs::write(
            dir.join("summary.md"),
            "| Field | Value |\n| Commit | `abc123-dirty` | more |\n| Commit | `second` |\n",
        )
        .expect("write the summary");
        assert_eq!(soak_commit(&dir), b"abc123-dirty");
        // A first row without backticks gives nothing, even when a later row has them.
        std::fs::write(
            dir.join("summary.md"),
            "| Commit | abc |\n| Commit | `x` |\n",
        )
        .expect("write the summary");
        assert_eq!(soak_commit(&dir), b"");
        std::fs::remove_dir_all(&dir).expect("remove the directory");
    }

    /// `git args` in `dir`, with the ambient configuration masked; its stdout chomped.
    fn setup_git(dir: &Path, args: &[&OsStr]) -> Vec<u8> {
        let output = std::process::Command::new("git")
            .args([
                "-c",
                "user.name=aios-test",
                "-c",
                "user.email=aios-test@example.invalid",
                "-c",
                "commit.gpgsign=false",
            ])
            .args(args)
            .current_dir(dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env_remove("GIT_DIR")
            .env_remove("GIT_WORK_TREE")
            .env_remove("GIT_INDEX_FILE")
            .stdin(std::process::Stdio::null())
            .output()
            .expect("run git");
        assert!(output.status.success(), "git {args:?} failed");
        let mut stdout = output.stdout;
        while stdout.last() == Some(&b'\n') {
            stdout.pop();
        }
        stdout
    }

    #[test]
    fn soak_relation_asks_git_about_the_recorded_bytes() {
        let dir = std::env::temp_dir().join(format!("aios-brief-relation-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create the repository directory");
        let os = |text: &'static str| OsStr::new(text);
        // Reftable, because APFS refuses a ref file whose name is not UTF-8. The parity
        // scenarios need it too, so a host without it fails here instead of skipping.
        let init = std::process::Command::new("git")
            .args(["init", "-q", "-b", "main", "--ref-format=reftable"])
            .current_dir(&dir)
            .env("GIT_CONFIG_GLOBAL", "/dev/null")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .stdin(std::process::Stdio::null())
            .status();
        if !init.is_ok_and(|status| status.success()) {
            let _ = std::fs::remove_dir_all(&dir);
            panic!(
                "git init --ref-format=reftable failed: this test and brief_parity need a git with reftable"
            );
        }
        setup_git(
            &dir,
            &[
                os("commit"),
                os("-q"),
                os("--allow-empty"),
                os("-m"),
                os("one"),
            ],
        );
        let name = b"claude/caf\xe9";
        setup_git(&dir, &[os("branch"), OsStr::from_bytes(name)]);
        let head = setup_git(&dir, &[os("rev-parse"), os("HEAD")]);
        // A recorded commit that is a ref name with a byte that is not UTF-8 resolves.
        let (relation, on_main) = soak_relation(&dir, Some(&head), name);
        assert_eq!(relation, b"origin/main tip");
        assert!(on_main);
        // The lossy name is another ref, which does not exist.
        let (relation, on_main) = soak_relation(&dir, Some(&head), "claude/caf\u{fffd}".as_bytes());
        assert_eq!(relation, b"commit not in this repository");
        assert!(!on_main);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn boots_cell_keeps_the_whole_row_without_a_closing_marker() {
        assert_eq!(
            boots_cell(b"| Boots | 10 (text mode) |\n"),
            b"10 (text mode)"
        );
        assert_eq!(boots_cell(b"| Boots |  |\n"), b"");
        assert_eq!(
            boots_cell(b"| Boots | 10 (text mode)\n"),
            b"| Boots | 10 (text mode)"
        );
        assert_eq!(boots_cell(b"| Boots | |\n"), b"| Boots | |");
        assert_eq!(boots_cell(b"nothing\n"), b"");
    }

    #[test]
    fn class_line_counts_every_row_after_the_header() {
        let tsv = b"boot\tmode\tclass\n1\ttext\tCLEAN\n2\ttext\tCLEAN\n3\ttext\tPANIC-LOCK\n";
        assert_eq!(
            class_line(tsv, ""),
            b"  - text mode: CLEAN 2/3, PCZERO 0, PANIC-LOCK 1, PANIC 0, EXCEPTION 0, WEDGE-STUCK 0, WEDGE-ALIVE 0, DEGRADED 0"
        );
        let old = b"boot\tmode\tclass\n1\tgpu\tWEDGE\n";
        assert_eq!(
            class_line(old, " so far"),
            b"  - gpu mode so far: CLEAN 0/1, PCZERO 0, PANIC-LOCK 0, PANIC 0, EXCEPTION 0, WEDGE-STUCK 0, WEDGE-ALIVE 0, DEGRADED 0, WEDGE 1"
        );
        assert_eq!(
            class_line(b"boot\tmode\tclass\n", ""),
            b"  - summary.tsv has no rows yet"
        );
        // A blank line is a row too, with an empty class and mode.
        assert_eq!(
            class_line(b"h\n\n", ""),
            b"  -  mode: CLEAN 0/1, PCZERO 0, PANIC-LOCK 0, PANIC 0, EXCEPTION 0, WEDGE-STUCK 0, WEDGE-ALIVE 0, DEGRADED 0"
        );
    }

    #[test]
    fn per_arm_line_follows_the_awk() {
        let dir = std::env::temp_dir().join(format!("aios-brief-arms-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("create the directory");
        let (arms, boots) = (dir.join("arms.tsv"), dir.join("boots.tsv"));
        std::fs::write(&arms, "arm\tcommit\nA\tc1\nB\tc2\n").expect("write arms");
        std::fs::write(
            &boots,
            "boot\tarm\tclass\n1\tA\tCLEAN\n2\tB\tPANIC\n3\tA\tCLEAN\n",
        )
        .expect("write boots");
        assert_eq!(
            per_arm_line(&arms, &boots),
            Some(b"  - CLEAN per arm: arm A `c1` 2/2; arm B `c2` 0/1".to_vec())
        );
        std::fs::write(&arms, "arm\tcommit\n").expect("write arms");
        assert_eq!(
            per_arm_line(&arms, &boots),
            Some(b"  - arms.tsv lists no arms".to_vec())
        );
        // No boots.tsv: the awk stops before its END block, and nothing is printed.
        std::fs::remove_file(&boots).expect("remove boots");
        assert_eq!(per_arm_line(&arms, &boots), None);
        std::fs::remove_dir_all(&dir).expect("remove the directory");
    }

    #[test]
    fn run_dirs_sort_bytewise_and_skip_links() {
        let dir = std::env::temp_dir().join(format!("aios-brief-dirs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for name in ["b", "a-b", "a"] {
            std::fs::create_dir_all(dir.join(name)).expect("create a run directory");
            std::fs::write(dir.join(name).join("summary.tsv"), "h\n").expect("write the summary");
        }
        std::os::unix::fs::symlink(dir.join("a/summary.tsv"), dir.join("b/summary.md"))
            .expect("link");
        std::fs::create_dir_all(dir.join("empty")).expect("create the directory");
        let found: Vec<PathBuf> = run_dirs(&dir);
        let names: Vec<&OsStr> = found.iter().filter_map(|path| path.file_name()).collect();
        // The directory paths compare as bytes: "a" < "a-b" < "b".
        assert_eq!(names, ["a", "a-b", "b"]);
        std::fs::remove_dir_all(&dir).expect("remove the directory");
    }
}
