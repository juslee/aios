//! Guard parity (R5, plan steps 2 and 8): the fixture of every decision the
//! Python guard's unit tests make (`tests/fixtures/guard/cases.jsonl`) and the
//! hook output the Python guard gives on each (`tests/golden/guard/`).
//!
//! - `goldens_match_python_tests`: every golden's decision (and, for raw hook
//!   calls, its exit code) is the one the unit test saw. Runs no guard.
//! - `python_reproduces_goldens`: the Python guard of this checkout, run as the
//!   hook entry runs it, still prints the goldens byte for byte.
//! - `record` (ignored): `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test
//!   guard_parity -- --ignored record` rewrites the goldens from the Python guard.

mod common;
mod guard_support;

use std::collections::BTreeMap;

use guard_support::{golden_dir, python_guard, read_goldens, replay, scenarios, Outcome, Scenario};
use serde_json::{Map, Value};

fn python_outcomes(all: &[Scenario]) -> Vec<Vec<Outcome>> {
    let argv = python_guard();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    assert!(
        std::path::Path::new(argv[0]).exists(),
        "{} is missing: the hook entry runs the guard with it",
        argv[0]
    );
    common::parallel_map(all, |s| replay(s, &argv))
}

fn by_class(all: &[Scenario]) -> BTreeMap<&str, Vec<usize>> {
    let mut map: BTreeMap<&str, Vec<usize>> = BTreeMap::new();
    for (i, s) in all.iter().enumerate() {
        map.entry(s.class()).or_default().push(i);
    }
    map
}

#[test]
#[ignore = "rewrites the goldens; run with AIOS_BLESS_GOLDENS=1"]
fn record() {
    assert_eq!(
        std::env::var("AIOS_BLESS_GOLDENS").as_deref(),
        Ok("1"),
        "set AIOS_BLESS_GOLDENS=1 to rewrite tests/golden/guard"
    );
    let all = scenarios();
    let outcomes = python_outcomes(&all);
    let dir = golden_dir();
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the golden directory");
    for (class, idx) in by_class(&all) {
        let mut map = Map::new();
        for i in idx {
            let list = outcomes[i].iter().map(Outcome::to_json).collect();
            map.insert(all[i].method().to_string(), Value::Array(list));
        }
        let text = serde_json::to_string_pretty(&Value::Object(map)).expect("serialise") + "\n";
        std::fs::write(guard_support::golden_path(class), text).expect("write a golden");
    }
}

#[test]
fn goldens_match_python_tests() {
    let all = scenarios();
    let mut failures = Vec::new();
    let mut checks = 0;
    let classes = by_class(&all);
    for (class, idx) in &classes {
        let goldens = match read_goldens(class) {
            Ok(g) => g,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        for &i in idx {
            let s = &all[i];
            let Some(list) = goldens.get(s.method()).and_then(Value::as_array) else {
                failures.push(format!("{}: no golden", s.id));
                continue;
            };
            let expected: Vec<&Value> = s.checks().collect();
            if list.len() != expected.len() {
                failures.push(format!(
                    "{}: {} golden outcomes for {} checks",
                    s.id,
                    list.len(),
                    expected.len()
                ));
                continue;
            }
            for (n, (got, want)) in list.iter().zip(expected).enumerate() {
                checks += 1;
                let got = Outcome::from_json(got);
                let level = want["expect"].as_str().expect("expect");
                if got.level() != level {
                    failures.push(format!(
                        "{} check {n}: golden {} but the test saw {level}: {want}",
                        s.id,
                        got.level()
                    ));
                }
                if let Some(exit) = want.get("exit").and_then(Value::as_i64) {
                    if i64::from(got.exit) != exit {
                        failures.push(format!(
                            "{} check {n}: golden exit {} but the test saw {exit}",
                            s.id, got.exit
                        ));
                    }
                }
            }
        }
        let methods: Vec<&str> = idx.iter().map(|&i| all[i].method()).collect();
        for key in goldens.keys() {
            if !methods.contains(&key.as_str()) {
                failures.push(format!("{class}.{key}: a golden with no scenario"));
            }
        }
    }
    for entry in std::fs::read_dir(golden_dir()).expect("read tests/golden/guard") {
        let name = entry.expect("a directory entry").file_name();
        let name = name.to_string_lossy();
        let class = name.strip_suffix(".json").unwrap_or(&name);
        if !classes.contains_key(class) {
            failures.push(format!("tests/golden/guard/{name}: no such test class"));
        }
    }
    assert!(
        failures.is_empty(),
        "{} mismatch(es) in {checks} checks:\n{}",
        failures.len(),
        failures.join("\n")
    );
    assert!(
        checks > 2000,
        "only {checks} checks: the fixture lost cases"
    );
}

#[test]
fn python_reproduces_goldens() {
    let all = scenarios();
    let outcomes = python_outcomes(&all);
    let mut failures = Vec::new();
    for (class, idx) in by_class(&all) {
        let goldens = match read_goldens(class) {
            Ok(g) => g,
            Err(e) => {
                failures.push(e);
                continue;
            }
        };
        for i in idx {
            let s = &all[i];
            let want = goldens.get(s.method()).and_then(Value::as_array);
            let want: Vec<Outcome> = want
                .map(|l| l.iter().map(Outcome::from_json).collect())
                .unwrap_or_default();
            if want.len() != outcomes[i].len() {
                failures.push(format!(
                    "{}: {} outcomes, {} goldens",
                    s.id,
                    outcomes[i].len(),
                    want.len()
                ));
                continue;
            }
            for (n, (got, want)) in outcomes[i].iter().zip(&want).enumerate() {
                if got != want {
                    failures.push(format!(
                        "{} check {n}:\n  golden: {want:?}\n  python: {got:?}",
                        s.id
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} difference(s):\n{}",
        failures.len(),
        failures.join("\n")
    );
}
