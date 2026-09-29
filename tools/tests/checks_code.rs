//! Code-derived checks test-count and lock-order (check.py L787-925).
//!
//! The expected findings were recorded by calling check.py's own `check_test_count`,
//! `check_lock_order` and `code_mutex_statics` on the same files (production order).

mod common;

use std::collections::BTreeMap;

use aios_tools::cmd::docs_check::checks::lock_order::{code_mutex_statics, LockOrder, TEST_LOCKS};
use aios_tools::cmd::docs_check::checks::test_count::TestCount;
use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const COUNT_SHARED_SRC_LIB_RS: &str = r#"//! Shared.

#[cfg(test)]
mod tests {
    #[test]
    fn one() {}

    // #[test] in a comment still counts: the check counts the text.
    #[test]
    fn two() {}
}
"#;
const COUNT_SHARED_SRC_NESTED_MOD_RS: &str = r#"#[test]
fn three() {}
"#;
const COUNT_SHARED_SRC_DATA_TXT: &str = r#"#[test]
"#;
const COUNT_SHARED_TESTS_OUTSIDE_RS: &str = r#"#[test]
fn outside() {}
"#;
const COUNT_KERNEL_SRC_LIB_RS: &str = r#"#[test]
fn kernel() {}
"#;
const COUNT_README_MD: &str = r#"# Readme

Host tests: <!-- gen:test-count --> 4

Stale: <!-- gen:test-count -->0012 and Currently 99 host tests pass.

Currently <!-- gen:test-count --> 12 unit tests; current test distribution (12 tests).

The current suite has 4 tests, and `currently 55 tests` counts inside code spans.

```text
Currently 77 tests
```
"#;
const COUNT_DOCS_PROJECT_DEVELOPER_GUIDE_MD: &str = r#"# Developer Guide

We currently run 4 host-side tests. Currently 100000 tests would be too many digits.
"#;
const COUNT__CLAUDE_RULES_10_TESTING_MD: &str = r#"# Testing

Current test distribution (5 tests).
"#;
const COUNT_DOCS_OTHER_MD: &str = r#"<!-- gen:test-count --> 9
"#;
const COUNT_DOCS_PHASES_01_MEMORY_MD: &str = r#"# Phase 1: Memory

## Milestone 3 — Done

Host tests: <!-- gen:test-count --> 1
"#;
const LOCK_DOCS_KERNEL_DEADLOCK_PREVENTION_MD: &str = r#"# Deadlock Prevention

## 3. Locks

### 3.3 Lock Hierarchy

| Rank | Lock | Notes |
|---|---|---|
| 1 | `ALPHA_LOCK` | first |
| 2 | `BETA_LOCK[cpu]` | per-CPU array |
| 3 | `GAMMA_LOCK` | ranked third |
| 04 | `DELTA_LOCK` | leading zero |
| x | `STALE_LOCK` | unranked and not in code |
| 5 | not a lock | `lowercase_lock` |

### 3.4 Test Locks

| Lock | Why |
|---|---|
| `EPSILON_LOCK` | documented without a rank |

### 3.5 Notes

| 9 | `AFTER_STOP` | ignored |
"#;
const LOCK_KERNEL_SRC_OTHER_RS: &str = r#"//! Other locks.

static NEW_LOCK: Mutex<u8> = Mutex::new(0);

/// LOCK ORDERING: NEW_LOCK before WRAITH_LOCK.
fn f() {}
"#;
const LOCK_KERNEL_SRC_SYNC_RS: &str = r#"//! Locks.

// Lock ordering: ALPHA_LOCK > PHANTOM_LOCK > PHANTOM_LOCK
// then TEST_CHANNEL and SPECTRE_LOCK.
//!
// UNSEEN_LOCK is after the block end.

use spin::Mutex;

pub static ALPHA_LOCK: Mutex<u32> = Mutex::new(0);
pub(crate) static BETA_LOCK: [spin::Mutex<()>; 4] = [const { spin::Mutex::new(()) }; 4];
static GAMMA_LOCK: Mutex <u8> = Mutex::new(0);
pub static DELTA_LOCK: spin::Mutex<u8> = spin::Mutex::new(0);
static EPSILON_LOCK: Mutex<u8> = Mutex::new(0);
static NEW_LOCK: Mutex<u8> = Mutex::new(0);
static TEST_CHANNEL: Mutex<u8> = Mutex::new(0);
static COUNTER: AtomicU32 = AtomicU32::new(0);
static RW: RwLock<u8> = RwLock::new(0);

#[cfg(test)]
fn helper() {}

static LATE_LOCK: Mutex<u8> = Mutex::new(0);

#[cfg(test)]

mod tests {
    // lock ordering: TEST_ONLY_LOCK is still scanned in comments.
    static TEST_ONLY_LOCK: Mutex<u8> = Mutex::new(0);
}

static AFTER_TESTS_LOCK: Mutex<u8> = Mutex::new(0);
"#;
const LOCK_KERNEL_SRC_DRIVERS_TESTS_RS: &str = r#"// Lock ordering: TESTS_RS_LOCK only.
static TESTS_RS_LOCK: Mutex<u8> = Mutex::new(0);
"#;
const LOCK_KERNEL_SRC_TESTS_HELPERS_RS: &str = r#"static HELPER_LOCK: Mutex<u8> = Mutex::new(0);
"#;
const LOCK_CLAUDE_MD: &str = r#"# Project

## Key Technical Facts

```text
Lock ordering (full, test):   ALPHA_LOCK > GAMMA_LOCK > {DELTA_LOCK, GHOST_LOCK >
                              BETA_LOCK} > TEST_CHANNEL >
                              ALPHA_LOCK
Capability enforcement:       not part of the chain
Lock ordering (second):       ZETA_LOCK
```
"#;
const NO_DOC_KERNEL_SRC_SYNC_RS: &str = r#"static A_LOCK: Mutex<u8> = Mutex::new(0);
"#;

fn lock_repo(label: &str) -> TestRepo {
    TestRepo::with_files(
        label,
        &[
            (
                "docs/kernel/deadlock-prevention.md",
                LOCK_DOCS_KERNEL_DEADLOCK_PREVENTION_MD,
            ),
            ("kernel/src/other.rs", LOCK_KERNEL_SRC_OTHER_RS),
            ("kernel/src/sync.rs", LOCK_KERNEL_SRC_SYNC_RS),
            (
                "kernel/src/drivers/tests.rs",
                LOCK_KERNEL_SRC_DRIVERS_TESTS_RS,
            ),
            (
                "kernel/src/tests/helpers.rs",
                LOCK_KERNEL_SRC_TESTS_HELPERS_RS,
            ),
            ("CLAUDE.md", LOCK_CLAUDE_MD),
        ],
    )
}

fn open(t: &TestRepo) -> Repo {
    Repo::open(t.path_str()).expect("open the test repository")
}

fn finding(check: &'static str, file: &str, target: &str, message: &str, line: usize) -> Finding {
    Finding::new(check, file, target, message, line)
}

#[test]
fn test_count_reports_stale_claims_in_whole_file_regions() {
    let t = TestRepo::with_files(
        "test-count",
        &[
            ("shared/src/lib.rs", COUNT_SHARED_SRC_LIB_RS),
            ("shared/src/nested/mod.rs", COUNT_SHARED_SRC_NESTED_MOD_RS),
            ("shared/src/data.txt", COUNT_SHARED_SRC_DATA_TXT),
            ("shared/tests/outside.rs", COUNT_SHARED_TESTS_OUTSIDE_RS),
            ("kernel/src/lib.rs", COUNT_KERNEL_SRC_LIB_RS),
            ("README.md", COUNT_README_MD),
            (
                "docs/project/developer-guide.md",
                COUNT_DOCS_PROJECT_DEVELOPER_GUIDE_MD,
            ),
            (
                ".claude/rules/10-testing.md",
                COUNT__CLAUDE_RULES_10_TESTING_MD,
            ),
            ("docs/other.md", COUNT_DOCS_OTHER_MD),
            ("docs/phases/01-memory.md", COUNT_DOCS_PHASES_01_MEMORY_MD),
        ],
    );
    // Milestone 3 is merged: its phase-doc section is a current-state region with an
    // explicit line set, which test-count skips (phase docs record historical counts).
    t.commit("Phase 1 M3: Step 1 — allocator");
    let repo = open(&t);
    let got = TestCount.run(&repo).expect("test-count runs");
    let want = vec![
        finding(
            "test-count",
            ".claude/rules/10-testing.md",
            "claimed:5",
            "states 5 tests; shared/src has 4 #[test] functions",
            3,
        ),
        finding(
            "test-count",
            "README.md",
            "claimed:12",
            "states 12 tests; shared/src has 4 #[test] functions",
            5,
        ),
        finding(
            "test-count",
            "README.md",
            "claimed:99",
            "states 99 tests; shared/src has 4 #[test] functions",
            5,
        ),
        finding(
            "test-count",
            "README.md",
            "claimed:12",
            "states 12 tests; shared/src has 4 #[test] functions",
            7,
        ),
        finding(
            "test-count",
            "README.md",
            "claimed:55",
            "states 55 tests; shared/src has 4 #[test] functions",
            9,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn lock_order_reports_table_chain_and_comment_drift() {
    let t = lock_repo("lock-order");
    let repo = open(&t);
    let got = LockOrder.run(&repo).expect("lock-order runs");
    let want = vec![
        finding(
            "lock-order",
            "docs/kernel/deadlock-prevention.md",
            "undocumented:LATE_LOCK",
            "production lock LATE_LOCK is not in §3.3/§3.4",
            0,
        )
        .with_detail("defined at kernel/src/sync.rs:23"),
        finding(
            "lock-order",
            "docs/kernel/deadlock-prevention.md",
            "undocumented:NEW_LOCK",
            "production lock NEW_LOCK is not in §3.3/§3.4",
            0,
        )
        .with_detail("defined at kernel/src/other.rs:3"),
        finding(
            "lock-order",
            "docs/kernel/deadlock-prevention.md",
            "stale:STALE_LOCK",
            "§3.3/§3.4 lists STALE_LOCK, which is not a Mutex static in kernel/src",
            13,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "unknown:GHOST_LOCK",
            "lock ordering names GHOST_LOCK, which is not a Mutex static in kernel/src",
            6,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "unknown:TEST_CHANNEL",
            "lock ordering names TEST_CHANNEL, which is not a Mutex static in kernel/src",
            7,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "order:GAMMA_LOCK>BETA_LOCK",
            "CLAUDE.md orders GAMMA_LOCK before BETA_LOCK, §3.3 ranks them 3 and 2",
            7,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "order:GAMMA_LOCK>ALPHA_LOCK",
            "CLAUDE.md orders GAMMA_LOCK before ALPHA_LOCK, §3.3 ranks them 3 and 1",
            6,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "order:DELTA_LOCK>ALPHA_LOCK",
            "CLAUDE.md orders DELTA_LOCK before ALPHA_LOCK, §3.3 ranks them 4 and 1",
            6,
        ),
        finding(
            "lock-order",
            "CLAUDE.md",
            "order:BETA_LOCK>ALPHA_LOCK",
            "CLAUDE.md orders BETA_LOCK before ALPHA_LOCK, §3.3 ranks them 2 and 1",
            6,
        ),
        finding(
            "lock-order",
            "kernel/src/drivers/tests.rs",
            "unknown:TESTS_RS_LOCK",
            "lock-ordering comment names TESTS_RS_LOCK, which is not a Mutex static",
            1,
        ),
        finding(
            "lock-order",
            "kernel/src/other.rs",
            "unknown:WRAITH_LOCK",
            "lock-ordering comment names WRAITH_LOCK, which is not a Mutex static",
            5,
        ),
        finding(
            "lock-order",
            "kernel/src/sync.rs",
            "unknown:PHANTOM_LOCK",
            "lock-ordering comment names PHANTOM_LOCK, which is not a Mutex static",
            3,
        ),
        finding(
            "lock-order",
            "kernel/src/sync.rs",
            "unknown:SPECTRE_LOCK",
            "lock-ordering comment names SPECTRE_LOCK, which is not a Mutex static",
            4,
        ),
        finding(
            "lock-order",
            "kernel/src/sync.rs",
            "unknown:TEST_ONLY_LOCK",
            "lock-ordering comment names TEST_ONLY_LOCK, which is not a Mutex static",
            28,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn code_mutex_statics_skips_test_files_test_modules_and_test_locks() {
    let t = lock_repo("lock-statics");
    let repo = open(&t);
    let want: BTreeMap<String, (String, usize)> = [
        ("ALPHA_LOCK", ("kernel/src/sync.rs", 10)),
        ("BETA_LOCK", ("kernel/src/sync.rs", 11)),
        ("DELTA_LOCK", ("kernel/src/sync.rs", 13)),
        ("EPSILON_LOCK", ("kernel/src/sync.rs", 14)),
        ("GAMMA_LOCK", ("kernel/src/sync.rs", 12)),
        ("LATE_LOCK", ("kernel/src/sync.rs", 23)),
        ("NEW_LOCK", ("kernel/src/other.rs", 3)),
    ]
    .into_iter()
    .map(|(name, (file, line))| (name.to_string(), (file.to_string(), line)))
    .collect();
    assert_eq!(code_mutex_statics(&repo), want);
    assert_eq!(TEST_LOCKS, ["TEST_CHANNEL", "PI_TEST_CHANNEL"]);
}

#[test]
fn lock_order_skips_without_the_deadlock_doc() {
    let t = TestRepo::with_files(
        "lock-order-skip",
        &[("kernel/src/sync.rs", NO_DOC_KERNEL_SRC_SYNC_RS)],
    );
    let repo = open(&t);
    let err = LockOrder
        .run(&repo)
        .expect_err("lock-order needs the deadlock doc");
    assert_eq!(
        err.downcast_ref::<Skip>(),
        Some(&Skip(
            "docs/kernel/deadlock-prevention.md not found".to_string()
        ))
    );
}

#[test]
fn registry_continues_with_the_code_checks() {
    let names: Vec<&str> = registry().iter().map(|c| c.name()).collect();
    assert_eq!(names[7..9], ["test-count", "lock-order"]);
}
