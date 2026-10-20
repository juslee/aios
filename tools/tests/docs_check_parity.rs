//! docs-check parity with the deleted scripts/docs/check.py (R1 parity: the goldens).
//!
//! `goldens_match_aios` replays every case of `fixture::cases()` (22 on a snapshot of
//! main at `SNAPSHOT_SHA` plus `SNAPSHOT_MIGRATION`, 48 on the fixture repository) and
//! compares `exit N\n` plus stdout, and the written baseline, byte for byte with
//! `tests/golden/docs-check/`. The goldens were recorded from check.py before R1 deleted
//! it, so they are the parity gate. `AIOS_BLESS_GOLDENS=1` rewrites them from aios
//! instead (for an intentional output change; review the diff).
//!
//! The project memory moved to `.claude/CLAUDE.md` in #218, after check.py was deleted,
//! and aios reads only that path. The fixture bundles and the snapshot (through
//! `SNAPSHOT_MIGRATION`) carry it there, and the goldens carry the moved path. The
//! `list-checks` goldens carry the `lock-order` description crash-fix step 1b gave
//! check.py (56c4bf4, on its branch, before R1 deleted check.py on main): it counts
//! `IrqSpinLock` statics beside `Mutex` ones.

mod common;

use common::fixture::{self, Case, Source};
use common::{parallel_map, run_aios, Run, TestRepo};
use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

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

/// Run every case with aios: `--update-baseline` cases in a fresh materialization,
/// the others in the shared one.
fn run_cases(cases: &[Case]) -> Vec<Outcome> {
    let shared = Shared::new(cases);
    parallel_map(cases, |case| {
        if case.writes_baseline {
            let fresh = fixture::materialize(case.source);
            run_case(&fresh, case)
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
