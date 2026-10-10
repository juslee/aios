//! The soak classifier against its goldens and against the oracle,
//! `CLASSIFY_AWK` in the deleted `scripts/soak-qemu.sh` (read from git history
//! at `ORACLE_COMMIT`).
//!
//! - `classify_goldens_match_aios` compares the classifier line (refined class
//!   first) of every case in `tests/fixtures/soak/synthetic.txt` with
//!   `tests/golden/soak/classify.golden` (`<case>\t<11 fields>` per line);
//!   `AIOS_BLESS_GOLDENS=1` rewrites it from aios.
//! - `classify_differential_against_oracle` is the fold differential: on every
//!   case, `base_line()` (the line with the base class, WEDGE-STUCK and
//!   WEDGE-ALIVE read as WEDGE, PANIC-LOCK as PANIC, DEGRADED as CLEAN) is
//!   byte-identical to the oracle's line. So crash-fix step 1a's refinements
//!   changed nothing else.
//! - `line_and_base_line_differ_in_the_class_only` checks that `line()` and
//!   `base_line()` differ in field 1 alone, on every case.
//! - `ipc_iterations_match_the_kernel_bench` keeps the classifier's CLEAN
//!   threshold equal to `IPC_ITERATIONS` in `kernel/src/bench.rs`.
//! - `class_lists_match_class_all` keeps the class lists in the `just soak`
//!   recipe comment and the developer guide's `just soak` row equal to
//!   `Class::ALL`.
//! - `classify_differential_on_real_logs` (ignored) runs the fold differential
//!   for every `*.log` but `build.log` under the directories in `AIOS_SOAK_REAL_LOGS`
//!   (colon-separated; real soak logs are never committed, owner decision
//!   2026-09-29), prints the transition matrix (oracle class -> class) and every
//!   boot whose class changed, and checks each log's class under the same fold
//!   against the `class` column of the `summary.tsv` beside it, if any.

mod common;

use aios_tools::cmd::soak::classify::{classify, Class, IPC_ITERATIONS};
use common::fixture::repo_root;
use common::soak::{check_golden, oracle_classify, synthetic_cases, write_cases};
use common::unique_dir;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};

/// `<case>\t<classifier line>\n` for every case, the classify.golden format.
fn golden_lines<'a>(rows: impl IntoIterator<Item = (&'a str, Vec<u8>)>) -> Vec<u8> {
    let mut out = Vec::new();
    for (name, line) in rows {
        out.extend_from_slice(name.as_bytes());
        out.push(b'\t');
        out.extend(line);
        out.push(b'\n');
    }
    out
}

/// Field 1 of a classifier line.
fn class_field(line: &[u8]) -> String {
    let end = line.iter().position(|&b| b == b'\t').unwrap_or(line.len());
    String::from_utf8_lossy(&line[..end]).into_owned()
}

/// A class name read as its base class: WEDGE-STUCK and WEDGE-ALIVE as WEDGE,
/// PANIC-LOCK as PANIC, DEGRADED as CLEAN; the script's names as themselves.
fn fold(name: &str) -> &str {
    Class::from_name(name).map_or(name, |c| c.base().name())
}

#[test]
fn corpus_covers_every_class_and_stays_public_safe() {
    let cases = synthetic_cases();
    assert_eq!(cases.len(), 127, "update this count with the corpus");
    for class in Class::ALL {
        let n = cases
            .iter()
            .filter(|c| classify(&c.log, c.stall).class == class)
            .count();
        assert!(n >= 5, "only {n} {} cases", class.name());
    }
    let text = std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/soak/synthetic.txt"),
    )
    .expect("the corpus");
    assert!(
        !text.windows(7).any(|w| w == b"/Users/"),
        "a local path in the corpus"
    );
    assert!(
        !text.windows(6).any(|w| w == b"/home/"),
        "a local path in the corpus"
    );
}

#[test]
fn ipc_iterations_match_the_kernel_bench() {
    let path = repo_root().join("kernel/src/bench.rs");
    let text = std::fs::read_to_string(&path).expect("kernel/src/bench.rs");
    let decl = "const IPC_ITERATIONS: usize = ";
    let values: Vec<u64> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix(decl))
        .map(|v| {
            let v = v.strip_suffix(';').expect("a `;` after the value");
            v.replace('_', "").parse().expect("a decimal literal")
        })
        .collect();
    assert_eq!(
        values,
        [IPC_ITERATIONS],
        "{}: one `{decl}...;` declaration, equal to the classifier's",
        path.display()
    );
}

/// The one line of `rel` (repo-relative) that starts with `prefix`.
fn line_starting_with(rel: &str, prefix: &str) -> String {
    let path = repo_root().join(rel);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{rel}: {e}"));
    let lines: Vec<&str> = text.lines().filter(|l| l.starts_with(prefix)).collect();
    assert_eq!(lines.len(), 1, "{rel}: one line starting {prefix:?}");
    lines[0].to_string()
}

#[test]
fn class_lists_match_class_all() {
    let all = Class::ALL.map(Class::name).join("/");
    let recipe = line_starting_with("justfile", "# Soak-test boots:");
    assert!(
        recipe.ends_with(&format!(" classified {all}")),
        "justfile `just soak` comment lists other classes than {all}: {recipe}"
    );
    let row = line_starting_with("docs/project/developer-guide.md", "| `just soak` |");
    assert!(
        row.contains(&format!("({all})")),
        "developer-guide `just soak` row lists other classes than {all}: {row}"
    );
}

#[test]
fn classify_goldens_match_aios() {
    let cases = synthetic_cases();
    let actual = golden_lines(
        cases
            .iter()
            .map(|c| (c.name.as_str(), classify(&c.log, c.stall).line())),
    );
    if let Some(diff) = check_golden("classify.golden", &actual) {
        panic!("{diff}");
    }
}

#[test]
fn classify_differential_against_oracle() {
    let dir = unique_dir("soak-diff-classify");
    let cases = synthetic_cases();
    let rels = write_cases(&dir, &cases);
    let mut diffs = Vec::new();
    for (case, rel) in cases.iter().zip(&rels) {
        let want = oracle_classify(&dir.join(rel), case.stall);
        let got = classify(&case.log, case.stall).base_line();
        if want != got {
            diffs.push(format!(
                "{}\n  oracle {}\n  aios   {}",
                case.name,
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got)
            ));
        }
    }
    assert!(
        diffs.is_empty(),
        "{} of {} cases differ:\n{}",
        diffs.len(),
        cases.len(),
        diffs.join("\n")
    );
}

#[test]
fn line_and_base_line_differ_in_the_class_only() {
    for case in synthetic_cases() {
        let c = classify(&case.log, case.stall);
        let (line, base) = (c.line(), c.base_line());
        let tail = |l: &[u8]| l[l.iter().position(|&b| b == b'\t').expect("a tab")..].to_vec();
        assert_eq!(tail(&line), tail(&base), "{}", case.name);
        assert_eq!(class_field(&line), c.class.name(), "{}", case.name);
        assert_eq!(class_field(&base), c.class.base().name(), "{}", case.name);
    }
}

/// Every `*.log` below `dir`, sorted, except the harness's `build.log`.
fn logs_under(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<PathBuf> = entries.map(|e| e.expect("an entry").path()).collect();
    paths.sort();
    for p in paths {
        if p.is_dir() {
            logs_under(&p, out);
        } else if p.extension().is_some_and(|e| e == "log")
            && p.file_name().is_some_and(|n| n != "build.log")
        {
            out.push(p);
        }
    }
}

/// The `class` column of every `summary.tsv` below `dir`, by the log it names.
fn recorded_classes(dir: &Path, out: &mut HashMap<PathBuf, String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries {
        let p = entry.expect("an entry").path();
        if p.is_dir() {
            recorded_classes(&p, out);
        } else if p.file_name().is_some_and(|n| n == "summary.tsv") {
            let text = std::fs::read(&p).expect("read a summary.tsv");
            let mut rows = text.split(|&b| b == b'\n').filter(|r| !r.is_empty());
            let header: Vec<&[u8]> = rows
                .next()
                .map(|h| h.split(|&b| b == b'\t').collect())
                .unwrap_or_default();
            let column = |name: &[u8]| header.iter().position(|h| *h == name);
            let (Some(class), Some(log)) = (column(b"class"), column(b"log")) else {
                panic!("{}: no class or log column", p.display());
            };
            for row in rows {
                let cells: Vec<&[u8]> = row.split(|&b| b == b'\t').collect();
                let at = |i: usize| String::from_utf8_lossy(cells.get(i).copied().unwrap_or(b""));
                let parent = p.parent().expect("a parent");
                out.insert(parent.join(at(log).as_ref()), at(class).into_owned());
            }
        }
    }
}

#[test]
#[ignore = "needs local real soak logs: AIOS_SOAK_REAL_LOGS=dir[:dir...]"]
fn classify_differential_on_real_logs() {
    let dirs = std::env::var_os("AIOS_SOAK_REAL_LOGS")
        .expect("set AIOS_SOAK_REAL_LOGS to directories of soak logs");
    let mut logs = Vec::new();
    let mut recorded = HashMap::new();
    for dir in std::env::split_paths(&dirs) {
        logs_under(&dir, &mut logs);
        recorded_classes(&dir, &mut recorded);
    }
    assert!(!logs.is_empty(), "no *.log files under AIOS_SOAK_REAL_LOGS");
    let mut counts: BTreeMap<Class, usize> = BTreeMap::new();
    let mut transitions: BTreeMap<(String, Class), usize> = BTreeMap::new();
    let mut changed = Vec::new();
    let mut diffs = Vec::new();
    let mut checked = 0;
    for log in &logs {
        let raw = std::fs::read(log).expect("read a log");
        let got = classify(&raw, None);
        *counts.entry(got.class).or_insert(0) += 1;
        let want = oracle_classify(log, None);
        let oracle = class_field(&want);
        *transitions.entry((oracle.clone(), got.class)).or_insert(0) += 1;
        if oracle != got.class.name() {
            changed.push(format!(
                "  {}: {oracle} -> {}",
                log.display(),
                got.class.name()
            ));
        }
        if want != got.base_line() {
            diffs.push(format!(
                "{}\n  oracle {}\n  base   {}",
                log.display(),
                String::from_utf8_lossy(&want),
                String::from_utf8_lossy(&got.base_line())
            ));
        }
        if let Some(rec) = recorded.get(log) {
            checked += 1;
            if fold(rec) != fold(got.class.name()) {
                diffs.push(format!(
                    "{}: summary.tsv records {rec}, aios classifies {}",
                    log.display(),
                    got.class.name()
                ));
            }
        }
    }
    let names: BTreeMap<&str, usize> = counts.iter().map(|(c, n)| (c.name(), *n)).collect();
    eprintln!("{} real logs: {names:?}", logs.len());
    eprintln!("{checked} of them checked against a summary.tsv class");
    eprintln!("transitions (oracle class -> class: boots):");
    for ((from, to), n) in &transitions {
        eprintln!("  {from} -> {}: {n}", to.name());
    }
    eprintln!("{} boots changed class:", changed.len());
    for line in &changed {
        eprintln!("{line}");
    }
    assert!(
        diffs.is_empty(),
        "{} differences:\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
