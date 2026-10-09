//! docs-check parity with scripts/docs/check.py (R1 parity: goldens and parity tests).
//!
//! - `goldens_match_aios` replays every case of `fixture::cases()` (22 on a snapshot of
//!   main at `SNAPSHOT_SHA` plus `SNAPSHOT_MIGRATION`, 48 on the fixture repository)
//!   and compares `exit N\n` plus stdout, and the written baseline, byte for byte with
//!   `tests/golden/docs-check/`. `AIOS_BLESS_GOLDENS=1` rewrites the goldens from aios
//!   instead (for an intentional output change; review the diff).
//! - `record_goldens_from_check_py` (ignored) records those goldens from check.py.
//! - `differential_against_check_py` runs check.py and aios side by side on every case
//!   and on the live checkout. R1 deleted check.py, so both tests run the last version
//!   of it, materialised from git history (`fixture::check_py`: the blob at
//!   `SNAPSHOT_SHA`), wherever `python3` is on Unicode 16.0 (CPython 3.14; see
//!   `fixture::ORACLE_UNIDATA_VERSION`). Without such a `python3` or that git object (a
//!   shallow clone) the differential prints the reason and returns, and
//!   `goldens_match_aios` alone is the parity gate; `AIOS_REQUIRE_CHECK_PY=1` (set in
//!   CI's Tools (host) job) turns that skip into a failure, so the oracle cannot go
//!   quiet there unnoticed.
//!   R2-R5 (the ports of the other host scripts) are expected to reuse this same
//!   pattern: a differential against their own script, goldens recorded from it, and
//!   after that script is deleted, the same script materialised from git history.
//!
//! The project memory moved to `.claude/CLAUDE.md` in #218, after check.py was deleted,
//! and aios reads only that path. The fixture bundles and the snapshot (through
//! `SNAPSHOT_MIGRATION`) carry it there, and the oracle makes the same move:
//! `fixture::CHECK_PY_MIGRATION` patches the materialised check.py's `CLAUDE.md` paths
//! and its `BEFORE_CLAUDE_RE` exactly as #218 changed aios, and no check logic, so both
//! tests compare aios against check.py on the same migrated inputs and goldens.

mod common;

use common::fixture::{self, Case, Source};
use common::{run_aios, Run, TestRepo};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// Read-only modes compared on the live checkout by the differential test.
const LIVE_CASES: [(&str, &[&str]); 4] = [
    ("default", &[]),
    ("all", &["--all"]),
    ("json-all", &["--json", "--all"]),
    ("markdown", &["--markdown"]),
];

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

#[derive(Clone, Copy)]
enum Tool {
    Aios,
    CheckPy,
}

/// Run one case in `repo` with `tool`.
fn run_case(tool: Tool, repo: &TestRepo, case: &Case) -> Outcome {
    let dir = case.run_dir(repo.path());
    let run = match tool {
        Tool::Aios => run_aios(&dir, &case.aios_args()),
        Tool::CheckPy => fixture::run_check_py(repo.path(), &dir, &case.flag_args()),
    };
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

/// Run every case with `tool`: `--update-baseline` cases in a fresh materialization,
/// the others in the shared one.
fn run_cases(cases: &[Case], tool: Tool) -> Vec<Outcome> {
    let shared = Shared::new(cases);
    parallel_map(cases, |case| {
        if case.writes_baseline {
            let fresh = fixture::materialize(case.source);
            run_case(tool, &fresh, case)
        } else {
            run_case(tool, shared.get(case.source), case)
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
    let outcomes = run_cases(&cases, Tool::Aios);
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

#[test]
#[ignore = "records the goldens from check.py (materialised from git history); run after adding or changing a case"]
fn record_goldens_from_check_py() {
    if let Err(reason) = fixture::check_py() {
        panic!("recording the goldens needs check.py: {reason}");
    }
    let cases = fixture::cases();
    let outcomes = run_cases(&cases, Tool::CheckPy);
    write_goldens(&cases, &outcomes);
}

/// Set to `1` to fail, instead of skip, when the check.py oracle is unavailable.
const REQUIRE_CHECK_PY: &str = "AIOS_REQUIRE_CHECK_PY";

#[test]
fn differential_against_check_py() {
    let oracle = match fixture::check_py() {
        Ok(oracle) => oracle,
        Err(reason) => {
            assert!(
                std::env::var(REQUIRE_CHECK_PY).as_deref() != Ok("1"),
                "differential_against_check_py: {REQUIRE_CHECK_PY}=1 but {reason}"
            );
            eprintln!("differential_against_check_py: skipped, {reason}");
            return;
        }
    };
    let cases = fixture::cases();
    let shared = Shared::new(&cases);
    let mut failures: Vec<String> = parallel_map(&cases, |case| {
        let (want, got) = if case.writes_baseline {
            let for_python = fixture::materialize(case.source);
            let for_aios = fixture::materialize(case.source);
            (
                run_case(Tool::CheckPy, &for_python, case),
                run_case(Tool::Aios, &for_aios, case),
            )
        } else {
            let repo = shared.get(case.source);
            (
                run_case(Tool::CheckPy, repo, case),
                run_case(Tool::Aios, repo, case),
            )
        };
        differences(&case.label(), &want, &got)
    })
    .into_iter()
    .flatten()
    .collect();
    let root = fixture::repo_root();
    for (name, flags) in LIVE_CASES {
        let want = Outcome::of(&fixture::run_check_py(&root, &root, flags), None);
        let got = Outcome::of(
            &run_aios(&root, &[&["docs-check"][..], flags].concat()),
            None,
        );
        failures.extend(differences(&format!("live/{name}"), &want, &got));
    }
    eprintln!(
        "differential_against_check_py: compared {} cases and {} live modes, {} difference(s) \
         ({} running {} with {})",
        cases.len(),
        LIVE_CASES.len(),
        failures.len(),
        oracle.version,
        fixture::check_py_object(),
        fixture::CHECK_PY_MIGRATION
    );
    assert!(
        failures.is_empty(),
        "{} difference(s) between check.py and aios:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
