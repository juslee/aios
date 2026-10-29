//! Guard parity (R5, plan steps 2 and 8): the fixture of every decision the
//! Python guard's unit tests make (`tests/fixtures/guard/cases.jsonl`) and the
//! hook output the Python guard gives on each (`tests/golden/guard/`).
//!
//! - `goldens_match_python_tests`: every golden's decision (and, for raw hook
//!   calls, its exit code) is the one the unit test saw. Runs no guard.
//! - `python_reproduces_goldens`: the Python guard of this checkout, run as the
//!   hook entry runs it, still prints the goldens byte for byte, once its stderr
//!   drops the interpreter's own warning records (`PYTHON_WARNINGS`), which are
//!   not guard output and depend on the Python version.
//! - `aios_guard_reproduces_goldens`: the Rust port (`aios guard`) prints them
//!   byte for byte too: the same exit code, stdout and stderr on every check.
//!
//! The goldens are recorded with `/usr/bin/python3` (CPython 3.13 in CI and the
//! cloud container) and reproduce on 3.9 (the Mac's `/usr/bin/python3`) as well:
//! no committed check reaches the places where the two differ (the payload
//! error texts of `guard::pyjson` and the recursion bounds, which the unit tests
//! pin to 3.13).
//! - `record` (ignored): `AIOS_BLESS_GOLDENS=1 cargo test -p aios-tools --test
//!   guard_parity -- --ignored record` rewrites the goldens from the Python guard.
//! - `python_parses_like_the_shell_table`: the Python guard's `parse_commands`
//!   still gives every parse in `tests/fixtures/guard/shell.jsonl`, the table the
//!   Rust shell model's unit test (`guard::shell`) checks against. `record_shell`
//!   (ignored, `AIOS_BLESS_GOLDENS=1`) rewrites it.

mod common;
mod guard_support;

use std::collections::BTreeMap;
use std::sync::LazyLock;

use guard_support::{
    bash_commands, golden_dir, python_guard, python_parse, read_goldens, replay, scenarios,
    shell_fixture_path, Outcome, Scenario,
};
use serde_json::{Map, Value};

fn python_outcomes(all: &[Scenario]) -> Vec<Vec<Outcome>> {
    let argv = python_guard();
    let argv: Vec<&str> = argv.iter().map(String::as_str).collect();
    assert!(
        std::path::Path::new(argv[0]).exists(),
        "{} is missing: the hook entry runs the guard with it",
        argv[0]
    );
    let outcomes = common::parallel_map(all, |s| replay(s, &argv));
    outcomes
        .into_iter()
        .map(|list| {
            list.into_iter()
                .map(|o| Outcome {
                    stderr: PYTHON_WARNINGS.replace_all(&o.stderr, "").into_owned(),
                    ..o
                })
                .collect()
        })
        .collect()
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
    assert_reproduces_goldens("python", &all, &outcomes);
}

#[test]
fn aios_guard_reproduces_goldens() {
    let all = scenarios();
    let argv = [env!("CARGO_BIN_EXE_aios"), "guard"];
    let outcomes = common::parallel_map(&all, |s| replay(s, &argv));
    assert_reproduces_goldens("aios guard", &all, &outcomes);
}

/// A warning record that CPython's `warnings` module prints on stderr for a
/// line of the guard script: the location, category and message, then the
/// source line. Python 3.13 prints a `DeprecationWarning` for `re.split`'s
/// positional `maxsplit` in `yaml_scalar` on every frontmatter read, and 3.9
/// prints none; the interpreter writes them, not the guard, so the goldens and
/// the port have no such lines.
static PYTHON_WARNINGS: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(
        r"(?m)^\{REPO\}/\.claude/hooks/git-push-guard\.py:[0-9]+: [A-Za-z]*Warning: [^\n]*\n  [^\n]*\n",
    )
    .unwrap()
});

/// Every scenario's outcomes (`who` ran them) equal its goldens.
fn assert_reproduces_goldens(who: &str, all: &[Scenario], outcomes: &[Vec<Outcome>]) {
    let mut failures = Vec::new();
    for (class, idx) in by_class(all) {
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
                        "{} check {n}:\n  golden: {want:?}\n  {who}: {got:?}",
                        s.id
                    ));
                }
            }
        }
    }
    assert!(
        failures.is_empty(),
        "{} difference(s) from {who}:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

/// Shell text that reaches each branch of the parser, beyond the fixture's
/// commands: redirections, heredocs, quotes, escapes, `case`, substitutions,
/// brace expansion, the parse errors and the nesting limit.
const SHELL_EDGE_CASES: &[(&str, usize)] = &[
    ("echo >", 0),
    ("echo > ; ls", 0),
    ("echo >\nls", 0),
    ("\"1\">x y", 0),
    ("2>&1 cmd", 0),
    ("cmd 2>/dev/null", 0),
    ("cmd &>log", 0),
    ("cmd &>>log", 0),
    ("cmd <in >out", 0),
    ("cmd 0<in 1<x", 0),
    ("cmd <>f", 0),
    ("cmd >|f >>g", 0),
    ("cmd >&2 2>&-", 0),
    ("cmd < <(ls)", 0),
    ("diff <(a x) >(b y)", 0),
    ("²>x y", 0),
    ("cat <<EOF\nline one\nEOF\nnext", 0),
    ("cat <<-'E'\n\tx\n\tE\nnext", 0),
    ("cat <<\"A\" <<B\na\nA\nb\nB\nafter", 0),
    ("cat <<", 0),
    ("cat << ;", 0),
    ("cat <<'x", 0),
    ("cat <<\\E\nbody\nE", 0),
    ("a <<E; b <<F\n1\nE\n2\nF\nc", 0),
    ("echo $(cat <<E\nx\nE\n)", 0),
    ("x=$(cat <<E) y\nbody\nE\n", 0),
    ("cat <<E\nno end", 0),
    ("echo 'a b' \"c $d\" $e", 0),
    ("echo \"$(git push)\" \"`ls`\"", 0),
    ("echo \"a\\\"b\\$c\\\\d\\e\\\nf\"", 0),
    ("echo `echo \\`x\\``", 0),
    ("echo $'a\\tb\\x41\\u00e9\\101\\cA\\q\\'\\\\'", 0),
    ("echo $'\\x' $'\\xZ' $'\\u' $'\\c'", 0),
    ("echo $'\\U0001F600\\U00110000\\777'", 0),
    ("echo $'abc", 0),
    ("echo $'abc\\", 0),
    ("echo $\"x y\"", 0),
    ("a\\\nb", 0),
    ("a\\", 0),
    ("echo \\$x \\ y", 0),
    ("echo # comment\nls", 0),
    ("echo a#b #c", 0),
    ("case $x in a) git push;; b|c) ls;& *) y;; esac; z", 0),
    ("case x in (a) y ;; esac", 0),
    ("case \"esac\" in esac) a;; esac", 0),
    ("case x in\n  a) one ;;\n  b) two ;;\nesac\nafter", 0),
    ("echo case x; case y in z) w;; esac", 0),
    ("if true; then git push; fi", 0),
    ("for i in a b; do echo $i; done", 0),
    ("f() { git push; }; f", 0),
    ("(cd x && ls) | grep y", 0),
    ("a && b || c; d & e |& f", 0),
    ("git {push,origin,main}", 0),
    ("echo {a,b}{c,d} \"{e,f}\" {g} x{,y}", 0),
    ("echo $((1+2)) $(( (3) ))", 0),
    ("echo $((1", 0),
    ("echo $(echo $(echo x))", 0),
    ("echo $(", 0),
    ("echo `x", 0),
    ("echo 'x", 0),
    ("echo \"x", 0),
    ("echo $( ) )", 0),
    ("))", 0),
    ("a | b | c", 0),
    ("x=1 y=2 cmd", 0),
    ("echo ${x} $1 $@ $# $? $$ $! $- $* $_", 0),
    ("echo $ $% $= \"$ x\" \"$%\"", 0),
    ("a\r\nb", 0),
    ("x <<<word y", 0),
    ("x <<< \"$(cat f)\"", 0),
    ("sed s/a/b/ <(cat f)", 0),
    ("echo é ünï | wc", 0),
    ("\t\t", 0),
    ("", 0),
    (";", 0),
    ("&&", 0),
    ("a;;b", 0),
    ("echo x", 8),
    ("echo $(a)", 8),
    ("echo `a`", 8),
    ("echo x", 9),
    ("echo $(a $(b $(c $(d $(e $(f $(g $(h $(i)))))))))", 0),
];

fn shell_inputs() -> Vec<(String, usize)> {
    let mut inputs: Vec<(String, usize)> = SHELL_EDGE_CASES
        .iter()
        .map(|&(text, depth)| (text.to_string(), depth))
        .collect();
    // The deeply wrapped commands of the nesting tests (up to 90 KB each) are
    // left out: their parses are megabytes of repeated bodies, and the edge cases
    // above reach the nesting limit.
    inputs.extend(
        bash_commands(&scenarios())
            .into_iter()
            .filter(|c| c.len() <= 8192)
            .map(|c| (c, 0)),
    );
    inputs
}

#[test]
#[ignore = "rewrites the shell table; run with AIOS_BLESS_GOLDENS=1"]
fn record_shell() {
    assert_eq!(
        std::env::var("AIOS_BLESS_GOLDENS").as_deref(),
        Ok("1"),
        "set AIOS_BLESS_GOLDENS=1 to rewrite tests/fixtures/guard/shell.jsonl"
    );
    let lines = python_parse(&shell_inputs());
    std::fs::write(shell_fixture_path(), lines.join("\n") + "\n").expect("write the shell table");
}

#[test]
fn python_parses_like_the_shell_table() {
    let want = std::fs::read_to_string(shell_fixture_path()).expect("read the shell table");
    let want: Vec<&str> = want.lines().collect();
    let got = python_parse(&shell_inputs());
    assert_eq!(
        got.len(),
        want.len(),
        "the shell table has a different case count"
    );
    let diffs: Vec<String> = got
        .iter()
        .zip(&want)
        .filter(|(g, w)| g != w)
        .map(|(g, w)| format!("  table:  {w}\n  python: {g}"))
        .collect();
    assert!(
        diffs.is_empty(),
        "{} difference(s):\n{}",
        diffs.len(),
        diffs.join("\n")
    );
}
