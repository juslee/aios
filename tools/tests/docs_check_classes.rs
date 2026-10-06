//! docs-check with Python's exact `\d` and `\s` classes (#205): small repositories whose
//! docs put non-ASCII decimal digits (Arabic-Indic `٠`-`٩`, fullwidth `０`-`９`) or the
//! control separators U+001C..U+001F where a check's pattern, `str.isdigit()` or `int()`
//! meets them.
//!
//! Each test asserts the exit code and stdout that check.py (at 33c6b3d) prints for the
//! same repository. Those expectations were recorded with `fixture::run_check_py`, and
//! wherever the oracle is available (`fixture::check_py`) each test also runs check.py
//! again and requires the same bytes, so the expectations cannot drift from Python.
//! Before #205 (`[0-9]` for `\d`, the crate's plain `\s`), aios's exit code or stdout
//! differed from check.py's for every one of these repositories.
//!
//! A few converted patterns have no input here because none can tell the two classes
//! apart: the text they capture is stripped or split on whitespace, or only names are
//! taken from it (`STATUS_RE`, `TOOLS_RE`, `CHAIN_RE`, `FRONTMATTER_KEY_RE`,
//! `TREE_PREFIX_RE`).

mod common;

use common::fixture;
use common::{run_aios, TestRepo};
use serde_json::Value;

/// `aios docs-check <args>` in `repo`: its exit code and stdout, after requiring that
/// check.py prints the same when it can run.
fn run_like_check_py(repo: &TestRepo, args: &[&str]) -> (i32, String) {
    let aios = run_aios(repo.path(), &[&["docs-check"][..], args].concat());
    let got = (
        aios.code,
        String::from_utf8_lossy(&aios.stdout).into_owned(),
    );
    match fixture::check_py() {
        Ok(_) => {
            let python = fixture::run_check_py(repo.path(), repo.path(), args);
            assert_eq!(
                got,
                (
                    python.code,
                    String::from_utf8_lossy(&python.stdout).into_owned()
                ),
                "aios (left) and check.py (right) differ on {args:?}; aios stderr: {}; \
                 check.py stderr: {}",
                String::from_utf8_lossy(&aios.stderr),
                String::from_utf8_lossy(&python.stderr)
            );
        }
        Err(reason) => eprintln!("check.py not run: {reason}"),
    }
    got
}

/// `aios docs-check <args>` in `repo` exits `code` with exactly `stdout`, check.py's
/// recorded output, and prints what check.py prints when it can run.
fn assert_like_check_py(repo: &TestRepo, args: &[&str], code: i32, stdout: &str) {
    let (got_code, got_stdout) = run_like_check_py(repo, args);
    assert_eq!(
        (got_code, got_stdout.as_str()),
        (code, stdout),
        "aios docs-check {args:?} (left) against check.py's recorded output (right)"
    );
}

/// check.py's stdout for `decimal_digits_in_links_paths_and_names`.
const DIGITS_LINKS: &str = "\
docs-check: 4 findings across 4 checks - 4 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               1     1
  section-refs           2     2
  repo-paths             1     1
  knowledge-hygiene      0     0

New drift since baseline:

[md-links]
 + docs/a.md:7: broken link -> missing-list.md

[section-refs]
 + docs/a.md:3: no §1.٥ heading in docs/other.md or its hub members
 + docs/a.md:3: no §٤ heading in docs/other.md or its hub members

[repo-paths]
 + CLAUDE.md:3: path does not exist: kernel/src/gone.rs
";

/// check.py's stdout for `decimal_digits_in_history_tables_and_claims`.
const DIGITS_STATUS: &str = "\
docs-check: 9 findings across 4 checks - 9 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  test-count             3     3
  lock-order             1     1
  milestone-status       3     3
  phase-count            2     2

New drift since baseline:

[test-count]
 + README.md:7: states 12 tests; shared/src has 0 #[test] functions
 + README.md:9: states 34 tests; shared/src has 0 #[test] functions
 + README.md:11: states 7 tests; shared/src has 0 #[test] functions

[lock-order]
 + CLAUDE.md:3: CLAUDE.md orders ALPHA_LOCK before BETA_LOCK, §3.3 ranks them 2 and 1

[milestone-status]
 + docs/phases/٠١-arabic.md:3: all milestones (M3) are merged but status is 'Planned'
 + docs/phases/٠١-arabic.md:7: merged milestone M3 still has 1 unchecked task(s)
 + docs/project/development-plan.md:7: all milestones (M3) are merged but status is 'Planned'

[phase-count]
 + CLAUDE.md:5: says ٤ phases; development-plan §8 lists 2
 + README.md:5: says ٣ phases; development-plan §8 lists 2
";

/// check.py's stdout for `control_separators_in_links_and_anchors`.
const SPACE_LINKS: &str = "\
docs-check: 4 findings across 3 checks - 4 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               3     3
  section-refs           1     1
  anchors                0     0

New drift since baseline:

[md-links]
 + docs/b.md:7: broken link -> missing-a.md
 + docs/b.md:9: broken link -> missing-r.md
 + docs/b.md:22: broken link -> missing-list.md

[section-refs]
 + docs/b.md:16: no §9 heading in docs/other.md or its hub members
";

/// check.py's stdout for `control_separators_in_harness_layout_and_code`.
const SPACE_HARNESS: &str = "\
docs-check: 17 findings across 7 checks - 17 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  repo-paths             1     1
  test-count             1     1
  lock-order             4     4
  layout                 1     1
  harness-tables         3     3
  pointer-doctor         7     7
  knowledge-hygiene      0     0

New drift since baseline:

[repo-paths]
 + CLAUDE.md:17: path does not exist: kernel/src/gone.rs

[test-count]
 + CLAUDE.md:19: states 12 tests; shared/src has 0 #[test] functions

[lock-order]
 + CLAUDE.md:21: lock ordering names ALPHA_LOCK, which is not a Mutex static in kernel/src
 + docs/kernel/deadlock-prevention.md: production lock DELTA_LOCK is not in §3.3/§3.4 (defined at kernel/src/sync.rs:2)
 + docs/kernel/deadlock-prevention.md: production lock GAMMA_LOCK is not in §3.3/§3.4 (defined at kernel/src/sync.rs:1)
 + docs/kernel/deadlock-prevention.md:7: §3.3/§3.4 lists ALPHA_LOCK, which is not a Mutex static in kernel/src

[layout]
 + CLAUDE.md: Workspace Layout does not list kernel module kernel/src/sync.rs

[harness-tables]
 + CLAUDE.md: CLAUDE.md agents-table omits agent worker
 + CLAUDE.md: CLAUDE.md layout-skills lists skill ghost, which is not in .claude/
 + CLAUDE.md: CLAUDE.md skills-table omits skill build

[pointer-doctor]
 + .claude/agents/worker.md:7: agent bob is not defined in .claude/agents
 + .claude/agents/worker.md:7: points to CLAUDE.md 'Build Matrix', which is not a section of CLAUDE.md
 + .claude/agents/worker.md:7: /nope is not a project skill or built-in command
 + .claude/agents/worker.md:9: points to CLAUDE.md 'Deploy Table', which is not a section of CLAUDE.md
 + .claude/agents/worker.md:13: points to CLAUDE.md 'Release Notes', which is not a section of CLAUDE.md
 + .claude/agents/worker.md:14: points to CLAUDE.md 'Ship Log', which is not a section of CLAUDE.md
 + .claude/agents/worker.md:15: rule file ٠١-x.md does not exist
";

/// check.py's stdout for `control_separators_in_milestone_status`.
const SPACE_STATUS: &str = "\
docs-check: 1 findings across 1 checks - 1 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  milestone-status       1     1

New drift since baseline:

[milestone-status]
 + docs/phases/00-a.md:7: merged milestone M1 still has 1 unchecked task(s)
";

/// check.py's stdout for `baseline_counts_follow_python_int`.
const COUNT_ARABIC: &str = "\
docs-check: 1 findings across 1 checks - 0 new, 1 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               1     0

No new drift since baseline.
";

/// `\d` in `SECTION_REF_RE` (§٤ is reported, §٣ and §３.٢ resolve through
/// `HEADING_NUM_RE`, and §1.٥ is reported whole, not resolved as §1, which pins the
/// fractional `\.\d+`), `LIST_ITEM_RE` (after `١.` an indented line is a list
/// continuation, not code), every `\d` of `LINE_SUFFIX_RE` (`:١٢`, the range end in
/// `:1-١٢` and the comma list in `:1,٣` are line suffixes) and `KNOWLEDGE_NAME_RE`.
#[test]
fn decimal_digits_in_links_paths_and_names() {
    let repo = TestRepo::with_files(
        "classes-digits-links",
        &[
            (
                "docs/a.md",
                "# A\n\n\
                 See [o](other.md) §٣, [o](other.md) §٤, [o](other.md) §３.٢ and \
                 [o](other.md) §1.٥.\n\n\
                 ١. A numbered item\n\n\
                 \x20   [under the list item](missing-list.md)\n",
            ),
            (
                "docs/other.md",
                "# Other\n\n## 1 One\n\n## ٣ Arabic three\n\n### ３.٢ Mixed\n",
            ),
            (
                "CLAUDE.md",
                "# C\n\nCode in `kernel/src/main.rs:١٢`, `kernel/src/main.rs:1-١٢`, \
                 `kernel/src/main.rs:1,٣` and `kernel/src/gone.rs:٣`.\n",
            ),
            ("kernel/src/main.rs", "fn main() {}\n"),
            (
                "docs/knowledge/decisions/٢٠٢٦-٠١-٠١-ab-arabic.md",
                "---\nauthor: x\ndate: y\ntags: [a]\nstatus: final\n---\n# N\n",
            ),
        ],
    );
    assert_like_check_py(
        &repo,
        &[
            "--check",
            "md-links,section-refs,repo-paths,knowledge-hygiene",
        ],
        1,
        DIGITS_LINKS,
    );
}

/// `\d` and `int()` in the commit subjects, phase doc names and `## Milestone N`
/// headings, milestone tokens and ranges (README, §8.1), the §8 table (`str.isdigit()`
/// cells, the phase-count row count), both phase-count claim patterns (`N phases
/// across` in README, `N phases` in CLAUDE.md), all three test-count claim patterns and
/// the lock table's rank cells.
#[test]
fn decimal_digits_in_history_tables_and_claims() {
    let repo = TestRepo::with_files(
        "classes-digits-status",
        &[
            (
                "docs/phases/٠١-arabic.md",
                "# Phase 1: Arabic\n\n**Status:** Planned\n\n\
                 ## Milestone ٣ — Three\n\n- [ ] Open task\n",
            ),
            (
                "docs/phases/02-fullwidth.md",
                "# Phase 2: Fullwidth\n\n**Status:** Complete\n\n\
                 ## Milestone ４ — Four\n\n- [x] Done\n",
            ),
            // Only the range supplies the latest merged milestone, M4: the single
            // tokens are M2 and M5.
            (
                "README.md",
                "# R\n\nStatus: M٢–M٥ merged.\n\n\
                 ٣ phases across two tiers.\n\n\
                 Tests: <!-- gen:test-count -->١٢\n\n\
                 Currently ٣٤ host tests pass.\n\n\
                 Current test distribution (٧ tests).\n",
            ),
            (
                "docs/project/development-plan.md",
                "# Plan\n\n## 8. Phases\n\n\
                 | Phase | Name | Weeks | Deliverable | Milestones | Status |\n\
                 |---|---|---|---|---|---|\n\
                 | ١ | Arabic | 1 | a | M٣ | Planned |\n\
                 | ２ | Fullwidth | 1 | b | M４ | Complete |\n\n\
                 ### Velocity Summary\n\n\
                 | a | b | c | d | Milestones |\n\
                 |---|---|---|---|---|\n\
                 | x | x | x | x | M٣, M４ |\n\n\
                 ## 9. Next\n",
            ),
            (
                "docs/kernel/deadlock-prevention.md",
                "# Deadlock\n\n### 3.3 Lock Hierarchy\n\n\
                 | Rank | Lock |\n|---|---|\n\
                 | ٢ | `ALPHA_LOCK` |\n| １ | `BETA_LOCK` |\n\n### 3.5 End\n",
            ),
            (
                "kernel/src/main.rs",
                "static ALPHA_LOCK: Mutex<()> = Mutex::new(());\n\
                 static BETA_LOCK: Mutex<()> = Mutex::new(());\n",
            ),
            (
                "CLAUDE.md",
                "# C\n\nLock ordering: ALPHA_LOCK > BETA_LOCK\n\n٤ phases in all.\n",
            ),
        ],
    );
    repo.commit("Phase ١ M٣: Step 1 — arabic");
    repo.commit("Phase 2 M４: Step 1 — fullwidth");
    assert_like_check_py(
        &repo,
        &[
            "--check",
            "milestone-status,phase-count,test-count,lock-order",
        ],
        1,
        DIGITS_STATUS,
    );
    // The §8 row's phase goes through int() into its key: `§8:phase-1`, in ASCII.
    let (code, json) = run_like_check_py(&repo, &["--json", "--check", "milestone-status"]);
    assert_eq!(code, 1);
    let json: Value = serde_json::from_str(&json).expect("--json prints JSON");
    let keys: Vec<&str> = json["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .map(|f| f["key"].as_str().expect("a string key"))
        .collect();
    assert_eq!(
        keys,
        [
            "milestone-status|docs/phases/٠١-arabic.md|status",
            "milestone-status|docs/phases/٠١-arabic.md|M3:unchecked",
            "milestone-status|docs/project/development-plan.md|§8:phase-1",
        ]
    );
}

/// `\s` meeting U+001F (inside a line) and U+001C (in raw text) in `FENCE_RE`,
/// `HEADING_RE`, `INLINE_LINK_RE`, `REF_DEF_RE` (`\S`), `SECTION_REF_RE`, the
/// `<a name>` pattern, the hub marker `Part of:\s*[...]`, `LIST_ITEM_RE`, and
/// `HEADING_NUM_RE`'s `§\s*` and lookahead (§5 and §6 resolve).
#[test]
fn control_separators_in_links_and_anchors() {
    let repo = TestRepo::with_files(
        "classes-space-links",
        &[
            (
                "docs/b.md",
                "#\x1fControl Heading\n\n\
                 \x1f```\n[hidden in a fence](fenced-missing.md)\n\x1f```\n\n\
                 A [titled](missing-a.md\x1f\"title\") link.\n\n\
                 [r]: missing-r.md\x1fextra\n\n\
                 Anchors [h](#control-heading) and [c](#custom-id).\n\n\
                 <a\x1cname=\"custom-id\"></a>\n\n\
                 Refs [o](other.md)\x1f§\x1f9 and [hub](hub.md) §7.\n\n\
                 Refs [o](other.md) §5 and [o](other.md) §6.\n\n\
                 -\x1fitem\n\n\
                 \x20   [under the list item](missing-list.md)\n",
            ),
            (
                "docs/other.md",
                "# Other\n\n## 1 One\n\n## §\x1f5 Five\n\n## 6\x1fSix\n",
            ),
            ("docs/hub.md", "# Hub\n\n## 1 Intro\n"),
            (
                "docs/part.md",
                "Part of:\x1c[hub.md](hub.md)\n\n# Part\n\n## 7 Detail\n",
            ),
        ],
    );
    assert_like_check_py(
        &repo,
        &["--check", "md-links,section-refs,anchors"],
        1,
        SPACE_LINKS,
    );
}

/// `\s` meeting U+001F and U+001C in the layout tree entries, `REPO_PATH_RE` (`\S`), the
/// `gen:test-count` claim, `STATIC_RE`, `\bMutex\s*<` and the inline test module header
/// (`TEST_MOD_RE`), pointer-doctor's agent and skill patterns and all three section-name
/// patterns (`BEFORE_CLAUDE_RE`, `AFTER_CLAUDE_RE`, `LABELLED_ITEM_RE`), and the
/// knowledge frontmatter's closing `---\s*`; and `\d` meeting Arabic-Indic digits in
/// `LABELLED_ITEM_RE`'s item number and `RULE_REF_RE`'s `\d\d` (worker.md lines 14-15).
#[test]
fn control_separators_in_harness_layout_and_code() {
    let repo = TestRepo::with_files(
        "classes-space-harness",
        &[
            (
                "CLAUDE.md",
                "# C\n\n## Workspace Layout\n\n```text\n\
                 ├── .claude/\n\
                 │   ├──\x1fskills/\x1fbuild, ghost\n\
                 ├── kernel/src/\n\
                 │   ├──\x1fmm/          memory\n\
                 │   └── (top-level)  main\n\
                 ├── shared/src/\n\
                 ├── uefi-stub/\n\
                 ```\n\n\
                 ## Rules\n\n\
                 Paths `kernel/src/gone.rs\x1fnote` and `kernel/src/main.rs`.\n\n\
                 Tests: <!--\x1fgen:test-count\x1f-->\x1f12\n\n\
                 Lock ordering: ALPHA_LOCK > GAMMA_LOCK\n",
            ),
            (
                ".claude/agents/worker.md",
                "---\nname: worker\ntools: Read\n---\n# Worker\n\n\
                 Ask the `bob`\x1fagent, run `/nope\x1fnow`, and follow the \
                 Build\x1fMatrix in CLAUDE.md.\n\n\
                 Then read CLAUDE.md:\x1fDeploy\x1fTable for the targets.\n\n\
                 ## After editing CLAUDE.md\n\n\
                 1.\x1fUpdate:\x1fRelease\x1fNotes\n\
                 ٢. Update: Ship Log\n\
                 Also follow rules/٠١-x.md here.\n",
            ),
            (".claude/skills/build/SKILL.md", "# Build\n"),
            ("kernel/src/main.rs", "fn main() {}\n"),
            ("kernel/src/mm/mod.rs", "pub fn f() {}\n"),
            (
                "kernel/src/sync.rs",
                "\x1fstatic GAMMA_LOCK: Mutex<()> = Mutex::new(());\n\
                 static DELTA_LOCK: spin::Mutex\x1f<()> = spin::Mutex::new(());\n\
                 #[cfg(test)]\n\
                 \x1fmod tests {\n\
                 static TEST_ONLY_LOCK: Mutex<()> = Mutex::new(());\n\
                 }\n",
            ),
            (
                "docs/kernel/deadlock-prevention.md",
                "# Deadlock\n\n### 3.3 Lock Hierarchy\n\n\
                 | Rank | Lock |\n|---|---|\n| 1 | `ALPHA_LOCK` |\n\n### 3.5 End\n",
            ),
            (
                "docs/knowledge/decisions/2026-01-01-ab-ctrl.md",
                "---\nauthor: x\ndate: y\ntags: [a]\nstatus: final\n---\x1c\n# N\n",
            ),
        ],
    );
    assert_like_check_py(
        &repo,
        &[
            "--check",
            "repo-paths,test-count,lock-order,layout,harness-tables,pointer-doctor,knowledge-hygiene",
        ],
        1,
        SPACE_HARNESS,
    );
}

/// `\s` meeting U+001F in the open-task pattern and in a README milestone range.
#[test]
fn control_separators_in_milestone_status() {
    let repo = TestRepo::with_files(
        "classes-space-status",
        &[
            (
                "docs/phases/00-a.md",
                "# Phase 0: A\n\n**Status:** Complete\n\n\
                 ## Milestone 1 — One\n\n\x1f- [ ] Open task\n\n\
                 ## Milestone 3 — Three\n\n- [x] Done\n",
            ),
            ("README.md", "# R\n\nStatus: M2\x1f–\x1fM4.\n"),
        ],
    );
    repo.commit("Phase 0 M1: Step 1 — one");
    repo.commit("Phase 0 M3: Step 2 — three");
    assert_like_check_py(&repo, &["--check", "milestone-status"], 1, SPACE_STATUS);
}

/// `int()` on a baselined `count` string: Unicode decimal digits, and only White_Space
/// stripped.
#[test]
fn baseline_counts_follow_python_int() {
    // A finding on two lines stays within a baseline `count` of "٢" (int("٢") == 2);
    // a count of "\x1c2" makes CPython's int() raise, so check.py exits 2.
    let files = [
        ("docs/a.md", "# A\n\n[x](gone.md)\n\n[y](gone.md)\n"),
        (
            "scripts/docs/baseline.json",
            "{\"findings\": [{\"key\": \"md-links|docs/a.md|gone.md\", \"count\": \"٢\"}]}\n",
        ),
    ];
    let repo = TestRepo::with_files("classes-count-arabic", &files);
    assert_like_check_py(&repo, &["--check", "md-links"], 0, COUNT_ARABIC);
    let control = TestRepo::with_files(
        "classes-count-control",
        &[
            files[0],
            (
                "scripts/docs/baseline.json",
                "{\"findings\": [{\"key\": \"md-links|docs/a.md|gone.md\", \"count\": \"\\u001c2\"}]}\n",
            ),
        ],
    );
    assert_like_check_py(&control, &["--check", "md-links"], 2, "");
}
