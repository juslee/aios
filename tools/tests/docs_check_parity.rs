//! docs-check parity with scripts/docs/check.py (R1 parity: the goldens).
//!
//! `goldens_match_aios` replays every case of `fixture::cases()` (22 on a snapshot of
//! main at `SNAPSHOT_SHA` plus `SNAPSHOT_MIGRATION`, 48 on the fixture repository) and
//! compares `exit N\n` plus stdout, and the written baseline, byte for byte with
//! `tests/golden/docs-check/`. It is the only parity gate: the goldens were recorded
//! from check.py at `SNAPSHOT_SHA` (33c6b3d), before R1 deleted it, and later adapted
//! for the project memory's move to `.claude/CLAUDE.md` (see `SNAPSHOT_MIGRATION`) and
//! for check.py's `lock-order` description at 56c4bf4 (`IrqSpinLock` statics).
//! `AIOS_BLESS_GOLDENS=1` rewrites the goldens from aios instead (for an intentional
//! output change; review the diff).
//!
//! R2-R5 (the ports of the other host scripts) can recover the recorder and the
//! side-by-side differential against a script that still exists from git history
//! (#207), then gate on goldens alone once that script is deleted in turn.

mod common;

use common::fixture::{self, Case, Source};
use common::{run_aios, Run, TestRepo};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// What a run produced: `exit <code>\n` + stdout, and the written baseline for
/// `--update-baseline` cases.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Outcome {
    golden: Vec<u8>,
    baseline: Option<Vec<u8>>,
}

impl Outcome {
    fn of(run: &Run, baseline: Option<Vec<u8>>) -> Outcome {
        let mut golden = format!("exit {}\n", run.code).into_bytes();
        golden.extend_from_slice(&run.stdout);
        Outcome { golden, baseline }
    }
}

/// Run one case in `repo` with aios.
fn run_case(repo: &TestRepo, case: &Case) -> Outcome {
    let run = run_aios(&case.run_dir(repo.path()), &case.aios_args());
    let baseline = case.writes_baseline.then(|| {
        let path = repo.path().join(case.baseline_rel);
        fs::read(&path)
            .unwrap_or_else(|e| panic!("{}: cannot read {}: {e}", case.label(), path.display()))
    });
    Outcome::of(&run, baseline)
}

/// Map `f` over `items` on one worker thread per available CPU, keeping input order.
/// Each case spawns a process, so this bounds the wall-clock time of the real-snapshot
/// cases with a debug-build `aios`.
fn parallel_map<T: Sync, R: Send>(items: &[T], f: impl Fn(&T) -> R + Sync) -> Vec<R> {
    let workers = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .clamp(1, items.len().max(1));
    let next = AtomicUsize::new(0);
    let (next, f) = (&next, &f);
    let mut done: Vec<(usize, R)> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(move || {
                    let mut out = Vec::new();
                    loop {
                        let i = next.fetch_add(1, Ordering::Relaxed);
                        let Some(item) = items.get(i) else { break };
                        out.push((i, f(item)));
                    }
                    out
                })
            })
            .collect();
        handles
            .into_iter()
            .flat_map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    done.sort_by_key(|(i, _)| *i);
    done.into_iter().map(|(_, r)| r).collect()
}

/// One read-only materialization per source, shared by every case that does not
/// write the baseline; the directories are removed when this drops.
struct Shared {
    repos: HashMap<String, TestRepo>,
}

impl Shared {
    fn new(cases: &[Case]) -> Shared {
        let mut seen = BTreeSet::new();
        let sources: Vec<Source> = cases
            .iter()
            .filter(|case| !case.writes_baseline)
            .map(|case| case.source)
            .filter(|source| seen.insert(source.key()))
            .collect();
        let repos = parallel_map(&sources, |source| fixture::materialize(*source));
        Shared {
            repos: sources
                .iter()
                .map(|source| source.key())
                .zip(repos)
                .collect(),
        }
    }

    fn get(&self, source: Source) -> &TestRepo {
        self.repos
            .get(&source.key())
            .unwrap_or_else(|| panic!("no shared materialization for {}", source.key()))
    }
}

/// Run every case: `--update-baseline` cases in a fresh materialization, the others in
/// the shared one.
fn run_cases(cases: &[Case]) -> Vec<Outcome> {
    let shared = Shared::new(cases);
    parallel_map(cases, |case| {
        if case.writes_baseline {
            run_case(&fixture::materialize(case.source), case)
        } else {
            run_case(shared.get(case.source), case)
        }
    })
}

/// The first line where two outputs differ.
fn first_difference(want: &[u8], got: &[u8]) -> String {
    let want = String::from_utf8_lossy(want);
    let got = String::from_utf8_lossy(got);
    let want: Vec<&str> = want.split('\n').collect();
    let got: Vec<&str> = got.split('\n').collect();
    for i in 0..want.len().max(got.len()) {
        let (w, g) = (want.get(i).copied(), got.get(i).copied());
        if w != g {
            return format!(
                "line {}: expected {:?}, got {:?}",
                i + 1,
                w.unwrap_or("<end of output>"),
                g.unwrap_or("<end of output>")
            );
        }
    }
    "the outputs differ only in bytes that are not valid UTF-8".to_string()
}

/// Every way `got` differs from `want`, one line each.
fn differences(label: &str, want: &Outcome, got: &Outcome) -> Vec<String> {
    let mut out = Vec::new();
    if want.golden != got.golden {
        out.push(format!(
            "{label}: exit code or stdout differs: {}",
            first_difference(&want.golden, &got.golden)
        ));
    }
    if want.baseline != got.baseline {
        out.push(format!(
            "{label}: written baseline differs: {}",
            first_difference(
                want.baseline.as_deref().unwrap_or_default(),
                got.baseline.as_deref().unwrap_or_default()
            )
        ));
    }
    out
}

/// The recorded outcome of a case.
fn read_golden(case: &Case) -> Result<Outcome, String> {
    let read = |path: PathBuf| {
        fs::read(&path).map_err(|e| {
            format!(
                "{}: cannot read golden {}: {e}",
                case.label(),
                path.display()
            )
        })
    };
    let golden = read(case.golden_path())?;
    let baseline = if case.writes_baseline {
        Some(read(case.baseline_golden_path())?)
    } else {
        None
    };
    Ok(Outcome { golden, baseline })
}

fn list_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let path = entry.expect("readable golden directory entry").path();
        if path.is_dir() {
            list_files(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// Golden files that no case produces (left over from a renamed or removed case).
fn unexpected_golden_files(cases: &[Case]) -> Vec<String> {
    let mut expected = BTreeSet::new();
    for case in cases {
        expected.insert(case.golden_path());
        if case.writes_baseline {
            expected.insert(case.baseline_golden_path());
        }
    }
    let mut found = Vec::new();
    list_files(&fixture::golden_root(), &mut found);
    found
        .into_iter()
        .filter(|path| !expected.contains(path))
        .map(|path| {
            format!(
                "unexpected golden file {} (no case produces it)",
                path.display()
            )
        })
        .collect()
}

fn write_file(path: &Path, bytes: &[u8]) {
    let parent = path.parent().expect("golden path has a parent");
    fs::create_dir_all(parent)
        .unwrap_or_else(|e| panic!("cannot create {}: {e}", parent.display()));
    fs::write(path, bytes).unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
}

/// Replace `tests/golden/docs-check/` with these outcomes.
fn write_goldens(cases: &[Case], outcomes: &[Outcome]) {
    let root = fixture::golden_root();
    match fs::remove_dir_all(&root) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => panic!("cannot clear {}: {e}", root.display()),
    }
    for (case, outcome) in cases.iter().zip(outcomes) {
        write_file(&case.golden_path(), &outcome.golden);
        if let Some(baseline) = &outcome.baseline {
            write_file(&case.baseline_golden_path(), baseline);
        }
    }
    eprintln!(
        "wrote the goldens of {} cases under {}",
        cases.len(),
        root.display()
    );
}

#[test]
fn goldens_match_aios() {
    let cases = fixture::cases();
    let outcomes = run_cases(&cases);
    if std::env::var("AIOS_BLESS_GOLDENS").as_deref() == Ok("1") {
        write_goldens(&cases, &outcomes);
        return;
    }
    let mut failures = Vec::new();
    for (case, got) in cases.iter().zip(&outcomes) {
        match read_golden(case) {
            Ok(want) => failures.extend(differences(&case.label(), &want, got)),
            Err(message) => failures.push(message),
        }
    }
    failures.extend(unexpected_golden_files(&cases));
    assert!(
        failures.is_empty(),
        "{} golden mismatch(es):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
