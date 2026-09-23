---
author: jl + claude
date: 2026-09-22
tags: [tooling, agent-loop]
status: in-progress
---

# Tools R1: aios crate, shim, CI, docs-check — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the 1,670-line `scripts/docs/check.py` with a byte-identical Rust port that ships as `aios docs-check` from a new host-only workspace member `tools/`, reachable from hooks, skills and `just` through the `.claude/hooks/aios` shim.

**Architecture:** `tools/` is a std, host-only crate (package `aios-tools`, lib `aios_tools`, binary `aios`) listed in the workspace `members` but never in `default-members`, so bare-metal builds never touch it; R1 fills it with one subcommand, `docs-check`, layered as a Python-semantics base (`pystr`, `paths`, `proc`), a docs-check model (findings, baseline, comparison), Markdown and repository models, and fifteen checks behind a `Check` trait and an ordered registry. Nothing calls `cargo` directly: hooks, skills and recipes call `.claude/hooks/aios`, which runs the MAIN checkout's `target/tools/release/aios` and rebuilds it in the foreground, in the background, or fails closed, depending on the subcommand and on how stale the binary is. Parity is proved before the Python is deleted — goldens recorded from `check.py` (22 real-repository cases at `33c6b3d`, 48 fixture cases including 15 single-drift variants) plus a differential test that runs both implementations while both exist.

**Tech Stack:** Rust pinned `nightly-2026-09-22`, edition 2021; clap 4.6.7 (derive), anyhow 1.0.104, serde 1.0.229 (derive), serde_json 1.0.151 (`preserve_order`), regex 1.13.1 (no lookaround, no backreferences), time 0.3.55 (declared for R2, unused in R1); POSIX sh for the shim; `just`; GitHub Actions; CPython 3.14 `scripts/docs/check.py` as the reference implementation and golden recorder.

**Spec:** [Rust agent tooling (`aios` host binary)](../discussions/2026-09-22-jl-rust-agent-tools.md), sections `§1`-`§4`. This plan covers PR R1 only; R2-R4 (agent-loop commands, soak port, guard port) are separate PRs.

## Global Constraints

Every task's requirements implicitly include this section.

- Work only in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1` on branch `claude/tools-crate-docs-check` (base main `33c6b3d`). The approved spec is `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` `§1`-`§4`; this plan covers PR R1 only. The plan file is committed before Task 1 (`Plan: Tools R1 docs-check port`).
- Toolchain: pinned `nightly-2026-09-22` (`rust-toolchain.toml`), edition 2021 via `edition.workspace`, license BSD-2-Clause via `license.workspace`. `tools/` is a host-only std crate: package `aios-tools`, lib `aios_tools`, binary `aios`; in `members`, never in `default-members`. Dependencies exactly: anyhow 1.0.104, clap 4.6.7 (derive), regex 1.13.1, serde 1.0.229 (derive), serde_json 1.0.151 (preserve_order), time 0.3.55. Do not run `cargo update`; adding the dependencies updates `Cargo.lock` minimally.
- Code: `#![forbid(unsafe_code)]`; no `#[allow]` in `src/` (only `#![allow(dead_code)]` in `tools/tests/common/mod.rs`); no TODO comments, no placeholders, no stubs; clean under `cargo fmt --check -p aios-tools` and `cargo clippy -p aios-tools -- -D warnings` on the pinned nightly. Errors: `anyhow::Result`; `expect` only on invariants (constant regexes).
- Parity is byte-identical with `scripts/docs/check.py` (read it at `git show 33c6b3d:scripts/docs/check.py` after Task 15): stdout, exit codes (0 no new drift, 1 new drift, 2 checker error; the shim adds 3), and the bytes of a baseline written by `--update-baseline`. Port functions line by line; lines are Python `splitlines()`, whitespace is Python `isspace()`, files are lossy UTF-8 with universal newlines; `\d` is `[0-9]`; the five lookaround patterns use the code replacements spelled out where they are ported — R8 and R25 in Task 4, R24 in Task 5, R43 in Task 9, R45 in Task 10. Accepted Unicode edge divergences are listed in the module docs, never silently.
- Tests: TDD per task (failing test, run, implement, run). Integration tests use `tools/tests/common` (isolated git env, temp dirs under `CARGO_TARGET_TMPDIR`); goldens are recorded from check.py by the ignored recorder, never hand-edited. No `.md` files under `tools/`.
- The shim runs the MAIN checkout's binary, which does not exist until R1 merges. In this worktree run docs-check as `just tools && AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check ...`. Before Task 15 `just docs-check` still runs check.py.
- In plan prose, wrap example links, `[[wiki]]` notes, `§N` references and paths in backticks; docs-check scans this plan file while it exists.
- Never edit `.claude/settings.json` from a task: Task 16's settings step is done by the MAIN SESSION with owner approval. Nothing wires `aios guard` into hooks in R1.
- Commits: one per task, plain descriptive subject (no `Phase N MK:` prefix), body ending with `Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>`. Stage explicit paths. After each commit `git push -u origin claude/tools-crate-docs-check`. Never push to or merge into `main`; the owner merges the PR.
- `scripts/docs/baseline.json` stays byte-identical; if a docs edit legitimately changes a check's findings, update it with `--update-baseline` and state why in the commit and PR body.
- Reference conventions inside the tasks: `check.py L<a>-<b>` means those lines of `git show 33c6b3d:scripts/docs/check.py`; `spec §N` is a section of the spec linked above; `contract §N` is a section of the R1 architect contract that fixed the shared interfaces, and every interface, format and file it fixes is reproduced inline in the task that needs it, so no other document is needed to execute a task; `RNN` is a row of that contract's regex-portability table, quoted with its pattern where it is ported; `TN` in the file tree below is Task N.

## File Structure

Final state of R1. `(TN)` marks the task that creates the file.

```text
Cargo.toml                         members += "tools"; default-members stays ["kernel", "shared"]
Cargo.lock                         new host deps (committed in T1)
tools/
├── Cargo.toml                     package aios-tools; [lib] aios_tools; [[bin]] aios (added in T6)
├── src/
│   ├── main.rs                    clap CLI `aios docs-check ...`; Err → exit 2           (T6)
│   ├── lib.rs                     #![forbid(unsafe_code)]; pub mod cmd, paths, proc, pystr (T1; cmd added T3)
│   ├── pystr.rs                   Python str semantics                                    (T1)
│   ├── paths.rs                   posixpath semantics (normpath, relpath, realpath, ...)  (T1)
│   ├── proc.rs                    subprocess capture                                      (T1)
│   └── cmd/
│       ├── mod.rs                 pub mod docs_check;                                     (T3)
│       └── docs_check/
│           ├── mod.rs             Args, run, run_checks, select_checks, repo_root         (T3 stub of mod lines; T6 body)
│           ├── model.rs           CHECKS table, Finding, Skip, baseline, compare, collate (T3)
│           ├── markdown.rs        parsing helpers, shared regexes, R8/R25 replacements   (T4)
│           ├── repo.rs            Repo (listing, caches, git facts), recipe_name (R24)   (T5)
│           ├── output.rs          text, markdown, JSON, list renderers                   (T6)
│           └── checks/
│               ├── mod.rs         trait Check, registry()                                 (T6, grown T7-T12)
│               ├── links.rs       md-links, section-refs, anchors, wiki-links             (T7)
│               ├── doc_map.rs     doc-map                                                 (T8)
│               ├── repo_paths.rs  repo-paths                                              (T8)
│               ├── just_recipes.rs just-recipes                                           (T8)
│               ├── test_count.rs  test-count                                              (T9)
│               ├── lock_order.rs  lock-order, split_outside_braces (R43)                  (T9)
│               ├── milestones.rs  milestone-status, phase-count, is_unchecked_task (R45)  (T10)
│               ├── layout.rs      layout (+ layout_block, tree_entries)                   (T11)
│               ├── harness.rs     harness-tables (+ project_skills, agents, tables, lists) (T11)
│               ├── pointer_doctor.rs pointer-doctor                                       (T12)
│               └── knowledge.rs   knowledge-hygiene                                       (T12)
└── tests/
    ├── common/
    │   ├── mod.rs                 #![allow(dead_code)]; unique_dir, isolated, git, TestRepo (T2); Run, run_aios (T6); pub mod fixture (T13)
    │   └── fixture.rs             bundle parser, materializer, real snapshot, variants, cases, goldens (T13, T14)
    ├── shim.rs                    shim behaviour with a fake `just`                        (T2)
    ├── repo.rs                    Repo listing/exists/resolve/git facts                    (T5)
    ├── cli.rs                     binary: usage errors, not-a-repo, list-checks            (T6)
    ├── checks_links.rs            (T7)   checks_paths.rs (T8)   checks_code.rs (T9)
    ├── checks_status.rs           (T10)  checks_harness.rs (T11) checks_pointer.rs (T12)
    ├── docs_check_fixture.rs      each variant injects exactly its drift                   (T13)
    ├── docs_check_parity.rs       goldens, recorder (ignored), differential                (T14)
    ├── fixtures/docs-check/
    │   ├── base.txt               Appendix A, byte for byte                                (T13)
    │   └── variants/<name>.txt    Appendix B (18 files)                                    (T13)
    └── golden/docs-check/
        ├── real/<case>.golden, real/update-baseline.baseline.json                         (T14)
        └── fixture/<variant>/<case>.golden, fixture/{base,skip}/update-baseline.baseline.json (T14)
.claude/hooks/aios                 POSIX sh shim, mode 100755                               (T2)
```

No `.md` file may be created under `tools/` (docs-check would scan it); fixtures and goldens use `.txt`, `.golden` and `.json`. Do not use `.bin` or `.log` (ignored by `.gitignore`).

---

### Task 1: Crate scaffold, workspace wiring, Python-semantics helpers

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/Cargo.toml` (package `aios-tools`, no `[[bin]]` yet — Task 6 adds it), `tools/src/lib.rs`, `tools/src/pystr.rs`, `tools/src/paths.rs`, `tools/src/proc.rs`
- Modify: `Cargo.toml` (workspace `members`), `Cargo.lock` (regenerated by the first cargo run), `justfile` (contract `§6.3` T1)
- Test: unit tests inside `tools/src/pystr.rs`, `tools/src/paths.rs`, `tools/src/proc.rs` (filter `cargo test -p aios-tools --lib`)

**Interfaces:**
- Consumes: nothing from this plan (first task). External: `anyhow 1.0.104` and `std` only; the other five dependencies are declared now so later PRs do not churn `Cargo.lock`.
- Produces (`aios_tools::pystr`, contract `§4.1`; Tasks 3-15 use these names):

  ```rust
  pub fn is_space(c: char) -> bool;                // str.isspace
  pub fn strip(s: &str) -> &str;                   // str.strip()
  pub fn lstrip(s: &str) -> &str;
  pub fn rstrip(s: &str) -> &str;
  pub fn split_ws(s: &str) -> Vec<&str>;           // str.split()
  pub fn splitlines(s: &str) -> Vec<&str>;         // str.splitlines()
  pub fn decode_text(bytes: &[u8]) -> String;      // lossy UTF-8 + universal newlines
  pub fn char_prefix(s: &str, n: usize) -> &str;   // s[:n] in code points
  pub fn indent_width(line: &str) -> usize;        // leading spaces of line.expandtabs(4)
  pub fn is_ascii_digits(s: &str) -> bool;         // non-empty, all '0'..='9' (str.isdigit, ASCII)
  pub fn parse_uint(s: &str) -> Option<u64>;       // int(s) for ASCII digits; None on overflow
  pub fn int_str(digits: &str) -> String;          // str(int(digits)), any length
  pub fn url_unquote(s: &str) -> String;           // urllib.parse.unquote
  ```

- Produces (`aios_tools::paths`, contract `§4.2`):

  ```rust
  pub fn normpath(path: &str) -> String;                        // posixpath.normpath
  pub fn join(a: &str, b: &str) -> String;                      // posixpath.join(a, b)
  pub fn dirname(path: &str) -> &str;                           // posixpath.dirname
  pub fn basename(path: &str) -> &str;                          // posixpath.basename
  pub fn splitext(path: &str) -> (&str, &str);                  // posixpath.splitext
  pub fn abspath(path: &str, cwd: &str) -> String;              // normpath(join(cwd, path))
  pub fn relpath(path: &str, start: &str, cwd: &str) -> String; // posixpath.relpath
  pub fn realpath(path: &str, cwd: &str) -> String;             // posixpath.realpath(strict=False)
  ```

- Produces (`aios_tools::proc`, contract `§4.3`):

  ```rust
  pub fn capture(program: &str, args: &[&str], cwd: &std::path::Path)
      -> anyhow::Result<std::process::Output>;
  ```

**Notes (check.py at `33c6b3d`; re-read each range while porting):**
- These three modules carry the Python semantics that every later task depends on. Their callers in check.py: `str.splitlines()` at L257, L328, L457, L474, L512, L824, L865, L902, L939, L946, L1178, L1285, L1343; `line.expandtabs(4)` at L275; `str.split()` at L1172, L1232-1233, L1240, L1322; `str.isdigit()` at L854, L963, L1000; `urllib.parse.unquote` (imported L41) at L571, L617, L620, L661; `posixpath.dirname`/`basename`/`splitext`/`normpath`/`join` at L386-389, L431-433, L636-641, L678, L822, L1056-1058, L1355; `os.path.realpath` and `os.path.relpath` at L422-425 (`Repo.exists`) and L1612 (`baseline_rel`); text-mode reading (lossy UTF-8, universal newlines) at L399-401; `subprocess.run(..., capture_output=True, text=True)` at L396-397 (`Repo.git`) and L1578 (`repo_root`).
- `realpath` is CPython 3.14's `posixpath.realpath(strict=False)` line by line: a component stack, a `seen` map from symlink path to fully resolved target (`None` while unresolved, which is how a loop is detected), `path[:path.rindex('/')] or '/'` for `..`, `lstat`/`readlink` errors ignored so missing components are appended, and an absolute link target resetting the resolved path to `/`. The repository has a tracked symlink (`.claude/skills/obsidian -> ../../.agents/skills/obsidian`) that `Repo::exists` (Task 5) resolves through it.
- Accepted divergences, stated in each module's doc comment (contract `§1.9`): `is_ascii_digits`/`parse_uint` accept ASCII digits only where Python's `str.isdigit()`/`int()` also accept other Unicode decimal digits; integers that do not fit `u64` are treated as no match; a symlink target that is not valid UTF-8 goes through `to_string_lossy` where CPython uses surrogate escapes.
- Every expected value in the tests below was recorded from CPython 3.14 (`python3 -c ...`) by calling the method the function ports.
- Dependency licences stay inside `deny.toml`'s allow list (clap, anyhow, serde, serde_json, regex, time and their transitive crates are MIT / Apache-2.0 / Unicode-3.0 / BSD), and `[bans] multiple-versions = "warn"`, so `just deny` and `just audit` (CI's Security job) stay green; there is no new `--target aarch64-unknown-none` work because `tools` is not a default member.

- [ ] **Step 1: Write the failing tests**

Create `tools/Cargo.toml`:

```toml
[package]
name = "aios-tools"
version = "0.1.0"
edition.workspace = true
license.workspace = true
publish = false
description = "AIOS host tooling: the aios binary (docs-check; agent-loop commands in later PRs)"

[lib]
name = "aios_tools"
path = "src/lib.rs"

[dependencies]
anyhow = "1.0.104"
clap = { version = "4.6.7", features = ["derive"] }
regex = "1.13.1"
serde = { version = "1.0.229", features = ["derive"] }
serde_json = { version = "1.0.151", features = ["preserve_order"] }
time = "0.3.55"
```

Edit the workspace `Cargo.toml`, replacing

```toml
members = ["kernel", "shared", "uefi-stub"]
```

with

```toml
members = ["kernel", "shared", "uefi-stub", "tools"]
```

(`default-members = ["kernel", "shared"]` is unchanged: the aarch64 builds must never select the host crate.)

Create `tools/src/lib.rs`:

```rust
//! AIOS host tooling: the library behind the `aios` binary.
//!
//! R1 provides `docs-check`, a byte-for-byte port of `scripts/docs/check.py`;
//! later PRs add the agent-loop commands. The crate is host-only (`std`) and is
//! a workspace member but never a default member, so `cargo build --target
//! aarch64-unknown-none` never selects it.
#![forbid(unsafe_code)]

pub mod paths;
pub mod proc;
pub mod pystr;
```

Create `tools/src/pystr.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Every expected value below was recorded from CPython 3.14 by calling the
    // `str` method (or `urllib.parse.unquote`) that the function ports.

    #[test]
    fn is_space_matches_python() {
        for c in [
            ' ', '\t', '\n', '\r', '\u{b}', '\u{c}', '\u{1c}', '\u{1d}', '\u{1e}', '\u{1f}',
            '\u{85}', '\u{a0}', '\u{2028}', '\u{2029}', '\u{3000}',
        ] {
            assert!(is_space(c), "Python says {c:?} is whitespace");
        }
        for c in ['a', '0', '\0', '\u{200b}', '\u{180e}'] {
            assert!(!is_space(c), "Python says {c:?} is not whitespace");
        }
    }

    #[test]
    fn strip_family_matches_python() {
        assert_eq!(strip(" \u{1c} a b \t\n"), "a b");
        assert_eq!(lstrip("\u{a0}x "), "x ");
        assert_eq!(rstrip(" x\u{2028}"), " x");
        assert_eq!(strip("   "), "");
        assert_eq!(strip(""), "");
        assert_eq!(strip("no-whitespace"), "no-whitespace");
    }

    #[test]
    fn split_ws_matches_python() {
        assert_eq!(split_ws(" a\u{1c}b  c "), vec!["a", "b", "c"]);
        assert!(split_ws("").is_empty());
        assert!(split_ws("   ").is_empty());
        assert_eq!(split_ws("one"), vec!["one"]);
    }

    #[test]
    fn splitlines_matches_python() {
        let text = "a\nb\r\nc\rd\u{b}e\u{c}f\u{1c}g\u{1d}h\u{1e}i\u{85}j\u{2028}k\u{2029}l";
        assert_eq!(
            splitlines(text),
            vec!["a", "b", "c", "d", "e", "f", "g", "h", "i", "j", "k", "l"]
        );
        assert!(splitlines("").is_empty());
        assert_eq!(splitlines("a\n"), vec!["a"]);
        assert_eq!(splitlines("\n"), vec![""]);
        assert_eq!(splitlines("a\n\nb"), vec!["a", "", "b"]);
        assert_eq!(splitlines("trailing"), vec!["trailing"]);
    }

    #[test]
    fn decode_text_is_lossy_with_universal_newlines() {
        assert_eq!(decode_text(b"a\r\nb\rc\n"), "a\nb\nc\n");
        assert_eq!(decode_text("café".as_bytes()), "café");
        assert_eq!(decode_text(b"x\xe2\x82y"), "x\u{fffd}y");
        assert_eq!(decode_text(b""), "");
    }

    #[test]
    fn char_prefix_counts_code_points() {
        assert_eq!(char_prefix("héllo", 3), "hél");
        assert_eq!(char_prefix("ab", 5), "ab");
        assert_eq!(char_prefix("ab", 0), "");
        assert_eq!(char_prefix("", 3), "");
    }

    #[test]
    fn indent_width_expands_tabs_to_four() {
        for (line, width) in [
            ("\tx", 4),
            ("  \tx", 4),
            ("    x", 4),
            ("\t\tx", 8),
            (" x", 1),
            ("a\tb", 0),
            ("   ", 3),
            ("", 0),
        ] {
            assert_eq!(indent_width(line), width, "indent of {line:?}");
        }
    }

    #[test]
    fn digits_and_ints_match_python() {
        assert!(is_ascii_digits("0123"));
        assert!(!is_ascii_digits(""));
        assert!(!is_ascii_digits("12a"));
        // Accepted divergence: Python's str.isdigit() is true for these.
        assert!(!is_ascii_digits("١٢"));

        assert_eq!(parse_uint("007"), Some(7));
        assert_eq!(parse_uint("18446744073709551615"), Some(u64::MAX));
        assert_eq!(parse_uint("18446744073709551616"), None);
        assert_eq!(parse_uint(""), None);
        assert_eq!(parse_uint("1x"), None);

        assert_eq!(int_str("007"), "7");
        assert_eq!(int_str("0000"), "0");
        assert_eq!(int_str("42"), "42");
        assert_eq!(
            int_str("0012300000000000000000000000045"),
            "12300000000000000000000000045"
        );
    }

    #[test]
    fn url_unquote_matches_python() {
        for (raw, want) in [
            ("a%2Fb", "a/b"),
            ("%2", "%2"),
            ("%zz", "%zz"),
            ("100%", "100%"),
            ("caf%C3%A9", "café"),
            ("a%C3%A9b", "aéb"),
            ("%E2%82", "\u{fffd}"),
            ("x y", "x y"),
            ("a%20b%", "a b%"),
            ("%41%42", "AB"),
            ("é%41", "éA"),
            ("", ""),
        ] {
            assert_eq!(url_unquote(raw), want, "unquote({raw:?})");
        }
    }
}
```

Create `tools/src/paths.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A private directory under the system temp dir, removed when dropped.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> TempTree {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("aios-tools-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create the temp tree");
            let root = std::fs::canonicalize(&root).expect("canonicalize the temp tree");
            TempTree { root }
        }

        fn base(&self) -> String {
            self.root.to_str().expect("a UTF-8 temp path").to_string()
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    // Every expected value below was recorded from CPython 3.14's posixpath.

    #[test]
    fn normpath_matches_posixpath() {
        for (raw, want) in [
            ("", "."),
            (".", "."),
            ("a/./b", "a/b"),
            ("a//b", "a/b"),
            ("/a/../b", "/b"),
            ("a/../..", ".."),
            ("../a", "../a"),
            ("//a/b", "//a/b"),
            ("///a", "/a"),
            ("a/b/", "a/b"),
            ("/", "/"),
            ("..", ".."),
            ("a/..", "."),
            ("/../x", "/x"),
        ] {
            assert_eq!(normpath(raw), want, "normpath({raw:?})");
        }
    }

    #[test]
    fn join_matches_posixpath() {
        for ((a, b), want) in [
            (("a", "b"), "a/b"),
            (("a/", "b"), "a/b"),
            (("", "b"), "b"),
            (("a", "/b"), "/b"),
            (("a", ""), "a/"),
            (("/", "x"), "/x"),
        ] {
            assert_eq!(join(a, b), want, "join({a:?}, {b:?})");
        }
    }

    #[test]
    fn dirname_basename_splitext_match_posixpath() {
        for (raw, dir, base, stem, ext) in [
            ("a/b/c", "a/b", "c", "a/b/c", ""),
            ("a", "", "a", "a", ""),
            ("/a", "/", "a", "/a", ""),
            ("/", "/", "", "/", ""),
            ("a/", "a", "", "a/", ""),
            ("docs/x.md", "docs", "x.md", "docs/x", ".md"),
            (".bashrc", "", ".bashrc", ".bashrc", ""),
            ("a/.b.c", "a", ".b.c", "a/.b", ".c"),
            ("x.", "", "x.", "x", "."),
            ("a.tar.gz", "", "a.tar.gz", "a.tar", ".gz"),
        ] {
            assert_eq!(dirname(raw), dir, "dirname({raw:?})");
            assert_eq!(basename(raw), base, "basename({raw:?})");
            assert_eq!(splitext(raw), (stem, ext), "splitext({raw:?})");
        }
    }

    #[test]
    fn abspath_and_relpath_match_posixpath() {
        assert_eq!(abspath("scripts/docs", "/r"), "/r/scripts/docs");
        assert_eq!(abspath("/tmp/x", "/r"), "/tmp/x");
        assert_eq!(abspath(".", "/r"), "/r");

        for ((path, start), want) in [
            (("/r/scripts/docs/baseline.json", "/r"), "scripts/docs/baseline.json"),
            (("/r", "/r"), "."),
            (("/a/b", "/a/c"), "../b"),
            (("/r/x", "/r/y/z"), "../../x"),
        ] {
            assert_eq!(relpath(path, start, "/cwd"), want, "relpath({path:?}, {start:?})");
        }
        // A relative argument is resolved against `cwd`, as check.py L1612 does.
        assert_eq!(relpath("base.json", "/r", "/r/sub"), "sub/base.json");
    }

    #[test]
    fn realpath_resolves_links_and_keeps_missing_and_looping_parts() {
        let tree = TempTree::new("realpath");
        let base = tree.base();
        std::fs::create_dir_all(tree.root.join("a/sub")).expect("create a/sub");
        std::fs::write(tree.root.join("a/sub/f"), "x").expect("write a/sub/f");
        std::os::unix::fs::symlink("a", tree.root.join("link")).expect("link -> a");
        std::os::unix::fs::symlink("loop", tree.root.join("loop")).expect("loop -> loop");
        std::os::unix::fs::symlink("/nowhere", tree.root.join("abs")).expect("abs -> /nowhere");

        assert_eq!(realpath("link/sub/f", &base), format!("{base}/a/sub/f"));
        assert_eq!(realpath("link/../a", &base), format!("{base}/a"));
        assert_eq!(realpath("a/../a/sub/f", &base), format!("{base}/a/sub/f"));
        assert_eq!(realpath("loop", &base), format!("{base}/loop"));
        assert_eq!(realpath("loop/x", &base), format!("{base}/loop/x"));
        assert_eq!(realpath("missing/x", &base), format!("{base}/missing/x"));
        assert_eq!(realpath("abs/y", &base), "/nowhere/y");
        assert_eq!(
            realpath(&format!("{base}/link/sub"), "/elsewhere"),
            format!("{base}/a/sub")
        );
        assert_eq!(realpath("/..", "/"), "/");
        assert_eq!(realpath("/aios-no-such-dir/..", "/"), "/");
    }
}
```

Create `tools/src/proc.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_returns_both_streams_and_the_status() {
        let dir = std::env::temp_dir();
        let out = capture("sh", &["-c", "printf out; printf err >&2; exit 3"], &dir)
            .expect("sh starts");
        assert_eq!(out.stdout, b"out");
        assert_eq!(out.stderr, b"err");
        assert_eq!(out.status.code(), Some(3));
    }

    #[test]
    fn capture_runs_in_the_given_directory() {
        let dir = std::fs::canonicalize(std::env::temp_dir()).expect("canonicalize the temp dir");
        let out = capture("sh", &["-c", "pwd -P"], &dir).expect("sh starts");
        assert_eq!(
            String::from_utf8_lossy(&out.stdout).trim_end(),
            dir.to_string_lossy()
        );
    }

    #[test]
    fn capture_errors_when_the_program_cannot_start() {
        let err = capture("aios-no-such-program", &[], &std::env::temp_dir())
            .expect_err("there is no such program");
        assert!(
            format!("{err:#}").contains("cannot run aios-no-such-program"),
            "{err:#}"
        );
    }
}
```

Edit the `justfile`, replacing

```just
# Run host-side unit tests (kernel is no_std, excluded from host tests)
test:
    cargo test --workspace --exclude kernel --exclude uefi-stub --target-dir target/host-tests

# Run clippy with deny warnings (both kernel and stub targets)
clippy:
    cargo clippy --target {{target}} -- -D warnings
    cargo clippy -p uefi-stub --target {{uefi_target}} -- -D warnings
```

with

```just
# Run host-side unit tests (kernel is no_std, excluded from host tests; the tools
# crate runs `cargo test -p aios-tools` in CI's Tools (host) job, which has full history)
test:
    cargo test --workspace --exclude kernel --exclude uefi-stub --exclude aios-tools --target-dir target/host-tests

# Run clippy with deny warnings (kernel and stub targets, plus the host tools crate)
clippy:
    cargo clippy --target {{target}} -- -D warnings
    cargo clippy -p uefi-stub --target {{uefi_target}} -- -D warnings
    cargo clippy -p aios-tools -- -D warnings
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --lib`
Expected: FAIL. The run first resolves the six new dependencies and rewrites `Cargo.lock` (this needs network access once), then the test targets fail to compile with `error[E0425]: cannot find function ...` for `is_space`, `strip`, `splitlines`, `decode_text`, `normpath`, `realpath`, `capture` and the rest: the tests exist, the functions do not.

- [ ] **Step 3: Write the implementation**

Insert above the `#[cfg(test)]` module in `tools/src/pystr.rs`:

```rust
//! Python `str` semantics that the docs-check port depends on.
//!
//! `scripts/docs/check.py` runs on CPython, so the port must strip, split and
//! slice text exactly as Python does: `str.isspace` (check.py's strips and
//! `split()` calls), `str.splitlines()` (L257, L328, L457, L474, L512, L824,
//! L865, L902, L939, L946, L1178, L1285, L1343), text-mode reading (L399-401),
//! `str.expandtabs(4)` (L275), `str.isdigit()` (L854, L963, L1000), `int()` /
//! `str(int())` and `urllib.parse.unquote` (L571, L617, L620, L661).
//!
//! Accepted divergences (contract §1.9; no tracked file reaches them):
//! `is_ascii_digits` and `parse_uint` accept ASCII digits only, where Python's
//! `str.isdigit()` and `int()` also accept other Unicode decimal digits, and
//! numbers that do not fit `u64` are treated as no match. The `regex` crate's
//! `\s` lacks U+001C..U+001F, so ported code uses these helpers instead of a
//! pattern wherever Python whitespace matters.

/// `str.isspace()` for one character: Unicode whitespace plus U+001C..U+001F,
/// which Python counts as whitespace and `char::is_whitespace` does not.
pub fn is_space(c: char) -> bool {
    c.is_whitespace() || ('\u{1c}'..='\u{1f}').contains(&c)
}

/// `str.strip()`.
pub fn strip(s: &str) -> &str {
    lstrip(rstrip(s))
}

/// `str.lstrip()`.
pub fn lstrip(s: &str) -> &str {
    s.trim_start_matches(is_space)
}

/// `str.rstrip()`.
pub fn rstrip(s: &str) -> &str {
    s.trim_end_matches(is_space)
}

/// `str.split()` with no argument: split on runs of whitespace, dropping the
/// empty parts at both ends.
pub fn split_ws(s: &str) -> Vec<&str> {
    s.split(is_space).filter(|part| !part.is_empty()).collect()
}

/// `str.splitlines()`: breaks at `\n`, `\r`, `\r\n`, `\x0b`, `\x0c`, `\x1c`,
/// `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029, with no trailing empty element.
pub fn splitlines(s: &str) -> Vec<&str> {
    let mut lines = Vec::new();
    let mut start = 0usize;
    let mut chars = s.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        let is_break = if c == '\r' {
            if chars.peek().map(|&(_, next)| next) == Some('\n') {
                chars.next();
            }
            true
        } else {
            matches!(
                c,
                '\n' | '\u{b}' | '\u{c}' | '\u{1c}' | '\u{1d}' | '\u{1e}' | '\u{85}' | '\u{2028}'
                    | '\u{2029}'
            )
        };
        if is_break {
            lines.push(&s[start..i]);
            start = chars.peek().map_or(s.len(), |&(j, _)| j);
        }
    }
    if start < s.len() {
        lines.push(&s[start..]);
    }
    lines
}

/// Python text mode: decode UTF-8 with replacement, then universal newlines
/// (`"\r\n"` then `"\r"` become `"\n"`).
pub fn decode_text(bytes: &[u8]) -> String {
    let text = String::from_utf8_lossy(bytes);
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.into_owned()
    }
}

/// `s[:n]`, counting code points as Python does.
pub fn char_prefix(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// The number of leading spaces of `line.expandtabs(4)`.
pub fn indent_width(line: &str) -> usize {
    let mut width = 0;
    for c in line.chars() {
        match c {
            ' ' => width += 1,
            '\t' => width = width / 4 * 4 + 4,
            _ => break,
        }
    }
    width
}

/// `str.isdigit()` restricted to ASCII digits (contract §1.9).
pub fn is_ascii_digits(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

/// `int(s)` for a run of ASCII digits; `None` when `s` is not one, or overflows.
pub fn parse_uint(s: &str) -> Option<u64> {
    if !is_ascii_digits(s) {
        return None;
    }
    s.parse::<u64>().ok()
}

/// `str(int(digits))` for a run of ASCII digits of any length: leading zeros go.
pub fn int_str(digits: &str) -> String {
    let trimmed = digits.trim_start_matches('0');
    if trimmed.is_empty() {
        "0".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `urllib.parse.unquote(s)`: only ASCII runs are unquoted, each run's bytes
/// are decoded as UTF-8 with replacement, and non-ASCII runs pass through.
pub fn url_unquote(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while !rest.is_empty() {
        let ascii = rest.find(|c: char| !c.is_ascii()).unwrap_or(rest.len());
        if ascii > 0 {
            out.push_str(&unquote_ascii(&rest[..ascii]));
            rest = &rest[ascii..];
        }
        let other = rest.find(|c: char| c.is_ascii()).unwrap_or(rest.len());
        out.push_str(&rest[..other]);
        rest = &rest[other..];
    }
    out
}

/// `unquote_to_bytes` on one ASCII run, then a lossy UTF-8 decode.
fn unquote_ascii(run: &str) -> String {
    let bytes = run.as_bytes();
    let mut raw: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            if let Some(byte) = hex_byte(bytes.get(i + 1), bytes.get(i + 2)) {
                raw.push(byte);
                i += 3;
                continue;
            }
        }
        raw.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&raw).into_owned()
}

fn hex_byte(hi: Option<&u8>, lo: Option<&u8>) -> Option<u8> {
    let hi = char::from(*hi?).to_digit(16)?;
    let lo = char::from(*lo?).to_digit(16)?;
    Some((hi * 16 + lo) as u8)
}
```

Insert above the `#[cfg(test)]` module in `tools/src/paths.rs`:

```rust
//! `posixpath` semantics used by check.py: `normpath`, `join`, `dirname`,
//! `basename` and `splitext` (L386-389, L431-433, L636-641, L678, L822,
//! L1056-1058, L1355), `relpath` (L425, L1612) and non-strict `realpath`
//! (L422-425, which resolves through the repository's tracked
//! `.claude/skills/obsidian` symlink).
//!
//! Every function works on `/`-separated strings, never on `std::path`
//! components, because check.py's paths are repository-relative POSIX paths.
//!
//! Accepted divergences (contract §1.9): a symlink target that is not valid
//! UTF-8 goes through `to_string_lossy` where CPython uses surrogate escapes,
//! and `relpath` treats an empty `path` as the current directory where Python
//! raises `ValueError` (docs-check never passes one: `--baseline ""` counts as
//! absent, check.py L1611).

use std::collections::HashMap;

/// `posixpath.normpath`.
pub fn normpath(path: &str) -> String {
    if path.is_empty() {
        return ".".to_string();
    }
    let mut initial_slashes = usize::from(path.starts_with('/'));
    if initial_slashes == 1 && path.starts_with("//") && !path.starts_with("///") {
        initial_slashes = 2;
    }
    let mut comps: Vec<&str> = Vec::new();
    for comp in path.split('/') {
        if comp.is_empty() || comp == "." {
            continue;
        }
        if comp != ".."
            || (initial_slashes == 0 && comps.is_empty())
            || comps.last() == Some(&"..")
        {
            comps.push(comp);
        } else if !comps.is_empty() {
            comps.pop();
        }
    }
    let mut out = "/".repeat(initial_slashes);
    out.push_str(&comps.join("/"));
    if out.is_empty() {
        ".".to_string()
    } else {
        out
    }
}

/// `posixpath.join(a, b)`.
pub fn join(a: &str, b: &str) -> String {
    if b.starts_with('/') {
        b.to_string()
    } else if a.is_empty() || a.ends_with('/') {
        format!("{a}{b}")
    } else {
        format!("{a}/{b}")
    }
}

/// `posixpath.dirname`.
pub fn dirname(path: &str) -> &str {
    let end = path.rfind('/').map_or(0, |i| i + 1);
    let head = &path[..end];
    if !head.is_empty() && head.bytes().any(|b| b != b'/') {
        head.trim_end_matches('/')
    } else {
        head
    }
}

/// `posixpath.basename`.
pub fn basename(path: &str) -> &str {
    let start = path.rfind('/').map_or(0, |i| i + 1);
    &path[start..]
}

/// `posixpath.splitext`: leading dots of the basename never start an extension.
pub fn splitext(path: &str) -> (&str, &str) {
    let sep = path.rfind('/');
    let Some(dot) = path.rfind('.') else {
        return (path, "");
    };
    if !sep.is_none_or(|i| dot > i) {
        return (path, "");
    }
    let mut i = sep.map_or(0, |s| s + 1);
    while i < dot {
        if path.as_bytes()[i] != b'.' {
            return (&path[..dot], &path[dot..]);
        }
        i += 1;
    }
    (path, "")
}

/// `posixpath.abspath` with an explicit working directory.
pub fn abspath(path: &str, cwd: &str) -> String {
    normpath(&join(cwd, path))
}

/// `posixpath.relpath(path, start)` with an explicit working directory.
pub fn relpath(path: &str, start: &str, cwd: &str) -> String {
    let start_abs = abspath(start, cwd);
    let path_abs = abspath(path, cwd);
    let start_list: Vec<&str> = start_abs.split('/').filter(|p| !p.is_empty()).collect();
    let path_list: Vec<&str> = path_abs.split('/').filter(|p| !p.is_empty()).collect();
    let common = start_list
        .iter()
        .zip(path_list.iter())
        .take_while(|(a, b)| a == b)
        .count();
    let mut rel: Vec<&str> = vec![".."; start_list.len() - common];
    rel.extend_from_slice(&path_list[common..]);
    if rel.is_empty() {
        return ".".to_string();
    }
    rel.join("/")
}

/// `posixpath.realpath(path, strict=False)` with an explicit working directory:
/// CPython 3.14's algorithm, so a symlink loop keeps the unresolved path and
/// missing components are appended verbatim.
pub fn realpath(path: &str, cwd: &str) -> String {
    /// A component still to resolve, or the marker CPython pushes to record a
    /// symlink's fully resolved target in `seen`.
    enum Frame {
        Name(String),
        Mark(String),
    }

    let mut rest: Vec<Frame> = path
        .split('/')
        .rev()
        .map(|part| Frame::Name(part.to_string()))
        .collect();
    let mut part_count = rest.len();
    let mut resolved = if path.starts_with('/') {
        "/".to_string()
    } else {
        cwd.to_string()
    };
    let mut seen: HashMap<String, Option<String>> = HashMap::new();

    while part_count > 0 {
        let Some(frame) = rest.pop() else { break };
        let name = match frame {
            Frame::Mark(link) => {
                seen.insert(link, Some(resolved.clone()));
                continue;
            }
            Frame::Name(name) => name,
        };
        part_count -= 1;
        if name.is_empty() || name == "." {
            continue;
        }
        if name == ".." {
            let cut = resolved.rfind('/').unwrap_or(0);
            resolved.truncate(cut);
            if resolved.is_empty() {
                resolved.push('/');
            }
            continue;
        }
        let newpath = if resolved == "/" {
            format!("/{name}")
        } else {
            format!("{resolved}/{name}")
        };
        let link_target = match std::fs::symlink_metadata(&newpath) {
            Ok(meta) if meta.file_type().is_symlink() => match seen.get(&newpath).cloned() {
                Some(Some(cached)) => {
                    resolved = cached;
                    continue;
                }
                Some(None) => {
                    resolved = newpath;
                    continue;
                }
                None => std::fs::read_link(&newpath).ok(),
            },
            Ok(_) => {
                resolved = newpath;
                continue;
            }
            Err(_) => None,
        };
        let Some(link_target) = link_target else {
            resolved = newpath;
            continue;
        };
        let target = link_target.to_string_lossy().into_owned();
        if target.starts_with('/') {
            resolved = "/".to_string();
        }
        seen.insert(newpath.clone(), None);
        rest.push(Frame::Mark(newpath));
        let parts: Vec<&str> = target.split('/').collect();
        part_count += parts.len();
        rest.extend(parts.into_iter().rev().map(|p| Frame::Name(p.to_string())));
    }
    resolved
}
```

Insert above the `#[cfg(test)]` module in `tools/src/proc.rs`:

```rust
//! Subprocess capture, the port's single entry point for running `git`
//! (check.py L396-397 `Repo.git` and L1578 `repo_root`).

use std::path::Path;
use std::process::{Command, Output};

use anyhow::{Context, Result};

/// Run `program args` in `cwd`, capturing stdout and stderr. The result is an
/// error only when the process cannot start; a non-zero exit is reported in the
/// returned `Output`, as `subprocess.run(..., capture_output=True)` does.
pub fn capture(program: &str, args: &[&str], cwd: &Path) -> Result<Output> {
    Command::new(program)
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("cannot run {program} in {}", cwd.display()))
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --lib`
Expected: PASS. 17 tests run (`paths::tests::{abspath_and_relpath_match_posixpath, dirname_basename_splitext_match_posixpath, join_matches_posixpath, normpath_matches_posixpath, realpath_resolves_links_and_keeps_missing_and_looping_parts}`, `proc::tests::{capture_errors_when_the_program_cannot_start, capture_returns_both_streams_and_the_status, capture_runs_in_the_given_directory}`, `pystr::tests::{char_prefix_counts_code_points, decode_text_is_lossy_with_universal_newlines, digits_and_ints_match_python, indent_width_expands_tabs_to_four, is_space_matches_python, split_ws_matches_python, splitlines_matches_python, strip_family_matches_python, url_unquote_matches_python}`) and the run ends `test result: ok. 17 passed; 0 failed`.

- [ ] **Step 5: Format, lint and the workspace gates**

Run:

```bash
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
just check
just test
cargo metadata --no-deps --format-version 1 | jq -r '.workspace_default_members[]'
cargo build --target aarch64-unknown-none -v 2>&1 | grep -c 'aios[-_]tools' || true
```

Expected: `cargo fmt --check` prints nothing; clippy ends with `Finished` and no warning; `just check` (fmt-check, the three clippy lines, build, build-stub) passes with zero warnings; `just test` runs the shared-crate host tests only (no `aios-tools` target in its output); `workspace_default_members` prints exactly the two lines ending `/kernel#0.1.0` and `/shared#0.1.0`; the last command prints `0`, proving the aarch64 build never compiles the host crate.

- [ ] **Step 6: Commit and push**

```bash
git add Cargo.toml Cargo.lock justfile tools/Cargo.toml tools/src/lib.rs tools/src/pystr.rs tools/src/paths.rs tools/src/proc.rs
git commit -F - <<'MSG'
Add the aios-tools host crate with the Python-semantics helpers

tools/ (package aios-tools, library aios_tools) is a host-only std crate:
a workspace member, never a default member, so cargo build --target
aarch64-unknown-none keeps building only the kernel and shared crates.

pystr, paths and proc reproduce the CPython str, posixpath and subprocess
behaviour that the docs-check port depends on, with the expected values in
the unit tests recorded from CPython 3.14. just clippy gains the host
crate; just test excludes it, because its tests need full git history and
run in CI's Tools (host) job.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 2: Shim, SessionStart prebuild, CI Tools job

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `.claude/hooks/aios` (mode `100755`), `tools/tests/common/mod.rs`, `tools/tests/shim.rs`
- Modify: `.claude/hooks/setup-dev-env.sh`, `.github/workflows/ci.yml`
- Test: `tools/tests/shim.rs` (`cargo test -p aios-tools --test shim`)

**Interfaces:**
- Consumes: nothing from Task 1's library (the shim is POSIX sh; the test crate only needs the package to exist).
- Produces: the shim contract that `just docs-check`, `just docs-check-all` (Task 15), the SessionStart hook and every later subcommand rely on:

  | invocation | binary fresh | binary stale | binary missing |
  |---|---|---|---|
  | `aios guard ...` | run it | run it, start one background `just tools` | print the PreToolUse `ask` JSON, exit 0 |
  | `aios <other> ...` | run it | rebuild in the foreground, then run it | build, then run; exit 3 naming `just tools` if the build fails |
  | `aios --prebuild` | exit 0 | start a background build, exit 0 | start a background build, exit 0 |

  `AIOS_TOOLS_BIN` overrides the resolved binary (and `--prebuild` then does nothing); a missing override exits 3, or prints the ask JSON for `guard`. Fresh means the binary is newer than every file under the main checkout's `tools/` and than its `Cargo.lock`.

- Produces (`tools/tests/common/mod.rs`, contract `§4.11`; Tasks 5-14 extend and use it):

  ```rust
  pub fn unique_dir(label: &str) -> std::path::PathBuf;
  pub fn isolated(cmd: &mut std::process::Command) -> &mut std::process::Command;
  pub fn git(dir: &std::path::Path, args: &[&str]) -> String;
  pub struct TestRepo { /* root */ }
  impl TestRepo {
      pub fn new(label: &str) -> TestRepo;
      pub fn adopt(root: std::path::PathBuf) -> TestRepo;
      pub fn with_files(label: &str, files: &[(&str, &str)]) -> TestRepo;
      pub fn write(&self, rel: &str, content: &str);
      pub fn commit(&self, subject: &str);
      pub fn path(&self) -> &std::path::Path;
      pub fn path_str(&self) -> &str;
  }
  ```

**Notes:**
- Nothing here ports check.py: the shim, the SessionStart prebuild and the CI job are the delivery path for the binary that Task 15 makes `just docs-check` call. The shim text is contract `§6.1` verbatim, `setup-dev-env.sh` is `§6.2`, the CI job is `§6.4`.
- Every branch of the shim was exercised against a fake `just` before this plan was written (missing/stale/fresh × `guard`/`docs-check`/`--prebuild`, the override, and a linked worktree); the assertions below are the observed outputs, byte for byte.
- R1's binary has no `guard` subcommand, so nothing may wire `aios guard` into `.claude/settings.json` before R5: the shim's guard branch exists only so later PRs add a subcommand and nothing else.
- The freshness check is `find "$main/tools" "$main/Cargo.lock" -newer "$bin"`, so the tests set mtimes with `touch -t 200001010000` (stale) and `touch -t 209901010000` (fresh) rather than sleeping.
- `CARGO_TARGET_TMPDIR` lies inside the outer checkout (`target/tmp`), so a bare `unique_dir` is inside the outer repository; `TestRepo::new` therefore always runs `git init`, and `isolated()` strips the ambient git environment so a developer's global config cannot change the result. `isolated()` covers only the processes these helpers spawn, so `TestRepo::new` also pins `core.excludesFile=/dev/null` in the repository's own config: that is what the in-process `Repo::open` of Tasks 5-12 reads, and `git ls-files --others --exclude-standard` consults `~/.config/git/ignore` even when no `core.excludesFile` is configured.
- Background builds redirect their own stdin, stdout and stderr (`§6.1` `build_bg`), so `Command::output()` never blocks on an inherited pipe; the tests wait for `just.log` and for the `target/tools/.building` lock to disappear.
- The fake `just` installs its binary with a write to `aios.new` and `mv -f`, never a truncating `cat >` over the live path: in `a_stale_guard_runs_at_once_and_rebuilds_in_the_background` the shim `exec`s that file while the background build writes it, and a script (unlike an ELF image) has no `ETXTBSY` protection, so an in-place rewrite can make `/bin/sh` read a truncated file and fail the exact-stdout assertion intermittently.

- [ ] **Step 1: Write the failing tests**

Create `tools/tests/common/mod.rs`:

```rust
#![allow(dead_code)]
//! Shared helpers for the aios-tools integration tests: a temp directory per
//! test, an isolated git environment, and a throwaway repository.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};

static COUNTER: AtomicUsize = AtomicUsize::new(0);

/// A fresh empty directory under `CARGO_TARGET_TMPDIR`.
pub fn unique_dir(label: &str) -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir =
        Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("{label}-{}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create the test directory");
    dir
}

/// Strip the ambient git and Python environment so a test never depends on the
/// developer's configuration.
pub fn isolated(cmd: &mut Command) -> &mut Command {
    let home = Path::new(env!("CARGO_TARGET_TMPDIR")).join("home");
    std::fs::create_dir_all(&home).expect("create the isolated HOME");
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", &home)
        .env("PYTHONUTF8", "1")
        .env("PYTHONDONTWRITEBYTECODE", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("AIOS_TOOLS_BIN")
}

/// Run git in `dir` with a fixed identity; panics with git's stderr on failure.
pub fn git(dir: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    isolated(&mut cmd)
        .arg("-c")
        .arg("user.name=aios-test")
        .arg("-c")
        .arg("user.email=aios-test@example.invalid")
        .arg("-c")
        .arg("commit.gpgsign=false")
        .args(args)
        .current_dir(dir);
    let out = cmd.output().expect("run git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).expect("git printed UTF-8")
}

/// A throwaway git repository, removed when dropped.
pub struct TestRepo {
    root: PathBuf,
}

impl TestRepo {
    pub fn new(label: &str) -> TestRepo {
        let root = unique_dir(label);
        git(&root, &["init", "-q", "-b", "main"]);
        // `isolated()` only reaches the commands these helpers spawn; the in-process
        // `Repo::open` of Tasks 5-12 runs git with the test process's environment.
        // Pin the excludes file in the repository's own config, which outranks the
        // developer's `~/.gitconfig` and `~/.config/git/ignore`, so a global ignore
        // rule can never hide a fixture file from
        // `git ls-files --cached --others --exclude-standard`.
        git(&root, &["config", "core.excludesFile", "/dev/null"]);
        TestRepo { root }
    }

    /// Take ownership of an existing directory (it need not be a repository).
    pub fn adopt(root: PathBuf) -> TestRepo {
        TestRepo { root }
    }

    pub fn with_files(label: &str, files: &[(&str, &str)]) -> TestRepo {
        let repo = TestRepo::new(label);
        for (rel, content) in files {
            repo.write(rel, content);
        }
        repo.commit("Initial");
        repo
    }

    pub fn write(&self, rel: &str, content: &str) {
        let path = self.root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("create the parent directories");
        }
        std::fs::write(&path, content).expect("write a test file");
    }

    pub fn commit(&self, subject: &str) {
        git(&self.root, &["add", "-A"]);
        git(&self.root, &["commit", "-q", "--allow-empty", "-m", subject]);
    }

    pub fn path(&self) -> &Path {
        &self.root
    }

    pub fn path_str(&self) -> &str {
        self.root.to_str().expect("a UTF-8 test path")
    }
}

impl Drop for TestRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
```

Create `tools/tests/shim.rs`:

```rust
//! `.claude/hooks/aios`: freshness, the guard branch, foreground and background
//! builds, `AIOS_TOOLS_BIN` and linked worktrees, exercised with a fake `just`.

mod common;

use common::{isolated, unique_dir, TestRepo};

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::thread::sleep;
use std::time::{Duration, Instant};

/// The PreToolUse decision the shim prints when the binary is missing.
const ASK_JSON: &str = r#"{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"aios tools not built; run just tools"}}"#;

/// A `just` that only knows `tools`: it logs the build and writes a binary that
/// echoes its arguments. `FAKE_JUST_FAIL` makes it fail, `FAKE_JUST_DELAY`
/// slows it down, `FAKE_EXIT` sets the exit status of the binary it writes.
const FAKE_JUST: &str = r#"#!/bin/sh
set -u
if [ "${1:-}" != "tools" ]; then
    echo "fake just: unexpected recipe ${*}" >&2
    exit 2
fi
echo "fake just: building the aios binary"
printf 'build\n' >> just.log
if [ -n "${FAKE_JUST_FAIL:-}" ]; then
    echo "fake just: the build failed" >&2
    exit 1
fi
sleep "${FAKE_JUST_DELAY:-0}"
mkdir -p target/tools/release
# Install atomically: the stale-guard branch execs this file while this build
# runs, and truncating a script in place can leave /bin/sh reading a half-written
# file. mv within one directory is rename(2), so the running shim keeps the old
# inode.
cat > target/tools/release/aios.new <<'BIN'
#!/bin/sh
printf 'fake:%s\n' "$*"
exit ${FAKE_EXIT:-0}
BIN
chmod 755 target/tools/release/aios.new
mv -f target/tools/release/aios.new target/tools/release/aios
"#;

/// The same binary the fake `just` writes, installed directly by a test.
const FAKE_BIN: &str = r#"#!/bin/sh
printf 'fake:%s\n' "$*"
exit ${FAKE_EXIT:-0}
"#;

fn repo_shim() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the tools crate has a parent directory")
        .join(".claude/hooks/aios")
}

fn shim_source() -> String {
    let path = repo_shim();
    std::fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("read {}: {err}", path.display()))
}

fn make_executable(path: &Path) {
    let mut perms = std::fs::metadata(path)
        .unwrap_or_else(|err| panic!("stat {}: {err}", path.display()))
        .permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(path, perms).expect("chmod 755");
}

fn wait_for(label: &str, mut ready: impl FnMut() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if ready() {
            return;
        }
        sleep(Duration::from_millis(50));
    }
    panic!("timed out after 10 s waiting for {label}");
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn code(out: &Output) -> i32 {
    out.status.code().expect("the shim exited normally")
}

/// A main checkout holding the shim, with a fake `just` first on `PATH`.
struct Sandbox {
    repo: TestRepo,
    bin_dir: TestRepo,
}

impl Sandbox {
    fn new(label: &str) -> Sandbox {
        let repo = TestRepo::new(label);
        repo.write(".claude/hooks/aios", &shim_source());
        repo.write("tools/src/lib.rs", "// the shim only needs tools/ to exist\n");
        repo.write("Cargo.lock", "# the shim only needs Cargo.lock to exist\n");
        make_executable(&repo.path().join(".claude/hooks/aios"));
        repo.commit("Initial");

        let bin_dir = TestRepo::adopt(unique_dir(&format!("{label}-path")));
        let just = bin_dir.path().join("just");
        std::fs::write(&just, FAKE_JUST).expect("write the fake just");
        make_executable(&just);

        Sandbox { repo, bin_dir }
    }

    fn shim(&self) -> PathBuf {
        self.repo.path().join(".claude/hooks/aios")
    }

    fn bin(&self) -> PathBuf {
        self.repo.path().join("target/tools/release/aios")
    }

    fn lock(&self) -> PathBuf {
        self.repo.path().join("target/tools/.building")
    }

    fn just_log(&self) -> PathBuf {
        self.repo.path().join("just.log")
    }

    fn install_bin(&self, fresh: bool) {
        let bin = self.bin();
        std::fs::create_dir_all(bin.parent().expect("the binary has a parent"))
            .expect("create target/tools/release");
        std::fs::write(&bin, FAKE_BIN).expect("write the fake binary");
        make_executable(&bin);
        let stamp = if fresh { "209901010000" } else { "200001010000" };
        let status = Command::new("touch")
            .arg("-t")
            .arg(stamp)
            .arg(&bin)
            .status()
            .expect("run touch");
        assert!(status.success(), "touch -t {stamp} failed");
    }

    fn run_at(&self, shim: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
        let mut cmd = Command::new(shim);
        isolated(&mut cmd);
        let path = format!(
            "{}:{}",
            self.bin_dir.path().display(),
            std::env::var("PATH").unwrap_or_default()
        );
        cmd.env("PATH", path).current_dir(self.repo.path()).args(args);
        for (key, value) in envs {
            cmd.env(key, value);
        }
        cmd.output().expect("run the shim")
    }

    fn run(&self, args: &[&str]) -> Output {
        self.run_at(&self.shim(), args, &[])
    }

    fn run_env(&self, args: &[&str], envs: &[(&str, &str)]) -> Output {
        self.run_at(&self.shim(), args, envs)
    }
}

#[test]
fn the_shim_is_posix_sh() {
    let out = Command::new("sh")
        .arg("-n")
        .arg(repo_shim())
        .output()
        .expect("run sh -n");
    assert!(
        out.status.success(),
        "sh -n rejected the shim: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[test]
fn guard_without_a_binary_asks_and_does_not_build() {
    let sandbox = Sandbox::new("shim-guard-missing");
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), format!("{ASK_JSON}\n"));
    assert!(!sandbox.just_log().exists(), "the guard branch must not build");
    assert!(!sandbox.bin().exists());
}

#[test]
fn a_missing_binary_is_built_then_run() {
    let sandbox = Sandbox::new("shim-missing");
    let out = sandbox.run(&["docs-check", "--json"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check --json\n");
    assert!(
        stderr(&out).contains("fake just: building the aios binary"),
        "build output belongs on stderr: {}",
        stderr(&out)
    );
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
}

#[test]
fn a_missing_binary_with_a_failing_build_exits_3() {
    let sandbox = Sandbox::new("shim-build-fails");
    let out = sandbox.run_env(&["docs-check"], &[("FAKE_JUST_FAIL", "1")]);
    assert_eq!(code(&out), 3);
    assert!(stderr(&out).contains("run: just tools"), "{}", stderr(&out));
    assert!(stdout(&out).is_empty());
}

#[test]
fn a_fresh_binary_runs_without_building_and_passes_its_status_on() {
    let sandbox = Sandbox::new("shim-fresh");
    sandbox.install_bin(true);

    let out = sandbox.run(&["docs-check", "--all"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check --all\n");
    assert!(!sandbox.just_log().exists(), "a fresh binary must not build");

    let out = sandbox.run_env(&["docs-check"], &[("FAKE_EXIT", "7")]);
    assert_eq!(code(&out), 7);
    assert!(!sandbox.just_log().exists());
}

#[test]
fn a_stale_binary_is_rebuilt_in_the_foreground() {
    let sandbox = Sandbox::new("shim-stale");
    sandbox.install_bin(false);
    let out = sandbox.run(&["docs-check"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
    assert!(!sandbox.lock().exists(), "a foreground build takes no lock");
}

#[test]
fn a_stale_guard_runs_at_once_and_rebuilds_in_the_background() {
    let sandbox = Sandbox::new("shim-stale-guard");
    sandbox.install_bin(false);
    let out = sandbox.run(&["guard", "PreToolUse"]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:guard PreToolUse\n");
    wait_for("the background build to log a line", || {
        sandbox.just_log().exists()
    });
    wait_for("the build lock to be released", || !sandbox.lock().exists());
    assert_eq!(
        std::fs::read_to_string(sandbox.just_log()).expect("read just.log"),
        "build\n"
    );
}

#[test]
fn prebuild_returns_at_once_and_builds_in_the_background() {
    let sandbox = Sandbox::new("shim-prebuild");
    let started = Instant::now();
    let out = sandbox.run_env(&["--prebuild"], &[("FAKE_JUST_DELAY", "1")]);
    assert_eq!(code(&out), 0);
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "--prebuild must not wait for the build"
    );
    wait_for("the background build to produce the binary", || {
        sandbox.bin().exists()
    });
    wait_for("the build lock to be released", || !sandbox.lock().exists());
}

#[test]
fn an_override_replaces_the_main_checkouts_binary() {
    let sandbox = Sandbox::new("shim-override");
    let other = sandbox.bin_dir.path().join("other-aios");
    std::fs::write(&other, "#!/bin/sh\nprintf 'override:%s\\n' \"$*\"\n")
        .expect("write the override binary");
    make_executable(&other);
    let out = sandbox.run_env(
        &["docs-check"],
        &[("AIOS_TOOLS_BIN", other.to_str().expect("a UTF-8 path"))],
    );
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "override:docs-check\n");
    assert!(!sandbox.just_log().exists(), "an override must not build");
}

#[test]
fn a_missing_override_fails_closed() {
    let sandbox = Sandbox::new("shim-override-missing");
    let missing = sandbox.bin_dir.path().join("not-built");
    let missing = missing.to_str().expect("a UTF-8 path");

    let out = sandbox.run_env(&["docs-check"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_eq!(code(&out), 3);
    assert!(
        stderr(&out).contains("is not an executable file"),
        "{}",
        stderr(&out)
    );

    let out = sandbox.run_env(&["guard", "PreToolUse"], &[("AIOS_TOOLS_BIN", missing)]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), format!("{ASK_JSON}\n"));
}

#[test]
fn a_linked_worktree_runs_the_main_checkouts_binary() {
    let sandbox = Sandbox::new("shim-worktree");
    sandbox.install_bin(true);

    let worktree = TestRepo::adopt(unique_dir("shim-worktree-linked"));
    common::git(
        sandbox.repo.path(),
        &["worktree", "add", "-q", "--detach", worktree.path_str()],
    );

    // A different binary inside the worktree must be ignored.
    let other = worktree.path().join("target/tools/release/aios");
    std::fs::create_dir_all(other.parent().expect("the binary has a parent"))
        .expect("create the worktree target directory");
    std::fs::write(&other, "#!/bin/sh\nprintf 'worktree:%s\\n' \"$*\"\n")
        .expect("write the worktree binary");
    make_executable(&other);

    let out = sandbox.run_at(&worktree.path().join(".claude/hooks/aios"), &["docs-check"], &[]);
    assert_eq!(code(&out), 0);
    assert_eq!(stdout(&out), "fake:docs-check\n");
    assert!(!sandbox.just_log().exists(), "the main binary is fresh");
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --test shim`
Expected: FAIL. The crate compiles, then every test panics in `shim_source` / `repo_shim` with `read /Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1/.claude/hooks/aios: No such file or directory` (and `the_shim_is_posix_sh` fails because `sh -n` cannot open the file): 11 failed, 0 passed.

- [ ] **Step 3: Write the implementation**

Create `.claude/hooks/aios` with exactly this content (contract `§6.1`):

```sh
#!/bin/sh
# .claude/hooks/aios - run the aios host tools binary built from tools/ (`just tools`).
#
# The binary comes from the MAIN checkout (the parent of git's common dir), so
# hooks, skills and recipes run merged, reviewed code even when called from a PR
# worktree. AIOS_TOOLS_BIN overrides it, to test a PR's own build explicitly.
#
# Fresh means newer than every file under the main checkout's tools/ and than
# its Cargo.lock.
#   aios guard ...    fresh: run. stale: run it and start one background
#                     `just tools` (lock dir target/tools/.building). missing:
#                     fail closed with a PreToolUse "ask" decision, exit 0.
#   aios <other> ...  fresh: run. stale: rebuild in the foreground, then run.
#                     missing: build, then run; exit 3 naming `just tools` if
#                     the build fails.
#   aios --prebuild   (SessionStart) start a background build when the binary
#                     is missing or stale; always exits 0.
# POSIX sh (macOS /bin/sh, dash). Build output goes to stderr or the build log,
# never to stdout, so JSON on stdout stays parseable.

set -u

here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P) || exit 3
if common=$(git -C "$here" rev-parse --path-format=absolute --git-common-dir 2>/dev/null); then
    main=$(dirname -- "$common")
else
    main=$(CDPATH='' cd -- "$here/../.." && pwd -P) || exit 3
fi
bin="$main/target/tools/release/aios"
lock="$main/target/tools/.building"
log="$main/target/tools/build.log"
ask='{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"ask","permissionDecisionReason":"aios tools not built; run just tools"}}'

is_stale() {
    [ -n "$(find "$main/tools" "$main/Cargo.lock" -newer "$bin" -print 2>/dev/null | head -n 1)" ]
}

build_fg() {
    (cd "$main" && just tools) >&2
}

build_bg() {
    mkdir -p "$main/target/tools" 2>/dev/null || return 0
    # A lock older than 30 minutes belongs to a build that died: take it over.
    if [ -n "$(find "$lock" -prune -mmin +30 2>/dev/null)" ]; then
        rmdir "$lock" 2>/dev/null
    fi
    mkdir "$lock" 2>/dev/null || return 0
    (
        cd "$main" && just tools >"$log" 2>&1
        rmdir "$lock" 2>/dev/null
    ) </dev/null >/dev/null 2>&1 &
}

if [ -n "${AIOS_TOOLS_BIN:-}" ]; then
    [ "${1:-}" = "--prebuild" ] && exit 0
    [ -x "$AIOS_TOOLS_BIN" ] && exec "$AIOS_TOOLS_BIN" "$@"
    if [ "${1:-}" = "guard" ]; then
        printf '%s\n' "$ask"
        exit 0
    fi
    echo "aios: AIOS_TOOLS_BIN=$AIOS_TOOLS_BIN is not an executable file" >&2
    exit 3
fi

case "${1:-}" in
--prebuild)
    if [ ! -x "$bin" ] || is_stale; then
        build_bg
    fi
    exit 0
    ;;
guard)
    if [ ! -x "$bin" ]; then
        printf '%s\n' "$ask"
        exit 0
    fi
    if is_stale; then
        build_bg
    fi
    exec "$bin" "$@"
    ;;
esac

if [ ! -x "$bin" ]; then
    if ! build_fg || [ ! -x "$bin" ]; then
        echo "aios: $bin is missing and the build failed; run: just tools" >&2
        exit 3
    fi
elif is_stale; then
    build_fg || echo "aios: rebuilding $bin failed; running the stale binary (run: just tools)" >&2
fi
exec "$bin" "$@"
```

Then `chmod 755 .claude/hooks/aios`.

Edit `.claude/hooks/setup-dev-env.sh`, replacing

```bash
set -euo pipefail

# Only run in remote/web environments
```

with

```bash
set -euo pipefail

# aios host tools (tools/, `just tools`): start a background build in the main
# checkout when target/tools/release/aios is missing or stale. Runs in local and
# remote sessions and returns at once (build log: target/tools/build.log).
"$(dirname "$0")/aios" --prebuild || true

# Only run in remote/web environments
```

Edit `.github/workflows/ci.yml`, replacing

```yaml
      - uses: extractions/setup-just@v4
      - run: just test

  security:
```

with

```yaml
      - uses: extractions/setup-just@v4
      - run: just test

  tools:
    name: Tools (host)
    # Host-only crate tools/ (package aios-tools, binary aios): fmt, clippy and
    # tests, including the docs-check parity goldens. The real-repository goldens
    # replay a pinned main commit, so the checkout needs full history.
    runs-on: ubuntu-24.04
    steps:
      - uses: actions/checkout@v7
        with:
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: nightly
          components: clippy, rustfmt
      - uses: Swatinem/rust-cache@v2
      - run: cargo fmt --check -p aios-tools
      - run: cargo clippy -p aios-tools -- -D warnings
      - run: cargo test -p aios-tools

  security:
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test shim`
Expected: PASS. The 11 tests (`the_shim_is_posix_sh`, `guard_without_a_binary_asks_and_does_not_build`, `a_missing_binary_is_built_then_run`, `a_missing_binary_with_a_failing_build_exits_3`, `a_fresh_binary_runs_without_building_and_passes_its_status_on`, `a_stale_binary_is_rebuilt_in_the_foreground`, `a_stale_guard_runs_at_once_and_rebuilds_in_the_background`, `prebuild_returns_at_once_and_builds_in_the_background`, `an_override_replaces_the_main_checkouts_binary`, `a_missing_override_fails_closed`, `a_linked_worktree_runs_the_main_checkouts_binary`) all pass: `test result: ok. 11 passed; 0 failed`.

- [ ] **Step 5: Check the shell, the YAML and the file mode**

Run:

```bash
sh -n .claude/hooks/aios
bash -n .claude/hooks/setup-dev-env.sh
grep -n 'aios --prebuild' .claude/hooks/setup-dev-env.sh
awk '/^  tools:/,/^  security:/' .github/workflows/ci.yml
chmod 755 .claude/hooks/aios
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
```

Expected: both syntax checks print nothing and exit 0; the grep prints the one prebuild line; the awk prints the new `Tools (host)` job followed by `  security:`, with two-space job indentation and no tab; `cargo fmt --check` prints nothing.

- [ ] **Step 6: Commit and push**

```bash
git add .claude/hooks/aios .claude/hooks/setup-dev-env.sh .github/workflows/ci.yml tools/tests/common/mod.rs tools/tests/shim.rs
git ls-files -s .claude/hooks/aios
git commit -F - <<'MSG'
Add the aios shim, the session prebuild and the Tools CI job

.claude/hooks/aios runs target/tools/release/aios from the MAIN checkout
(the parent of git's common dir), so hooks, skills and recipes run merged
code even from a PR worktree, and AIOS_TOOLS_BIN overrides it. A stale
binary is rebuilt in the foreground, except for `aios guard`, which runs
at once and rebuilds in the background; a missing binary makes guard fail
closed with a PreToolUse "ask" decision. setup-dev-env.sh starts that
build at session start.

The new CI job "Tools (host)" runs fmt, clippy and the tests for the host
crate with full history, which the docs-check parity goldens need.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

Expected: `git ls-files -s` prints a line starting `100755` for `.claude/hooks/aios` before the commit is made.

-----

### Task 3: docs-check model: findings, baseline, comparison

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/mod.rs`, `tools/src/cmd/docs_check/mod.rs` (module lines only; Tasks 4-6 add theirs), `tools/src/cmd/docs_check/model.rs`
- Modify: `tools/src/lib.rs` (add `pub mod cmd;`)
- Test: unit tests inside `tools/src/cmd/docs_check/model.rs` (`cargo test -p aios-tools --lib model`)

**Interfaces:**
- Consumes (Task 1, `aios_tools::pystr`):

  ```rust
  pub fn strip(s: &str) -> &str;
  ```

- Produces (`aios_tools::cmd::docs_check::model`, contract `§4.4`; Tasks 6-15 use these names):

  ```rust
  pub const BASELINE_REL: &str = "scripts/docs/baseline.json";
  pub const BASELINE_COMMENT: &str;                // the baseline's "comment" field
  pub const CHECKS: [(&str, &str); 15];            // (name, description) in CHECK_ORDER
  pub const CHECK_ORDER: [&str; 15];
  pub fn check_rank(check: &str) -> usize;         // index in CHECK_ORDER, else 99
  pub fn description(check: &str) -> &'static str; // "" when unknown

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Finding {
      pub check: &'static str,
      pub file: String,
      pub target: String,
      pub message: String,
      pub line: usize,
      pub also: Vec<usize>,
      pub detail: String,
  }
  impl Finding {
      pub fn new(check: &'static str, file: impl Into<String>, target: impl Into<String>,
                 message: impl Into<String>, line: usize) -> Finding;
      pub fn with_detail(self, detail: impl Into<String>) -> Finding;
      pub fn key(&self) -> String;
      pub fn count(&self) -> usize;
      pub fn location(&self) -> String;
      pub fn text(&self) -> String;
  }

  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Skip(pub String);
  impl std::fmt::Display for Skip {}
  impl std::error::Error for Skip {}

  pub fn collate(findings: Vec<Finding>) -> Vec<Finding>;

  pub type Baseline = serde_json::Map<String, serde_json::Value>;
  pub fn load_baseline(path: &std::path::Path, display: &str) -> anyhow::Result<Baseline>;
  pub fn baseline_count(entry: &serde_json::Value) -> anyhow::Result<i64>;
  pub fn entry_check(entry: &serde_json::Value) -> Option<&str>;
  pub fn py_truthy(v: &serde_json::Value) -> bool;
  pub fn py_str(v: &serde_json::Value) -> String;
  pub fn baseline_entry(f: &Finding, old: Option<&serde_json::Value>) -> serde_json::Value;
  pub fn updated_baseline(findings: &[Finding], baseline: &Baseline,
                          ran: &std::collections::BTreeSet<&str>) -> Baseline;
  pub fn render_baseline(entries: &Baseline) -> anyhow::Result<String>;
  pub fn write_baseline(path: &std::path::Path, entries: &Baseline) -> anyhow::Result<()>;

  #[derive(Debug, Clone, Default, PartialEq)]
  pub struct Comparison {
      pub new_keys: std::collections::BTreeSet<String>,
      pub grown: std::collections::BTreeMap<String, i64>,
      pub accepted: std::collections::BTreeMap<String, serde_json::Value>,
      pub resolved: Vec<String>,
      pub reduced: std::collections::BTreeMap<String, (i64, i64)>,
  }
  pub fn compare(findings: &[Finding], baseline: &Baseline,
                 ran: &std::collections::BTreeSet<&str>) -> anyhow::Result<Comparison>;
  ```

**Notes (check.py at `33c6b3d`; re-read each range while porting):**
- Port map: `BASELINE_REL` L47; `CHECK_DESCRIPTIONS` L96-112 (copy the 15 description strings from L97-111 character for character, backticks included) and `CHECK_ORDER = list(CHECK_DESCRIPTIONS)` L113; `Finding` L120-149 (`key` L131-133, `count` L135-138, `location` L140-146, `text` L148-149); `Skip` L152-153; `load_baseline` L1402-1412; `baseline_entry` L1415-1422; `write_baseline` L1424-1439; the merge and sort of `run_checks` L1442-1456; `Comparison` L1459-1466 and `compare` L1468-1485; the `--update-baseline` flow L1618-1625.
- Merging (L1449-1452): the first finding for a key is kept; a later finding with the same key appends its `line` to `also` only when `line != 0`, `line != first.line` and `line` is not already in `also`. The sort key is `(CHECK_ORDER index, file, line, target)` (L1454): Rust's `String` ordering is byte order, which for UTF-8 is code-point order, so it matches Python's.
- `write_baseline` sorts by `(CHECK_ORDER.index(check) if known else 99, key)`, counts entries per check in `CHECK_ORDER` order (only checks with at least one entry), and dumps with `indent=2`, `ensure_ascii=False` plus one trailing `"\n"`; `serde_json::to_string_pretty` produces the same bytes, which the round-trip test against the repository's real `scripts/docs/baseline.json` proves.
- `serde_json`'s `preserve_order` feature is what makes `Baseline` behave like a Python dict: inserting an existing key keeps its position, exactly as `entries[f.key] = ...` does at L1621.
- `compare` marks a key new when it is missing from the baseline, or when its count exceeds the baselined `count` (then `grown` records the baselined count, and the key can be both new and accepted); `resolved` lists baselined keys whose `check` ran and that no longer occur, sorted.
- Accepted divergences, stated in the module doc (contract `§1.9`): the `{exc}` text of `cannot read baseline {path}: {exc}` is Rust's `std::io::Error` / `serde_json::Error` message rather than CPython's (the prefix, the path and exit code 2 match; no golden covers it); a `count` beyond `i64` is clamped to `i64::MAX`, where CPython compares the exact integer; a non-string `reason` is rendered as compact JSON rather than a Python `repr`. Other malformed baselines (a top level that is not an object, a `findings` value that is not a list, an entry without a string `key`) are errors here and tracebacks in check.py: both exit 2.

- [ ] **Step 1: Write the failing tests**

Create `tools/src/cmd/mod.rs`:

```rust
//! The `aios` subcommands.

pub mod docs_check;
```

Create `tools/src/cmd/docs_check/mod.rs`:

```rust
//! `aios docs-check`: a byte-for-byte port of `scripts/docs/check.py`.

pub mod model;
```

Add `pub mod cmd;` to `tools/src/lib.rs`, so its module list reads:

```rust
pub mod cmd;
pub mod paths;
pub mod proc;
pub mod pystr;
```

Create `tools/src/cmd/docs_check/model.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;

    static COUNTER: AtomicUsize = AtomicUsize::new(0);

    /// A private directory under the system temp dir, removed when dropped.
    struct TempTree {
        root: PathBuf,
    }

    impl TempTree {
        fn new(label: &str) -> TempTree {
            let n = COUNTER.fetch_add(1, Ordering::Relaxed);
            let root = std::env::temp_dir()
                .join(format!("aios-tools-{label}-{}-{n}", std::process::id()));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).expect("create the temp tree");
            TempTree { root }
        }

        fn join(&self, name: &str) -> PathBuf {
            self.root.join(name)
        }

        fn write(&self, name: &str, content: &str) -> PathBuf {
            let path = self.join(name);
            std::fs::write(&path, content).expect("write a temp file");
            path
        }
    }

    impl Drop for TempTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn repo_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("the tools crate has a parent directory")
            .to_path_buf()
    }

    fn finding(check: &'static str, file: &str, target: &str, line: usize) -> Finding {
        Finding::new(check, file, target, format!("{target} drifted"), line)
    }

    /// The bytes check.py's write_baseline produces for the entries built in
    /// `updated_baseline_keeps_unrelated_and_accepted_entries` (recorded).
    const EXPECTED_BASELINE: &str = r#"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 1,
    "anchors": 1
  },
  "findings": [
    {
      "key": "md-links|a.md|t",
      "check": "md-links",
      "file": "a.md",
      "target": "t",
      "message": "new message",
      "count": 2,
      "reason": "accepted on purpose"
    },
    {
      "key": "anchors|a.md|#x",
      "check": "anchors",
      "file": "a.md",
      "target": "#x",
      "message": "gone"
    }
  ]
}
"#;

    #[test]
    fn the_model_check_table_matches_check_py() {
        assert_eq!(CHECKS.len(), 15);
        assert_eq!(CHECK_ORDER.len(), 15);
        for (i, (name, _)) in CHECKS.iter().enumerate() {
            assert_eq!(CHECK_ORDER[i], *name);
        }
        assert_eq!(CHECK_ORDER[0], "md-links");
        assert_eq!(CHECK_ORDER[14], "knowledge-hygiene");
        assert_eq!(
            description("repo-paths"),
            "backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)"
        );
        assert_eq!(
            description("just-recipes"),
            "backticked `just X` recipes exist; public recipes are documented"
        );
        assert_eq!(description("nope"), "");
        assert_eq!(check_rank("md-links"), 0);
        assert_eq!(check_rank("knowledge-hygiene"), 14);
        assert_eq!(check_rank("nope"), 99);
        assert_eq!(BASELINE_REL, "scripts/docs/baseline.json");
    }

    #[test]
    fn model_finding_key_count_location_and_text() {
        let mut f = Finding::new("md-links", "docs/a.md", "./b.md", "broken link -> ./b.md", 12);
        assert_eq!(f.key(), "md-links|docs/a.md|./b.md");
        assert_eq!(f.count(), 1);
        assert_eq!(f.location(), "docs/a.md:12");
        assert_eq!(f.text(), "broken link -> ./b.md");

        f.also = vec![20, 31];
        assert_eq!(f.count(), 3);
        assert_eq!(f.location(), "docs/a.md:12 (also 20, 31)");

        let file_level = Finding::new("layout", "CLAUDE.md", "stale:kernel/src/x/", "m", 0);
        assert_eq!(file_level.location(), "CLAUDE.md");

        let detailed = Finding::new(
            "lock-order",
            "docs/kernel/deadlock-prevention.md",
            "undocumented:A_LOCK",
            "production lock A_LOCK is not in §3.3/§3.4",
            0,
        )
        .with_detail("defined at kernel/src/a.rs:5");
        assert_eq!(
            detailed.text(),
            "production lock A_LOCK is not in §3.3/§3.4 (defined at kernel/src/a.rs:5)"
        );
    }

    #[test]
    fn model_skip_prints_only_its_message() {
        let skip = Skip("no justfile".to_string());
        assert_eq!(skip.to_string(), "no justfile");
        let boxed: Box<dyn std::error::Error> = Box::new(skip);
        assert_eq!(boxed.to_string(), "no justfile");
    }

    #[test]
    fn model_collate_merges_keys_and_sorts_like_run_checks() {
        let findings = vec![
            finding("anchors", "b.md", "t2", 9),
            finding("md-links", "b.md", "t1", 5),
            finding("md-links", "b.md", "t1", 5), // same line: ignored
            finding("md-links", "b.md", "t1", 0), // line 0: ignored
            finding("md-links", "b.md", "t1", 7), // appended to `also`
            finding("md-links", "b.md", "t1", 7), // already in `also`
            finding("md-links", "a.md", "t0", 3),
            finding("md-links", "b.md", "t0", 5),
        ];
        let out = collate(findings);
        let keys: Vec<String> = out.iter().map(Finding::key).collect();
        assert_eq!(
            keys,
            vec![
                "md-links|a.md|t0".to_string(),
                "md-links|b.md|t0".to_string(),
                "md-links|b.md|t1".to_string(),
                "anchors|b.md|t2".to_string(),
            ]
        );
        assert_eq!(out[2].also, vec![7]);
        assert_eq!(out[2].count(), 2);
        assert_eq!(out[2].location(), "b.md:5 (also 7)");
    }

    #[test]
    fn model_load_baseline_reads_entries_and_reports_failures() {
        let tree = TempTree::new("model-baseline");

        assert!(load_baseline(&tree.join("nope.json"), "nope.json")
            .expect("a missing baseline is empty")
            .is_empty());

        let good = tree.write(
            "good.json",
            r#"{"findings": [{"key": "a|b|c", "check": "md-links", "count": 2},
                            {"key": "d|e|f", "check": "anchors"}]}"#,
        );
        let entries = load_baseline(&good, "good.json").expect("a valid baseline loads");
        assert_eq!(
            entries.keys().map(String::as_str).collect::<Vec<_>>(),
            vec!["a|b|c", "d|e|f"],
            "entries keep file order"
        );
        assert_eq!(baseline_count(&entries["a|b|c"]).expect("count"), 2);
        assert_eq!(baseline_count(&entries["d|e|f"]).expect("default count"), 1);
        assert_eq!(entry_check(&entries["d|e|f"]), Some("anchors"));

        let broken = tree.write("broken.json", "{");
        let err = load_baseline(&broken, "broken.json").expect_err("invalid JSON");
        assert!(
            format!("{err:#}").starts_with("cannot read baseline broken.json: "),
            "{err:#}"
        );

        let err = load_baseline(&tree.root, "a-directory").expect_err("a directory");
        assert!(
            format!("{err:#}").starts_with("cannot read baseline a-directory: "),
            "{err:#}"
        );

        let list = tree.write("list.json", "[]");
        assert!(load_baseline(&list, "list.json").is_err());

        let keyless = tree.write("keyless.json", r#"{"findings": [{"check": "md-links"}]}"#);
        assert!(load_baseline(&keyless, "keyless.json").is_err());

        let empty = tree.write("empty.json", r#"{"version": 1}"#);
        assert!(load_baseline(&empty, "empty.json")
            .expect("no findings key")
            .is_empty());
    }

    #[test]
    fn model_baseline_count_follows_python_int() {
        let with_count = |value: Value| json!({ "count": value });
        assert_eq!(baseline_count(&json!({})).expect("default"), 1);
        assert_eq!(baseline_count(&with_count(json!(3))).expect("int"), 3);
        assert_eq!(baseline_count(&with_count(json!(2.9))).expect("float"), 2);
        assert_eq!(baseline_count(&with_count(json!(true))).expect("bool"), 1);
        assert_eq!(baseline_count(&with_count(json!(" 1_0 "))).expect("string"), 10);
        assert!(baseline_count(&with_count(json!("x"))).is_err());
        assert!(baseline_count(&with_count(json!(null))).is_err());
        assert!(baseline_count(&with_count(json!([]))).is_err());
    }

    #[test]
    fn model_python_truthiness_and_text() {
        assert!(!py_truthy(&json!(null)));
        assert!(!py_truthy(&json!("")));
        assert!(!py_truthy(&json!(0)));
        assert!(!py_truthy(&json!(false)));
        assert!(!py_truthy(&json!([])));
        assert!(!py_truthy(&json!({})));
        assert!(py_truthy(&json!("x")));
        assert!(py_truthy(&json!(1)));
        assert_eq!(py_str(&json!("plain text")), "plain text");
        assert_eq!(py_str(&json!({"a": 1})), "{\"a\":1}");
    }

    #[test]
    fn model_updated_baseline_keeps_unrelated_and_accepted_entries() {
        let mut baseline = Baseline::new();
        baseline.insert(
            "md-links|a.md|t".to_string(),
            json!({"key": "md-links|a.md|t", "check": "md-links", "file": "a.md",
                   "target": "t", "message": "old message", "count": 3,
                   "reason": "accepted on purpose"}),
        );
        baseline.insert(
            "anchors|a.md|#x".to_string(),
            json!({"key": "anchors|a.md|#x", "check": "anchors", "file": "a.md",
                   "target": "#x", "message": "gone"}),
        );

        let mut f = Finding::new("md-links", "a.md", "t", "new message", 4);
        f.also = vec![9];
        let ran: BTreeSet<&str> = ["md-links"].into_iter().collect();
        let updated = updated_baseline(&[f], &baseline, &ran);

        assert_eq!(updated.len(), 2);
        assert_eq!(updated["anchors|a.md|#x"]["message"], json!("gone"));
        let entry = &updated["md-links|a.md|t"];
        assert_eq!(entry["message"], json!("new message"));
        assert_eq!(entry["count"], json!(2));
        assert_eq!(entry["reason"], json!("accepted on purpose"));

        assert_eq!(render_baseline(&updated).expect("render"), EXPECTED_BASELINE);
    }

    #[test]
    fn model_render_baseline_reproduces_the_repository_baseline() {
        let path = repo_root().join("scripts/docs/baseline.json");
        let raw = std::fs::read_to_string(&path).expect("read scripts/docs/baseline.json");
        let entries =
            load_baseline(&path, "scripts/docs/baseline.json").expect("load the baseline");
        assert!(!entries.is_empty());
        assert_eq!(render_baseline(&entries).expect("render"), raw);
    }

    #[test]
    fn model_write_baseline_writes_the_rendered_bytes() {
        let tree = TempTree::new("model-write");
        let path = tree.join("out.json");
        let mut entries = Baseline::new();
        entries.insert(
            "md-links|a.md|t".to_string(),
            json!({"key": "md-links|a.md|t", "check": "md-links", "file": "a.md",
                   "target": "t", "message": "m"}),
        );
        write_baseline(&path, &entries).expect("write the baseline");
        assert_eq!(
            std::fs::read_to_string(&path).expect("read it back"),
            render_baseline(&entries).expect("render")
        );
    }

    #[test]
    fn model_compare_marks_new_grown_reduced_accepted_and_resolved() {
        let mut baseline = Baseline::new();
        for (key, entry) in [
            ("md-links|a.md|same", json!({"check": "md-links"})),
            (
                "md-links|a.md|grown",
                json!({"check": "md-links", "count": 2, "reason": "known"}),
            ),
            ("md-links|a.md|shrunk", json!({"check": "md-links", "count": 4})),
            ("md-links|a.md|gone", json!({"check": "md-links"})),
            ("anchors|a.md|skipped", json!({"check": "anchors"})),
        ] {
            baseline.insert(key.to_string(), entry);
        }

        let same = Finding::new("md-links", "a.md", "same", "m", 1);
        let mut grown = Finding::new("md-links", "a.md", "grown", "m", 1);
        grown.also = vec![2, 3];
        let mut shrunk = Finding::new("md-links", "a.md", "shrunk", "m", 1);
        shrunk.also = vec![2];
        let fresh = Finding::new("md-links", "a.md", "fresh", "m", 1);

        let ran: BTreeSet<&str> = ["md-links"].into_iter().collect();
        let cmp = compare(&[same, grown, shrunk, fresh], &baseline, &ran).expect("compare");

        let expected_new: BTreeSet<String> = [
            "md-links|a.md|fresh".to_string(),
            "md-links|a.md|grown".to_string(),
        ]
        .into_iter()
        .collect();
        assert_eq!(cmp.new_keys, expected_new);
        assert_eq!(cmp.grown.get("md-links|a.md|grown"), Some(&2));
        assert_eq!(cmp.reduced.get("md-links|a.md|shrunk"), Some(&(4, 2)));
        assert_eq!(cmp.accepted.get("md-links|a.md|grown"), Some(&json!("known")));
        assert_eq!(cmp.resolved, vec!["md-links|a.md|gone".to_string()]);
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --lib model`
Expected: FAIL to compile with `error[E0412]`/`error[E0425]`/`error[E0433]` ("cannot find type `Finding` in this scope", "cannot find value `CHECKS` in this scope", "failed to resolve: use of undeclared crate or module `serde_json`", and so on): the tests exist, the model does not.

- [ ] **Step 3: Write the implementation**

Insert above the `#[cfg(test)]` module in `tools/src/cmd/docs_check/model.rs`:

```rust
//! The check table, findings, the baseline file and the current-vs-baseline
//! comparison: check.py L47, L96-153 and L1402-1485.
//!
//! `Baseline` is a `serde_json::Map` built with the `preserve_order` feature, so
//! it behaves like the Python dict check.py keeps baseline entries in: raw
//! entries keep every field and its order, and re-inserting a key keeps its
//! position (L1621).
//!
//! Accepted divergences (contract §1.9; no golden covers them): the `{exc}` text
//! in `cannot read baseline {path}: {exc}` is Rust's `std::io::Error` or
//! `serde_json::Error` message rather than CPython's, with the same prefix, path
//! and exit code; a baselined `count` beyond `i64` is clamped to `i64::MAX`,
//! where CPython compares the exact integer; and a non-string `reason` is
//! rendered as compact JSON rather than a Python `repr`. Malformed baselines
//! that make check.py raise (a top level that is not an object, a `findings`
//! value that is not a list, an entry without a string `key` or `check`) are
//! errors here: both exit 2.

use std::collections::hash_map::Entry;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::Path;

use anyhow::{anyhow, bail, Context, Result};
use serde::Serialize;
use serde_json::{Map, Value};

use crate::pystr;

/// The baseline path, relative to the repository root (check.py L47).
pub const BASELINE_REL: &str = "scripts/docs/baseline.json";

/// The baseline's `comment` field (check.py L1435-1437).
pub const BASELINE_COMMENT: &str = "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline";

/// `(name, description)` for every check, in CHECK_ORDER (check.py L96-112).
pub const CHECKS: [(&str, &str); 15] = [
    (
        "md-links",
        "relative [text](path) links resolve to a tracked file or directory",
    ),
    (
        "section-refs",
        "[x.md](path) §N resolves to a numbered heading (hub subfolders included)",
    ),
    (
        "anchors",
        "#fragment links resolve to a GitHub-style heading slug",
    ),
    (
        "wiki-links",
        "[[Note]] links resolve to a note in the docs/ vault",
    ),
    (
        "doc-map",
        "doc-map.md paths exist and every architecture doc is listed",
    ),
    (
        "repo-paths",
        "backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)",
    ),
    (
        "just-recipes",
        "backticked `just X` recipes exist; public recipes are documented",
    ),
    (
        "test-count",
        "stated host test counts match #[test] in shared/src",
    ),
    (
        "lock-order",
        "production Mutex statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md",
    ),
    (
        "milestone-status",
        "merged 'Phase N MK:' milestones vs phase docs, README, development-plan",
    ),
    (
        "phase-count",
        "phase counts in prose match the development-plan §8 table",
    ),
    (
        "layout",
        "kernel/src and shared/src modules vs CLAUDE.md layout and rule 05",
    ),
    (
        "harness-tables",
        "CLAUDE.md skills/agents tables and layout lists vs .claude/ (plugin skills as plugin:skill)",
    ),
    (
        "pointer-doctor",
        "CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/",
    ),
    (
        "knowledge-hygiene",
        "docs/knowledge naming, frontmatter, and an empty plans/ dir",
    ),
];

/// The check names, in the order check.py runs and reports them (L113).
pub const CHECK_ORDER: [&str; 15] = {
    let mut names = [""; 15];
    let mut i = 0;
    while i < CHECKS.len() {
        names[i] = CHECKS[i].0;
        i += 1;
    }
    names
};

/// `CHECK_ORDER.index(check)`, or 99 for a name check.py does not know
/// (check.py L1425 uses the same fallback when sorting baseline entries).
pub fn check_rank(check: &str) -> usize {
    CHECK_ORDER
        .iter()
        .position(|name| *name == check)
        .unwrap_or(99)
}

/// The `--list-checks` description of a check, or `""` when it is unknown.
pub fn description(check: &str) -> &'static str {
    CHECKS
        .iter()
        .find(|(name, _)| *name == check)
        .map_or("", |(_, text)| *text)
}

/// One drift finding (check.py L120-149). `message` goes into the baseline, so
/// it must not contain line numbers; volatile context goes in `detail`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub check: &'static str,
    pub file: String,
    pub target: String,
    pub message: String,
    pub line: usize,
    pub also: Vec<usize>,
    pub detail: String,
}

impl Finding {
    pub fn new(
        check: &'static str,
        file: impl Into<String>,
        target: impl Into<String>,
        message: impl Into<String>,
        line: usize,
    ) -> Finding {
        Finding {
            check,
            file: file.into(),
            target: target.into(),
            message: message.into(),
            line,
            also: Vec::new(),
            detail: String::new(),
        }
    }

    pub fn with_detail(mut self, detail: impl Into<String>) -> Finding {
        self.detail = detail.into();
        self
    }

    /// `f"{check}|{file}|{target}"`: no line numbers, so a finding that moves
    /// keeps its baseline entry.
    pub fn key(&self) -> String {
        format!("{}|{}|{}", self.check, self.file, self.target)
    }

    /// The number of distinct lines reporting this key (1 for file-level ones).
    pub fn count(&self) -> usize {
        1 + self.also.len()
    }

    pub fn location(&self) -> String {
        if self.line == 0 {
            return self.file.clone();
        }
        if self.also.is_empty() {
            return format!("{}:{}", self.file, self.line);
        }
        let also: Vec<String> = self.also.iter().map(usize::to_string).collect();
        format!("{}:{} (also {})", self.file, self.line, also.join(", "))
    }

    pub fn text(&self) -> String {
        if self.detail.is_empty() {
            self.message.clone()
        } else {
            format!("{} ({})", self.message, self.detail)
        }
    }
}

/// A check that cannot run in this environment (check.py's `Skip` exception).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skip(pub String);

impl std::fmt::Display for Skip {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Skip {}

/// Merge findings with equal keys and sort them, as check.py L1444-1455 does:
/// the first finding for a key is kept, later lines are appended to its `also`,
/// and the result is ordered by (check, file, line, target).
pub fn collate(findings: Vec<Finding>) -> Vec<Finding> {
    let mut order: Vec<Finding> = Vec::with_capacity(findings.len());
    let mut index: HashMap<String, usize> = HashMap::new();
    for f in findings {
        match index.entry(f.key()) {
            Entry::Vacant(slot) => {
                slot.insert(order.len());
                order.push(f);
            }
            Entry::Occupied(slot) => {
                let first = &mut order[*slot.get()];
                if f.line != 0 && f.line != first.line && !first.also.contains(&f.line) {
                    first.also.push(f.line);
                }
            }
        }
    }
    order.sort_by(|a, b| {
        (check_rank(a.check), &a.file, a.line, &a.target).cmp(&(
            check_rank(b.check),
            &b.file,
            b.line,
            &b.target,
        ))
    });
    order
}

/// Baseline entries by key, kept raw and in file order (check.py L1412).
pub type Baseline = Map<String, Value>;

/// `load_baseline` (check.py L1402-1412). `display` is the path as the error
/// message must print it: `--baseline` as given, or the root-joined default.
pub fn load_baseline(path: &Path, display: &str) -> Result<Baseline> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Baseline::new()),
        Err(err) => return Err(anyhow!("cannot read baseline {display}: {err}")),
    };
    let data: Value = match serde_json::from_slice(&bytes) {
        Ok(data) => data,
        Err(err) => return Err(anyhow!("cannot read baseline {display}: {err}")),
    };
    let Value::Object(object) = data else {
        bail!("baseline {display} is not a JSON object");
    };
    let mut entries = Baseline::new();
    match object.get("findings") {
        None => return Ok(entries),
        Some(Value::Array(items)) => {
            for item in items {
                let key = item
                    .get("key")
                    .and_then(Value::as_str)
                    .ok_or_else(|| anyhow!("baseline {display}: a findings entry has no 'key'"))?;
                entries.insert(key.to_string(), item.clone());
            }
        }
        Some(_) => bail!("baseline {display}: 'findings' is not a list"),
    }
    Ok(entries)
}

/// `int(entry.get("count", 1))` (check.py L1477).
pub fn baseline_count(entry: &Value) -> Result<i64> {
    let Some(value) = entry.get("count") else {
        return Ok(1);
    };
    match value {
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                Ok(i)
            } else if n.as_u64().is_some() {
                Ok(i64::MAX)
            } else if let Some(f) = n.as_f64() {
                Ok(f.trunc() as i64)
            } else {
                Err(anyhow!("baseline count {value} is not an integer"))
            }
        }
        Value::Bool(b) => Ok(i64::from(*b)),
        Value::String(s) => python_int(pystr::strip(s))
            .ok_or_else(|| anyhow!("baseline count {value} is not an integer")),
        _ => Err(anyhow!("baseline count {value} is not an integer")),
    }
}

/// `int(s)` for the forms a baseline `count` string can take: an optional sign,
/// ASCII digits, and single underscores between digits.
fn python_int(s: &str) -> Option<i64> {
    let (sign, digits) = match s.strip_prefix('-') {
        Some(rest) => (-1i64, rest),
        None => (1i64, s.strip_prefix('+').unwrap_or(s)),
    };
    if digits.is_empty() {
        return None;
    }
    let bytes = digits.as_bytes();
    let mut clean = String::with_capacity(digits.len());
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'_' {
            let between = i > 0
                && bytes[i - 1].is_ascii_digit()
                && bytes.get(i + 1).is_some_and(u8::is_ascii_digit);
            if !between {
                return None;
            }
            continue;
        }
        if !b.is_ascii_digit() {
            return None;
        }
        clean.push(char::from(*b));
    }
    clean.parse::<i64>().ok().map(|value| sign * value)
}

/// `entry.get("check")` when it is a string (check.py L1484, L1619).
pub fn entry_check(entry: &Value) -> Option<&str> {
    entry.get("check").and_then(Value::as_str)
}

/// Python truthiness, which decides whether a `reason` marks a finding accepted.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// A JSON value as an f-string would print it: a string verbatim, anything else
/// as compact JSON (an accepted divergence from Python's `repr`).
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// The baseline record for a finding, keeping an accepted-false-positive
/// `reason` across rewrites (check.py L1415-1422).
pub fn baseline_entry(f: &Finding, old: Option<&Value>) -> Value {
    let mut entry = Map::new();
    entry.insert("key".to_string(), Value::String(f.key()));
    entry.insert("check".to_string(), Value::String(f.check.to_string()));
    entry.insert("file".to_string(), Value::String(f.file.clone()));
    entry.insert("target".to_string(), Value::String(f.target.clone()));
    entry.insert("message".to_string(), Value::String(f.message.clone()));
    if f.count() > 1 {
        entry.insert("count".to_string(), Value::from(f.count()));
    }
    let reason = old
        .and_then(|entry| entry.get("reason"))
        .filter(|value| py_truthy(value));
    if let Some(reason) = reason {
        entry.insert("reason".to_string(), reason.clone());
    }
    Value::Object(entry)
}

/// The entries `--update-baseline` writes (check.py L1619-1621): baselined
/// entries of checks that did not run are kept raw, the checks that ran are
/// replaced by their current findings.
pub fn updated_baseline(
    findings: &[Finding],
    baseline: &Baseline,
    ran: &BTreeSet<&str>,
) -> Baseline {
    let mut entries = Baseline::new();
    for (key, entry) in baseline {
        if entry_check(entry).is_none_or(|check| !ran.contains(check)) {
            entries.insert(key.clone(), entry.clone());
        }
    }
    for f in findings {
        let key = f.key();
        let old = baseline.get(&key).cloned();
        entries.insert(key, baseline_entry(f, old.as_ref()));
    }
    entries
}

#[derive(Serialize)]
struct BaselineFile<'a> {
    comment: &'a str,
    version: u32,
    counts: Map<String, Value>,
    findings: Vec<&'a Value>,
}

/// The whole baseline file, final newline included (check.py L1424-1439).
pub fn render_baseline(entries: &Baseline) -> Result<String> {
    let mut ordered: Vec<(usize, &str, &str, &Value)> = Vec::with_capacity(entries.len());
    for entry in entries.values() {
        let check = entry_check(entry)
            .ok_or_else(|| anyhow!("baseline entry without a string 'check': {entry}"))?;
        let key = entry
            .get("key")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("baseline entry without a string 'key': {entry}"))?;
        ordered.push((check_rank(check), key, check, entry));
    }
    ordered.sort_by(|a, b| (a.0, a.1).cmp(&(b.0, b.1)));

    let mut counts: HashMap<&str, usize> = HashMap::new();
    for (_, _, check, _) in &ordered {
        *counts.entry(*check).or_insert(0) += 1;
    }
    let mut counts_json = Map::new();
    for name in CHECK_ORDER {
        if let Some(n) = counts.get(name) {
            counts_json.insert(name.to_string(), Value::from(*n));
        }
    }

    let file = BaselineFile {
        comment: BASELINE_COMMENT,
        version: 1,
        counts: counts_json,
        findings: ordered.iter().map(|(_, _, _, entry)| *entry).collect(),
    };
    let mut text = serde_json::to_string_pretty(&file).context("serialize the baseline")?;
    text.push('\n');
    Ok(text)
}

/// Write the baseline file (check.py L1437-1439).
pub fn write_baseline(path: &Path, entries: &Baseline) -> Result<()> {
    let text = render_baseline(entries)?;
    std::fs::write(path, text)
        .with_context(|| format!("cannot write baseline {}", path.display()))
}

/// Current findings against the baseline (check.py L1459-1485).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Comparison {
    /// Keys missing from the baseline, or occurring on more lines than it says.
    pub new_keys: BTreeSet<String>,
    /// Key to baselined count, for keys whose count grew.
    pub grown: BTreeMap<String, i64>,
    /// Key to reason, for baselined false positives.
    pub accepted: BTreeMap<String, Value>,
    /// Baselined keys (of checks that ran) that no longer occur, sorted.
    pub resolved: Vec<String>,
    /// Key to (baselined count, current count), for keys that shrank.
    pub reduced: BTreeMap<String, (i64, i64)>,
}

pub fn compare(
    findings: &[Finding],
    baseline: &Baseline,
    ran: &BTreeSet<&str>,
) -> Result<Comparison> {
    let mut cmp = Comparison::default();
    let mut current: BTreeSet<String> = BTreeSet::new();
    for f in findings {
        let key = f.key();
        current.insert(key.clone());
        let Some(entry) = baseline.get(&key) else {
            cmp.new_keys.insert(key);
            continue;
        };
        let base = baseline_count(entry)?;
        let count = f.count() as i64;
        match count.cmp(&base) {
            std::cmp::Ordering::Greater => {
                cmp.new_keys.insert(key.clone());
                cmp.grown.insert(key.clone(), base);
            }
            std::cmp::Ordering::Less => {
                cmp.reduced.insert(key.clone(), (base, count));
            }
            std::cmp::Ordering::Equal => {}
        }
        if let Some(reason) = entry.get("reason").filter(|value| py_truthy(value)) {
            cmp.accepted.insert(key, reason.clone());
        }
    }
    cmp.resolved = baseline
        .iter()
        .filter(|(key, entry)| {
            entry_check(entry).is_some_and(|check| ran.contains(check)) && !current.contains(*key)
        })
        .map(|(key, _)| key.clone())
        .collect();
    cmp.resolved.sort();
    Ok(cmp)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --lib model`
Expected: PASS. The 11 tests under `cmd::docs_check::model::tests` (`the_model_check_table_matches_check_py`, `model_finding_key_count_location_and_text`, `model_skip_prints_only_its_message`, `model_collate_merges_keys_and_sorts_like_run_checks`, `model_load_baseline_reads_entries_and_reports_failures`, `model_baseline_count_follows_python_int`, `model_python_truthiness_and_text`, `model_updated_baseline_keeps_unrelated_and_accepted_entries`, `model_render_baseline_reproduces_the_repository_baseline`, `model_write_baseline_writes_the_rendered_bytes`, `model_compare_marks_new_grown_reduced_accepted_and_resolved`) pass: `test result: ok. 11 passed; 0 failed`. The round-trip test proves `render_baseline` reproduces the real `scripts/docs/baseline.json` byte for byte.

- [ ] **Step 5: Format, lint and the full library suite**

Run:

```bash
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
cargo test -p aios-tools --lib
```

Expected: `cargo fmt --check` prints nothing; clippy ends with `Finished` and no warning; the library suite passes (Task 1's 17 tests plus these 11).

- [ ] **Step 6: Commit and push**

```bash
git add tools/src/lib.rs tools/src/cmd/mod.rs tools/src/cmd/docs_check/mod.rs tools/src/cmd/docs_check/model.rs
git commit -F - <<'MSG'
Port the docs-check model: findings, baseline, comparison

model.rs ports check.py's check table (L96-113), Finding and Skip
(L120-153), the baseline reader and writer (L1402-1439), the key merge
and sort of run_checks (L1442-1456) and the baseline comparison
(L1459-1485). Baseline entries stay raw in a preserve_order serde_json
map, so unknown fields, their order and an accepted "reason" survive a
rewrite.

A unit test re-renders the repository's scripts/docs/baseline.json and
compares it byte for byte with the file on disk.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 4: Markdown helpers and lookaround replacements R8, R25

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/markdown.rs` (implementation, then its `#[cfg(test)] mod tests`)
- Modify: `tools/src/cmd/docs_check/mod.rs` (add `pub mod markdown;`)
- Test: `tools/src/cmd/docs_check/markdown.rs` (unit tests; filter `markdown`)

**Interfaces:**
- Consumes (Task 1, `aios_tools::pystr`):

  ```rust
  pub fn is_space(c: char) -> bool;
  pub fn strip(s: &str) -> &str;
  pub fn lstrip(s: &str) -> &str;
  pub fn splitlines(s: &str) -> Vec<&str>;
  pub fn indent_width(line: &str) -> usize;
  pub fn parse_uint(s: &str) -> Option<u64>;
  pub fn url_unquote(s: &str) -> String;
  ```

- Produces (`aios_tools::cmd::docs_check::markdown`, contract `§4.5`; Tasks 5 and 7-12 use these names):

  ```rust
  #[derive(Debug, Clone, PartialEq, Eq)]
  pub struct Heading { pub line: usize, pub level: usize, pub text: String }
  pub static FENCE_RE: LazyLock<Regex>;        // R2
  pub static HEADING_RE: LazyLock<Regex>;      // R3
  pub static INLINE_LINK_RE: LazyLock<Regex>;  // R4 (group 2 text, group 3 target)
  pub static REF_DEF_RE: LazyLock<Regex>;      // R5
  pub static WIKI_RE: LazyLock<Regex>;         // R6
  pub static SECTION_REF_RE: LazyLock<Regex>;  // R7
  pub static SCHEME_RE: LazyLock<Regex>;       // R9
  pub static HTML_COMMENT_RE: LazyLock<Regex>; // R10
  pub static LIST_ITEM_RE: LazyLock<Regex>;    // R11
  pub const PLACEHOLDER_CHARS: [&str; 11];
  pub fn mask_code_spans(line: &str) -> String;
  pub fn code_spans(line: &str) -> Vec<String>;
  pub fn mask_html_comments(line: &str) -> String;
  pub fn mask_prose(line: &str) -> String;
  pub fn is_escaped(s: &str, idx: usize) -> bool;          // idx = byte index of an ASCII char
  pub fn prose_lines(text: &str) -> Vec<(usize, &str)>;    // (1-based line, line)
  pub fn strip_inline_md(text: &str) -> String;
  pub fn gh_slug(text: &str) -> String;
  pub fn headings(text: &str) -> Vec<Heading>;
  pub fn brace_expand(s: &str) -> Vec<String>;
  pub fn section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)>;
  pub fn table_rows(lines: &[(usize, &str)]) -> Vec<(usize, Vec<String>)>;
  pub fn milestone_tokens(text: &str) -> BTreeSet<u64>;
  pub fn heading_number(text: &str) -> Option<String>;     // R8
  pub fn has_placeholder_word(path: &str) -> bool;         // R25
  pub fn is_placeholder(target: &str) -> bool;
  pub fn is_path_placeholder(path: &str) -> bool;
  pub fn clean_repo_path(token: &str) -> String;
  pub fn split_target(raw: &str) -> Option<(String, String)>; // (unquoted path, fragment)
  pub fn parse_frontmatter(text: &str) -> Option<HashMap<String, String>>;
  ```

**Notes (check.py at `33c6b3d`; re-read each range while porting):**
- Port map: regex constants L161-173 and L223-224; `mask_code_spans` L176-198; `code_spans` L201-220 (no "followed by a backtick" loop, unlike `mask_code_spans`); `mask_html_comments` and `mask_prose` L227-234; `is_escaped` L237-242; `iter_prose_lines` L245-291 (here `prose_lines`, returning a `Vec`); `strip_inline_md` and `gh_slug` L294-303; `headings` L306-313; `brace_expand` L316-323; `section_body` and `table_rows` L326-352; `milestone_tokens` L355-364; `PLACEHOLDER_WORD_RE`, `is_placeholder`, `is_path_placeholder` L534-546; `split_target` L561-572; `clean_repo_path` L727-730; `parse_frontmatter` L1338-1347.
- Lookaround rewrites (contract `§2`): R8 `HEADING_NUM_RE` (lookahead `(?=[.:\s)]|$)`) becomes `heading_number`: match the greedy number with `HEADING_NUM_HEAD`, accept it when the next character is `.`, `:`, `)`, whitespace (`pystr::is_space`) or the end, else fall back to the prefix before its last `.` (the only shorter prefix Python's backtracking can accept, because the character after it is that `.`). R25 `PLACEHOLDER_WORD_RE` (lookbehind and lookahead `(?<![A-Za-z])...(?![A-Za-z])`) becomes `has_placeholder_word`: some maximal ASCII-letter run (`ASCII_LETTERS`) equals one of `N NN K X XX XXX YYYY MM DD`. The unit tests pin the contract's case lists plus cases recorded from check.py.
- Other pattern translations: every `\d` is `[0-9]`; R12's `\1` template is `"${1}"`; R14 (`[^\w\- ]` in `gh_slug`) is a character filter keeping `_`, `-`, space and `char::is_alphanumeric`; R16's `re.fullmatch` is `^(?:...)$`; in R5 the `^` inside the class, in R15 the braces inside the class and in R70 the `-` inside the class are escaped (same language, unambiguous for the `regex` crate).
- Indexing: check.py indexes code points; this port uses byte offsets only at backticks, brackets and backslashes (ASCII, one byte), so the positions name the same characters. Masking emits one space per `char`, as Python emits one per code point.
- Accepted divergences are listed in the module doc (regex `\s` without U+001C..U+001F, `\d` ASCII only, `\w`/`\b`/`isalnum` Unicode edges, milestone numbers above `u64`).
- Every expected value in the tests was recorded by calling the check.py function of the same name on the same input with python3; `regexes_compile` forces every `LazyLock` so a bad pattern fails `cargo test`.

- [ ] **Step 1: Write the failing tests**

Create `tools/src/cmd/docs_check/markdown.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    // Expected values below were recorded by calling the check.py functions of the
    // same name on the same inputs (python3, check.py at 33c6b3d).

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 20] = [
            &FENCE_RE,
            &HEADING_RE,
            &INLINE_LINK_RE,
            &REF_DEF_RE,
            &WIKI_RE,
            &SECTION_REF_RE,
            &SCHEME_RE,
            &HTML_COMMENT_RE,
            &LIST_ITEM_RE,
            &HEADING_NUM_HEAD,
            &ASCII_LETTERS,
            &INLINE_MD_LINK_RE,
            &HTML_TAG_RE,
            &BRACE_RE,
            &SEPARATOR_CELL_RE,
            &MILESTONE_RANGE_RE,
            &MILESTONE_RE,
            &LINE_SUFFIX_RE,
            &FRONTMATTER_RE,
            &FRONTMATTER_KEY_RE,
        ];
        for rx in all {
            assert!(!LazyLock::force(rx).as_str().is_empty());
        }
    }

    #[test]
    fn heading_number_matches_the_lookahead_pattern() {
        let cases = [
            ("3.3 Lock", Some("3.3")),
            ("8. Plan", Some("8")),
            ("§ 4.2: x", Some("4.2")),
            ("A1.2 T", Some("A1.2")),
            ("4)", Some("4")),
            ("4", Some("4")),
            ("1.2a", Some("1")),
            ("1.2.3x", Some("1.2")),
            ("12a", None),
            ("Milestone 3", None),
            ("§3", Some("3")),
            ("2.1\tTabbed", Some("2.1")),
            ("B12.4.1 (x)", Some("B12.4.1")),
            ("1..2", Some("1")),
        ];
        for (text, want) in cases {
            assert_eq!(heading_number(text).as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn placeholder_word_matches_the_lookaround_pattern() {
        let cases = [
            ("docs/phases/NN-name.md", true),
            ("YYYY-MM-DD", true),
            ("aXb", false),
            ("KX", false),
            ("NNN", false),
            ("x/N", true),
            ("N_x", true),
            ("9K", true),
            ("MMX", false),
        ];
        for (path, want) in cases {
            assert_eq!(has_placeholder_word(path), want, "{path:?}");
        }
    }

    #[test]
    fn mask_code_spans_matches_check_py() {
        let cases = [
            ("a `b` c", "a     c"),
            ("``x`y`` z", "        z"),
            ("`unclosed", "`unclosed"),
            ("``a```b``", "         "),
            ("é `ü` ö", "é     ö"),
            ("x ``` y", "x ``` y"),
            ("`a` `b", "    `b"),
            ("a ``b`` `c` d", "a           d"),
        ];
        for (line, want) in cases {
            assert_eq!(mask_code_spans(line), want, "{line:?}");
        }
    }

    #[test]
    fn code_spans_matches_check_py() {
        let cases: [(&str, &[&str]); 6] = [
            ("run `just build` and ``a ` b``", &["just build", "a ` b"]),
            ("``a```b``", &["a", "b"]),
            ("` padded `", &["padded"]),
            ("`a` `b", &["a"]),
            ("no spans", &[]),
            ("``x`y`` z", &["x`y"]),
        ];
        for (line, want) in cases {
            assert_eq!(code_spans(line), want, "{line:?}");
        }
    }

    #[test]
    fn masking_keeps_columns() {
        assert_eq!(mask_html_comments("a <!-- é --> b"), "a            b");
        assert_eq!(mask_html_comments("<!--x--><!--y-->"), " ".repeat(16));
        assert_eq!(mask_html_comments("<!-- open"), "<!-- open");
        assert_eq!(mask_prose("`<!--` x <!-- y -->"), "       x           ");
        assert_eq!(mask_prose("[a](b) `[c](d)`"), "[a](b)         ");
    }

    #[test]
    fn is_escaped_counts_backslashes() {
        assert!(is_escaped("\\[", 1));
        assert!(!is_escaped("\\\\[", 2));
        assert!(!is_escaped("[", 0));
        assert!(is_escaped("a\\\\\\[", 4));
    }

    const PROSE_DOC: &str = concat!(
        "# Title\n",
        "Intro text\n",
        "```rust\n",
        "# not a heading\n",
        "```\n",
        "    indented code after blank? no, prev not blank\n",
        "text\n",
        "\n",
        "    indented code block\n",
        "\ttab indented too\n",
        "\n",
        "after indented\n",
        "- list item\n",
        "\n",
        "    continuation in list (not code)\n",
        "paragraph\n",
        "<!-- multi\n",
        "line comment -->\n",
        "<!-- single --> kept\n",
        "~~~~\n",
        "~~~ inner shorter fence\n",
        "~~~~~\n",
        "   ```\n",
        "   indented fence\n",
        "   ```\n",
        "## Heading ##\n",
        "####### seven\n",
        "#no space\n",
        "## C# language\n",
    );

    #[test]
    fn prose_lines_skip_code_and_comments() {
        let want = [
            (1, "# Title"),
            (2, "Intro text"),
            (6, "    indented code after blank? no, prev not blank"),
            (7, "text"),
            (8, ""),
            (12, "after indented"),
            (13, "- list item"),
            (14, ""),
            (15, "    continuation in list (not code)"),
            (16, "paragraph"),
            (19, "<!-- single --> kept"),
            (26, "## Heading ##"),
            (27, "####### seven"),
            (28, "#no space"),
            (29, "## C# language"),
        ];
        assert_eq!(prose_lines(PROSE_DOC), want);
    }

    #[test]
    fn headings_are_atx_headings_on_prose_lines() {
        let heading = |line, level, text: &str| Heading {
            line,
            level,
            text: text.to_string(),
        };
        assert_eq!(
            headings(PROSE_DOC),
            [
                heading(1, 1, "Title"),
                heading(26, 2, "Heading"),
                heading(29, 2, "C# language"),
            ]
        );
    }

    #[test]
    fn slugs_match_github() {
        let cases = [
            ("3.3 Lock Hierarchy", "33-lock-hierarchy"),
            (
                "`code` and **bold** [link](x.md) <br> *em*",
                "code-and-bold-link--em",
            ),
            ("Ünïcode Title_x", "ünïcode-title_x"),
            ("  Spaced  Out ", "spaced--out"),
            ("C# & C++: a/b (c)", "c--c-ab-c"),
            ("![img](p.png) Title", "img-title"),
            ("Ⅳ ½ ²", "ⅳ-½-²"),
        ];
        for (text, want) in cases {
            assert_eq!(gh_slug(text), want, "{text:?}");
        }
        assert_eq!(
            strip_inline_md("`code` and **bold** [link](x.md) <br> *em*"),
            "code and bold link  em"
        );
        assert_eq!(strip_inline_md("![img](p.png) Title"), "img Title");
    }

    #[test]
    fn brace_expand_is_depth_first() {
        let cases: [(&str, &[&str]); 6] = [
            (
                "docs/kernel/{a,b}.md",
                &["docs/kernel/a.md", "docs/kernel/b.md"],
            ),
            ("x{a,b}{c,d}", &["xac", "xad", "xbc", "xbd"]),
            ("p/{ a , b }/q", &["p/a/q", "p/b/q"]),
            ("none", &["none"]),
            ("{}", &[""]),
            ("a{b,{c,d}}e", &["abe", "ace", "abe", "ade"]),
        ];
        for (s, want) in cases {
            assert_eq!(brace_expand(s), want, "{s:?}");
        }
    }

    const TABLE_DOC: &str = concat!(
        "# D\n",
        "## 3. Locks\n",
        "### 3.3 Lock Hierarchy\n",
        "\n",
        "| Rank | Lock | Purpose |\n",
        "|---|:---:|---|\n",
        "| 1 | `ALPHA_LOCK` | First |\n",
        "|  |  |  |\n",
        "| | x | |\n",
        "  | 2 | `BETA` |\n",
        "|---|\n",
        "### 3.4 Test Locks\n",
        "text\n",
        "### 3.5 Notes\n",
    );

    #[test]
    fn section_body_and_table_rows() {
        let start = Regex::new(r"^### 3\.3 ").expect("valid regex");
        let stop = Regex::new(r"^### 3\.5 |^## 4\.").expect("valid regex");
        let body = section_body(TABLE_DOC, &start, &stop);
        let lines: Vec<usize> = body.iter().map(|&(line, _)| line).collect();
        assert_eq!(lines, (4..=13).collect::<Vec<usize>>());
        assert_eq!(body[1], (5, "| Rank | Lock | Purpose |"));
        let cells = |row: &[&str]| row.iter().map(|c| c.to_string()).collect::<Vec<String>>();
        assert_eq!(
            table_rows(&body),
            [
                (5, cells(&["Rank", "Lock", "Purpose"])),
                (7, cells(&["1", "`ALPHA_LOCK`", "First"])),
                (9, cells(&["", "x", ""])),
                (10, cells(&["2", "`BETA`"])),
            ]
        );
        let never = Regex::new(r"^## 9").expect("valid regex");
        assert!(section_body(TABLE_DOC, &never, &stop).is_empty());
    }

    #[test]
    fn milestone_tokens_expand_short_ranges() {
        let cases: [(&str, &[u64]); 4] = [
            (
                "M1–M3, M7 and M10-M12; M5-M2 (reversed), M1-M200",
                &[1, 2, 3, 5, 7, 10, 11, 12, 200],
            ),
            ("M4 - M6", &[4, 5, 6]),
            ("XM3 M03", &[3]),
            ("M1-M2-M3", &[1, 2, 3]),
        ];
        for (text, want) in cases {
            let got: Vec<u64> = milestone_tokens(text).into_iter().collect();
            assert_eq!(got, want, "{text:?}");
        }
    }

    #[test]
    fn placeholders() {
        let cases = [
            ("docs/phases/NN-name.md", false, true),
            ("shared/BootInfo", false, true),
            ("kernel/src/mm/", false, false),
            ("kernel/src/Foo.rs", false, false),
            ("docs/<x>.md", true, true),
            ("docs/a…", true, true),
            ("a/b...", true, true),
            ("kernel/src/mm/X/", false, true),
            ("scripts/YYYY-MM-DD.sh", false, true),
            ("kernel/src/main.rs", false, false),
            ("KX/y", false, false),
            ("a/Makefile", false, true),
            ("a/README", false, true),
        ];
        for (path, placeholder, path_placeholder) in cases {
            assert_eq!(is_placeholder(path), placeholder, "{path:?}");
            assert_eq!(is_path_placeholder(path), path_placeholder, "{path:?}");
        }
    }

    #[test]
    fn clean_repo_path_drops_line_suffixes() {
        let cases = [
            ("kernel/src/main.rs:12-40,", "kernel/src/main.rs:12-40"),
            ("shared/src/lib.rs::Foo", "shared/src/lib.rs"),
            ("scripts/x.sh:1,4,9", "scripts/x.sh"),
            ("kernel/src/mm/).", "kernel/src/mm/"),
            ("a:1:2", "a:1"),
            ("kernel/src/a.rs:3–5", "kernel/src/a.rs"),
            ("x.rs:12;", "x.rs:12"),
        ];
        for (token, want) in cases {
            assert_eq!(clean_repo_path(token), want, "{token:?}");
        }
    }

    #[test]
    fn split_target_matches_check_py() {
        let cases = [
            ("<a b.md>", Some(("a b.md", ""))),
            ("https://x.org/a", None),
            ("//host/x", None),
            ("#frag", Some(("", "frag"))),
            ("a%20b.md#Sec", Some(("a b.md", "Sec"))),
            ("a%20b.md?x=1#Sec", None),
            ("<>", None),
            ("docs/{x}.md", None),
            ("a.md#{x}", Some(("a.md", "{x}"))),
            ("  b.md  ", Some(("b.md", ""))),
            ("mailto:x@y", None),
            ("c%C3%A9.md", Some(("cé.md", ""))),
            ("%E9.md", Some(("\u{FFFD}.md", ""))),
            ("a.md#b#c", Some(("a.md", "b#c"))),
            ("?q#f", None),
        ];
        for (raw, want) in cases {
            let want = want.map(|(p, f): (&str, &str)| (p.to_string(), f.to_string()));
            assert_eq!(split_target(raw), want, "{raw:?}");
        }
    }

    #[test]
    fn frontmatter_matches_check_py() {
        let map = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|&(k, v)| (k.to_string(), v.to_string()))
                .collect::<HashMap<String, String>>()
        };
        let full = "---\nauthor: jl\ndate: 2026-01-01\ntags: [a]\nstatus: \"final\"\n---\n# T\n";
        assert_eq!(
            parse_frontmatter(full),
            Some(map(&[
                ("author", "jl"),
                ("date", "2026-01-01"),
                ("tags", "[a]"),
                ("status", "final"),
            ]))
        );
        assert_eq!(
            parse_frontmatter("---\r\nk: 'v'\r\n---\r\n"),
            Some(map(&[("k", "v")]))
        );
        assert_eq!(parse_frontmatter("no front"), None);
        assert_eq!(parse_frontmatter("---\n---\n"), None);
        assert_eq!(
            parse_frontmatter("---\nk: v\n---"),
            Some(map(&[("k", "v")]))
        );
        assert_eq!(
            parse_frontmatter("---\na: 1\n--- \nb"),
            Some(map(&[("a", "1")]))
        );
        assert_eq!(
            parse_frontmatter("---\n  indented: x\nbad line\nx-y_z: w \nx-y_z: later\n---\n"),
            Some(map(&[("x-y_z", "later")]))
        );
    }
}
```

In `tools/src/cmd/docs_check/mod.rs` replace the line `pub mod model;` with:

```rust
pub mod markdown;
pub mod model;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --lib markdown`
Expected: FAIL to compile with `error[E0425]` errors ("cannot find function heading_number in this scope", "cannot find value FENCE_RE in this scope", and so on): the tests exist, the helpers do not.

- [ ] **Step 3: Write the implementation**

Insert this block at the top of `tools/src/cmd/docs_check/markdown.rs`, above the `#[cfg(test)]` line from Step 1, with one blank line between the last `}` of the block and `#[cfg(test)]`:

```rust
//! Markdown parsing helpers of `aios docs-check`, ported line by line from
//! `scripts/docs/check.py` at 33c6b3d (L161-364, L534-572, L724-730, L1338-1347).
//!
//! Lines are Python `str.splitlines()` and whitespace is Python `str.isspace()`
//! (`crate::pystr`). Where check.py indexes a line at an ASCII character
//! (backtick, bracket, backslash), this port uses byte offsets; the positions are
//! the same characters because those characters are one byte in UTF-8.
//!
//! Python regex features the `regex` crate lacks are rewritten as code:
//! `heading_number` (R8, a lookahead) and `has_placeholder_word` (R25, a
//! lookbehind and a lookahead). Every other pattern is check.py's text with `\d`
//! written as `[0-9]`.
//!
//! Accepted divergences from check.py (no tracked file and no fixture exercises
//! them; the parity goldens prove the real inputs):
//! - regex `\s` does not match U+001C..U+001F here (Python's does);
//! - `\d` is `[0-9]`: non-ASCII decimal digits are not digits here;
//! - regex `\w`/`\b` and `gh_slug` use Unicode `Alphabetic`/`Numeric`, Python uses
//!   `str.isalnum()`; they differ for combining marks and a few numeric symbols;
//! - milestone numbers that do not fit `u64` are ignored.

use std::collections::{BTreeSet, HashMap};
use std::sync::LazyLock;

use regex::{Captures, Regex};

use crate::pystr;

/// An ATX heading outside code blocks (check.py `headings`, L306-313).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Heading {
    /// 1-based line number.
    pub line: usize,
    /// Number of leading `#` characters (1-6).
    pub level: usize,
    /// Heading text (HEADING_RE group 2).
    pub text: String,
}

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// R2 (check.py L161): an opening or closing fence at any indentation.
pub static FENCE_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^\s*(`{3,}|~{3,})"));
/// R3 (check.py L162).
pub static HEADING_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^(#{1,6})\s+(.*?)\s*#*\s*$"));
/// R4 (check.py L163-165): group 2 is the text, group 3 the target.
pub static INLINE_LINK_RE: LazyLock<Regex> = LazyLock::new(|| {
    compile(
        r#"(!?)\[((?:[^\[\]]|\[[^\]]*\])*)\]\(\s*(<[^>]*>|[^)\s]+)(?:\s+(?:"[^"]*"|'[^']*'))?\s*\)"#,
    )
});
/// R5 (check.py L166): reference definitions, not footnotes (`^` escaped in the class).
pub static REF_DEF_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^ {0,3}\[([^\]\^][^\]]*)\]:\s*(\S+)"));
/// R6 (check.py L167).
pub static WIKI_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(!?)\[\[([^\]|#]*)(?:#([^\]|]*))?(?:\|[^\]]*)?\]\]"));
/// R7 (check.py L168-170).
pub static SECTION_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    compile(r"\[[^\]]*\]\(([^)\s]+\.md)(#[^)]*)?\)\s*\**\s*§\s*([A-Z]?[0-9]+(?:\.[0-9]+)*)")
});
/// R9 (check.py L172).
pub static SCHEME_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^[a-zA-Z][a-zA-Z0-9+.-]*:"));
/// R10 (check.py L223).
pub static HTML_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"<!--.*?-->"));
/// R11 (check.py L224).
pub static LIST_ITEM_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^\s*(?:[-*+]|[0-9]+[.)])\s+"));

/// check.py L173.
pub const PLACEHOLDER_CHARS: [&str; 11] = ["<", ">", "{", "}", "$", "*", "?", "…", "...", "[", "]"];

/// R8 without its lookahead (see `heading_number`).
static HEADING_NUM_HEAD: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^(?:§\s*)?([A-Z]?[0-9]+(?:\.[0-9]+)*)"));
/// R25 rewritten: maximal runs of ASCII letters (see `has_placeholder_word`).
static ASCII_LETTERS: LazyLock<Regex> = LazyLock::new(|| compile(r"[A-Za-z]+"));
/// R12 (check.py L295).
static INLINE_MD_LINK_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"!?\[([^\]]*)\]\([^)]*\)"));
/// R13 (check.py L296).
static HTML_TAG_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"<[^>]+>"));
/// R15 (check.py L317); the braces are escaped inside the class too.
static BRACE_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"\{([^\{\}]*)\}"));
/// R16 (check.py L349, fullmatch).
static SEPARATOR_CELL_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^(?::?-{2,}:?)$"));
/// R17 (check.py L358).
static MILESTONE_RANGE_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"\bM([0-9]+)\s*[–-]\s*M([0-9]+)\b"));
/// R18 (check.py L362).
static MILESTONE_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"\bM([0-9]+)\b"));
/// R29 (check.py L729).
static LINE_SUFFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r":[0-9]+(?:[-–][0-9]+)?(?:,[0-9]+)*$"));
/// R69 (check.py L1339, re.S).
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"(?s)^---\r?\n(.*?)\r?\n---\s*(?:\r?\n|$)"));
/// R70 (check.py L1344).
static FRONTMATTER_KEY_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^([A-Za-z_][\w\-]*):\s*(.*)$"));

/// Byte index just past the run of backticks that starts at `start`.
fn backtick_run_end(bytes: &[u8], start: usize) -> usize {
    let mut end = start;
    while end < bytes.len() && bytes[end] == b'`' {
        end += 1;
    }
    end
}

/// Python `haystack.find(needle, from)` in byte offsets; `None` for -1.
fn find_from(haystack: &str, needle: &str, from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .find(needle)
        .map(|offset| from + offset)
}

/// check.py `mask_code_spans` (L176-198): inline code spans, backticks included,
/// become one space per character. The closing run is the next run of the same
/// length that is not followed by another backtick; unclosed runs stay.
pub fn mask_code_spans(line: &str) -> String {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut out = String::with_capacity(n);
    let mut copied = 0;
    let mut i = 0;
    while i < n {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let j = backtick_run_end(bytes, i);
        let tick = &line[i..j];
        let mut close = find_from(line, tick, j);
        while let Some(at) = close {
            let after = at + tick.len();
            if after < n && bytes[after] == b'`' {
                close = find_from(line, tick, after + 1);
            } else {
                break;
            }
        }
        let Some(at) = close else {
            i = j;
            continue;
        };
        let end = at + tick.len();
        out.push_str(&line[copied..i]);
        out.extend(line[i..end].chars().map(|_| ' '));
        copied = end;
        i = end;
    }
    out.push_str(&line[copied..]);
    out
}

/// check.py `code_spans` (L201-220): the stripped contents of inline code spans.
/// Unlike `mask_code_spans`, the closing run is simply the next run of the same
/// length, even when another backtick follows it.
pub fn code_spans(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let n = bytes.len();
    let mut spans = Vec::new();
    let mut i = 0;
    while i < n {
        if bytes[i] != b'`' {
            i += 1;
            continue;
        }
        let j = backtick_run_end(bytes, i);
        let tick = &line[i..j];
        match find_from(line, tick, j) {
            None => i = j,
            Some(close) => {
                spans.push(pystr::strip(&line[j..close]).to_string());
                i = close + tick.len();
            }
        }
    }
    spans
}

/// check.py `mask_html_comments` (L227-229): each `<!-- ... -->` on the line
/// becomes one space per character.
pub fn mask_html_comments(line: &str) -> String {
    HTML_COMMENT_RE
        .replace_all(line, |caps: &Captures<'_>| {
            " ".repeat(caps[0].chars().count())
        })
        .into_owned()
}

/// check.py `mask_prose` (L232-234): code spans, then inline HTML comments.
pub fn mask_prose(line: &str) -> String {
    mask_html_comments(&mask_code_spans(line))
}

/// check.py `is_escaped` (L237-242): an odd number of backslashes right before
/// byte index `idx` (the index of an ASCII character).
pub fn is_escaped(s: &str, idx: usize) -> bool {
    let bytes = s.as_bytes();
    let mut n = 0;
    while let Some(pos) = idx.checked_sub(n + 1) {
        if bytes.get(pos) != Some(&b'\\') {
            break;
        }
        n += 1;
    }
    n % 2 == 1
}

/// check.py `iter_prose_lines` (L245-291): `(1-based line number, line)` for the
/// lines outside fenced blocks (any indentation), multi-line HTML comments and
/// CommonMark indented code blocks (4+ columns after a blank line, outside a
/// list). Single-line comments stay; callers mask them with `mask_prose`.
pub fn prose_lines(text: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut fence: Option<&str> = None;
    let mut in_comment = false;
    let mut in_indented = false;
    let mut in_list = false;
    let mut prev_blank = true;
    for (index, line) in pystr::splitlines(text).into_iter().enumerate() {
        let lineno = index + 1;
        let run = FENCE_RE
            .captures(line)
            .and_then(|caps| caps.get(1))
            .map(|m| m.as_str());
        if fence.is_none() && run.is_some() && !in_indented {
            fence = run;
            prev_blank = false;
            continue;
        }
        if let Some(open) = fence {
            let closes = run.is_some_and(|run| {
                run.as_bytes()[0] == open.as_bytes()[0]
                    && run.len() >= open.len()
                    && pystr::strip(line) == run
            });
            if closes {
                fence = None;
            }
            continue;
        }
        if in_comment {
            if line.contains("-->") {
                in_comment = false;
            }
            continue;
        }
        if pystr::lstrip(line).starts_with("<!--") && !line.contains("-->") {
            in_comment = true;
            continue;
        }
        let blank = pystr::strip(line).is_empty();
        let indent = pystr::indent_width(line);
        if in_indented {
            if blank || indent >= 4 {
                prev_blank = blank;
                continue;
            }
            in_indented = false;
        } else if indent >= 4 && prev_blank && !(blank || in_list) {
            in_indented = true;
            continue;
        }
        if !blank {
            if LIST_ITEM_RE.is_match(line) {
                in_list = true;
            } else if indent == 0 && (prev_blank || line.starts_with('#')) {
                in_list = false;
            }
        }
        prev_blank = blank;
        out.push((lineno, line));
    }
    out
}

/// check.py `strip_inline_md` (L294-297).
pub fn strip_inline_md(text: &str) -> String {
    let unlinked = INLINE_MD_LINK_RE.replace_all(text, "${1}");
    let untagged = HTML_TAG_RE.replace_all(&unlinked, "");
    untagged.replace('`', "").replace("**", "").replace('*', "")
}

/// check.py `gh_slug` (L300-303): GitHub's heading anchor. R14 (`[^\w\- ]`) is
/// the character filter below.
pub fn gh_slug(text: &str) -> String {
    let stripped = strip_inline_md(text);
    pystr::strip(&stripped)
        .to_lowercase()
        .chars()
        .filter(|&c| c == '_' || c == '-' || c == ' ' || c.is_alphanumeric())
        .map(|c| if c == ' ' { '-' } else { c })
        .collect()
}

/// check.py `headings` (L306-313): ATX headings on prose lines.
pub fn headings(text: &str) -> Vec<Heading> {
    prose_lines(text)
        .into_iter()
        .filter_map(|(line, content)| {
            let caps = HEADING_RE.captures(content)?;
            Some(Heading {
                line,
                level: caps[1].len(),
                text: caps[2].to_string(),
            })
        })
        .collect()
}

/// check.py `brace_expand` (L316-323): expands the first `{a,b}` group, then
/// recurses on each result (depth-first).
pub fn brace_expand(s: &str) -> Vec<String> {
    let Some(caps) = BRACE_RE.captures(s) else {
        return vec![s.to_string()];
    };
    let (Some(whole), Some(inner)) = (caps.get(0), caps.get(1)) else {
        return vec![s.to_string()];
    };
    let prefix = &s[..whole.start()];
    let suffix = &s[whole.end()..];
    inner
        .as_str()
        .split(',')
        .flat_map(|part| brace_expand(&format!("{prefix}{}{suffix}", pystr::strip(part))))
        .collect()
}

/// check.py `section_body` (L326-339): the lines after the first line where
/// `start` matches (searched anywhere), up to the next line where `stop` matches.
pub fn section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)> {
    let mut out = Vec::new();
    let mut started = false;
    for (index, line) in pystr::splitlines(text).into_iter().enumerate() {
        if !started {
            if start.is_match(line) {
                started = true;
            }
            continue;
        }
        if stop.is_match(line) {
            break;
        }
        out.push((index + 1, line));
    }
    out
}

/// check.py `table_rows` (L342-352): the stripped cells of `|` table rows,
/// skipping separator rows (every non-empty cell is `:?-{2,}:?`).
pub fn table_rows(lines: &[(usize, &str)]) -> Vec<(usize, Vec<String>)> {
    let mut rows = Vec::new();
    for &(lineno, line) in lines {
        let s = pystr::strip(line);
        if !s.starts_with('|') {
            continue;
        }
        let cells: Vec<String> = s
            .trim_matches('|')
            .split('|')
            .map(|cell| pystr::strip(cell).to_string())
            .collect();
        if cells
            .iter()
            .filter(|cell| !cell.is_empty())
            .all(|cell| SEPARATOR_CELL_RE.is_match(cell))
        {
            continue;
        }
        rows.push((lineno, cells));
    }
    rows
}

/// check.py `milestone_tokens` (L355-364): milestones named as `M12` or as a
/// range `M3-M5` / `M3–M5` (at most 100 wide).
pub fn milestone_tokens(text: &str) -> BTreeSet<u64> {
    let mut out = BTreeSet::new();
    for caps in MILESTONE_RANGE_RE.captures_iter(text) {
        let lo = pystr::parse_uint(&caps[1]);
        let hi = pystr::parse_uint(&caps[2]);
        if let (Some(lo), Some(hi)) = (lo, hi) {
            if lo <= hi && hi - lo < 100 {
                out.extend(lo..=hi);
            }
        }
    }
    for caps in MILESTONE_RE.captures_iter(text) {
        if let Some(num) = pystr::parse_uint(&caps[1]) {
            out.insert(num);
        }
    }
    out
}

/// R8, check.py `HEADING_NUM_RE.match(text).group(1)` (L171): the section number
/// at the start of a heading. Python backtracks the greedy number when the
/// lookahead `(?=[.:\s)]|$)` fails; the only shorter prefix that can pass is the
/// one before the last `.`, whose next character is that `.`.
pub fn heading_number(text: &str) -> Option<String> {
    let num = HEADING_NUM_HEAD.captures(text)?.get(1)?;
    let passes = text[num.end()..]
        .chars()
        .next()
        .is_none_or(|c| matches!(c, '.' | ':' | ')') || pystr::is_space(c));
    let s = num.as_str();
    if passes {
        Some(s.to_string())
    } else {
        s.rfind('.').map(|dot| s[..dot].to_string())
    }
}

/// R25, check.py `PLACEHOLDER_WORD_RE.search(path)` (L534): a match needs a
/// non-letter (or an edge) on both sides of an all-letter word, so it exists
/// exactly when some maximal ASCII-letter run equals one of the words.
pub fn has_placeholder_word(path: &str) -> bool {
    ASCII_LETTERS.find_iter(path).any(|m| {
        matches!(
            m.as_str(),
            "N" | "NN" | "K" | "X" | "XX" | "XXX" | "YYYY" | "MM" | "DD"
        )
    })
}

/// check.py `is_placeholder` (L537-538).
pub fn is_placeholder(target: &str) -> bool {
    PLACEHOLDER_CHARS.iter().any(|c| target.contains(c))
}

/// check.py `is_path_placeholder` (L541-546): template paths
/// (`docs/phases/NN-name.md`) and type names (`shared/BootInfo`) are not paths.
pub fn is_path_placeholder(path: &str) -> bool {
    if is_placeholder(path) || has_placeholder_word(path) {
        return true;
    }
    let trimmed = path.trim_end_matches('/');
    let last = trimmed.rsplit_once('/').map_or(trimmed, |(_, last)| last);
    !(path.ends_with('/') || last.contains('.')) && last.chars().any(char::is_uppercase)
}

/// check.py `clean_repo_path` (L727-730): drops a `::item` suffix, a
/// `:line[-line][,line...]` suffix and trailing `.,;:)`.
pub fn clean_repo_path(token: &str) -> String {
    let head = token.split("::").next().unwrap_or(token);
    let without_lines = LINE_SUFFIX_RE.replace_all(head, "");
    without_lines
        .trim_end_matches(['.', ',', ';', ':', ')'])
        .to_string()
}

/// check.py `split_target` (L561-572): `(unquoted path, fragment)` of a local link
/// target, or `None` for an empty, external (`scheme:` or `//`) or placeholder target.
pub fn split_target(raw: &str) -> Option<(String, String)> {
    let mut t = pystr::strip(raw);
    if t.starts_with('<') && t.ends_with('>') {
        t = pystr::strip(&t[1..t.len() - 1]);
    }
    if t.is_empty() || SCHEME_RE.is_match(t) || t.starts_with("//") {
        return None;
    }
    if is_placeholder(t.split('#').next().unwrap_or(t)) {
        return None;
    }
    let (path, frag) = t.split_once('#').unwrap_or((t, ""));
    let path = path.split('?').next().unwrap_or(path);
    Some((pystr::url_unquote(path), frag.to_string()))
}

/// check.py `parse_frontmatter` (L1338-1347): `key: value` lines of the leading
/// `---` block (value stripped, then surrounding quotes removed; later keys win).
pub fn parse_frontmatter(text: &str) -> Option<HashMap<String, String>> {
    let caps = FRONTMATTER_RE.captures(text)?;
    let body = caps.get(1)?.as_str();
    let mut out = HashMap::new();
    for line in pystr::splitlines(body) {
        if let Some(kv) = FRONTMATTER_KEY_RE.captures(line) {
            let value = pystr::strip(&kv[2]).trim_matches(['"', '\'']);
            out.insert(kv[1].to_string(), value.to_string());
        }
    }
    Some(out)
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --lib markdown`
Expected: PASS: the 17 tests under `cmd::docs_check::markdown::tests` pass (`regexes_compile`, `heading_number_matches_the_lookahead_pattern`, `placeholder_word_matches_the_lookaround_pattern`, ..., `frontmatter_matches_check_py`), `test result: ok.`

- [ ] **Step 5: Format and lint**

Run: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools -- -D warnings`
Expected: no diff from `cargo fmt`; clippy ends with `Finished` and prints no warning.

- [ ] **Step 6: Commit and push**

```bash
git add tools/src/cmd/docs_check/markdown.rs tools/src/cmd/docs_check/mod.rs
git commit -F - <<'MSG'
Port the docs-check Markdown helpers to Rust

markdown.rs ports check.py's Markdown helpers (L161-364, L534-572,
L727-730, L1338-1347): prose lines, code-span and comment masking,
headings and GitHub slugs, brace expansion, tables, milestone tokens,
placeholders, link targets and frontmatter. The lookaround patterns R8
(heading numbers) and R25 (placeholder words) are rewritten as code;
the unit tests pin values recorded from check.py.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 5: Repository model

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/repo.rs` (implementation, then unit tests for `recipe_name` and the regexes)
- Create: `tools/tests/repo.rs` (integration tests on temporary git repositories)
- Modify: `tools/src/cmd/docs_check/mod.rs` (add `pub mod repo;`)
- Test: `tools/tests/repo.rs`, `tools/src/cmd/docs_check/repo.rs`

**Interfaces:**
- Consumes:
  - Task 1: `aios_tools::pystr::{strip, splitlines, decode_text, parse_uint}`, `aios_tools::paths::{dirname, join, normpath, relpath, realpath}` and

    ```rust
    pub fn capture(program: &str, args: &[&str], cwd: &std::path::Path) -> anyhow::Result<std::process::Output>; // aios_tools::proc
    ```

  - Task 3: `aios_tools::cmd::docs_check::model::Skip` (`pub struct Skip(pub String)`, `Clone`, `std::error::Error`).
  - Task 4: `markdown::{Heading, headings, gh_slug}`.
  - Task 2 test helpers (`tools/tests/common/mod.rs`): `unique_dir(label) -> PathBuf`, `git(dir, args) -> String`, `TestRepo::{new, adopt, with_files, write, commit, path, path_str}`.
- Produces (`aios_tools::cmd::docs_check::repo`, contract `§4.6`; Tasks 6-12 use these names):

  ```rust
  pub const CURRENT_STATE_DOCS: [&str; 5];
  pub const CURRENT_STATE_PREFIX: &str = ".claude/";
  pub type Region = (String, Option<BTreeSet<usize>>);
  pub struct Repo { /* private fields */ }
  impl Repo {
      pub fn open(root: &str) -> anyhow::Result<Repo>;                 // root must be absolute
      pub fn root(&self) -> &str;
      pub fn files(&self) -> &[String];
      pub fn md_files(&self) -> &[String];
      pub fn is_file(&self, rel: &str) -> bool;
      pub fn git(&self, args: &[&str]) -> anyhow::Result<String>;
      pub fn git_unchecked(&self, args: &[&str]) -> anyhow::Result<String>;
      pub fn text(&self, rel: &str) -> Rc<str>;
      pub fn headings(&self, rel: &str) -> Rc<Vec<Heading>>;
      pub fn slug_set(&self, rel: &str) -> Rc<HashSet<String>>;
      pub fn exists(&self, rel: &str) -> bool;
      pub fn resolve(&self, src: &str, target: &str) -> Option<String>;
      pub fn merged_milestones(&self) -> anyhow::Result<Rc<BTreeMap<u64, u64>>>; // Err downcasts to Skip when shallow
      pub fn phase_docs(&self) -> Vec<(u64, String)>;
      pub fn milestone_sections(&self, rel: &str) -> BTreeMap<u64, (usize, usize)>;
      pub fn current_state_regions(&self) -> anyhow::Result<Vec<Region>>;
      pub fn justfile_recipes(&self) -> (BTreeSet<String>, BTreeSet<String>); // (all, public)
  }
  pub fn recipe_name(line: &str) -> Option<String>;                    // R24
  ```

**Notes (check.py at `33c6b3d`; re-read each range while porting):**
- Port map: `Repo.__init__` L373-394 (`git ls-files -z --cached --others --exclude-standard`, dedupe, `lexists` as `symlink_metadata`, sort, ancestor `dir_set`, `md_files` as regular files following symlinks); `git` L396-400; `text` L402-409 (unreadable file is `""`); `headings` L411-414; `exists` L416-426 (non-strict `realpath` so the tracked `.claude/skills/obsidian` symlink resolves); `resolve` L428-436; `merged_milestones` L440-462 (shallow check, merge base with `origin/main` then `main`, `git log --first-parent --format=%s`, newest subject wins); `phase_docs` L464-470; `milestone_sections` L472-486; `current_state_regions` L488-505; `justfile_recipes` L507-526; `slug_set` L588-604 (a method here, cached per `Repo`; Task 7 only calls it).
- Lookaround rewrite R24 (L518 `^@?([A-Za-z_][A-Za-z0-9_-]*)\b[^:=]*:(?!=)`): `RECIPE_RE` drops `(?!=)` and `recipe_name` rejects a match whose next character is `=`. `[^:=]*` stops at the first `:` or `=` after the name however the name backtracks, so if that `:` is followed by `=` Python has no other match either.
- Text and git decoding: file text is `pystr::decode_text` (lossy UTF-8, universal newlines); git stdout must be valid UTF-8 (else an error, exit 2, as Python's `text=True` raises) and then gets universal newlines. `git(..)` fails on a non-zero exit with `git <args> failed: <stripped stderr>`; `git_unchecked` returns stdout regardless.
- Caching: the shallow-clone `Skip` is cached like the result (Python caches `_merged_error`); other errors (git cannot start, non-UTF-8 output) propagate uncached. `current_state_regions` treats `Skip` as "no merged milestones" and propagates other errors.
- Contract gap filled here: `Repo::open` requires an absolute root (the caller passes `git rev-parse --show-toplevel` output); `realpath`/`relpath` use the root as their `cwd` argument, which is ignored for absolute paths.
- The integration tests record check.py's `Repo` results on the same trees (listing with symlinks, a deleted tracked file, untracked and ignored files; the justfile attribute cases; merge-base and newest-subject history; a `--depth 1` clone). `Repo::open` runs git with the test process's environment, which `common::isolated` never reaches, so `TestRepo::new` (Task 2) pins `core.excludesFile=/dev/null` in every test repository's own config and `snapshot_real` (Task 14) does the same for its clone. Without it a developer's or a CI image's global ignore rule can drop a fixture file from `files()`, changing `md_files`, `current_state_regions`, `code_mutex_statics`, `project_skills` and every finding derived from them, so a parity bug could pass or a correct port fail depending on the machine.

- [ ] **Step 1: Write the failing tests**

Create `tools/tests/repo.rs`:

```rust
//! docs-check repository model: listing, exists/resolve, text decoding, slugs, the
//! justfile, and the git-derived facts. Expected values were recorded by running
//! check.py's `Repo` (33c6b3d) on the same trees.

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::os::unix::fs::symlink;

use aios_tools::cmd::docs_check::markdown::Heading;
use aios_tools::cmd::docs_check::model::Skip;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

#[test]
fn listing_follows_git_ls_files() {
    let repo = TestRepo::with_files(
        "repo-listing",
        &[
            (".gitignore", "ignored.txt\n"),
            ("b.md", "# B\n"),
            ("a/one.md", "# One\n"),
            ("a/deep/two.rs", ""),
            ("real/file.md", "# F\n"),
            ("gone.md", "# gone\n"),
            ("Z.md", "z\n"),
        ],
    );
    symlink("real", repo.path().join("alias")).expect("symlink alias");
    symlink("a", repo.path().join("dirlink.md")).expect("symlink dirlink.md");
    symlink("nowhere.md", repo.path().join("dangling.md")).expect("symlink dangling.md");
    repo.commit("Links");
    std::fs::remove_file(repo.path().join("gone.md")).expect("remove gone.md");
    repo.write("untracked.txt", "u\n");
    repo.write("ignored.txt", "i\n");

    let r = open(&repo);
    assert_eq!(
        r.files(),
        [
            ".gitignore",
            "Z.md",
            "a/deep/two.rs",
            "a/one.md",
            "alias",
            "b.md",
            "dangling.md",
            "dirlink.md",
            "real/file.md",
            "untracked.txt",
        ]
    );
    assert_eq!(r.md_files(), ["Z.md", "a/one.md", "b.md", "real/file.md"]);
    assert!(r.is_file("alias"));
    assert!(!r.is_file("a"));
    let exists = [
        ("", true),
        (".", true),
        ("a", true),
        ("a/", true),
        ("a/deep", true),
        ("alias/file.md", true),
        ("alias", true),
        ("alias/missing.md", false),
        ("../outside", false),
        ("missing", false),
        ("b.md", true),
        ("untracked.txt", true),
        ("ignored.txt", false),
        ("gone.md", false),
        ("a/deep/two.rs/", true),
        ("dirlink.md/one.md", true),
    ];
    for (rel, want) in exists {
        assert_eq!(r.exists(rel), want, "exists({rel:?})");
    }
}

#[test]
fn resolve_stays_inside_the_repository() {
    let repo = TestRepo::with_files("repo-resolve", &[("README.md", "# R\n")]);
    let r = open(&repo);
    let cases = [
        ("docs/a.md", "b.md", Some("docs/b.md")),
        ("docs/a.md", "../x.md", Some("x.md")),
        ("docs/a.md", "../../x.md", None),
        ("a.md", "/docs/b.md", Some("docs/b.md")),
        ("docs/a.md", "..", Some("")),
        ("a.md", "..x", None),
        ("docs/a.md", ".", Some("docs")),
        ("a.md", ".", Some("")),
        ("a.md", "//x", Some("x")),
        ("docs/a.md", "./c/../d.md", Some("docs/d.md")),
        ("a.md", "/", Some("")),
    ];
    for (src, target, want) in cases {
        assert_eq!(
            r.resolve(src, target).as_deref(),
            want,
            "resolve({src:?}, {target:?})"
        );
    }
}

#[test]
fn text_is_lossy_utf8_with_universal_newlines() {
    let repo = TestRepo::new("repo-text");
    std::fs::write(repo.path().join("crlf.md"), b"a\r\nb\rc\n").expect("write crlf.md");
    std::fs::write(repo.path().join("bad.md"), b"x\xffy\n").expect("write bad.md");
    repo.write("d/x.md", "x");
    repo.commit("Initial");
    let r = open(&repo);
    assert_eq!(&*r.text("crlf.md"), "a\nb\nc\n");
    assert_eq!(&*r.text("bad.md"), "x\u{FFFD}y\n");
    assert_eq!(&*r.text("missing.md"), "");
    assert_eq!(&*r.text("d"), "");
}

#[test]
fn headings_and_slugs() {
    let doc = concat!(
        "# Intro\n",
        "\n",
        "## Intro\n",
        "\n",
        "## Intro\n",
        "\n",
        "```\n",
        "# Intro\n",
        "```\n",
        "\n",
        "### `Code` **Bold** [Link](x.md)\n",
        "\n",
        "<a name=\"Custom-Anchor\"></a>\n",
        "<a  id=\"Other\">\n",
    );
    let repo = TestRepo::with_files("repo-slugs", &[("s.md", doc)]);
    let r = open(&repo);
    let heading = |line, level, text: &str| Heading {
        line,
        level,
        text: text.to_string(),
    };
    assert_eq!(
        *r.headings("s.md"),
        [
            heading(1, 1, "Intro"),
            heading(3, 2, "Intro"),
            heading(5, 2, "Intro"),
            heading(11, 3, "`Code` **Bold** [Link](x.md)"),
        ]
    );
    let slugs = r.slug_set("s.md");
    let mut sorted: Vec<&str> = slugs.iter().map(String::as_str).collect();
    sorted.sort_unstable();
    assert_eq!(
        sorted,
        [
            "code-bold-link",
            "custom-anchor",
            "intro",
            "intro-1",
            "intro-2",
            "other"
        ]
    );
}

#[test]
fn justfile_recipes_track_private_attributes() {
    let justfile = concat!(
        "# comment\n",
        "set shell := [\"bash\", \"-c\"]\n",
        "x := \"1\"\n",
        "\n",
        "default: build\n",
        "\n",
        "build:\n",
        "    echo build\n",
        "\n",
        "check: build\n",
        "    echo check\n",
        "\n",
        "[private]\n",
        "helper:\n",
        "    echo\n",
        "\n",
        "[private]\n",
        "# comment between\n",
        "\n",
        "[no-cd]\n",
        "secret:\n",
        "    echo\n",
        "\n",
        "[private]\n",
        "[group('x')]\n",
        "also-private arg=\"1\":\n",
        "    echo\n",
        "\n",
        "_hidden:\n",
        "    echo\n",
        "\n",
        "[group('dev')]\n",
        "dev *args:\n",
        "    echo\n",
        "\n",
        "@quiet:\n",
        "    echo\n",
        "\n",
        "name-with-dash-: x\n",
        "  indented: x\n",
        "a:=b\n",
    );
    let repo = TestRepo::with_files("repo-just", &[("justfile", justfile)]);
    let (names, public) = open(&repo).justfile_recipes();
    let set = |items: &[&str]| {
        items
            .iter()
            .map(|s| s.to_string())
            .collect::<BTreeSet<String>>()
    };
    assert_eq!(
        names,
        set(&[
            "_hidden",
            "build",
            "check",
            "default",
            "dev",
            "helper",
            "name-with-dash",
            "quiet",
            "secret",
        ])
    );
    assert_eq!(
        public,
        set(&[
            "build",
            "check",
            "default",
            "dev",
            "name-with-dash",
            "quiet"
        ])
    );
}

const PHASE0: &str = concat!(
    "# Phase 0: Foundation\n",
    "\n",
    "**Status:** In progress\n",
    "\n",
    "## Milestones\n",
    "\n",
    "| Milestone | Steps |\n",
    "|---|---|\n",
    "| **M1 — Boot** | 1 |\n",
    "\n",
    "## Milestone 1 — Boot\n",
    "\n",
    "- [x] a\n",
    "\n",
    "```\n",
    "## Milestone 7 — in a fence\n",
    "```\n",
    "\n",
    "### Milestone 9 — level three\n",
    "\n",
    "## Milestone 2 — UART\n",
    "\n",
    "- [ ] b\n",
);

const PHASE1: &str = concat!(
    "# Phase 1: Memory\n",
    "\n",
    "## Milestone 3 — Allocator\n",
    "\n",
    "- [ ] c\n",
);

#[test]
fn phase_docs_sections_and_current_state_regions() {
    let repo = TestRepo::with_files(
        "repo-regions",
        &[
            ("CLAUDE.md", "# C\n"),
            ("README.md", "# R\n"),
            (".claude/rules/01-x.md", "# X\n"),
            (".claude/notes.txt", "n\n"),
            ("docs/project/developer-guide.md", "# G\n"),
            ("docs/project/agent-loop.md", "# L\n"),
            ("docs/other.md", "# O\n"),
            ("docs/phases/00-foundation.md", PHASE0),
            ("docs/phases/01-memory.md", PHASE1),
            ("docs/phases/10-late.md", "# Late\n"),
            ("docs/phases/notes.md", "# N\n"),
        ],
    );
    repo.commit("Phase 0 M1: Step 1 — boot");
    let r = open(&repo);
    assert_eq!(
        *r.merged_milestones().expect("merged"),
        BTreeMap::from([(1, 0)])
    );
    assert_eq!(
        r.phase_docs(),
        [
            (0, "docs/phases/00-foundation.md".to_string()),
            (1, "docs/phases/01-memory.md".to_string()),
            (10, "docs/phases/10-late.md".to_string()),
        ]
    );
    assert_eq!(
        r.milestone_sections("docs/phases/00-foundation.md"),
        BTreeMap::from([(1, (11, 20)), (2, (21, 23))])
    );
    assert_eq!(
        r.milestone_sections("docs/phases/01-memory.md"),
        BTreeMap::from([(3, (3, 5))])
    );
    let whole = |rel: &str| (rel.to_string(), None);
    assert_eq!(
        r.current_state_regions().expect("regions"),
        [
            whole(".claude/rules/01-x.md"),
            whole("CLAUDE.md"),
            whole("README.md"),
            whole("docs/project/agent-loop.md"),
            whole("docs/project/developer-guide.md"),
            (
                "docs/phases/00-foundation.md".to_string(),
                Some((11..=20).collect::<BTreeSet<usize>>())
            ),
        ]
    );
}

#[test]
fn merged_milestones_use_the_merge_base_and_the_newest_subject() {
    let repo = TestRepo::with_files("repo-branches", &[("README.md", "# R\n")]);
    repo.commit("Phase 0 M1: Step 1 — boot");
    repo.commit("Phase 3 M1: Step 9 — renumbered later");
    common::git(repo.path(), &["checkout", "-q", "-b", "feature"]);
    repo.commit("Phase 0 M2: Step 2 — feature only");
    common::git(repo.path(), &["checkout", "-q", "main"]);
    repo.commit("Phase 1 M3: Step 1 — main after the fork");
    common::git(repo.path(), &["checkout", "-q", "feature"]);
    assert_eq!(
        *open(&repo).merged_milestones().expect("merged on feature"),
        BTreeMap::from([(1, 3)])
    );
    common::git(repo.path(), &["checkout", "-q", "main"]);
    assert_eq!(
        *open(&repo).merged_milestones().expect("merged on main"),
        BTreeMap::from([(1, 3), (3, 1)])
    );

    let plain = TestRepo::with_files("repo-no-phases", &[("README.md", "# R\n")]);
    assert!(open(&plain)
        .merged_milestones()
        .expect("no Phase subjects")
        .is_empty());
}

#[test]
fn shallow_clone_skips_git_facts() {
    let source = TestRepo::with_files("repo-shallow-src", &[("README.md", "# R\n")]);
    source.commit("Phase 0 M1: Step 1 — one");
    source.commit("Phase 0 M2: Step 2 — two");
    let clone = TestRepo::adopt(common::unique_dir("repo-shallow"));
    let url = format!("file://{}", source.path_str());
    common::git(
        source.path(),
        &[
            "clone",
            "-q",
            "--depth",
            "1",
            url.as_str(),
            clone.path_str(),
        ],
    );
    let r = open(&clone);
    for _ in 0..2 {
        let err = r
            .merged_milestones()
            .err()
            .expect("a shallow clone has no merged milestones");
        let skip = err.downcast_ref::<Skip>().expect("the error is a Skip");
        assert_eq!(
            skip.0,
            "shallow clone: git history unavailable (use fetch-depth: 0)"
        );
    }
    assert_eq!(
        r.current_state_regions().expect("regions"),
        [("README.md".to_string(), None)]
    );
}

#[test]
fn git_checked_and_unchecked() {
    let repo = TestRepo::with_files("repo-git", &[("README.md", "# R\n")]);
    let r = open(&repo);
    let err = r
        .git(&["rev-parse", "--verify", "no-such-ref"])
        .expect_err("an unknown ref fails");
    assert!(
        err.to_string()
            .starts_with("git rev-parse --verify no-such-ref failed: "),
        "{err}"
    );
    assert_eq!(
        r.git_unchecked(&["rev-parse", "--verify", "-q", "no-such-ref"])
            .expect("git runs"),
        ""
    );
    assert_eq!(
        r.git(&["rev-parse", "--is-shallow-repository"])
            .expect("git runs"),
        "false\n"
    );
}
```

Create `tools/src/cmd/docs_check/repo.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        let all: [&LazyLock<Regex>; 6] = [
            &PHASE_SUBJECT_RE,
            &PHASE_DOC_RE,
            &MILESTONE_HEADING_RE,
            &PRIVATE_ATTR_RE,
            &RECIPE_RE,
            &ANCHOR_ID_RE,
        ];
        for rx in all {
            assert!(!LazyLock::force(rx).as_str().is_empty());
        }
    }

    #[test]
    fn recipe_name_rejects_assignments() {
        let cases = [
            ("build:", Some("build")),
            ("check: build", Some("check")),
            ("x := 1", None),
            ("a:=b", None),
            ("docs-check *args:", Some("docs-check")),
            ("@quiet:", Some("quiet")),
            ("name-: x", Some("name")),
            ("# c:", None),
            ("set shell := [\"bash\", \"-c\"]", None),
            ("also-private arg=\"1\":", None),
            ("  indented: x", None),
        ];
        for (line, want) in cases {
            assert_eq!(recipe_name(line).as_deref(), want, "{line:?}");
        }
    }
}
```

In `tools/src/cmd/docs_check/mod.rs` replace the line `pub mod model;` with:

```rust
pub mod model;
pub mod repo;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --test repo`
Expected: FAIL to compile with `error[E0432]: unresolved import` for `aios_tools::cmd::docs_check::repo::Repo` (the module is still empty).

Run: `cargo test -p aios-tools --lib repo`
Expected: FAIL to compile with `error[E0425]` ("cannot find function recipe_name in this scope", "cannot find value RECIPE_RE in this scope").

- [ ] **Step 3: Write the implementation**

Insert this block at the top of `tools/src/cmd/docs_check/repo.rs`, above the `#[cfg(test)]` line from Step 1, with one blank line between the last `}` of the block and `#[cfg(test)]`:

```rust
//! Repository model of `aios docs-check`: check.py `Repo` (L372-526) and `slug_set`
//! (L588-604) at 33c6b3d.
//!
//! Files come from `git ls-files` (tracked plus untracked-but-not-ignored), never
//! from a filesystem walk, so linked worktrees and build output are never scanned.
//! File text is lossy UTF-8 with universal newlines (Python text mode); git output
//! is strict UTF-8 with universal newlines (Python `text=True`). Texts, headings,
//! slugs and the merged milestones are cached per `Repo`.

use std::cell::{OnceCell, RefCell};
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;
use std::rc::Rc;
use std::sync::LazyLock;

use anyhow::{bail, ensure};
use regex::Regex;

use crate::cmd::docs_check::markdown::{self, Heading};
use crate::cmd::docs_check::model::Skip;
use crate::{paths, proc, pystr};

/// Docs that describe the current state of the repository (check.py L52-58).
pub const CURRENT_STATE_DOCS: [&str; 5] = [
    "CLAUDE.md",
    "README.md",
    "CONTRIBUTING.md",
    "docs/project/developer-guide.md",
    "docs/project/agent-loop.md",
];
/// Every Markdown file under this prefix is current-state too (check.py L59).
pub const CURRENT_STATE_PREFIX: &str = ".claude/";

/// Allowed lines of a current-state region; `None` = the whole file.
pub type Region = (String, Option<BTreeSet<usize>>);

const SHALLOW_SKIP: &str = "shallow clone: git history unavailable (use fetch-depth: 0)";

/// Milestone number -> phase, or the reason git history is unavailable.
type Merged = Result<Rc<BTreeMap<u64, u64>>, Skip>;

fn compile(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

/// R19 (check.py L458).
static PHASE_SUBJECT_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^Phase ([0-9]+) M([0-9]+):"));
/// R20 (check.py L467).
static PHASE_DOC_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^docs/phases/([0-9]+)-[^/]+\.md$"));
/// R21 (check.py L478, re.match).
static MILESTONE_HEADING_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^Milestone ([0-9]+)\b"));
/// R22 (check.py L513).
static PRIVATE_ATTR_RE: LazyLock<Regex> = LazyLock::new(|| compile(r"^\[.*\bprivate\b.*\]"));
/// R24 without its negative lookahead (see `recipe_name`).
static RECIPE_RE: LazyLock<Regex> =
    LazyLock::new(|| compile(r"^@?([A-Za-z_][A-Za-z0-9_-]*)\b[^:=]*:"));
/// R26 (check.py L602).
static ANCHOR_ID_RE: LazyLock<Regex> = LazyLock::new(|| compile(r#"<a\s+(?:name|id)="([^"]+)""#));

/// R24, check.py L518 `^@?([A-Za-z_][A-Za-z0-9_-]*)\b[^:=]*:(?!=)`: the recipe name
/// a justfile line defines. `[^:=]*` stops at the first `:` or `=` after the name
/// however the name backtracks, so when that `:` is followed by `=` (an assignment)
/// no other match exists.
pub fn recipe_name(line: &str) -> Option<String> {
    let caps = RECIPE_RE.captures(line)?;
    let end = caps.get(0)?.end();
    if line[end..].starts_with('=') {
        return None;
    }
    Some(caps[1].to_string())
}

/// Runs git in `root`. `check` = check.py's `check=True`: a non-zero exit is an
/// error carrying git's stderr. Stdout must be UTF-8 (Python `text=True` raises
/// otherwise) and gets universal newlines.
fn run_git(root: &str, args: &[&str], check: bool) -> anyhow::Result<String> {
    let output = proc::capture("git", args, Path::new(root))?;
    if check && !output.status.success() {
        let stderr = pystr::decode_text(&output.stderr);
        bail!("git {} failed: {}", args.join(" "), pystr::strip(&stderr));
    }
    if std::str::from_utf8(&output.stdout).is_err() {
        bail!(
            "git {} printed output that is not valid UTF-8",
            args.join(" ")
        );
    }
    Ok(pystr::decode_text(&output.stdout))
}

/// The files of one repository checkout and the facts docs-check derives from them.
pub struct Repo {
    root: String,
    root_real: String,
    files: Vec<String>,
    file_set: HashSet<String>,
    dir_set: HashSet<String>,
    md_files: Vec<String>,
    texts: RefCell<HashMap<String, Rc<str>>>,
    heading_cache: RefCell<HashMap<String, Rc<Vec<Heading>>>>,
    slug_cache: RefCell<HashMap<String, Rc<HashSet<String>>>>,
    merged: OnceCell<Merged>,
}

impl Repo {
    /// check.py `Repo.__init__` (L373-394). `root` is the absolute repository root
    /// (`git rev-parse --show-toplevel`).
    pub fn open(root: &str) -> anyhow::Result<Repo> {
        ensure!(
            root.starts_with('/'),
            "repository root {root} is not an absolute path"
        );
        let listing = run_git(
            root,
            &[
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ],
            true,
        )?;
        let mut file_set: HashSet<String> = HashSet::new();
        let mut files = Vec::new();
        for rel in listing.split('\0') {
            if rel.is_empty() || file_set.contains(rel) {
                continue;
            }
            if std::fs::symlink_metadata(Path::new(root).join(rel)).is_ok() {
                file_set.insert(rel.to_string());
                files.push(rel.to_string());
            }
        }
        files.sort();
        let mut dir_set: HashSet<String> = HashSet::new();
        for file in &files {
            let mut dir = paths::dirname(file);
            while !(dir.is_empty() || dir_set.contains(dir)) {
                dir_set.insert(dir.to_string());
                dir = paths::dirname(dir);
            }
        }
        let md_files = files
            .iter()
            .filter(|f| f.ends_with(".md") && Path::new(root).join(f.as_str()).is_file())
            .cloned()
            .collect();
        Ok(Repo {
            root: root.to_string(),
            root_real: paths::realpath(root, root),
            files,
            file_set,
            dir_set,
            md_files,
            texts: RefCell::new(HashMap::new()),
            heading_cache: RefCell::new(HashMap::new()),
            slug_cache: RefCell::new(HashMap::new()),
            merged: OnceCell::new(),
        })
    }

    /// The repository root as given to `open`.
    pub fn root(&self) -> &str {
        &self.root
    }

    /// Every listed file, sorted.
    pub fn files(&self) -> &[String] {
        &self.files
    }

    /// Listed `.md` files that are regular files (symlinks followed), sorted.
    pub fn md_files(&self) -> &[String] {
        &self.md_files
    }

    /// `rel in file_set`.
    pub fn is_file(&self, rel: &str) -> bool {
        self.file_set.contains(rel)
    }

    /// check.py `Repo.git(..., check=True)` (L396-400).
    pub fn git(&self, args: &[&str]) -> anyhow::Result<String> {
        run_git(&self.root, args, true)
    }

    /// check.py `Repo.git(..., check=False)`: stdout even on a non-zero exit.
    pub fn git_unchecked(&self, args: &[&str]) -> anyhow::Result<String> {
        run_git(&self.root, args, false)
    }

    /// check.py `Repo.text` (L402-409): "" when the file cannot be read.
    pub fn text(&self, rel: &str) -> Rc<str> {
        if let Some(text) = self.texts.borrow().get(rel) {
            return Rc::clone(text);
        }
        let text: Rc<str> = match std::fs::read(Path::new(&self.root).join(rel)) {
            Ok(bytes) => Rc::from(pystr::decode_text(&bytes)),
            Err(_) => Rc::from(""),
        };
        self.texts
            .borrow_mut()
            .insert(rel.to_string(), Rc::clone(&text));
        text
    }

    /// check.py `Repo.headings` (L411-414).
    pub fn headings(&self, rel: &str) -> Rc<Vec<Heading>> {
        if let Some(found) = self.heading_cache.borrow().get(rel) {
            return Rc::clone(found);
        }
        let parsed = Rc::new(markdown::headings(&self.text(rel)));
        self.heading_cache
            .borrow_mut()
            .insert(rel.to_string(), Rc::clone(&parsed));
        parsed
    }

    /// check.py `slug_set` (L591-604): GitHub slugs of the headings (`-1`, `-2` for
    /// repeats) plus every `<a name|id="...">` in the raw text, lowercased.
    pub fn slug_set(&self, rel: &str) -> Rc<HashSet<String>> {
        if let Some(found) = self.slug_cache.borrow().get(rel) {
            return Rc::clone(found);
        }
        let mut slugs = HashSet::new();
        let mut counts: HashMap<String, usize> = HashMap::new();
        for heading in self.headings(rel).iter() {
            let slug = markdown::gh_slug(&heading.text);
            let seen = counts.entry(slug.clone()).or_insert(0);
            let n = *seen;
            *seen += 1;
            slugs.insert(if n == 0 { slug } else { format!("{slug}-{n}") });
        }
        for caps in ANCHOR_ID_RE.captures_iter(&self.text(rel)) {
            slugs.insert(caps[1].to_lowercase());
        }
        let slugs = Rc::new(slugs);
        self.slug_cache
            .borrow_mut()
            .insert(rel.to_string(), Rc::clone(&slugs));
        slugs
    }

    /// check.py `Repo.exists` (L416-426): a listed file or directory, or a path
    /// inside the repository that resolves (symlinks followed) to one.
    pub fn exists(&self, rel: &str) -> bool {
        let rel = rel.trim_end_matches('/');
        if rel.is_empty() || rel == "." {
            return true;
        }
        if self.file_set.contains(rel) || self.dir_set.contains(rel) {
            return true;
        }
        let real = paths::realpath(&paths::join(&self.root, rel), &self.root);
        if !real.starts_with(&format!("{}/", self.root_real)) {
            return false;
        }
        let back = paths::relpath(&real, &self.root_real, &self.root);
        self.file_set.contains(&back) || self.dir_set.contains(&back)
    }

    /// check.py `Repo.resolve` (L428-436): a link target relative to `src`, or
    /// `None` when it leaves the repository.
    pub fn resolve(&self, src: &str, target: &str) -> Option<String> {
        let path = if target.starts_with('/') {
            paths::normpath(target.trim_start_matches('/'))
        } else {
            paths::normpath(&paths::join(paths::dirname(src), target))
        };
        if path.starts_with("..") || path.starts_with('/') {
            return None;
        }
        Some(if path == "." { String::new() } else { path })
    }

    /// check.py `Repo.merged_milestones` (L440-462): milestone number -> phase for
    /// `Phase N MK:` subjects on main's first-parent history (the newest subject
    /// wins). The `Err` downcasts to `Skip` for a shallow clone (cached like the
    /// result); other errors are git failures and are not cached.
    pub fn merged_milestones(&self) -> anyhow::Result<Rc<BTreeMap<u64, u64>>> {
        let cached = match self.merged.get() {
            Some(cached) => cached,
            None => {
                let computed = self.read_merged_milestones()?;
                self.merged.get_or_init(|| computed)
            }
        };
        cached.clone().map_err(anyhow::Error::new)
    }

    fn read_merged_milestones(&self) -> anyhow::Result<Merged> {
        let shallow = self.git_unchecked(&["rev-parse", "--is-shallow-repository"])?;
        if pystr::strip(&shallow) == "true" {
            return Ok(Err(Skip(SHALLOW_SKIP.to_string())));
        }
        let mut base = "HEAD".to_string();
        for reference in ["origin/main", "main"] {
            let verified = self.git_unchecked(&["rev-parse", "--verify", "-q", reference])?;
            if pystr::strip(&verified).is_empty() {
                continue;
            }
            let merge_base = self.git_unchecked(&["merge-base", "HEAD", reference])?;
            let merge_base = pystr::strip(&merge_base);
            if !merge_base.is_empty() {
                base = merge_base.to_string();
                break;
            }
        }
        let log = self.git_unchecked(&["log", "--first-parent", "--format=%s", &base])?;
        let mut merged = BTreeMap::new();
        for subject in pystr::splitlines(&log) {
            let Some(caps) = PHASE_SUBJECT_RE.captures(subject) else {
                continue;
            };
            if let (Some(phase), Some(milestone)) =
                (pystr::parse_uint(&caps[1]), pystr::parse_uint(&caps[2]))
            {
                merged.entry(milestone).or_insert(phase);
            }
        }
        Ok(Ok(Rc::new(merged)))
    }

    /// check.py `Repo.phase_docs` (L464-470): `(phase number, path)`, sorted.
    pub fn phase_docs(&self) -> Vec<(u64, String)> {
        let mut out: Vec<(u64, String)> = self
            .md_files
            .iter()
            .filter_map(|f| {
                let caps = PHASE_DOC_RE.captures(f)?;
                Some((pystr::parse_uint(&caps[1])?, f.clone()))
            })
            .collect();
        out.sort();
        out
    }

    /// check.py `Repo.milestone_sections` (L472-486): milestone number -> (first,
    /// last line) of its `## Milestone N` section; a later duplicate wins.
    pub fn milestone_sections(&self, rel: &str) -> BTreeMap<u64, (usize, usize)> {
        let line_count = pystr::splitlines(&self.text(rel)).len();
        let starts: Vec<(usize, Option<u64>)> = self
            .headings(rel)
            .iter()
            .filter(|h| h.level == 2)
            .map(|h| {
                let num = MILESTONE_HEADING_RE
                    .captures(&h.text)
                    .and_then(|caps| pystr::parse_uint(&caps[1]));
                (h.line, num)
            })
            .collect();
        let mut out = BTreeMap::new();
        for (i, &(line, num)) in starts.iter().enumerate() {
            let Some(num) = num else {
                continue;
            };
            let end = starts.get(i + 1).map_or(line_count, |next| next.0 - 1);
            out.insert(num, (line, end));
        }
        out
    }

    /// check.py `Repo.current_state_regions` (L488-505): current-state docs (whole
    /// files, in `md_files` order), then the merged-milestone sections of each
    /// phase doc. A shallow clone means "no merged milestones" here.
    pub fn current_state_regions(&self) -> anyhow::Result<Vec<Region>> {
        let mut regions: Vec<Region> = self
            .md_files
            .iter()
            .filter(|f| {
                CURRENT_STATE_DOCS.contains(&f.as_str()) || f.starts_with(CURRENT_STATE_PREFIX)
            })
            .map(|f| (f.clone(), None))
            .collect();
        let merged = match self.merged_milestones() {
            Ok(merged) => merged,
            Err(err) if err.is::<Skip>() => Rc::new(BTreeMap::new()),
            Err(err) => return Err(err),
        };
        for (_, rel) in self.phase_docs() {
            let mut allowed = BTreeSet::new();
            for (num, (lo, hi)) in self.milestone_sections(&rel) {
                if merged.contains_key(&num) {
                    allowed.extend(lo..=hi);
                }
            }
            if !allowed.is_empty() {
                regions.push((rel, Some(allowed)));
            }
        }
        Ok(regions)
    }

    /// check.py `Repo.justfile_recipes` (L507-526): (all recipe names, public
    /// names). `[private]` marks the next recipe; attributes and comments between
    /// keep the mark; names starting with `_` are private too.
    pub fn justfile_recipes(&self) -> (BTreeSet<String>, BTreeSet<String>) {
        let mut names = BTreeSet::new();
        let mut public = BTreeSet::new();
        let mut private_next = false;
        let text = self.text("justfile");
        for line in pystr::splitlines(&text) {
            if PRIVATE_ATTR_RE.is_match(line) {
                private_next = true;
                continue;
            }
            if line.starts_with('[') {
                continue;
            }
            if let Some(name) = recipe_name(line) {
                if !line.starts_with([' ', '\t', '#']) {
                    if !(private_next || name.starts_with('_')) {
                        public.insert(name.clone());
                    }
                    names.insert(name);
                }
            }
            if !(pystr::strip(line).is_empty() || line.starts_with('#')) {
                private_next = false;
            }
        }
        (names, public)
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --lib repo`
Expected: PASS: `cmd::docs_check::repo::tests::regexes_compile` and `cmd::docs_check::repo::tests::recipe_name_rejects_assignments` pass, `test result: ok.`

Run: `cargo test -p aios-tools --test repo`
Expected: PASS: `test result: ok. 9 passed; 0 failed` (`listing_follows_git_ls_files`, `resolve_stays_inside_the_repository`, `text_is_lossy_utf8_with_universal_newlines`, `headings_and_slugs`, `justfile_recipes_track_private_attributes`, `phase_docs_sections_and_current_state_regions`, `merged_milestones_use_the_merge_base_and_the_newest_subject`, `shallow_clone_skips_git_facts`, `git_checked_and_unchecked`).

- [ ] **Step 5: Format and lint**

Run: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools -- -D warnings`
Expected: no diff from `cargo fmt`; clippy ends with `Finished` and prints no warning.

- [ ] **Step 6: Commit and push**

```bash
git add tools/src/cmd/docs_check/repo.rs tools/src/cmd/docs_check/mod.rs tools/tests/repo.rs
git commit -F - <<'MSG'
Port the docs-check repository model to Rust

repo.rs ports check.py's Repo (L372-526) and slug_set (L588-604): the
git ls-files listing, exists/resolve with non-strict realpath, cached
texts, headings and slugs, merged milestones from main's first-parent
history (Skip on a shallow clone), phase-doc sections, current-state
regions and justfile recipes. R24's negative lookahead is rewritten as
code in recipe_name. The integration tests replay check.py's results on
temporary repositories.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 6: Output, check registry, run pipeline, CLI, `just tools`

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/output.rs` (renderers, then unit tests)
- Create: `tools/src/cmd/docs_check/checks/mod.rs` (`Check` trait, empty `registry()`)
- Create: `tools/src/main.rs` (the `aios` binary)
- Create: `tools/tests/cli.rs`
- Modify: `tools/src/cmd/docs_check/mod.rs` (whole file: contract `§4.9`)
- Modify: `tools/Cargo.toml` (add the `[[bin]]` table)
- Modify: `tools/tests/common/mod.rs` (append `Run`, `run_aios`)
- Modify: `justfile` (add the `tools` recipe after `test`, contract `§6.3` T6)
- Test: `tools/tests/cli.rs`, `tools/src/cmd/docs_check/output.rs`

**Interfaces:**
- Consumes:
  - Task 1: `aios_tools::pystr::strip`, `aios_tools::paths::{join, relpath}`, `aios_tools::proc::capture`.
  - Task 3 (`aios_tools::cmd::docs_check::model`):

    ```rust
    pub const BASELINE_REL: &str = "scripts/docs/baseline.json";
    pub fn description(check: &str) -> &'static str;
    pub struct Finding { pub check: &'static str, pub file: String, pub target: String, pub message: String, pub line: usize, pub also: Vec<usize>, pub detail: String }
    impl Finding { pub fn new(check: &'static str, file: impl Into<String>, target: impl Into<String>, message: impl Into<String>, line: usize) -> Finding; pub fn with_detail(self, detail: impl Into<String>) -> Finding; pub fn key(&self) -> String; pub fn count(&self) -> usize; pub fn location(&self) -> String; pub fn text(&self) -> String; }
    pub struct Skip(pub String);
    pub fn collate(findings: Vec<Finding>) -> Vec<Finding>;
    pub type Baseline = serde_json::Map<String, serde_json::Value>;
    pub fn load_baseline(path: &std::path::Path, display: &str) -> anyhow::Result<Baseline>;
    pub fn baseline_count(entry: &serde_json::Value) -> anyhow::Result<i64>;
    pub fn py_str(v: &serde_json::Value) -> String;
    pub fn updated_baseline(findings: &[Finding], baseline: &Baseline, ran: &BTreeSet<&str>) -> Baseline;
    pub fn write_baseline(path: &std::path::Path, entries: &Baseline) -> anyhow::Result<()>;
    #[derive(Debug, Clone, Default, PartialEq)]
    pub struct Comparison { pub new_keys: BTreeSet<String>, pub grown: BTreeMap<String, i64>, pub accepted: BTreeMap<String, serde_json::Value>, pub resolved: Vec<String>, pub reduced: BTreeMap<String, (i64, i64)> }
    pub fn compare(findings: &[Finding], baseline: &Baseline, ran: &BTreeSet<&str>) -> anyhow::Result<Comparison>;
    ```

  - Task 5: `aios_tools::cmd::docs_check::repo::Repo` with `pub fn open(root: &str) -> anyhow::Result<Repo>` (the other methods as listed in Task 5).
  - Task 2 test helpers: `unique_dir`, `isolated(&mut Command) -> &mut Command`, `TestRepo::{with_files, adopt, path, path_str}`.
- Produces (Tasks 7-16 rely on these names):

  ```rust
  // aios_tools::cmd::docs_check::checks (contract §4.7)
  pub trait Check {
      fn name(&self) -> &'static str;
      fn describe(&self) -> &'static str { model::description(self.name()) }
      fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>>;
  }
  pub fn registry() -> Vec<Box<dyn Check>>;   // empty here; Tasks 7-12 append in CHECK_ORDER
  // aios_tools::cmd::docs_check::output (contract §4.8)
  pub struct Report<'a> { pub findings: &'a [Finding], pub cmp: &'a Comparison, pub skipped: &'a BTreeMap<&'static str, String>, pub names: &'a [&'static str], pub baseline: &'a Baseline, pub baseline_rel: &'a str }
  pub fn describe(f: &Finding, cmp: &Comparison) -> String;
  pub fn prune_notes(cmp: &Comparison, limit: usize) -> Vec<String>;
  pub fn render_text(r: &Report<'_>, show_all: bool) -> String;              // no final "\n"
  pub fn render_markdown(r: &Report<'_>) -> String;                           // ends with "\n"
  pub fn render_json(r: &Report<'_>, show_all: bool) -> anyhow::Result<String>; // no final "\n"
  pub fn render_list_checks(checks: &[Box<dyn Check>]) -> String;             // each line ends "\n"
  // aios_tools::cmd::docs_check (contract §4.9, plus select_from and run_with)
  pub struct Args { pub all: bool, pub json: bool, pub markdown: bool, pub check: String, pub baseline: Option<String>, pub update_baseline: bool, pub list_checks: bool }
  pub struct CheckRun { pub findings: Vec<Finding>, pub skipped: BTreeMap<&'static str, String> }
  pub fn select_checks(check_arg: &str) -> anyhow::Result<Vec<Box<dyn Check>>>;
  pub fn select_from(available: Vec<Box<dyn Check>>, check_arg: &str) -> anyhow::Result<Vec<Box<dyn Check>>>;
  pub fn run_checks(repo: &Repo, checks: &[Box<dyn Check>]) -> anyhow::Result<CheckRun>;
  pub fn repo_root(cwd: &Path) -> anyhow::Result<String>;
  pub fn run(args: &Args, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<u8>;
  pub fn run_with(args: &Args, cwd: &Path, available: Vec<Box<dyn Check>>, out: &mut dyn Write) -> anyhow::Result<u8>;
  // tools/tests/common/mod.rs (contract §4.11, T6 part)
  pub struct Run { pub code: i32, pub stdout: Vec<u8>, pub stderr: Vec<u8> }
  pub fn run_aios(cwd: &std::path::Path, args: &[&str]) -> Run;
  ```

  Binary `aios` (`aios docs-check [--all] [--json] [--markdown] [--check X] [--baseline F] [--update-baseline] [--list-checks]`, exit 0/1/2) and the recipe `just tools` (builds `target/tools/release/aios`).

**Notes (check.py at `33c6b3d`; re-read each range while porting):**
- Port map: `run_checks` L1442-1456 (merging and sorting live in Task 3's `collate`; `run_checks` concatenates each check's findings in registry order and records a `Skip` as skipped); `describe` L1488-1494; `prune_notes` L1497-1502; `render_text` L1505-1542; `render_markdown` L1545-1573; `repo_root` L1576-1582; `main` L1585-1662 (argparse, order of operations, update message, JSON document and exit status); the crash handler L1665-1670 is `main.rs` printing `docs-check: {err:#}` and exiting 2.
- Order of operations (contract `§1.2`), kept by `run_with`: `--list-checks` first (no validation, no git); then `--check` validation (so an unknown name exits 2 even outside a repository); then the git root; `--baseline ""` counts as absent; the baseline path is shown relative to the root (`paths::relpath` against `cwd`) and opened relative to `cwd`; checks run before the baseline is read; `--update-baseline` wins over the output flags; `--json` wins over `--markdown`; exit 1 only on new keys.
- CLI parity: clap's `infer_long_args` and `args_override_self` on the subcommand reproduce argparse's abbreviations (`--che`, `--j`) and repeated flags; clap's own usage errors exit 2 like argparse's. Stderr differs by design (argparse prints usage plus `check.py: error: ...`; `aios` prints `docs-check: unknown check(s): ... (see --list-checks)`); goldens record stdout only.
- Output: `render_text`/`render_json` omit Python `print`'s newline and `run_with` adds it; `render_markdown` already ends with one. `render_json` serializes `#[derive(Serialize)]` structs in check.py's key order with `checks` and `reduced` as insertion-ordered `serde_json::Map` (`preserve_order`), via `to_string_pretty` (same bytes as `json.dumps(indent=2, ensure_ascii=False)` for this data). The unit tests' expected strings were produced by check.py's own `render_text`, `render_markdown` and JSON code on the same findings; the CLI tests' bytes by running check.py (with `CHECK_FUNCS` replaced by the same stand-in checks for the pipeline cases) in a scratch repository.
- `--list-checks` prints the registry, which is empty until Task 7; Task 12 asserts the full 15-line text. `select_checks` validates against the registry, so until Task 12 names of checks not yet ported are reported as unknown.
- Contract gaps filled here: `select_from` and `run_with` take the available checks explicitly (`select_checks` and `run` pass `checks::registry()`), so the pipeline is tested with stand-in checks before any real check exists; the `Args` struct carries a plain comment instead of a doc comment so clap does not show it in `--help`; `main.rs` is contract `§4.10` plus a crate doc line.
- Interim docs drift: the new public recipe `tools` is not in the README and developer-guide build tables until Task 15, so `just docs-check` (still check.py until Task 15) reports one new finding, `just-recipes|justfile|undocumented:tools`. That is expected; do not baseline it.

- [ ] **Step 1: Write the failing tests**

Append to the end of `tools/tests/common/mod.rs`, after one blank line (`isolated` is already defined in this module):

```rust
/// Exit code and output of one `aios` run.
pub struct Run {
    pub code: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Runs the `aios` binary under test in `cwd` with `args` as given, in the
/// isolated environment. A run killed by a signal reports code -1.
pub fn run_aios(cwd: &std::path::Path, args: &[&str]) -> Run {
    let mut cmd = std::process::Command::new(env!("CARGO_BIN_EXE_aios"));
    isolated(&mut cmd).current_dir(cwd).args(args);
    let out = cmd.output().expect("run the aios binary");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}
```

Create `tools/tests/cli.rs`:

```rust
//! `aios docs-check` command line and pipeline: usage errors, the repository root,
//! the output formats with zero checks, baseline handling, and `run_checks` /
//! `run_with` on stand-in checks. Expected bytes were recorded from check.py
//! (33c6b3d) on the same inputs; its `CHECK_FUNCS` was replaced by the same
//! stand-ins for the pipeline cases.

mod common;

use std::path::Path;
use std::process::Command;

use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::output::render_list_checks;
use aios_tools::cmd::docs_check::repo::Repo;
use aios_tools::cmd::docs_check::{run_checks, run_with, select_from, Args};
use common::{run_aios, Run, TestRepo};

const ZERO_TEXT: &str = concat!(
    "docs-check: 0 findings across 0 checks - 0 new, 0 baselined (0 accepted false positives), ",
    "0 resolved (baseline scripts/docs/baseline.json)\n",
    "\n",
    "  check              total   new\n",
    "\n",
    "No new drift since baseline.\n",
);

const ZERO_JSON: &str = r##"{
  "baseline": "scripts/docs/baseline.json",
  "summary": {
    "total": 0,
    "new": 0,
    "baselined": 0,
    "accepted": 0,
    "resolved": 0,
    "reduced": 0
  },
  "checks": {},
  "findings": [],
  "resolved": [],
  "reduced": {}
}
"##;

const ZERO_MARKDOWN: &str = concat!(
    "## Docs drift check\n",
    "\n",
    "**0 new** finding(s) since baseline; 0 total (0 accepted false positives), 0 resolved. ",
    "Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.\n",
    "\n",
    "| Check | Total | New |\n",
    "|---|---:|---:|\n",
);

const HAND_BASELINE: &str = r##"{
  "comment": "hand written",
  "version": 1,
  "counts": {"md-links": 7},
  "findings": [
    {"key": "retired|README.md|x", "check": "retired", "file": "README.md", "target": "x", "message": "kept", "note": "extra"},
    {"key": "md-links|b.md|é.md", "check": "md-links", "file": "b.md", "target": "é.md", "message": "broken link -> é.md", "count": 2},
    {"key": "anchors|a.md|#x", "check": "anchors", "file": "a.md", "target": "#x", "message": "no heading", "reason": "accepted"}
  ]
}
"##;

const HAND_REWRITTEN: &str = r##"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 1,
    "anchors": 1
  },
  "findings": [
    {
      "key": "md-links|b.md|é.md",
      "check": "md-links",
      "file": "b.md",
      "target": "é.md",
      "message": "broken link -> é.md",
      "count": 2
    },
    {
      "key": "anchors|a.md|#x",
      "check": "anchors",
      "file": "a.md",
      "target": "#x",
      "message": "no heading",
      "reason": "accepted"
    },
    {
      "key": "retired|README.md|x",
      "check": "retired",
      "file": "README.md",
      "target": "x",
      "message": "kept",
      "note": "extra"
    }
  ]
}
"##;

const PIPELINE_BASELINE: &str = r##"{
  "comment": "pipeline fixture",
  "version": 1,
  "counts": {},
  "findings": [
    {"key": "milestone-status|README.md|latest:M3", "check": "milestone-status", "file": "README.md", "target": "latest:M3", "message": "README status does not mention the latest merged milestone M3"},
    {"key": "md-links|docs/old.md|z.md", "check": "md-links", "file": "docs/old.md", "target": "z.md", "message": "broken link -> z.md"},
    {"key": "anchors|docs/a.md|#gone", "check": "anchors", "file": "docs/a.md", "target": "#gone", "message": "no heading for anchor #gone in docs/a.md", "reason": "false positive"},
    {"key": "md-links|README.md|y.md", "check": "md-links", "file": "README.md", "target": "y.md", "message": "broken link -> y.md"}
  ]
}
"##;

const PIPELINE_TEXT: &str = r##"docs-check: 3 findings across 2 checks - 1 new, 2 baselined (1 accepted false positives), 1 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               2     1
  anchors                1     0
  milestone-status    skip        no 'Phase N MK:' commits found on main's first-parent history

New drift since baseline:

[md-links]
 + docs/a.md:7 (also 3): broken link -> x.md

1 baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:
  - md-links|docs/old.md|z.md
"##;

const PIPELINE_REWRITTEN: &str = r##"{
  "comment": "Accepted docs drift. A finding is new when its key is missing here or it occurs on more lines than 'count' (default 1). 'reason' marks an accepted false positive and survives regeneration. Regenerate with: just docs-check --update-baseline",
  "version": 1,
  "counts": {
    "md-links": 2,
    "anchors": 1,
    "milestone-status": 1
  },
  "findings": [
    {
      "key": "md-links|README.md|y.md",
      "check": "md-links",
      "file": "README.md",
      "target": "y.md",
      "message": "broken link -> y.md"
    },
    {
      "key": "md-links|docs/a.md|x.md",
      "check": "md-links",
      "file": "docs/a.md",
      "target": "x.md",
      "message": "broken link -> x.md",
      "count": 2
    },
    {
      "key": "anchors|docs/a.md|#gone",
      "check": "anchors",
      "file": "docs/a.md",
      "target": "#gone",
      "message": "no heading for anchor #gone in docs/a.md",
      "reason": "false positive"
    },
    {
      "key": "milestone-status|README.md|latest:M3",
      "check": "milestone-status",
      "file": "README.md",
      "target": "latest:M3",
      "message": "README status does not mention the latest merged milestone M3"
    }
  ]
}
"##;

const NO_PHASE_HISTORY: &str = "no 'Phase N MK:' commits found on main's first-parent history";

fn text(bytes: &[u8]) -> &str {
    std::str::from_utf8(bytes).expect("UTF-8 output")
}

/// Runs aios in `dir` with git unable to find a repository above it.
fn aios_outside_git(dir: &Path, args: &[&str]) -> Run {
    let ceiling = dir.parent().expect("unique_dir has a parent");
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aios"));
    common::isolated(&mut cmd)
        .current_dir(dir)
        .env("GIT_CEILING_DIRECTORIES", ceiling)
        .args(args);
    let out = cmd.output().expect("run aios");
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}

struct Fixed {
    name: &'static str,
    findings: Vec<Finding>,
}

impl Check for Fixed {
    fn name(&self) -> &'static str {
        self.name
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Ok(self.findings.clone())
    }
}

struct Skipped(&'static str);

impl Check for Skipped {
    fn name(&self) -> &'static str {
        self.0
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Err(Skip(NO_PHASE_HISTORY.to_string()).into())
    }
}

struct Failing;

impl Check for Failing {
    fn name(&self) -> &'static str {
        "md-links"
    }

    fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        Err(anyhow::anyhow!("stand-in failure"))
    }
}

/// md-links reports x.md on lines 7, 3, 7 and 0 (merged into 7, also 3) and y.md;
/// anchors reports #gone; milestone-status is skipped.
fn standins() -> Vec<Box<dyn Check>> {
    let x = |line| Finding::new("md-links", "docs/a.md", "x.md", "broken link -> x.md", line);
    vec![
        Box::new(Fixed {
            name: "md-links",
            findings: vec![
                x(7),
                x(3),
                x(7),
                x(0),
                Finding::new("md-links", "README.md", "y.md", "broken link -> y.md", 2),
            ],
        }),
        Box::new(Fixed {
            name: "anchors",
            findings: vec![Finding::new(
                "anchors",
                "docs/a.md",
                "#gone",
                "no heading for anchor #gone in docs/a.md",
                4,
            )],
        }),
        Box::new(Skipped("milestone-status")),
    ]
}

fn names(checks: &[Box<dyn Check>]) -> Vec<&'static str> {
    checks.iter().map(|check| check.name()).collect()
}

#[test]
fn unknown_checks_fail_before_any_git_call() {
    let dir = common::unique_dir("cli-unknown");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check", "--check", "bogus,nope,bogus"]);
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert_eq!(
        text(&run.stderr),
        "docs-check: unknown check(s): bogus, nope, bogus (see --list-checks)\n"
    );
}

#[test]
fn outside_a_repository_exits_2() {
    let dir = common::unique_dir("cli-norepo");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check"]);
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert_eq!(
        text(&run.stderr),
        "docs-check: not inside a git repository\n"
    );
}

#[test]
fn list_checks_prints_the_registry_without_git() {
    let dir = common::unique_dir("cli-list");
    let _cleanup = TestRepo::adopt(dir.clone());
    let run = aios_outside_git(&dir, &["docs-check", "--list-checks"]);
    assert_eq!(run.code, 0);
    assert_eq!(text(&run.stdout), render_list_checks(&registry()));
    assert_eq!(text(&run.stderr), "");
}

#[test]
fn clap_usage_errors_exit_2() {
    let repo = TestRepo::with_files("cli-clap", &[("README.md", "# R\n")]);
    let run = run_aios(repo.path(), &["docs-check", "--no-such-flag"]);
    assert_eq!(run.code, 2);
    assert!(run.stdout.is_empty());
}

#[test]
fn zero_checks_render_every_format() {
    let repo = TestRepo::with_files("cli-zero", &[("README.md", "# R\n")]);
    let plain = run_aios(repo.path(), &["docs-check", "--check", ","]);
    assert_eq!((plain.code, text(&plain.stdout)), (0, ZERO_TEXT));
    let json = run_aios(
        repo.path(),
        &[
            "docs-check",
            "--che",
            ",",
            "--j",
            "--json",
            "--all",
            "--all",
        ],
    );
    assert_eq!((json.code, text(&json.stdout)), (0, ZERO_JSON));
    let markdown = run_aios(repo.path(), &["docs-check", "--check", " , ", "--markdown"]);
    assert_eq!((markdown.code, text(&markdown.stdout)), (0, ZERO_MARKDOWN));
}

#[test]
fn baseline_path_is_shown_relative_to_the_root() {
    let repo = TestRepo::with_files(
        "cli-baseline-rel",
        &[("README.md", "# R\n"), ("sub/keep.txt", "k\n")],
    );
    let run = run_aios(
        &repo.path().join("sub"),
        &[
            "docs-check",
            "--baseline",
            "../other/base.json",
            "--check",
            ",",
        ],
    );
    assert_eq!(run.code, 0);
    assert_eq!(
        text(&run.stdout),
        ZERO_TEXT.replace("scripts/docs/baseline.json", "other/base.json")
    );
    let empty = run_aios(
        repo.path(),
        &["docs-check", "--baseline", "", "--check", ","],
    );
    assert_eq!(text(&empty.stdout), ZERO_TEXT);
}

#[test]
fn unreadable_baseline_exits_2() {
    let repo = TestRepo::with_files("cli-bad-baseline", &[("other/bad.json", "{")]);
    let run = run_aios(
        repo.path(),
        &["docs-check", "--baseline", "other/bad.json", "--check", ","],
    );
    assert_eq!(run.code, 2);
    assert_eq!(text(&run.stdout), "");
    assert!(
        text(&run.stderr).starts_with("docs-check: cannot read baseline other/bad.json: "),
        "{}",
        text(&run.stderr)
    );
}

#[test]
fn update_baseline_with_zero_checks_keeps_and_sorts_every_entry() {
    let repo = TestRepo::with_files(
        "cli-update-zero",
        &[
            ("README.md", "# R\n"),
            ("scripts/docs/baseline.json", HAND_BASELINE),
        ],
    );
    let run = run_aios(
        repo.path(),
        &["docs-check", "--check", ",", "--update-baseline"],
    );
    assert_eq!(
        (run.code, text(&run.stdout)),
        (
            0,
            "docs-check: wrote 3 findings to scripts/docs/baseline.json\n"
        )
    );
    let written = std::fs::read_to_string(repo.path().join("scripts/docs/baseline.json"))
        .expect("read the rewritten baseline");
    assert_eq!(written, HAND_REWRITTEN);
}

#[test]
fn select_from_orders_deduplicates_and_rejects_unknown_names() {
    assert_eq!(
        names(&select_from(standins(), "").expect("empty selects all")),
        ["md-links", "anchors", "milestone-status"]
    );
    assert_eq!(
        names(&select_from(standins(), "milestone-status, md-links,md-links").expect("known")),
        ["md-links", "milestone-status"]
    );
    assert!(select_from(standins(), " , ").expect("no names").is_empty());
    let err = select_from(standins(), "anchors,bogus,,nope")
        .err()
        .expect("unknown names are an error");
    assert_eq!(
        err.to_string(),
        "unknown check(s): bogus, nope (see --list-checks)"
    );
}

#[test]
fn run_checks_merges_keys_and_collects_skips() {
    let repo = TestRepo::with_files("cli-run-checks", &[("README.md", "# R\n")]);
    let r = Repo::open(repo.path_str()).expect("open the test repository");
    let run = run_checks(&r, &standins()).expect("stand-in checks run");
    let got: Vec<(String, usize, Vec<usize>)> = run
        .findings
        .iter()
        .map(|f| (f.key(), f.line, f.also.clone()))
        .collect();
    assert_eq!(
        got,
        [
            ("md-links|README.md|y.md".to_string(), 2, vec![]),
            ("md-links|docs/a.md|x.md".to_string(), 7, vec![3]),
            ("anchors|docs/a.md|#gone".to_string(), 4, vec![]),
        ]
    );
    assert_eq!(run.skipped.len(), 1);
    assert_eq!(run.skipped["milestone-status"], NO_PHASE_HISTORY);

    let failing: Vec<Box<dyn Check>> = vec![Box::new(Failing)];
    let err = run_checks(&r, &failing)
        .err()
        .expect("a failing check fails the run");
    assert_eq!(err.to_string(), "stand-in failure");
}

#[test]
fn run_with_matches_check_py_on_stand_in_checks() {
    let repo = TestRepo::with_files(
        "cli-pipeline",
        &[
            ("README.md", "# R\n"),
            ("scripts/docs/baseline.json", PIPELINE_BASELINE),
        ],
    );
    let args = Args {
        check: "milestone-status,anchors,md-links".to_string(),
        ..Args::default()
    };
    let mut out = Vec::new();
    let code = run_with(&args, repo.path(), standins(), &mut out).expect("the pipeline runs");
    assert_eq!(code, 1);
    assert_eq!(text(&out), PIPELINE_TEXT);

    let update = Args {
        update_baseline: true,
        ..args
    };
    let mut out = Vec::new();
    let code = run_with(&update, repo.path(), standins(), &mut out).expect("the update runs");
    assert_eq!(code, 0);
    assert_eq!(
        text(&out),
        "docs-check: wrote 4 findings to scripts/docs/baseline.json (skipped: milestone-status)\n"
    );
    let written = std::fs::read_to_string(repo.path().join("scripts/docs/baseline.json"))
        .expect("read the rewritten baseline");
    assert_eq!(written, PIPELINE_REWRITTEN);
}
```

Create `tools/src/cmd/docs_check/output.rs` containing only this test module:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::docs_check::repo::Repo;

    // Expected strings were recorded from check.py's render_text, render_markdown and
    // main()'s JSON document (33c6b3d) on the same findings and comparison.

    fn finding(
        check: &'static str,
        file: &str,
        target: &str,
        message: &str,
        line: usize,
        also: &[usize],
    ) -> Finding {
        let mut f = Finding::new(check, file, target, message, line);
        f.also = also.to_vec();
        f
    }

    fn sample_findings() -> Vec<Finding> {
        vec![
            finding(
                "md-links",
                "docs/a.md",
                "x.md",
                "broken link -> x.md",
                3,
                &[9],
            ),
            finding(
                "md-links",
                "docs/b.md",
                "old.md",
                "broken link -> old.md",
                5,
                &[6, 7],
            ),
            finding(
                "anchors",
                "docs/a.md",
                "#gone",
                "no heading for anchor #gone in docs/a.md",
                4,
                &[],
            ),
            finding(
                "pointer-doctor",
                ".claude/agents/w.md",
                "path:docs/r.md",
                "path does not exist: docs/r.md",
                0,
                &[],
            )
            .with_detail("cited on purpose"),
        ]
    }

    fn sample_cmp() -> Comparison {
        Comparison {
            new_keys: ["md-links|docs/a.md|x.md", "md-links|docs/b.md|old.md"]
                .into_iter()
                .map(String::from)
                .collect(),
            grown: BTreeMap::from([("md-links|docs/b.md|old.md".to_string(), 2)]),
            accepted: BTreeMap::from([(
                "anchors|docs/a.md|#gone".to_string(),
                Value::from("false positive: generated anchor"),
            )]),
            resolved: vec!["md-links|docs/c.md|gone.md".to_string()],
            reduced: BTreeMap::from([(
                "pointer-doctor|.claude/agents/w.md|path:docs/r.md".to_string(),
                (2, 1),
            )]),
        }
    }

    fn sample_baseline() -> Baseline {
        [
            json!({"key": "md-links|docs/b.md|old.md", "check": "md-links", "count": 2}),
            json!({"key": "anchors|docs/a.md|#gone", "check": "anchors",
                   "reason": "false positive: generated anchor"}),
            json!({"key": "pointer-doctor|.claude/agents/w.md|path:docs/r.md",
                   "check": "pointer-doctor", "count": 2}),
            json!({"key": "md-links|docs/c.md|gone.md", "check": "md-links"}),
            json!({"key": "milestone-status|README.md|latest:M9", "check": "milestone-status"}),
        ]
        .into_iter()
        .map(|entry| (entry["key"].as_str().unwrap_or_default().to_string(), entry))
        .collect()
    }

    fn sample_skipped() -> BTreeMap<&'static str, String> {
        BTreeMap::from([(
            "milestone-status",
            "no 'Phase N MK:' commits found on main's first-parent history".to_string(),
        )])
    }

    const NAMES: [&str; 4] = ["md-links", "anchors", "milestone-status", "pointer-doctor"];

    const TEXT_HEAD: &str = r"docs-check: 4 findings across 3 checks - 2 new, 2 baselined (1 accepted false positives), 1 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               2     2
  anchors                1     0
  milestone-status    skip        no 'Phase N MK:' commits found on main's first-parent history
  pointer-doctor         1     0
";

    const TEXT_PRUNE: &str = r"
2 baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:
  - md-links|docs/c.md|gone.md
  - pointer-doctor|.claude/agents/w.md|path:docs/r.md (2 -> 1 occurrences)";

    const TEXT_NEW: &str = r"
New drift since baseline:

[md-links]
 + docs/a.md:3 (also 9): broken link -> x.md
 + docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]
";

    const TEXT_ALL: &str = r"
All findings ('+' = new since baseline, '~' = accepted false positive):

[md-links]
 + docs/a.md:3 (also 9): broken link -> x.md
 + docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]

[anchors]
 ~ docs/a.md:4: no heading for anchor #gone in docs/a.md [accepted: false positive: generated anchor]

[pointer-doctor]
   .claude/agents/w.md: path does not exist: docs/r.md (cited on purpose)
";

    const MARKDOWN: &str = r"## Docs drift check

**2 new** finding(s) since baseline; 4 total (1 accepted false positives), 1 resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.

| Check | Total | New |
|---|---:|---:|
| md-links | 2 | 2 |
| anchors | 1 | 0 |
| milestone-status | skipped | no 'Phase N MK:' commits found on main's first-parent history |
| pointer-doctor | 1 | 0 |

### New findings

- `md-links` docs/a.md:3 (also 9): broken link -> x.md
- `md-links` docs/b.md:5 (also 6, 7): broken link -> old.md [3 occurrences, baseline 2]

2 baselined finding(s) no longer occur or occur less often (run `just docs-check --update-baseline`).
";

    const JSON_HEAD: &str = r##"{
  "baseline": "scripts/docs/baseline.json",
  "summary": {
    "total": 4,
    "new": 2,
    "baselined": 2,
    "accepted": 1,
    "resolved": 1,
    "reduced": 1
  },
  "checks": {
    "md-links": {
      "total": 2,
      "new": 2
    },
    "anchors": {
      "total": 1,
      "new": 0
    },
    "milestone-status": {
      "skipped": "no 'Phase N MK:' commits found on main's first-parent history"
    },
    "pointer-doctor": {
      "total": 1,
      "new": 0
    }
  },
  "findings": [
    {
      "key": "md-links|docs/a.md|x.md",
      "check": "md-links",
      "file": "docs/a.md",
      "line": 3,
      "also": [
        9
      ],
      "count": 2,
      "baseline_count": null,
      "target": "x.md",
      "message": "broken link -> x.md",
      "detail": "",
      "new": true,
      "accepted": null
    },
    {
      "key": "md-links|docs/b.md|old.md",
      "check": "md-links",
      "file": "docs/b.md",
      "line": 5,
      "also": [
        6,
        7
      ],
      "count": 3,
      "baseline_count": 2,
      "target": "old.md",
      "message": "broken link -> old.md",
      "detail": "",
      "new": true,
      "accepted": null
    }"##;

    const JSON_BASELINED: &str = r##",
    {
      "key": "anchors|docs/a.md|#gone",
      "check": "anchors",
      "file": "docs/a.md",
      "line": 4,
      "also": [],
      "count": 1,
      "baseline_count": 1,
      "target": "#gone",
      "message": "no heading for anchor #gone in docs/a.md",
      "detail": "",
      "new": false,
      "accepted": "false positive: generated anchor"
    },
    {
      "key": "pointer-doctor|.claude/agents/w.md|path:docs/r.md",
      "check": "pointer-doctor",
      "file": ".claude/agents/w.md",
      "line": 0,
      "also": [],
      "count": 1,
      "baseline_count": 2,
      "target": "path:docs/r.md",
      "message": "path does not exist: docs/r.md",
      "detail": "cited on purpose",
      "new": false,
      "accepted": null
    }"##;

    const JSON_TAIL: &str = r##"
  ],
  "resolved": [
    "md-links|docs/c.md|gone.md"
  ],
  "reduced": {
    "pointer-doctor|.claude/agents/w.md|path:docs/r.md": {
      "baseline": 2,
      "current": 1
    }
  }
}"##;

    fn with_sample<T>(render: impl FnOnce(&Report<'_>) -> T) -> T {
        let findings = sample_findings();
        let cmp = sample_cmp();
        let skipped = sample_skipped();
        let baseline = sample_baseline();
        render(&Report {
            findings: &findings,
            cmp: &cmp,
            skipped: &skipped,
            names: &NAMES,
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        })
    }

    #[test]
    fn text_matches_check_py() {
        let text = with_sample(|r| render_text(r, false));
        assert_eq!(text, format!("{TEXT_HEAD}{TEXT_NEW}{TEXT_PRUNE}"));
        let all = with_sample(|r| render_text(r, true));
        assert_eq!(all, format!("{TEXT_HEAD}{TEXT_ALL}{TEXT_PRUNE}"));
    }

    #[test]
    fn markdown_matches_check_py() {
        assert_eq!(with_sample(render_markdown), MARKDOWN);
    }

    #[test]
    fn json_matches_check_py() {
        let new_only = with_sample(|r| render_json(r, false)).expect("render JSON");
        assert_eq!(new_only, format!("{JSON_HEAD}{JSON_TAIL}"));
        let all = with_sample(|r| render_json(r, true)).expect("render JSON");
        assert_eq!(all, format!("{JSON_HEAD}{JSON_BASELINED}{JSON_TAIL}"));
    }

    #[test]
    fn empty_reports() {
        let cmp = Comparison::default();
        let skipped = BTreeMap::new();
        let baseline = Baseline::new();
        let report = Report {
            findings: &[],
            cmp: &cmp,
            skipped: &skipped,
            names: &[],
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        };
        let head = "docs-check: 0 findings across 0 checks - 0 new, 0 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)\n\n  check              total   new";
        assert_eq!(
            render_text(&report, false),
            format!("{head}\n\nNo new drift since baseline.")
        );
        assert_eq!(render_text(&report, true), head);
        assert_eq!(
            render_markdown(&report),
            "## Docs drift check\n\n**0 new** finding(s) since baseline; 0 total (0 accepted false positives), 0 resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.\n\n| Check | Total | New |\n|---|---:|---:|\n"
        );
    }

    #[test]
    fn prune_notes_truncate_after_the_limit() {
        let cmp = Comparison {
            resolved: (0..51).map(|i| format!("k{i:02}")).collect(),
            reduced: BTreeMap::from([("r1".to_string(), (3, 1)), ("r0".to_string(), (2, 1))]),
            ..Comparison::default()
        };
        let notes = prune_notes(&cmp, 50);
        assert_eq!(notes.len(), 51);
        assert_eq!(notes[0], "  - k00");
        assert_eq!(notes[49], "  - k49");
        assert_eq!(notes[50], "  ... and 3 more");
        let short = prune_notes(&cmp, 60);
        assert_eq!(short[51], "  - r0 (2 -> 1 occurrences)");
        assert_eq!(short[52], "  - r1 (3 -> 1 occurrences)");
    }

    #[test]
    fn markdown_lists_at_most_100_new_findings() {
        let findings: Vec<Finding> = (0..101)
            .map(|i| {
                Finding::new(
                    "md-links",
                    format!("f{i:03}.md"),
                    "x.md",
                    "broken link -> x.md",
                    1,
                )
            })
            .collect();
        let cmp = Comparison {
            new_keys: findings.iter().map(Finding::key).collect(),
            ..Comparison::default()
        };
        let skipped = BTreeMap::new();
        let baseline = Baseline::new();
        let report = Report {
            findings: &findings,
            cmp: &cmp,
            skipped: &skipped,
            names: &["md-links"],
            baseline: &baseline,
            baseline_rel: "scripts/docs/baseline.json",
        };
        let text = render_markdown(&report);
        assert!(text.contains("| md-links | 101 | 101 |\n"), "{text}");
        assert!(
            text.ends_with("- `md-links` f099.md:1: broken link -> x.md\n- ... and 1 more\n"),
            "{text}"
        );
    }

    struct Named(&'static str);

    impl Check for Named {
        fn name(&self) -> &'static str {
            self.0
        }

        fn run(&self, _repo: &Repo) -> anyhow::Result<Vec<Finding>> {
            Ok(Vec::new())
        }
    }

    #[test]
    fn list_checks_pads_names_to_18() {
        let checks: Vec<Box<dyn Check>> = vec![
            Box::new(Named("md-links")),
            Box::new(Named("knowledge-hygiene")),
        ];
        assert_eq!(
            render_list_checks(&checks),
            "md-links           relative [text](path) links resolve to a tracked file or directory\nknowledge-hygiene  docs/knowledge naming, frontmatter, and an empty plans/ dir\n"
        );
    }
}
```

In `tools/src/cmd/docs_check/mod.rs` replace the line `pub mod model;` with:

```rust
pub mod model;
pub mod output;
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p aios-tools`
Expected: FAIL to compile: `error: environment variable CARGO_BIN_EXE_aios not defined at compile time` in `tests/common/mod.rs`, `error[E0432]: unresolved import` for `aios_tools::cmd::docs_check::checks` and for `run_checks`, `run_with`, `select_from`, `Args` in `tests/cli.rs`, and `error[E0433]` / `error[E0425]` for `checks`, `Report` and the render functions in `output.rs`'s tests.

- [ ] **Step 3: Write the implementation**

Create `tools/src/cmd/docs_check/checks/mod.rs`:

```rust
//! The docs-check checks. Each check is a unit struct implementing [`Check`];
//! [`registry`] lists the implemented checks in CHECK_ORDER (check.py `CHECK_FUNCS`,
//! L1379-1395 at 33c6b3d).

use crate::cmd::docs_check::{model::Finding, repo::Repo};

/// One docs drift check.
pub trait Check {
    /// Name as in CHECK_ORDER, e.g. "md-links".
    fn name(&self) -> &'static str;
    /// `--list-checks` description.
    fn describe(&self) -> &'static str {
        crate::cmd::docs_check::model::description(self.name())
    }
    /// Findings in check.py's production order; `Err` downcasting to `Skip` when the check cannot run.
    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>>;
}

/// Implemented checks in CHECK_ORDER. Each check task appends its checks here, in
/// CHECK_ORDER, so the order of this list is the order of the report.
pub fn registry() -> Vec<Box<dyn Check>> {
    Vec::new()
}
```

Insert this block at the top of `tools/src/cmd/docs_check/output.rs`, above the `#[cfg(test)]` line from Step 1, with one blank line between the last `}` of the block and `#[cfg(test)]`:

```rust
//! Report renderers of `aios docs-check`: check.py `describe`, `prune_notes`,
//! `render_text` and `render_markdown` (L1488-1573), the `--json` document built in
//! `main` (L1629-1657) and `--list-checks` (L1597-1600), at 33c6b3d.
//!
//! Each renderer returns check.py's bytes. `render_text` and `render_json` return
//! them without the final newline that Python's `print` adds; the caller adds it.

use std::collections::BTreeMap;

use serde::Serialize;
use serde_json::{json, Map, Value};

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::model::{self, Baseline, Comparison, Finding};

/// Findings listed in the prune note before it is truncated (check.py L1497).
const PRUNE_LIMIT: usize = 50;
/// New findings listed in the Markdown summary (check.py L1565).
const MARKDOWN_LIMIT: usize = 100;

/// Everything one rendering needs.
pub struct Report<'a> {
    /// Collated findings (check.py `run_checks` order).
    pub findings: &'a [Finding],
    pub cmp: &'a Comparison,
    /// Skip message of every selected check that could not run.
    pub skipped: &'a BTreeMap<&'static str, String>,
    /// Selected checks in CHECK_ORDER, skipped ones included.
    pub names: &'a [&'static str],
    pub baseline: &'a Baseline,
    /// Baseline path relative to the repository root.
    pub baseline_rel: &'a str,
}

impl Report<'_> {
    fn is_new(&self, f: &Finding) -> bool {
        self.cmp.new_keys.contains(&f.key())
    }

    fn new_findings(&self) -> Vec<&Finding> {
        self.findings.iter().filter(|f| self.is_new(f)).collect()
    }

    fn total_of(&self, name: &str) -> usize {
        self.findings.iter().filter(|f| f.check == name).count()
    }

    fn new_of(&self, name: &str) -> usize {
        self.findings
            .iter()
            .filter(|f| f.check == name && self.is_new(f))
            .count()
    }
}

/// check.py `describe` (L1488-1494): the finding text plus the grown and accepted notes.
pub fn describe(f: &Finding, cmp: &Comparison) -> String {
    let key = f.key();
    let mut text = f.text();
    if let Some(base) = cmp.grown.get(&key) {
        text.push_str(&format!(" [{} occurrences, baseline {base}]", f.count()));
    }
    if let Some(reason) = cmp.accepted.get(&key) {
        text.push_str(&format!(" [accepted: {}]", model::py_str(reason)));
    }
    text
}

/// check.py `prune_notes` (L1497-1502): resolved keys, then reduced keys, at most
/// `limit` lines plus a `... and N more` line.
pub fn prune_notes(cmp: &Comparison, limit: usize) -> Vec<String> {
    let mut notes: Vec<String> = cmp
        .resolved
        .iter()
        .map(|key| format!("  - {key}"))
        .collect();
    notes.extend(
        cmp.reduced
            .iter()
            .map(|(key, (base, current))| format!("  - {key} ({base} -> {current} occurrences)")),
    );
    if notes.len() > limit {
        let more = notes.len() - limit;
        notes.truncate(limit);
        notes.push(format!("  ... and {more} more"));
    }
    notes
}

/// check.py `render_text` (L1505-1542), without the final newline.
pub fn render_text(r: &Report<'_>, show_all: bool) -> String {
    let new = r.new_findings();
    let total = r.findings.len();
    let mut out = vec![
        format!(
            "docs-check: {total} findings across {} checks - {} new, {} baselined ({} accepted false positives), {} resolved (baseline {})",
            r.names.len().saturating_sub(r.skipped.len()),
            new.len(),
            total.saturating_sub(new.len()),
            r.cmp.accepted.len(),
            r.cmp.resolved.len(),
            r.baseline_rel,
        ),
        String::new(),
        format!("  {:<18} {:>5} {:>5}", "check", "total", "new"),
    ];
    for &name in r.names {
        match r.skipped.get(name) {
            Some(message) => out.push(format!("  {name:<18} {:>5}        {message}", "skip")),
            None => {
                let fresh = new.iter().filter(|f| f.check == name).count();
                out.push(format!("  {name:<18} {:>5} {fresh:>5}", r.total_of(name)));
            }
        }
    }
    let shown: Vec<&Finding> = if show_all {
        r.findings.iter().collect()
    } else {
        new
    };
    if !shown.is_empty() {
        let heading = if show_all {
            "All findings ('+' = new since baseline, '~' = accepted false positive):"
        } else {
            "New drift since baseline:"
        };
        out.push(String::new());
        out.push(heading.to_string());
        let mut current: Option<&str> = None;
        for f in shown {
            if current != Some(f.check) {
                current = Some(f.check);
                out.push(format!("\n[{}]", f.check));
            }
            let key = f.key();
            let mark = if r.cmp.new_keys.contains(&key) {
                '+'
            } else if r.cmp.accepted.contains_key(&key) {
                '~'
            } else {
                ' '
            };
            out.push(format!(" {mark} {}: {}", f.location(), describe(f, r.cmp)));
        }
    } else if !show_all {
        out.push(String::new());
        out.push("No new drift since baseline.".to_string());
    }
    let notes = prune_notes(r.cmp, PRUNE_LIMIT);
    if !notes.is_empty() {
        out.push(String::new());
        out.push(format!(
            "{} baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:",
            r.cmp.resolved.len() + r.cmp.reduced.len()
        ));
        out.extend(notes);
    }
    out.join("\n")
}

/// check.py `render_markdown` (L1545-1573), ending with one newline.
pub fn render_markdown(r: &Report<'_>) -> String {
    let new = r.new_findings();
    let mut out = vec![
        "## Docs drift check".to_string(),
        String::new(),
        format!(
            "**{} new** finding(s) since baseline; {} total ({} accepted false positives), {} resolved. Report-only: fix new drift in this PR, or accept it with `just docs-check --update-baseline`.",
            new.len(),
            r.findings.len(),
            r.cmp.accepted.len(),
            r.cmp.resolved.len(),
        ),
        String::new(),
        "| Check | Total | New |".to_string(),
        "|---|---:|---:|".to_string(),
    ];
    for &name in r.names {
        match r.skipped.get(name) {
            Some(message) => out.push(format!("| {name} | skipped | {message} |")),
            None => {
                let fresh = new.iter().filter(|f| f.check == name).count();
                out.push(format!("| {name} | {} | {fresh} |", r.total_of(name)));
            }
        }
    }
    if !new.is_empty() {
        out.push(String::new());
        out.push("### New findings".to_string());
        out.push(String::new());
        for f in new.iter().take(MARKDOWN_LIMIT) {
            out.push(format!(
                "- `{}` {}: {}",
                f.check,
                f.location(),
                describe(f, r.cmp)
            ));
        }
        if new.len() > MARKDOWN_LIMIT {
            out.push(format!("- ... and {} more", new.len() - MARKDOWN_LIMIT));
        }
    }
    if !(r.cmp.resolved.is_empty() && r.cmp.reduced.is_empty()) {
        out.push(String::new());
        out.push(format!(
            "{} baselined finding(s) no longer occur or occur less often (run `just docs-check --update-baseline`).",
            r.cmp.resolved.len() + r.cmp.reduced.len()
        ));
    }
    let mut text = out.join("\n");
    text.push('\n');
    text
}

#[derive(Serialize)]
struct JsonReport<'a> {
    baseline: &'a str,
    summary: JsonSummary,
    checks: Map<String, Value>,
    findings: Vec<JsonFinding<'a>>,
    resolved: &'a [String],
    reduced: Map<String, Value>,
}

#[derive(Serialize)]
struct JsonSummary {
    total: usize,
    new: usize,
    baselined: usize,
    accepted: usize,
    resolved: usize,
    reduced: usize,
}

#[derive(Serialize)]
struct JsonFinding<'a> {
    key: String,
    check: &'a str,
    file: &'a str,
    line: usize,
    also: &'a [usize],
    count: usize,
    baseline_count: Option<i64>,
    target: &'a str,
    message: &'a str,
    detail: &'a str,
    new: bool,
    accepted: Option<&'a Value>,
}

/// check.py's `--json` document (L1629-1657), key order included, without the
/// final newline. Lists every finding with `show_all`, else only the new ones.
pub fn render_json(r: &Report<'_>, show_all: bool) -> anyhow::Result<String> {
    let mut checks = Map::new();
    for &name in r.names {
        let value = match r.skipped.get(name) {
            Some(message) => json!({ "skipped": message }),
            None => json!({ "total": r.total_of(name), "new": r.new_of(name) }),
        };
        checks.insert(name.to_string(), value);
    }
    let mut findings = Vec::new();
    for f in r.findings {
        let key = f.key();
        let new = r.cmp.new_keys.contains(&key);
        if !(show_all || new) {
            continue;
        }
        let baseline_count = r
            .baseline
            .get(&key)
            .map(model::baseline_count)
            .transpose()?;
        let accepted = r.cmp.accepted.get(&key);
        findings.push(JsonFinding {
            key,
            check: f.check,
            file: &f.file,
            line: f.line,
            also: &f.also,
            count: f.count(),
            baseline_count,
            target: &f.target,
            message: &f.message,
            detail: &f.detail,
            new,
            accepted,
        });
    }
    let mut reduced = Map::new();
    for (key, (base, current)) in &r.cmp.reduced {
        reduced.insert(key.clone(), json!({ "baseline": base, "current": current }));
    }
    let report = JsonReport {
        baseline: r.baseline_rel,
        summary: JsonSummary {
            total: r.findings.len(),
            new: r.cmp.new_keys.len(),
            baselined: r.findings.len().saturating_sub(r.cmp.new_keys.len()),
            accepted: r.cmp.accepted.len(),
            resolved: r.cmp.resolved.len(),
            reduced: r.cmp.reduced.len(),
        },
        checks,
        findings,
        resolved: &r.cmp.resolved,
        reduced,
    };
    Ok(serde_json::to_string_pretty(&report)?)
}

/// check.py `--list-checks` (L1597-1600): `{name:<18} {description}` per check.
pub fn render_list_checks(checks: &[Box<dyn Check>]) -> String {
    let mut out = String::new();
    for check in checks {
        out.push_str(&format!("{:<18} {}\n", check.name(), check.describe()));
    }
    out
}
```

Replace the whole of `tools/src/cmd/docs_check/mod.rs` with:

```rust
//! `aios docs-check`: the deterministic docs drift checker, a port of
//! `scripts/docs/check.py` at 33c6b3d. `run` is check.py `main()` (L1585-1662);
//! `run_checks` is check.py `run_checks` (L1442-1456).
//!
//! Accepted divergence (contract §1.9): check.py resolves the repository from its own
//! directory (L1576-1582, `os.path.dirname(os.path.abspath(__file__))`); `repo_root`
//! resolves it from the process working directory, because the binary has no script
//! directory. `just` runs recipes from the justfile's directory and the shim passes the
//! caller's working directory through, so both tools check the same checkout in every
//! supported invocation; the parity goldens run check.py from inside each materialized
//! repository for the same reason.

pub mod checks;
pub mod markdown;
pub mod model;
pub mod output;
pub mod repo;

use std::collections::{BTreeMap, BTreeSet};
use std::io::Write;
use std::path::Path;

use anyhow::{anyhow, bail, Context};

use crate::{paths, proc, pystr};
use checks::Check;
use model::{Finding, Skip};
use repo::Repo;

// check.py's argparse options (L1586-1595). A plain comment, not a doc comment:
// clap would show a doc comment here in `aios docs-check --help`.
#[derive(clap::Args, Debug, Clone, Default, PartialEq, Eq)]
pub struct Args {
    /// list every finding, not only new ones
    #[arg(long)]
    pub all: bool,
    /// print JSON instead of text
    #[arg(long)]
    pub json: bool,
    /// print a Markdown summary (for $GITHUB_STEP_SUMMARY)
    #[arg(long)]
    pub markdown: bool,
    /// comma-separated checks to run (default: all)
    #[arg(long, default_value = "", hide_default_value = true)]
    pub check: String,
    /// baseline file (default: scripts/docs/baseline.json)
    #[arg(long)]
    pub baseline: Option<String>,
    /// rewrite the baseline from the current findings (counts included; 'reason' on accepted false positives is kept)
    #[arg(long)]
    pub update_baseline: bool,
    /// list available checks
    #[arg(long)]
    pub list_checks: bool,
}

/// The result of running the selected checks.
pub struct CheckRun {
    /// Collated: merged by key and sorted (`model::collate`).
    pub findings: Vec<Finding>,
    /// Skip message per check that could not run.
    pub skipped: BTreeMap<&'static str, String>,
}

/// check.py L1602-1608 against `checks::registry()`.
pub fn select_checks(check_arg: &str) -> anyhow::Result<Vec<Box<dyn Check>>> {
    select_from(checks::registry(), check_arg)
}

/// check.py L1602-1608 against `available`: an empty `check_arg` selects every
/// check; otherwise the comma-separated names (Python-stripped, empty parts
/// dropped) select checks in `available` order, each once. Unknown names, in the
/// order given and with repeats, are an error.
pub fn select_from(
    available: Vec<Box<dyn Check>>,
    check_arg: &str,
) -> anyhow::Result<Vec<Box<dyn Check>>> {
    if check_arg.is_empty() {
        return Ok(available);
    }
    let requested: Vec<&str> = check_arg
        .split(',')
        .map(pystr::strip)
        .filter(|name| !name.is_empty())
        .collect();
    let unknown: Vec<&str> = requested
        .iter()
        .copied()
        .filter(|name| !available.iter().any(|check| check.name() == *name))
        .collect();
    if !unknown.is_empty() {
        bail!(
            "unknown check(s): {} (see --list-checks)",
            unknown.join(", ")
        );
    }
    Ok(available
        .into_iter()
        .filter(|check| requested.contains(&check.name()))
        .collect())
}

/// check.py `run_checks` (L1442-1456): runs each check in order; a `Skip` error
/// records the check as skipped, any other error aborts the run.
pub fn run_checks(repo: &Repo, checks: &[Box<dyn Check>]) -> anyhow::Result<CheckRun> {
    let mut findings = Vec::new();
    let mut skipped = BTreeMap::new();
    for check in checks {
        match check.run(repo) {
            Ok(found) => findings.extend(found),
            Err(err) => match err.downcast::<Skip>() {
                Ok(skip) => {
                    skipped.insert(check.name(), skip.0);
                }
                Err(err) => return Err(err),
            },
        }
    }
    Ok(CheckRun {
        findings: model::collate(findings),
        skipped,
    })
}

/// check.py `repo_root` (L1576-1582), run in `cwd`.
pub fn repo_root(cwd: &Path) -> anyhow::Result<String> {
    let output = proc::capture("git", &["rev-parse", "--show-toplevel"], cwd)?;
    if !output.status.success() {
        bail!("not inside a git repository");
    }
    let stdout = std::str::from_utf8(&output.stdout)
        .context("git rev-parse --show-toplevel printed output that is not valid UTF-8")?;
    Ok(pystr::strip(stdout).to_string())
}

/// The whole of check.py `main()` with the registered checks: writes stdout to
/// `out`, returns the exit code (0, or 1 when there is new drift).
pub fn run(args: &Args, cwd: &Path, out: &mut dyn Write) -> anyhow::Result<u8> {
    run_with(args, cwd, checks::registry(), out)
}

/// `run` with an explicit list of available checks (tests pass stand-in checks).
pub fn run_with(
    args: &Args,
    cwd: &Path,
    available: Vec<Box<dyn Check>>,
    out: &mut dyn Write,
) -> anyhow::Result<u8> {
    if args.list_checks {
        out.write_all(output::render_list_checks(&available).as_bytes())?;
        return Ok(0);
    }
    let selected = select_from(available, &args.check)?;
    let names: Vec<&'static str> = selected.iter().map(|check| check.name()).collect();
    let root = repo_root(cwd)?;
    let cwd_text = cwd
        .to_str()
        .ok_or_else(|| anyhow!("the current directory {} is not valid UTF-8", cwd.display()))?;
    let baseline_path = match args.baseline.as_deref() {
        Some(path) if !path.is_empty() => path.to_string(),
        _ => paths::join(&root, model::BASELINE_REL),
    };
    let baseline_rel = paths::relpath(&baseline_path, &root, cwd_text);
    let repo = Repo::open(&root)?;
    let CheckRun { findings, skipped } = run_checks(&repo, &selected)?;
    let ran: BTreeSet<&str> = names
        .iter()
        .copied()
        .filter(|name| !skipped.contains_key(name))
        .collect();
    let baseline_file = cwd.join(&baseline_path);
    let baseline = model::load_baseline(&baseline_file, &baseline_path)?;

    if args.update_baseline {
        let entries = model::updated_baseline(&findings, &baseline, &ran);
        model::write_baseline(&baseline_file, &entries)?;
        let mut message = format!(
            "docs-check: wrote {} findings to {baseline_rel}",
            entries.len()
        );
        if !skipped.is_empty() {
            let skipped_names: Vec<&str> = skipped.keys().copied().collect();
            message.push_str(&format!(" (skipped: {})", skipped_names.join(", ")));
        }
        writeln!(out, "{message}")?;
        return Ok(0);
    }

    let cmp = model::compare(&findings, &baseline, &ran)?;
    let report = output::Report {
        findings: &findings,
        cmp: &cmp,
        skipped: &skipped,
        names: &names,
        baseline: &baseline,
        baseline_rel: &baseline_rel,
    };
    if args.json {
        writeln!(out, "{}", output::render_json(&report, args.all)?)?;
    } else if args.markdown {
        out.write_all(output::render_markdown(&report).as_bytes())?;
    } else {
        writeln!(out, "{}", output::render_text(&report, args.all))?;
    }
    Ok(if cmp.new_keys.is_empty() { 0 } else { 1 })
}
```

Create `tools/src/main.rs`:

```rust
//! `aios`: AIOS host tooling. R1 ships `aios docs-check`; later PRs add subcommands.
#![forbid(unsafe_code)]

use std::io::Write;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aios", version, about = "AIOS host tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deterministic docs drift check against scripts/docs/baseline.json (exit 1 on new drift)
    #[command(name = "docs-check", infer_long_args = true, args_override_self = true)]
    DocsCheck(aios_tools::cmd::docs_check::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::DocsCheck(args) => {
            let cwd = match std::env::current_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    eprintln!("docs-check: cannot read the current directory: {err}");
                    return ExitCode::from(2);
                }
            };
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let result = aios_tools::cmd::docs_check::run(&args, &cwd, &mut out);
            let flushed = out.flush();
            match (result, flushed) {
                (Ok(code), Ok(())) => ExitCode::from(code),
                (Ok(_), Err(err)) => {
                    eprintln!("docs-check: cannot write output: {err}");
                    ExitCode::from(2)
                }
                (Err(err), _) => {
                    eprintln!("docs-check: {err:#}");
                    ExitCode::from(2)
                }
            }
        }
    }
}
```

In `tools/Cargo.toml` replace

```toml
[lib]
name = "aios_tools"
path = "src/lib.rs"

[dependencies]
```

with

```toml
[lib]
name = "aios_tools"
path = "src/lib.rs"

[[bin]]
name = "aios"
path = "src/main.rs"

[dependencies]
```

In `justfile` replace

```just
    cargo test --workspace --exclude kernel --exclude uefi-stub --exclude aios-tools --target-dir target/host-tests

# Run clippy with deny warnings (kernel and stub targets, plus the host tools crate)
```

with

```just
    cargo test --workspace --exclude kernel --exclude uefi-stub --exclude aios-tools --target-dir target/host-tests

# Build the host tools binary target/tools/release/aios (run through .claude/hooks/aios).
# The touch marks a no-op build fresh for the shim's freshness check.
tools:
    cargo build --release -p aios-tools --target-dir target/tools
    touch target/tools/release/aios

# Run clippy with deny warnings (kernel and stub targets, plus the host tools crate)
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --lib output`
Expected: PASS: the 7 tests under `cmd::docs_check::output::tests` pass (`text_matches_check_py`, `markdown_matches_check_py`, `json_matches_check_py`, `empty_reports`, `prune_notes_truncate_after_the_limit`, `markdown_lists_at_most_100_new_findings`, `list_checks_pads_names_to_18`), `test result: ok.`

Run: `cargo test -p aios-tools --test cli`
Expected: PASS: `test result: ok. 11 passed; 0 failed`.

Run: `cargo test -p aios-tools`
Expected: PASS: every test binary (lib unit tests, `src/main.rs` with 0 tests, `cli`, `repo`, `shim`) ends with `test result: ok.`

- [ ] **Step 5: Build the binary with `just tools` and exercise it**

Run: `just tools && ls -l target/tools/release/aios`
Expected: cargo builds `aios-tools` in release mode into `target/tools`, then `ls` shows an executable `target/tools/release/aios`.

Run: `target/tools/release/aios docs-check --check bogus; echo "exit=$?"`
Expected: stderr `docs-check: unknown check(s): bogus (see --list-checks)`, then `exit=2`.

Run: `target/tools/release/aios docs-check --list-checks; echo "exit=$?"`
Expected: no check lines yet (the registry fills in Tasks 7-12), then `exit=0`.

Run: `target/tools/release/aios --version`
Expected: `aios 0.1.0`.

- [ ] **Step 6: Format, lint and the repository gate**

Run: `cargo fmt --check -p aios-tools && cargo clippy -p aios-tools -- -D warnings && just check`
Expected: no diff from `cargo fmt`; clippy prints no warning; `just check` (fmt-check, clippy on the kernel and stub targets and on `aios-tools`, both builds) finishes with exit 0.

- [ ] **Step 7: Commit and push**

```bash
git add tools/src/cmd/docs_check/output.rs tools/src/cmd/docs_check/checks/mod.rs \
    tools/src/cmd/docs_check/mod.rs tools/src/main.rs tools/Cargo.toml \
    tools/tests/common/mod.rs tools/tests/cli.rs justfile
git commit -F - <<'MSG'
Add the aios binary with docs-check output, registry and CLI

output.rs renders check.py's text, Markdown, JSON and --list-checks
output byte for byte (L1488-1573, L1597-1600, L1629-1657); checks/mod.rs
defines the Check trait and the (still empty) registry; docs_check::run
is check.py main() (L1585-1662) with run_checks (L1442-1456); main.rs is
the aios CLI (exit 0/1/2). just tools builds target/tools/release/aios.
The CLI tests replay check.py's outputs for usage errors, zero-check
runs, baseline paths and rewrites, and stand-in checks.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
MSG
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 7: Link checks: md-links, section-refs, anchors, wiki-links

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/links.rs`
- Create: `tools/tests/checks_links.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (add `pub mod links;`; `registry()` returns the four link checks)
- Test: `tools/tests/checks_links.rs`

**Interfaces:**
- Consumes (T6, `checks/mod.rs`): `pub trait Check { fn name(&self) -> &'static str; fn describe(&self) -> &'static str; fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>>; }` and `pub fn registry() -> Vec<Box<dyn Check>>`.
- Consumes (T3, `model`): `Finding::new(check: &'static str, file: impl Into<String>, target: impl Into<String>, message: impl Into<String>, line: usize) -> Finding`; `Finding: Debug + Clone + PartialEq + Eq`.
- Consumes (T5, `repo`): `Repo::open(root: &str) -> anyhow::Result<Repo>`, `Repo::files(&self) -> &[String]`, `Repo::md_files(&self) -> &[String]`, `Repo::is_file(&self, rel: &str) -> bool`, `Repo::text(&self, rel: &str) -> Rc<str>`, `Repo::headings(&self, rel: &str) -> Rc<Vec<Heading>>`, `Repo::slug_set(&self, rel: &str) -> Rc<HashSet<String>>` (check.py `slug_set` L588-604, already ported), `Repo::exists(&self, rel: &str) -> bool`, `Repo::resolve(&self, src: &str, target: &str) -> Option<String>`.
- Consumes (T4, `markdown`): `prose_lines(text: &str) -> Vec<(usize, &str)>`, `mask_prose(line: &str) -> String`, `is_escaped(s: &str, idx: usize) -> bool`, `split_target(raw: &str) -> Option<(String, String)>`, `is_placeholder(target: &str) -> bool`, `strip_inline_md(text: &str) -> String`, `heading_number(text: &str) -> Option<String>`, `pub struct Heading { pub line: usize, pub level: usize, pub text: String }`, and the `LazyLock<Regex>` statics `INLINE_LINK_RE`, `REF_DEF_RE`, `SECTION_REF_RE`, `WIKI_RE`, `SCHEME_RE`.
- Consumes (T1): `pystr::strip(s: &str) -> &str`, `pystr::url_unquote(s: &str) -> String`, `pystr::char_prefix(s: &str, n: usize) -> &str`, `paths::splitext(path: &str) -> (&str, &str)`, `paths::dirname(path: &str) -> &str`, `paths::basename(path: &str) -> &str`.
- Consumes (T2, `tools/tests/common`): `TestRepo::with_files(label: &str, files: &[(&str, &str)]) -> TestRepo`, `TestRepo::path_str(&self) -> &str`.
- Produces (`aios_tools::cmd::docs_check::checks::links`): unit structs `MdLinks`, `SectionRefs`, `Anchors`, `WikiLinks` (each `impl Check`, names `md-links`, `section-refs`, `anchors`, `wiki-links`); `pub fn iter_links(repo: &Repo, rel: &str) -> Vec<(usize, String)>`; `pub fn heading_numbers(repo: &Repo, rel: &str) -> BTreeSet<String>`; `pub fn hub_members(repo: &Repo, hub: &str) -> Vec<String>`; `pub fn number_resolves(num: &str, nums: &BTreeSet<String>) -> bool`. `registry()` returns `[md-links, section-refs, anchors, wiki-links]`, the first four entries of CHECK_ORDER.

**Parity notes** (re-read each range in `scripts/docs/check.py`, or `git show 33c6b3d:scripts/docs/check.py` once Task 15 has deleted it):
- `iter_links` (L549-558): prose lines only; both regexes run on `mask_prose(line)`. An inline link counts unless the `[` before its label (byte `start(2) - 1`) is escaped; a reference definition counts unless the first `[` of the masked line is escaped. Inline links of a line come before its reference definition. Footnote definitions (`[^1]: ...`) never match `REF_DEF_RE`.
- `md-links` (L575-585): skip when `split_target` returns `None` or an empty path; a finding when `resolve` gives `None` (the path leaves the repository) or the path does not `exist`. Target and message use the unquoted path (`parts[0]`), not the raw link text.
- `anchors` (L607-622): `t = strip(raw)` then strip any run of `<` and `>` from both ends; skip without `#` or with a scheme; skip an empty or placeholder fragment or a placeholder path part; the path part drops `?query` and is unquoted before `resolve`; only `.md` targets in the file listing are checked; the slug lookup is `url_unquote(frag).lower()`, while target and message keep `t` and the raw `frag`.
- `heading_numbers` (L625-631), `hub_members` (L634-643: folder members first, then flat siblings whose first 2,000 code points, not bytes, contain `Part of:\s*[<basename>]`), `number_resolves` (L646-647).
- `section-refs` (L650-669): the escape test is at the match start (the `[`); `url_unquote(raw)` with no `?` split; a target missing from the file listing is left to md-links; target text `"{raw} §{num}"`.
- `wiki-links` (L672-698): the vault is every listed file under `docs/` (not only Markdown); the escape test is at byte `start(2) - 2`; the note is Python-stripped; an empty note (a same-note heading link) is skipped; `.md` is optional on both sides.
- Python's module-level `_SLUG_CACHE` becomes the per-`Repo` cache of T5; nothing else in these checks keeps state between runs.
- Expected findings in the tests were recorded by calling check.py's own `check_md_links`, `check_section_refs`, `check_anchors`, `check_wiki_links`, `iter_links`, `heading_numbers`, `hub_members` and `number_resolves` on the same fixture files, so they are in production order (before `model::collate`).

- [ ] **Step 1: Write the failing test**

Create `tools/tests/checks_links.rs`:

````rust
//! Link checks md-links, section-refs, anchors and wiki-links (check.py L549-698).
//!
//! The expected findings were recorded by calling check.py's own `check_md_links`,
//! `check_section_refs`, `check_anchors` and `check_wiki_links` on the same files, so they
//! are in production order (before `model::collate` merges and sorts them).

mod common;

use std::collections::BTreeSet;

use aios_tools::cmd::docs_check::checks::links::{
    heading_numbers, hub_members, iter_links, number_resolves, Anchors, MdLinks, SectionRefs,
    WikiLinks,
};
use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::Finding;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const LINKS_README_MD: &str = r#"# Readme

See [guide](docs/guide.md) and [missing](docs/nope.md).
Absolute [root link](/docs/guide.md) and [escape](../outside.md).
Web [site](https://example.com), [mail](mailto:a@b.c) and [proto](//cdn.example.com/x).
Templates [tmpl](docs/<name>.md) and [glob](docs/*.md) are skipped.
Escaped \[not a link](docs/escaped-missing.md) and image ![pic](img/missing.png).
Code `[code](docs/code-missing.md)` and <!-- [comment](docs/comment-missing.md) --> are masked.
Angle [spaced](<docs/my guide.md>), encoded [enc](docs/my%20guide.md), query [q](docs/guide.md?plain=1).
Directory [dir](docs/), [self](#readme) and a titled [t](docs/nope.md "Title").
[ref]: docs/ref-missing.md
[^1]: docs/footnote-missing.md

```text
[fenced](docs/fenced-missing.md)
```

    [indented](docs/indented-missing.md)

<!--
[multi](docs/multi-comment-missing.md)
-->
"#;
const LINKS_DOCS_GUIDE_MD: &str = r#"# Guide

## 1. Overview

### 1.2 Details `code` **bold**

## 1. Overview

<a name="Custom-Anchor"></a>
<a id="other"></a>

Links [a](#1-overview), [b](#1-overview-1), [c](#1-overview-2), [d](#12-details-code-bold).
More [e](#custom-anchor), [f](#OTHER), [g](../README.md#nope), [h](../README.md#readme).
Encoded [i](guide.md#%31-overview), empty [j](#), template [k](#{slug}), angle [l](<#bad anchor>).
Other [m](other.txt#x), [n](https://x.y/#z), [o](missing.md#x), [p](my%20guide.md#my-guide).
"#;
const LINKS_DOCS_MY_GUIDE_MD: &str = r#"# My Guide

Back to [guide](guide.md#guide).
"#;
const LINKS_DOCS_OTHER_TXT: &str = r#"plain text
"#;
const LINKS_DOCS_KERNEL_HUB_MD: &str = r#"# Hub

## 1. Intro

### 1.1 Scope

## §5 Legacy
"#;
const LINKS_DOCS_KERNEL_HUB_PART_MD: &str = r#"# Part

## 4.2 Part Detail
"#;
const LINKS_DOCS_KERNEL_SIBLING_MD: &str = r#"# Sibling

Part of: [hub.md](hub.md)

## 7. Sibling
"#;
const LINKS_DOCS_KERNEL_REFS_MD: &str = r#"# Refs

Resolved [hub](hub.md) §1, [hub](hub.md) §1.1, [hub](hub.md#1-intro) **§ 1.1** and [hub](hub.md) §5.
Members [hub](hub.md) §4.2, [hub](hub.md) §4, [hub](hub.md) §7 and [hub](hub.md) §9.
Missing [hub](hub.md) §2, [hub](hub.md) §1.1.3, [hub](hub.md) §8 and [hub](hub.md) §A1.
Skipped [x](nothere.md) §1, [t](<n>.md) §1, \[hub](hub.md) §6 and `[hub](hub.md) §6`.
"#;
const LINKS_DOCS_WIKI_MD: &str = r#"# Wiki

Notes [[guide]], [[Guide.md]], [[kernel/hub]], [[kernel/hub.md|Hub]] and [[other.txt]].
Headings [[#Local heading]] and ![[guide#Overview]] resolve; [[ missing note ]] does not.
Also missing: [[kernel/nothing]] and [[README]].
Skipped \[[escaped-missing]] and `[[code-missing]]`.
"#;

/// The link fixture. `sibling2.md` puts its hub marker within the first 2,000 code points
/// but beyond 2,000 bytes; `late.md` puts it beyond 2,000 code points.
fn links_repo() -> TestRepo {
    let sibling_two = format!(
        "# Sibling Two\n\n{}\n\nPart of: [hub.md](hub.md)\n\n## 9. Sibling Two\n",
        "é".repeat(1900)
    );
    let late = format!(
        "# Late\n\n{}\n\nPart of: [hub.md](hub.md)\n\n## 8. Late\n",
        "x".repeat(2000)
    );
    TestRepo::with_files(
        "links",
        &[
            ("README.md", LINKS_README_MD),
            ("docs/guide.md", LINKS_DOCS_GUIDE_MD),
            ("docs/my guide.md", LINKS_DOCS_MY_GUIDE_MD),
            ("docs/other.txt", LINKS_DOCS_OTHER_TXT),
            ("docs/kernel/hub.md", LINKS_DOCS_KERNEL_HUB_MD),
            ("docs/kernel/hub/part.md", LINKS_DOCS_KERNEL_HUB_PART_MD),
            ("docs/kernel/sibling.md", LINKS_DOCS_KERNEL_SIBLING_MD),
            ("docs/kernel/refs.md", LINKS_DOCS_KERNEL_REFS_MD),
            ("docs/wiki.md", LINKS_DOCS_WIKI_MD),
            ("docs/kernel/sibling2.md", sibling_two.as_str()),
            ("docs/kernel/late.md", late.as_str()),
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
fn md_links_reports_targets_that_do_not_resolve() {
    let t = links_repo();
    let repo = open(&t);
    let got = MdLinks.run(&repo).expect("the check runs");
    let want = vec![
        finding(
            "md-links",
            "README.md",
            "docs/nope.md",
            "broken link -> docs/nope.md",
            3,
        ),
        finding(
            "md-links",
            "README.md",
            "../outside.md",
            "broken link -> ../outside.md",
            4,
        ),
        finding(
            "md-links",
            "README.md",
            "img/missing.png",
            "broken link -> img/missing.png",
            7,
        ),
        finding(
            "md-links",
            "README.md",
            "docs/nope.md",
            "broken link -> docs/nope.md",
            10,
        ),
        finding(
            "md-links",
            "README.md",
            "docs/ref-missing.md",
            "broken link -> docs/ref-missing.md",
            11,
        ),
        finding(
            "md-links",
            "docs/guide.md",
            "missing.md",
            "broken link -> missing.md",
            15,
        ),
        finding(
            "md-links",
            "docs/kernel/refs.md",
            "nothere.md",
            "broken link -> nothere.md",
            6,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn section_refs_reports_numbers_missing_from_the_hub_and_its_members() {
    let t = links_repo();
    let repo = open(&t);
    let got = SectionRefs.run(&repo).expect("the check runs");
    let want = vec![
        finding(
            "section-refs",
            "docs/kernel/refs.md",
            "hub.md §2",
            "no §2 heading in docs/kernel/hub.md or its hub members",
            5,
        ),
        finding(
            "section-refs",
            "docs/kernel/refs.md",
            "hub.md §1.1.3",
            "no §1.1.3 heading in docs/kernel/hub.md or its hub members",
            5,
        ),
        finding(
            "section-refs",
            "docs/kernel/refs.md",
            "hub.md §8",
            "no §8 heading in docs/kernel/hub.md or its hub members",
            5,
        ),
        finding(
            "section-refs",
            "docs/kernel/refs.md",
            "hub.md §A1",
            "no §A1 heading in docs/kernel/hub.md or its hub members",
            5,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn anchors_reports_fragments_without_a_heading_slug() {
    let t = links_repo();
    let repo = open(&t);
    let got = Anchors.run(&repo).expect("the check runs");
    let want = vec![
        finding(
            "anchors",
            "docs/guide.md",
            "#1-overview-2",
            "no heading for anchor #1-overview-2 in docs/guide.md",
            12,
        ),
        finding(
            "anchors",
            "docs/guide.md",
            "../README.md#nope",
            "no heading for anchor #nope in README.md",
            13,
        ),
        finding(
            "anchors",
            "docs/guide.md",
            "#bad anchor",
            "no heading for anchor #bad anchor in docs/guide.md",
            14,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn wiki_links_reports_notes_missing_from_the_docs_vault() {
    let t = links_repo();
    let repo = open(&t);
    let got = WikiLinks.run(&repo).expect("the check runs");
    let want = vec![
        finding(
            "wiki-links",
            "docs/wiki.md",
            "missing note",
            "no note named [[missing note]] in the docs/ vault",
            4,
        ),
        finding(
            "wiki-links",
            "docs/wiki.md",
            "kernel/nothing",
            "no note named [[kernel/nothing]] in the docs/ vault",
            5,
        ),
        finding(
            "wiki-links",
            "docs/wiki.md",
            "README",
            "no note named [[README]] in the docs/ vault",
            5,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn iter_links_yields_inline_links_and_reference_definitions_outside_code() {
    let t = links_repo();
    let repo = open(&t);
    let got = iter_links(&repo, "README.md");
    let want: Vec<(usize, String)> = [
        (3, "docs/guide.md"),
        (3, "docs/nope.md"),
        (4, "/docs/guide.md"),
        (4, "../outside.md"),
        (5, "https://example.com"),
        (5, "mailto:a@b.c"),
        (5, "//cdn.example.com/x"),
        (6, "docs/<name>.md"),
        (6, "docs/*.md"),
        (7, "img/missing.png"),
        (9, "<docs/my guide.md>"),
        (9, "docs/my%20guide.md"),
        (9, "docs/guide.md?plain=1"),
        (10, "docs/"),
        (10, "#readme"),
        (10, "docs/nope.md"),
        (11, "docs/ref-missing.md"),
    ]
    .into_iter()
    .map(|(line, target)| (line, target.to_string()))
    .collect();
    assert_eq!(got, want);
}

#[test]
fn heading_numbers_and_hub_members_follow_check_py() {
    let t = links_repo();
    let repo = open(&t);
    let nums = heading_numbers(&repo, "docs/kernel/hub.md");
    let want: BTreeSet<String> = ["1", "1.1", "5"].into_iter().map(String::from).collect();
    assert_eq!(nums, want);
    assert_eq!(
        hub_members(&repo, "docs/kernel/hub.md"),
        vec![
            "docs/kernel/hub/part.md",
            "docs/kernel/sibling.md",
            "docs/kernel/sibling2.md"
        ]
    );
}

#[test]
fn number_resolves_accepts_the_number_or_a_dotted_child() {
    let set =
        |items: &[&str]| -> BTreeSet<String> { items.iter().map(|s| s.to_string()).collect() };
    assert!(number_resolves("4", &set(&["4.2"])));
    assert!(number_resolves("4.2", &set(&["4.2"])));
    assert!(!number_resolves("4.2", &set(&["4"])));
    assert!(!number_resolves("1", &set(&["10"])));
    assert!(!number_resolves("1.1", &set(&["1.10"])));
    assert!(number_resolves("A1", &set(&["A1.2"])));
    assert!(!number_resolves("2", &set(&[])));
}

#[test]
fn registry_starts_with_the_link_checks() {
    let names: Vec<&str> = registry().iter().map(|c| c.name()).collect();
    assert_eq!(
        names[..4],
        ["md-links", "section-refs", "anchors", "wiki-links"]
    );
}
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_links`
Expected: FAIL to compile with error E0432, unresolved import of `aios_tools::cmd::docs_check::checks::links` (the module does not exist yet).

- [ ] **Step 3: Implement the link checks**

Create `tools/src/cmd/docs_check/checks/links.rs`:

```rust
//! Link checks, ported from check.py L549-698: `md-links`, `section-refs`, `anchors` and
//! `wiki-links`.
//!
//! Every check walks `Repo::md_files` in order and reads prose lines only (`prose_lines`
//! skips fenced and indented code and multi-line HTML comments); code spans and single-line
//! HTML comments are masked with `mask_prose` before matching. Match offsets are byte
//! offsets into the masked line and are only used on that same string (`is_escaped` looks
//! for ASCII backslashes), so they agree with check.py's character offsets. Findings are
//! returned in check.py's production order; `model::collate` merges and sorts them.
//!
//! Accepted divergences from check.py (Unicode edge cases that no tracked file exercises):
//! the shared link regexes and the hub marker `Part of:\s*[...]` use Rust's `\s`, which
//! lacks U+001C..U+001F, and `[0-9]` where Python's `\d` also accepts non-ASCII digits.

use std::collections::{BTreeSet, HashSet};

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{
    heading_number, is_escaped, is_placeholder, mask_prose, prose_lines, split_target,
    strip_inline_md, INLINE_LINK_RE, REF_DEF_RE, SCHEME_RE, SECTION_REF_RE, WIKI_RE,
};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::{paths, pystr};

/// Inline links and reference definitions outside code (check.py `iter_links`, L549-558):
/// `(line number, raw target)`, a line's inline links first, then its reference definition.
pub fn iter_links(repo: &Repo, rel: &str) -> Vec<(usize, String)> {
    let text = repo.text(rel);
    let mut out = Vec::new();
    for (lineno, line) in prose_lines(&text) {
        let masked = mask_prose(line);
        for caps in INLINE_LINK_RE.captures_iter(&masked) {
            let (Some(label), Some(target)) = (caps.get(2), caps.get(3)) else {
                continue;
            };
            // The '[' just before the label decides; an escaped '!' still leaves a link.
            if !is_escaped(&masked, label.start() - 1) {
                out.push((lineno, target.as_str().to_string()));
            }
        }
        let ref_target = REF_DEF_RE
            .captures(&masked)
            .and_then(|caps| caps.get(2))
            .filter(|_| {
                masked
                    .find('[')
                    .is_some_and(|bracket| !is_escaped(&masked, bracket))
            });
        if let Some(target) = ref_target {
            out.push((lineno, target.as_str().to_string()));
        }
    }
    out
}

/// Section numbers of `rel`'s headings (check.py `heading_numbers`, L625-631).
pub fn heading_numbers(repo: &Repo, rel: &str) -> BTreeSet<String> {
    repo.headings(rel)
        .iter()
        .filter_map(|h| heading_number(pystr::strip(&strip_inline_md(&h.text))))
        .collect()
}

/// Sub-documents of a hub (check.py `hub_members`, L634-643): Markdown files in the
/// `<dir>/<stem>/` folder, then flat siblings whose first 2,000 code points contain
/// `Part of: [<hub basename>]`.
pub fn hub_members(repo: &Repo, hub: &str) -> Vec<String> {
    let (stem, _) = paths::splitext(hub);
    let hub_dir = paths::dirname(hub);
    let marker = Regex::new(&format!(
        r"Part of:\s*\[{}\]",
        regex::escape(paths::basename(hub))
    ))
    .expect("an escaped file name between literal brackets is a valid pattern");
    let mut members: Vec<String> = repo
        .md_files()
        .iter()
        .filter(|f| paths::dirname(f) == stem)
        .cloned()
        .collect();
    for f in repo.md_files() {
        if paths::dirname(f) == hub_dir
            && f.as_str() != hub
            && marker.is_match(pystr::char_prefix(&repo.text(f), 2000))
        {
            members.push(f.clone());
        }
    }
    members
}

/// `num` names a heading when it is one of `nums` or a dotted prefix of one (check.py
/// `number_resolves`, L646-647).
pub fn number_resolves(num: &str, nums: &BTreeSet<String>) -> bool {
    let child_prefix = format!("{num}.");
    nums.contains(num) || nums.iter().any(|n| n.starts_with(child_prefix.as_str()))
}

/// `md-links` (check.py `check_md_links`, L575-585).
pub struct MdLinks;

impl Check for MdLinks {
    fn name(&self) -> &'static str {
        "md-links"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for rel in repo.md_files() {
            for (lineno, raw) in iter_links(repo, rel) {
                let Some((path, _fragment)) = split_target(&raw) else {
                    continue;
                };
                if path.is_empty() {
                    continue;
                }
                let resolves = repo
                    .resolve(rel, &path)
                    .is_some_and(|resolved| repo.exists(&resolved));
                if !resolves {
                    out.push(Finding::new(
                        "md-links",
                        rel.as_str(),
                        path.as_str(),
                        format!("broken link -> {path}"),
                        lineno,
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// `section-refs` (check.py `check_section_refs`, L650-669).
pub struct SectionRefs;

impl Check for SectionRefs {
    fn name(&self) -> &'static str {
        "section-refs"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for rel in repo.md_files() {
            let text = repo.text(rel);
            for (lineno, line) in prose_lines(&text) {
                let masked = mask_prose(line);
                for caps in SECTION_REF_RE.captures_iter(&masked) {
                    let (Some(whole), Some(raw), Some(num)) =
                        (caps.get(0), caps.get(1), caps.get(3))
                    else {
                        continue;
                    };
                    if is_escaped(&masked, whole.start()) {
                        continue;
                    }
                    let (raw, num) = (raw.as_str(), num.as_str());
                    if is_placeholder(raw) || SCHEME_RE.is_match(raw) {
                        continue;
                    }
                    let Some(target) = repo.resolve(rel, &pystr::url_unquote(raw)) else {
                        continue;
                    };
                    if !repo.is_file(&target) {
                        continue; // md-links reports the missing file
                    }
                    if number_resolves(num, &heading_numbers(repo, &target)) {
                        continue;
                    }
                    if hub_members(repo, &target)
                        .iter()
                        .any(|member| number_resolves(num, &heading_numbers(repo, member)))
                    {
                        continue;
                    }
                    out.push(Finding::new(
                        "section-refs",
                        rel.as_str(),
                        format!("{raw} §{num}"),
                        format!("no §{num} heading in {target} or its hub members"),
                        lineno,
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// `anchors` (check.py `check_anchors`, L607-622).
pub struct Anchors;

impl Check for Anchors {
    fn name(&self) -> &'static str {
        "anchors"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for rel in repo.md_files() {
            for (lineno, raw) in iter_links(repo, rel) {
                // raw.strip().strip("<>")
                let t = pystr::strip(&raw).trim_matches(['<', '>']);
                let Some((path_part, frag)) = t.split_once('#') else {
                    continue;
                };
                if SCHEME_RE.is_match(t) {
                    continue;
                }
                if frag.is_empty() || is_placeholder(frag) || is_placeholder(path_part) {
                    continue;
                }
                let target = if path_part.is_empty() {
                    Some(rel.clone())
                } else {
                    let path = path_part.split_once('?').map_or(path_part, |(p, _)| p);
                    repo.resolve(rel, &pystr::url_unquote(path))
                };
                let Some(target) = target else {
                    continue;
                };
                if !target.ends_with(".md") || !repo.is_file(&target) {
                    continue;
                }
                let slug = pystr::url_unquote(frag).to_lowercase();
                if !repo.slug_set(&target).contains(&slug) {
                    out.push(Finding::new(
                        "anchors",
                        rel.as_str(),
                        t,
                        format!("no heading for anchor #{frag} in {target}"),
                        lineno,
                    ));
                }
            }
        }
        Ok(out)
    }
}

/// `wiki-links` (check.py `check_wiki_links`, L672-698).
pub struct WikiLinks;

impl Check for WikiLinks {
    fn name(&self) -> &'static str {
        "wiki-links"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        // The vault is every file under docs/, not only Markdown.
        let mut names: HashSet<String> = HashSet::new();
        let mut vault_paths: HashSet<String> = HashSet::new();
        for f in repo.files() {
            let Some(inner) = f.strip_prefix("docs/") else {
                continue;
            };
            let base = paths::basename(f);
            names.insert(base.to_lowercase());
            vault_paths.insert(inner.to_lowercase());
            // `f` ends with ".md" exactly when both its basename and `inner` do.
            if let (Some(base_stem), Some(inner_stem)) =
                (base.strip_suffix(".md"), inner.strip_suffix(".md"))
            {
                names.insert(base_stem.to_lowercase());
                vault_paths.insert(inner_stem.to_lowercase());
            }
        }
        let mut out = Vec::new();
        for rel in repo.md_files() {
            let text = repo.text(rel);
            for (lineno, line) in prose_lines(&text) {
                let masked = mask_prose(line);
                for caps in WIKI_RE.captures_iter(&masked) {
                    let Some(note) = caps.get(2) else {
                        continue;
                    };
                    // The first '[' of "[[" sits two bytes before the note.
                    if is_escaped(&masked, note.start() - 2) {
                        continue;
                    }
                    let note = pystr::strip(note.as_str());
                    if note.is_empty() {
                        continue; // [[#heading]] refers to the same note
                    }
                    let key = note.to_lowercase();
                    let key_stem = key.strip_suffix(".md").unwrap_or(key.as_str());
                    if names.contains(&key)
                        || vault_paths.contains(&key)
                        || vault_paths.contains(key_stem)
                    {
                        continue;
                    }
                    out.push(Finding::new(
                        "wiki-links",
                        rel.as_str(),
                        note,
                        format!("no note named [[{note}]] in the docs/ vault"),
                        lineno,
                    ));
                }
            }
        }
        Ok(out)
    }
}
```

In `tools/src/cmd/docs_check/checks/mod.rs`, add the module declaration directly above the first `use` line (followed by one blank line):

```rust
pub mod links;
```

and replace the whole `pub fn registry()` item that Task 6 wrote (it returns an empty `Vec`; keep its doc comment) with:

```rust
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
    ]
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_links`
Expected: PASS, `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

Run: `cargo test -p aios-tools`
Expected: every test binary reports `test result: ok` (earlier tasks' tests still pass with a non-empty registry).

- [ ] **Step 5: Differential check against check.py on this worktree**

```bash
just tools
diff <(python3 scripts/docs/check.py --all --check md-links,section-refs,anchors,wiki-links) \
     <(target/tools/release/aios docs-check --all --check md-links,section-refs,anchors,wiki-links) && echo "text identical"
diff <(python3 scripts/docs/check.py --json --all --check md-links,section-refs,anchors,wiki-links) \
     <(target/tools/release/aios docs-check --json --all --check md-links,section-refs,anchors,wiki-links) && echo "json identical"
```

Expected: no diff lines, then `text identical` and `json identical`. (On the worktree at plan time the first line of both was `docs-check: 28 findings across 4 checks - 0 new, 28 baselined (1 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)`; the numbers may move as this branch adds files, but the two outputs must stay byte-identical.)

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: `cargo fmt --check` prints nothing and exits 0; clippy finishes with no warnings.

- [ ] **Step 7: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/links.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_links.rs
git commit -F - <<'EOF'
Port docs-check link checks to Rust (md-links, section-refs, anchors, wiki-links)

Line-by-line ports of check.py iter_links, check_md_links,
check_section_refs (heading_numbers, hub_members, number_resolves),
check_anchors and check_wiki_links (L549-698), registered as the first
four checks. The tests pin the production-order findings that check.py's
own functions return for the same fixture files.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 8: doc-map, repo-paths, just-recipes

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/doc_map.rs`
- Create: `tools/src/cmd/docs_check/checks/repo_paths.rs` (unit test `regexes_compile`)
- Create: `tools/src/cmd/docs_check/checks/just_recipes.rs` (unit test `regexes_compile`)
- Create: `tools/tests/checks_paths.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (module block; `registry()` gains three checks)
- Test: `tools/tests/checks_paths.rs`

**Interfaces:**
- Consumes (T6): `Check`, `registry()` as in Task 7. (T3): `Finding::new(...)`, `pub struct Skip(pub String)` (`Debug + Clone + PartialEq + Eq + Display + Error`).
- Consumes (T5, `repo`): `Repo::open`, `Repo::md_files`, `Repo::is_file`, `Repo::text`, `Repo::exists` (as in Task 7); `pub type Region = (String, Option<BTreeSet<usize>>)`; `Repo::current_state_regions(&self) -> anyhow::Result<Vec<Region>>`; `Repo::justfile_recipes(&self) -> (BTreeSet<String>, BTreeSet<String>)` (all names, public names).
- Consumes (T4, `markdown`): `prose_lines(text: &str) -> Vec<(usize, &str)>`, `code_spans(line: &str) -> Vec<String>`, `brace_expand(s: &str) -> Vec<String>`, `clean_repo_path(token: &str) -> String`, `is_path_placeholder(path: &str) -> bool`, `section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)>`.
- Consumes (T1): `pystr::lstrip(s: &str) -> &str`. (T2): `TestRepo::with_files`, `TestRepo::commit(&self, subject: &str)`, `TestRepo::path_str`.
- Produces: `checks::doc_map::{DocMap, DOC_MAP_REL, DOC_MAP_ALLOWLIST}` with `pub const DOC_MAP_REL: &str = "docs/project/doc-map.md"` and `pub const DOC_MAP_ALLOWLIST: [&str; 1]`; `checks::repo_paths::RepoPaths`; `checks::just_recipes::{JustRecipes, documented_recipes}` with `pub fn documented_recipes(repo: &Repo) -> BTreeSet<String>`. `registry()` entries 5-7 are `doc-map`, `repo-paths`, `just-recipes`.

**Parity notes** (check.py line ranges; re-read each before porting):
- `doc-map` (L701-721): a missing doc map yields exactly one finding (`docs/project/doc-map.md` | `missing` | line 0) and no `Skip`. Code spans come from the raw prose line, so a span inside a single-line HTML comment still counts (kept on purpose, the fixture pins it). `brace_expand` is depth-first, innermost group first; every expanded path is checked, duplicates included. The unlisted pass walks `md_files` in sorted order and skips `docs/phases/`, `docs/knowledge/` and the allowlist; an exact path match is required (a listed directory does not cover its files).
- `repo-paths` (L724-748): regions come in `current_state_regions` order (whole-file docs in listing order, then merged-milestone line sets of phase docs); lines outside a region's set are skipped; spans are matched with `REPO_PATH_RE` (`re.match`), then `clean_repo_path`, then the empty/placeholder skip; the target is the cleaned path.
- `just-recipes` (L751-784): `Skip("no justfile")` before any other work when `justfile` is not in the listing; spans are matched with `^just ([A-Za-z0-9_-]+)` against all recipe names (private included); documented names come only from table rows (`lstrip` starts with `|`) inside README `^## Build Commands` .. `^## ` and developer guide `^### 5\.1 ` .. `^#{2,3} `, each only when the file is listed; undocumented = `sorted(public - documented - {"default"})`, which a `BTreeSet` iteration gives directly.
- Expected findings in the tests were recorded from check.py's own `check_doc_map`, `check_repo_paths`, `check_just_recipes` and `documented_recipes` on the same files.

- [ ] **Step 1: Write the failing test**

Create `tools/tests/checks_paths.rs`:

````rust
//! Path checks doc-map, repo-paths and just-recipes (check.py L701-784).
//!
//! The expected findings were recorded by calling check.py's own `check_doc_map`,
//! `check_repo_paths` and `check_just_recipes` on the same files (production order).

mod common;

use std::collections::BTreeSet;

use aios_tools::cmd::docs_check::checks::doc_map::{DocMap, DOC_MAP_ALLOWLIST, DOC_MAP_REL};
use aios_tools::cmd::docs_check::checks::just_recipes::{documented_recipes, JustRecipes};
use aios_tools::cmd::docs_check::checks::repo_paths::RepoPaths;
use aios_tools::cmd::docs_check::checks::{registry, Check};
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const DOC_MAP_README_MD: &str = r#"# Readme
"#;
const DOC_MAP_DOCS_PROJECT_DOC_MAP_MD: &str = r#"# Doc Map

| Topic | Doc |
|---|---|
| Kernel | `docs/kernel/{alpha,beta}.md` |
| Nested | `docs/{kernel/{gamma,delta},project/plan}.md` |
| Directory | `docs/kernel/` |
| Code | `kernel/src/main.rs` |

<!-- `docs/commented.md` -->

```text
`docs/fenced.md`
```
"#;
const DOC_MAP_DOCS_KERNEL_ALPHA_MD: &str = r#"# Alpha
"#;
const DOC_MAP_DOCS_KERNEL_GAMMA_MD: &str = r#"# Gamma
"#;
const DOC_MAP_DOCS_KERNEL_UNLISTED_MD: &str = r#"# Unlisted
"#;
const DOC_MAP_DOCS_KERNEL_SUB_DEEP_MD: &str = r#"# Deep
"#;
const DOC_MAP_DOCS_KERNEL_DIAGRAM_SVG: &str = r#"<svg/>
"#;
const DOC_MAP_DOCS_PROJECT_PLAN_MD: &str = r#"# Plan
"#;
const DOC_MAP_DOCS_PHASES_00_BOOT_MD: &str = r#"# Phase 0: Boot
"#;
const DOC_MAP_DOCS_KNOWLEDGE_DECISIONS_2026_01_01_AB_CHOICE_MD: &str = r#"# Choice
"#;
const MISSING_README_MD: &str = r#"# Readme
"#;
const MISSING_DOCS_KERNEL_ALPHA_MD: &str = r#"# Alpha
"#;
const PATHS_CLAUDE_MD: &str = r#"# Project

Paths `kernel/src/main.rs`, `kernel/src/gone.rs`, `shared/src/lib.rs:12` and `kernel/src/main.rs::kernel_main`.
Ranges `scripts/tool.sh:3-5,9`, directories `uefi-stub/` and `kernel/src/missing/`.
Placeholders `kernel/src/<name>.rs`, `shared/BootInfo`, `kernel/src/NN-x.rs`, `kernel/src/Foo` and `docs/nope.md`.
Punctuation `kernel/src/gone2.rs).`, words `kernel/src/main.rs extra words` and prefix `kernelx/src/a.rs`.
Comments are not masked: <!-- `kernel/src/in-comment.rs` --> and ``kernel/src/double.rs`` counts.

```text
`kernel/src/fenced.rs`
```
"#;
const PATHS__CLAUDE_AGENTS_READER_MD: &str = r#"# Reader

Reads `kernel/src/agent-missing.rs` and `kernel/src/main.rs`.
"#;
const PATHS_DOCS_OTHER_MD: &str = r#"Not current state: `kernel/src/ignored.rs`.
"#;
const PATHS_DOCS_PROJECT_DEVELOPER_GUIDE_MD: &str = r#"# Developer Guide

Run `scripts/nope.py` first.

```text
`kernel/src/fenced2.rs`
```
"#;
const PATHS_DOCS_PHASES_01_MEMORY_MD: &str = r#"# Phase 1: Memory

## Milestone 3 — Done

Uses `kernel/src/merged-missing.rs`.

## Milestone 4 — Planned

Will add `kernel/src/future.rs`.
"#;
const PATHS_KERNEL_SRC_MAIN_RS: &str = r#"fn main() {}
"#;
const PATHS_SHARED_SRC_LIB_RS: &str = r#"//! Shared.
"#;
const PATHS_SCRIPTS_TOOL_SH: &str = r#"#!/bin/sh
"#;
const PATHS_UEFI_STUB_SRC_MAIN_RS: &str = r#"fn main() {}
"#;
const JUST_JUSTFILE: &str = r#"# Test justfile

set shell := ["bash", "-c"]
target := "aarch64"

default: build

build:
    echo build

# Documented in the README prose only
check: build
    echo check

[private]
helper:
    echo helper

[no-cd]
[private]
stacked:
    echo stacked

_hidden:
    echo hidden

undoc:
    echo undoc

[positional-arguments]
docs-check *args:
    echo "$@"

@quiet:
    echo quiet
"#;
const JUST_README_MD: &str = r#"# Readme

## Build Commands

| Command | Description |
|---|---|
| `just build` | Build |
| `just docs-check --all` | Docs drift |

Prose `just check` is not a table row.

## Other

| `just undoc` | outside the section |

Run `just nope`, `just build`, `just helper` and `just _hidden`; `just` alone and `justify` are not recipes.
"#;
const JUST_DOCS_PROJECT_DEVELOPER_GUIDE_MD: &str = r#"# Developer Guide

## 5. Build

### 5.1 Just Commands

| Command | What it does |
|---|---|
| `just quiet` | Quiet |

Also run `just gone`.

### 5.2 Tests

| `just check` | after the section |
"#;
const JUST__CLAUDE_SKILLS_S_SKILL_MD: &str = r#"# Skill

Run `just ghost` then `just stacked`.
"#;
const JUST_DOCS_OTHER_MD: &str = r#"Run `just other-missing`.
"#;
const NO_JUST_README_MD: &str = r#"Run `just build`.
"#;

fn open(t: &TestRepo) -> Repo {
    Repo::open(t.path_str()).expect("open the test repository")
}

fn finding(check: &'static str, file: &str, target: &str, message: &str, line: usize) -> Finding {
    Finding::new(check, file, target, message, line)
}

#[test]
fn doc_map_reports_missing_listed_paths_then_unlisted_docs() {
    let t = TestRepo::with_files(
        "doc-map",
        &[
            ("README.md", DOC_MAP_README_MD),
            ("docs/project/doc-map.md", DOC_MAP_DOCS_PROJECT_DOC_MAP_MD),
            ("docs/kernel/alpha.md", DOC_MAP_DOCS_KERNEL_ALPHA_MD),
            ("docs/kernel/gamma.md", DOC_MAP_DOCS_KERNEL_GAMMA_MD),
            ("docs/kernel/unlisted.md", DOC_MAP_DOCS_KERNEL_UNLISTED_MD),
            ("docs/kernel/sub/deep.md", DOC_MAP_DOCS_KERNEL_SUB_DEEP_MD),
            ("docs/kernel/diagram.svg", DOC_MAP_DOCS_KERNEL_DIAGRAM_SVG),
            ("docs/project/plan.md", DOC_MAP_DOCS_PROJECT_PLAN_MD),
            ("docs/phases/00-boot.md", DOC_MAP_DOCS_PHASES_00_BOOT_MD),
            (
                "docs/knowledge/decisions/2026-01-01-ab-choice.md",
                DOC_MAP_DOCS_KNOWLEDGE_DECISIONS_2026_01_01_AB_CHOICE_MD,
            ),
        ],
    );
    let repo = open(&t);
    let got = DocMap.run(&repo).expect("doc-map runs");
    let want = vec![
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/kernel/beta.md",
            "listed path does not exist: docs/kernel/beta.md",
            5,
        ),
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/kernel/delta.md",
            "listed path does not exist: docs/kernel/delta.md",
            6,
        ),
        finding(
            "doc-map",
            "docs/project/doc-map.md",
            "missing:docs/commented.md",
            "listed path does not exist: docs/commented.md",
            10,
        ),
        finding(
            "doc-map",
            "docs/kernel/sub/deep.md",
            "unlisted",
            "docs/kernel/sub/deep.md is not listed in docs/project/doc-map.md",
            0,
        ),
        finding(
            "doc-map",
            "docs/kernel/unlisted.md",
            "unlisted",
            "docs/kernel/unlisted.md is not listed in docs/project/doc-map.md",
            0,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn doc_map_reports_a_missing_doc_map_once() {
    let t = TestRepo::with_files(
        "doc-map-missing",
        &[
            ("README.md", MISSING_README_MD),
            ("docs/kernel/alpha.md", MISSING_DOCS_KERNEL_ALPHA_MD),
        ],
    );
    let repo = open(&t);
    let got = DocMap.run(&repo).expect("doc-map runs");
    let want = vec![finding(
        "doc-map",
        "docs/project/doc-map.md",
        "missing",
        "docs/project/doc-map.md does not exist",
        0,
    )];
    assert_eq!(got, want);
    assert_eq!(DOC_MAP_REL, "docs/project/doc-map.md");
    assert_eq!(DOC_MAP_ALLOWLIST, ["docs/project/doc-map.md"]);
}

#[test]
fn repo_paths_reports_missing_paths_in_current_state_regions() {
    let t = TestRepo::with_files(
        "repo-paths",
        &[
            ("CLAUDE.md", PATHS_CLAUDE_MD),
            (".claude/agents/reader.md", PATHS__CLAUDE_AGENTS_READER_MD),
            ("docs/other.md", PATHS_DOCS_OTHER_MD),
            (
                "docs/project/developer-guide.md",
                PATHS_DOCS_PROJECT_DEVELOPER_GUIDE_MD,
            ),
            ("docs/phases/01-memory.md", PATHS_DOCS_PHASES_01_MEMORY_MD),
            ("kernel/src/main.rs", PATHS_KERNEL_SRC_MAIN_RS),
            ("shared/src/lib.rs", PATHS_SHARED_SRC_LIB_RS),
            ("scripts/tool.sh", PATHS_SCRIPTS_TOOL_SH),
            ("uefi-stub/src/main.rs", PATHS_UEFI_STUB_SRC_MAIN_RS),
        ],
    );
    // Milestone 3 is merged, so only its section of the phase doc is current state.
    t.commit("Phase 1 M3: Step 1 — allocator");
    let repo = open(&t);
    let got = RepoPaths.run(&repo).expect("repo-paths runs");
    let want = vec![
        finding(
            "repo-paths",
            ".claude/agents/reader.md",
            "kernel/src/agent-missing.rs",
            "path does not exist: kernel/src/agent-missing.rs",
            3,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/gone.rs",
            "path does not exist: kernel/src/gone.rs",
            3,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/missing/",
            "path does not exist: kernel/src/missing/",
            4,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/gone2.rs",
            "path does not exist: kernel/src/gone2.rs",
            6,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/in-comment.rs",
            "path does not exist: kernel/src/in-comment.rs",
            7,
        ),
        finding(
            "repo-paths",
            "CLAUDE.md",
            "kernel/src/double.rs",
            "path does not exist: kernel/src/double.rs",
            7,
        ),
        finding(
            "repo-paths",
            "docs/project/developer-guide.md",
            "scripts/nope.py",
            "path does not exist: scripts/nope.py",
            3,
        ),
        finding(
            "repo-paths",
            "docs/phases/01-memory.md",
            "kernel/src/merged-missing.rs",
            "path does not exist: kernel/src/merged-missing.rs",
            5,
        ),
    ];
    assert_eq!(got, want);
}

#[test]
fn just_recipes_reports_unknown_recipes_then_undocumented_public_ones() {
    let t = TestRepo::with_files(
        "just-recipes",
        &[
            ("justfile", JUST_JUSTFILE),
            ("README.md", JUST_README_MD),
            (
                "docs/project/developer-guide.md",
                JUST_DOCS_PROJECT_DEVELOPER_GUIDE_MD,
            ),
            (".claude/skills/s/SKILL.md", JUST__CLAUDE_SKILLS_S_SKILL_MD),
            ("docs/other.md", JUST_DOCS_OTHER_MD),
        ],
    );
    let repo = open(&t);
    let got = JustRecipes.run(&repo).expect("just-recipes runs");
    let want = vec![
        finding(
            "just-recipes",
            ".claude/skills/s/SKILL.md",
            "ghost",
            "`just ghost` is not a justfile recipe",
            3,
        ),
        finding(
            "just-recipes",
            "README.md",
            "nope",
            "`just nope` is not a justfile recipe",
            16,
        ),
        finding(
            "just-recipes",
            "docs/project/developer-guide.md",
            "gone",
            "`just gone` is not a justfile recipe",
            11,
        ),
        finding(
            "just-recipes",
            "justfile",
            "undocumented:check",
            "public recipe `check` is missing from the README Build \
             Commands and developer-guide §5.1 tables",
            0,
        ),
        finding(
            "just-recipes",
            "justfile",
            "undocumented:undoc",
            "public recipe `undoc` is missing from the README Build \
             Commands and developer-guide §5.1 tables",
            0,
        ),
    ];
    assert_eq!(got, want);
    let documented: BTreeSet<String> = ["build", "docs-check", "quiet"]
        .into_iter()
        .map(String::from)
        .collect();
    assert_eq!(documented_recipes(&repo), documented);
}

#[test]
fn just_recipes_skips_without_a_justfile() {
    let t = TestRepo::with_files("just-recipes-skip", &[("README.md", NO_JUST_README_MD)]);
    let repo = open(&t);
    let err = JustRecipes
        .run(&repo)
        .expect_err("just-recipes needs a justfile");
    assert_eq!(
        err.downcast_ref::<Skip>(),
        Some(&Skip("no justfile".to_string()))
    );
}

#[test]
fn registry_continues_with_the_path_checks() {
    let names: Vec<&str> = registry().iter().map(|c| c.name()).collect();
    assert_eq!(names[4..7], ["doc-map", "repo-paths", "just-recipes"]);
}
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_paths`
Expected: FAIL to compile with error E0432, unresolved imports of `aios_tools::cmd::docs_check::checks::doc_map`, `...::just_recipes` and `...::repo_paths`.

- [ ] **Step 3: Implement the three checks**

Create `tools/src/cmd/docs_check/checks/doc_map.rs`:

```rust
//! `doc-map`, ported from check.py `check_doc_map` (L701-721): every `docs/...` path in a code
//! span of `docs/project/doc-map.md` (brace-expanded) must exist, and every Markdown file under
//! `docs/` outside `docs/phases/` and `docs/knowledge/` must be listed.
//!
//! Like check.py, code spans are read from the raw prose line, so a code span inside a
//! single-line HTML comment still counts as listed.

use std::collections::HashSet;

use super::Check;
use crate::cmd::docs_check::markdown::{brace_expand, code_spans, prose_lines};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;

/// The doc map itself.
pub const DOC_MAP_REL: &str = "docs/project/doc-map.md";
/// Docs under `docs/` that are indexes rather than topics, so the doc map need not list them.
pub const DOC_MAP_ALLOWLIST: [&str; 1] = [DOC_MAP_REL];

/// `doc-map` (check.py `check_doc_map`, L701-721).
pub struct DocMap;

impl Check for DocMap {
    fn name(&self) -> &'static str {
        "doc-map"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(DOC_MAP_REL) {
            return Ok(vec![Finding::new(
                "doc-map",
                DOC_MAP_REL,
                "missing",
                "docs/project/doc-map.md does not exist",
                0,
            )]);
        }
        let mut out = Vec::new();
        let mut listed: HashSet<String> = HashSet::new();
        let text = repo.text(DOC_MAP_REL);
        for (lineno, line) in prose_lines(&text) {
            for span in code_spans(line) {
                if !span.starts_with("docs/") {
                    continue;
                }
                for path in brace_expand(&span) {
                    if !repo.exists(&path) {
                        out.push(Finding::new(
                            "doc-map",
                            DOC_MAP_REL,
                            format!("missing:{path}"),
                            format!("listed path does not exist: {path}"),
                            lineno,
                        ));
                    }
                    listed.insert(path);
                }
            }
        }
        for f in repo.md_files() {
            if !f.starts_with("docs/")
                || f.starts_with("docs/phases/")
                || f.starts_with("docs/knowledge/")
            {
                continue;
            }
            if listed.contains(f) || DOC_MAP_ALLOWLIST.contains(&f.as_str()) {
                continue;
            }
            out.push(Finding::new(
                "doc-map",
                f.as_str(),
                "unlisted",
                format!("{f} is not listed in {DOC_MAP_REL}"),
                0,
            ));
        }
        Ok(out)
    }
}
```

Create `tools/src/cmd/docs_check/checks/repo_paths.rs`:

```rust
//! `repo-paths`, ported from check.py `check_repo_paths` (L724-748): code spans that start
//! with `kernel/`, `shared/`, `uefi-stub/` or `scripts/` in current-state regions must name
//! an existing path.
//!
//! Like check.py, code spans are read from the raw prose line (HTML comments are not
//! masked). Accepted divergence: `\S` in `REPO_PATH_RE` is Rust's, which treats
//! U+001C..U+001F as non-space.

use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{
    clean_repo_path, code_spans, is_path_placeholder, prose_lines,
};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;

/// check.py `REPO_PATH_RE` (L724), applied with `re.match`.
static REPO_PATH_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^((?:kernel|shared|uefi-stub|scripts)/\S*)").expect("valid regex")
});

/// `repo-paths` (check.py `check_repo_paths`, L733-748).
pub struct RepoPaths;

impl Check for RepoPaths {
    fn name(&self) -> &'static str {
        "repo-paths"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for (rel, allowed) in repo.current_state_regions()? {
            let text = repo.text(&rel);
            for (lineno, line) in prose_lines(&text) {
                if allowed
                    .as_ref()
                    .is_some_and(|lines| !lines.contains(&lineno))
                {
                    continue;
                }
                for span in code_spans(line) {
                    let Some(caps) = REPO_PATH_RE.captures(&span) else {
                        continue;
                    };
                    let path = clean_repo_path(&caps[1]);
                    if path.is_empty() || is_path_placeholder(&path) {
                        continue;
                    }
                    if !repo.exists(&path) {
                        out.push(Finding::new(
                            "repo-paths",
                            rel.as_str(),
                            path.as_str(),
                            format!("path does not exist: {path}"),
                            lineno,
                        ));
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        LazyLock::force(&REPO_PATH_RE);
    }
}
```

Create `tools/src/cmd/docs_check/checks/just_recipes.rs`:

```rust
//! `just-recipes`, ported from check.py `documented_recipes` and `check_just_recipes`
//! (L751-784): every `` `just X` `` code span in a current-state region must name a justfile
//! recipe, and every public recipe except `default` must appear in a table row of the README
//! "Build Commands" section or the developer guide's section 5.1.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{code_spans, prose_lines, section_body};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::pystr;

/// check.py L754-755: the README and developer-guide sections that document recipes.
static README_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## Build Commands").expect("valid regex"));
static README_STOP: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
static GUIDE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 5\.1 ").expect("valid regex"));
static GUIDE_STOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{2,3} ").expect("valid regex"));
/// check.py L762 (`re.finditer` over a table row).
static DOCUMENTED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"`just ([A-Za-z0-9_-]+)").expect("valid regex"));
/// check.py L777 (`re.match` on a code span).
static JUST_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^just ([A-Za-z0-9_-]+)").expect("valid regex"));

/// Recipes named as `` `just X`` in the table rows of the README "Build Commands" section and
/// the developer guide's section 5.1 (check.py `documented_recipes`, L751-764). A source file
/// that is not in the repository listing is skipped.
pub fn documented_recipes(repo: &Repo) -> BTreeSet<String> {
    let mut documented = BTreeSet::new();
    collect_documented(
        repo,
        "README.md",
        &README_START,
        &README_STOP,
        &mut documented,
    );
    collect_documented(
        repo,
        "docs/project/developer-guide.md",
        &GUIDE_START,
        &GUIDE_STOP,
        &mut documented,
    );
    documented
}

fn collect_documented(
    repo: &Repo,
    rel: &str,
    start: &Regex,
    stop: &Regex,
    documented: &mut BTreeSet<String>,
) {
    if !repo.is_file(rel) {
        return;
    }
    let text = repo.text(rel);
    for (_, line) in section_body(&text, start, stop) {
        if pystr::lstrip(line).starts_with('|') {
            for caps in DOCUMENTED_RE.captures_iter(line) {
                documented.insert(caps[1].to_string());
            }
        }
    }
}

/// `just-recipes` (check.py `check_just_recipes`, L767-784).
pub struct JustRecipes;

impl Check for JustRecipes {
    fn name(&self) -> &'static str {
        "just-recipes"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file("justfile") {
            return Err(Skip("no justfile".to_string()).into());
        }
        let (names, public) = repo.justfile_recipes();
        let mut out = Vec::new();
        for (rel, allowed) in repo.current_state_regions()? {
            let text = repo.text(&rel);
            for (lineno, line) in prose_lines(&text) {
                if allowed
                    .as_ref()
                    .is_some_and(|lines| !lines.contains(&lineno))
                {
                    continue;
                }
                for span in code_spans(line) {
                    let Some(caps) = JUST_SPAN_RE.captures(&span) else {
                        continue;
                    };
                    let name = &caps[1];
                    if !names.contains(name) {
                        out.push(Finding::new(
                            "just-recipes",
                            rel.as_str(),
                            name,
                            format!("`just {name}` is not a justfile recipe"),
                            lineno,
                        ));
                    }
                }
            }
        }
        let documented = documented_recipes(repo);
        // sorted(public - documented - {"default"}): `public` is a BTreeSet, so it is sorted.
        for name in &public {
            if documented.contains(name) || name == "default" {
                continue;
            }
            out.push(Finding::new(
                "just-recipes",
                "justfile",
                format!("undocumented:{name}"),
                format!(
                    "public recipe `{name}` is missing from the README Build \
                     Commands and developer-guide §5.1 tables"
                ),
                0,
            ));
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        for re in [
            &README_START,
            &README_STOP,
            &GUIDE_START,
            &GUIDE_STOP,
            &DOCUMENTED_RE,
            &JUST_SPAN_RE,
        ] {
            LazyLock::force(re);
        }
    }
}
```

In `tools/src/cmd/docs_check/checks/mod.rs`, replace the line `pub mod links;` with the module block below (alphabetical, as rustfmt's `reorder_modules` keeps it):

```rust
pub mod doc_map;
pub mod just_recipes;
pub mod links;
pub mod repo_paths;
```

and replace the whole `pub fn registry()` item (keep its doc comment) with:

```rust
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
    ]
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_paths`
Expected: PASS, `test result: ok. 6 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

Run: `cargo test -p aios-tools --lib repo_paths && cargo test -p aios-tools --lib just_recipes`
Expected: PASS; each run lists its module's `regexes_compile` test and ends with `test result: ok. 1 passed`.

Run: `cargo test -p aios-tools`
Expected: every test binary reports `test result: ok`.

- [ ] **Step 5: Differential check against check.py on this worktree**

```bash
just tools
diff <(python3 scripts/docs/check.py --all --check doc-map,repo-paths,just-recipes) \
     <(target/tools/release/aios docs-check --all --check doc-map,repo-paths,just-recipes) && echo "text identical"
diff <(python3 scripts/docs/check.py --json --all --check doc-map,repo-paths,just-recipes) \
     <(target/tools/release/aios docs-check --json --all --check doc-map,repo-paths,just-recipes) && echo "json identical"
```

Expected: no diff lines, then `text identical` and `json identical` (plan-time first line of both: `docs-check: 5 findings across 3 checks - 0 new, 5 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)`).

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: no output from `cargo fmt --check`; clippy finishes with no warnings.

- [ ] **Step 7: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/doc_map.rs tools/src/cmd/docs_check/checks/repo_paths.rs tools/src/cmd/docs_check/checks/just_recipes.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_paths.rs
git commit -F - <<'EOF'
Port docs-check doc-map, repo-paths and just-recipes to Rust

Line-by-line ports of check.py check_doc_map, check_repo_paths,
documented_recipes and check_just_recipes (L701-784), registered as
checks five to seven. The tests pin the production-order findings that
check.py's own functions return for the same fixture files, including
the just-recipes Skip without a justfile.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 9: test-count, lock-order (R43)

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/test_count.rs` (unit tests `regexes_compile`, `claim_patterns_capture_the_stated_number`)
- Create: `tools/src/cmd/docs_check/checks/lock_order.rs` (unit tests `regexes_compile`, `split_outside_braces_matches_the_lookahead_split`, `lock_cells_and_statics_match_like_check_py`)
- Create: `tools/tests/checks_code.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (module block; `registry()` gains two checks)
- Test: `tools/tests/checks_code.rs`

**Interfaces:**
- Consumes (T6): `Check`, `registry()`. (T3): `Finding::new(...)`, `Finding::with_detail(self, detail: impl Into<String>) -> Finding`, `Skip(pub String)`.
- Consumes (T5): `Repo::open`, `Repo::files`, `Repo::is_file`, `Repo::text`, `Repo::current_state_regions(&self) -> anyhow::Result<Vec<Region>>`.
- Consumes (T4): `prose_lines(text: &str) -> Vec<(usize, &str)>`, `section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)>`, `table_rows(lines: &[(usize, &str)]) -> Vec<(usize, Vec<String>)>`.
- Consumes (T1): `pystr::splitlines(s: &str) -> Vec<&str>`, `pystr::strip(s: &str) -> &str`, `pystr::is_ascii_digits(s: &str) -> bool`, `pystr::parse_uint(s: &str) -> Option<u64>`, `pystr::int_str(digits: &str) -> String`, `paths::basename(path: &str) -> &str`. (T2): `TestRepo::with_files`, `TestRepo::commit`, `TestRepo::path_str`.
- Produces: `checks::test_count::TestCount`; `checks::lock_order::{LockOrder, TEST_LOCKS, code_mutex_statics, split_outside_braces}` with `pub const TEST_LOCKS: [&str; 2]`, `pub fn code_mutex_statics(repo: &Repo) -> BTreeMap<String, (String, usize)>`, `pub fn split_outside_braces(s: &str) -> Vec<&str>` (replaces the lookahead split R43). `registry()` entries 8-9 are `test-count`, `lock-order`.

**Parity notes** (check.py line ranges; re-read each before porting):
- `test-count` (L787-810): `actual` is the number of `#[test]` substrings in every listed `shared/src/**.rs` file (text count: an attribute inside a comment counts). Only regions whose line set is `None` are read (phase docs record historical counts). Claims are matched on the raw prose line (code spans count), the three regexes of L792-796 in order, each with `finditer`. A claim is compared and printed as Python's `int()` (`pystr::int_str`, any length, leading zeros dropped); the dedupe key is `(line, claimed)` per file. The claim regexes use `[0-9]` where Python has `\d`.
- `code_mutex_statics` (L817-833): skip basename `tests.rs` and any path containing `/tests/`; a line whose strip is `#[cfg(test)]` ends the file when its next non-blank line matches the test-module regex (L828); `STATIC_RE` (L814) plus `\bMutex\s*<` in the type and a name outside `TEST_LOCKS`; the first definition in listing order wins (`setdefault`).
- `lock-order` (L836-925): `Skip("docs/kernel/deadlock-prevention.md not found")` first. Table (L841-855): `section_body(^### 3\.3 , ^### 3\.5 |^## 4\.)` → `table_rows`; the first cell that fully matches `` `([A-Z][A-Z0-9_]*)(?:\[[^\]]*\])?` `` (L847) names the lock; first line wins; `ranks[name]` is overwritten whenever cell 0 is ASCII digits (a value beyond `u64` leaves the lock unranked, an accepted divergence). Findings in this order: undocumented statics (L856-859: sorted, line 0, detail `defined at {file}:{line}`), stale table entries (L860-862: sorted, table line), the `CLAUDE.md` chain (L864-897: first `^Lock ordering[^:]*:\s*(.*)$` line, fences included, plus continuation lines that start with three spaces and are not blank; unknown names first, and here `TEST_LOCKS` are NOT excluded; then every ranked pair `a` in an earlier group, `b` in a later group with `rank(a) > rank(b)`, at the first chain line naming `b`), then comment blocks (L898-925: every `kernel/src/**.rs`, test files included; a `//` line whose lowercase contains `lock ordering` opens a block that continues over `//` lines whose `strip(rstrip("/!"))` is non-empty; names that are not statics and not `TEST_LOCKS`, once per block; the scan resumes at the line that ended the block).
- R43: `re.split(r">(?![^{]*})", s)` becomes `split_outside_braces` (a `>` splits unless the first brace character after it is `}`); the unit test pins Python's results, including `"A>>B"` → `["A", "", "B"]` and `""` → `[""]`.
- Expected findings in the tests were recorded from check.py's own `check_test_count`, `check_lock_order` and `code_mutex_statics` on the same files; the unit-test expectations for the claim, cell, static and split patterns were checked against Python's `re`.

- [ ] **Step 1: Write the failing test**

Create `tools/tests/checks_code.rs`:

````rust
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
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_code`
Expected: FAIL to compile with error E0432, unresolved imports of `aios_tools::cmd::docs_check::checks::lock_order` and `aios_tools::cmd::docs_check::checks::test_count`.

- [ ] **Step 3: Implement the two checks**

Create `tools/src/cmd/docs_check/checks/test_count.rs`:

```rust
//! `test-count`, ported from check.py `check_test_count` (L787-810): host test counts stated
//! in whole-file current-state docs must equal the number of `#[test]` occurrences in
//! `shared/src/**.rs`.
//!
//! The count is textual, like check.py's `re.findall(r"#\[test\]")`: an attribute inside a
//! comment counts too. Claims are matched on the raw prose line (code spans included). A
//! claimed number is compared and printed as Python's `int()` would (`pystr::int_str`: leading
//! zeros dropped, any length). Accepted divergences: `[0-9]` replaces Python's `\d`, and the
//! regex crate's `\b` follows Unicode word characters that differ slightly from Python's.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::prose_lines;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr;

/// The three claim patterns of check.py L792-796 (`claim_res`), tried in this order on every line.
static CLAIM_RES: LazyLock<[Regex; 3]> = LazyLock::new(|| {
    [
        Regex::new(r"<!--\s*gen:test-count\s*-->\s*([0-9]+)").expect("valid regex"),
        Regex::new(
            r"(?i)\bcurrent(?:ly)?\b[^\n]{0,60}?\b([0-9]{2,5}) (?:host(?:-side)? |unit )?tests\b",
        )
        .expect("valid regex"),
        Regex::new(r"(?i)\bcurrent test distribution \(([0-9]+) tests\)").expect("valid regex"),
    ]
});

/// `test-count` (check.py `check_test_count`, L787-810).
pub struct TestCount;

impl Check for TestCount {
    fn name(&self) -> &'static str {
        "test-count"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let actual: usize = repo
            .files()
            .iter()
            .filter(|f| f.starts_with("shared/src/") && f.ends_with(".rs"))
            .map(|f| repo.text(f).matches("#[test]").count())
            .sum();
        let actual_str = actual.to_string();
        let mut out = Vec::new();
        for (rel, allowed) in repo.current_state_regions()? {
            if allowed.is_some() {
                continue; // phase docs record historical counts
            }
            let mut seen_lines: HashSet<(usize, String)> = HashSet::new();
            let text = repo.text(&rel);
            for (lineno, line) in prose_lines(&text) {
                for rx in CLAIM_RES.iter() {
                    for caps in rx.captures_iter(line) {
                        let claimed = pystr::int_str(&caps[1]);
                        if claimed != actual_str && seen_lines.insert((lineno, claimed.clone())) {
                            out.push(Finding::new(
                                "test-count",
                                rel.as_str(),
                                format!("claimed:{claimed}"),
                                format!(
                                    "states {claimed} tests; shared/src has {actual} \
                                     #[test] functions"
                                ),
                                lineno,
                            ));
                        }
                    }
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        LazyLock::force(&CLAIM_RES);
    }

    #[test]
    fn claim_patterns_capture_the_stated_number() {
        let first = |i: usize, line: &str| -> Option<String> {
            CLAIM_RES[i].captures(line).map(|caps| caps[1].to_string())
        };
        assert_eq!(
            first(0, "n: <!-- gen:test-count -->0012").as_deref(),
            Some("0012")
        );
        assert_eq!(
            first(1, "Currently 99 host tests pass").as_deref(),
            Some("99")
        );
        assert_eq!(
            first(1, "currently 12 host-side tests").as_deref(),
            Some("12")
        );
        assert_eq!(first(1, "We currently run 4 host-side tests"), None);
        assert_eq!(first(1, "Currently 100000 tests"), None);
        assert_eq!(
            first(2, "Current test distribution (5 tests)").as_deref(),
            Some("5")
        );
    }
}
```

Create `tools/src/cmd/docs_check/checks/lock_order.rs`:

```rust
//! `lock-order`, ported from check.py `code_mutex_statics` and `check_lock_order`
//! (L813-925): production `Mutex` statics in `kernel/src` versus the lock table of
//! `docs/kernel/deadlock-prevention.md` sections 3.3-3.4, the `Lock ordering` chain in
//! `CLAUDE.md`, and `lock ordering` comment blocks in kernel code.
//!
//! `split_outside_braces` replaces check.py's `re.split(r">(?![^{]*})", ...)` (the regex
//! crate has no lookaround). Accepted divergences: a rank cell counts only when it is ASCII
//! digits that fit `u64` (Python's `isdigit()`/`int()` also accept other decimal digits and
//! any size), and `\b`/`\w`/`\s` follow the regex crate's Unicode classes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use super::Check;
use crate::cmd::docs_check::markdown::{section_body, table_rows};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::{paths, pystr};

/// Test-only locks named as excluded in deadlock-prevention.md section 3.3.
pub const TEST_LOCKS: [&str; 2] = ["TEST_CHANNEL", "PI_TEST_CHANNEL"];

const DEADLOCK_DOC: &str = "docs/kernel/deadlock-prevention.md";

/// check.py `LOCK_NAME_RE` (L813), used with `findall`.
static LOCK_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b[A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+\b").expect("valid regex"));
/// check.py `STATIC_RE` (L814), applied with `re.match`.
static STATIC_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?static\s+([A-Z][A-Z0-9_]*)\s*:\s*(.*)$")
        .expect("valid regex")
});
/// check.py L828 (`re.match`): an inline test module header.
static TEST_MOD_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{").expect("valid regex")
});
/// check.py L831 (`re.search` on the static's type).
static MUTEX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\bMutex\s*<").expect("valid regex"));
/// check.py L841: the section 3.3-3.4 body of the deadlock doc.
static TABLE_START: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 3\.3 ").expect("valid regex"));
static TABLE_STOP: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### 3\.5 |^## 4\.").expect("valid regex"));
/// check.py L847 (`re.fullmatch` on a table cell).
static LOCK_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^`([A-Z][A-Z0-9_]*)(?:\[[^\]]*\])?`$").expect("valid regex"));
/// check.py L869 (`re.match` on each CLAUDE.md line).
static CHAIN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Lock ordering[^:]*:\s*(.*)$").expect("valid regex"));

/// Split at every `>` that is not inside `{...}`: check.py's `re.split(r">(?![^{]*})", s)`.
/// The lookahead fails exactly when a `}` comes before any `{` after the `>`, i.e. the `>`
/// sits inside a brace group.
pub fn split_outside_braces(s: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut start = 0;
    for (i, c) in s.char_indices() {
        if c != '>' {
            continue;
        }
        let inside = s[i + 1..].chars().find(|&ch| ch == '{' || ch == '}') == Some('}');
        if !inside {
            parts.push(&s[start..i]);
            start = i + 1;
        }
    }
    parts.push(&s[start..]);
    parts
}

/// Production `Mutex` statics in `kernel/src`: name -> (file, 1-based line) of the first
/// definition in file order (check.py `code_mutex_statics`, L817-833). Files named
/// `tests.rs` or under a `tests/` directory are skipped, and a `#[cfg(test)]` line whose
/// next non-blank line opens a module ends the scan of that file.
pub fn code_mutex_statics(repo: &Repo) -> BTreeMap<String, (String, usize)> {
    let mut statics = BTreeMap::new();
    for f in repo.files() {
        if !(f.starts_with("kernel/src/") && f.ends_with(".rs")) {
            continue;
        }
        if paths::basename(f) == "tests.rs" || f.contains("/tests/") {
            continue;
        }
        let text = repo.text(f);
        let lines = pystr::splitlines(&text);
        for (i, line) in lines.iter().copied().enumerate() {
            if pystr::strip(line) == "#[cfg(test)]" {
                let next = lines[i + 1..]
                    .iter()
                    .copied()
                    .find(|l| !pystr::strip(l).is_empty())
                    .unwrap_or_default();
                if TEST_MOD_RE.is_match(next) {
                    break; // an inline test module runs to the end of the file by convention
                }
            }
            let Some(caps) = STATIC_RE.captures(line) else {
                continue;
            };
            let name = &caps[1];
            if MUTEX_RE.is_match(&caps[2]) && !TEST_LOCKS.contains(&name) {
                statics
                    .entry(name.to_string())
                    .or_insert_with(|| (f.clone(), i + 1));
            }
        }
    }
    statics
}

/// `lock-order` (check.py `check_lock_order`, L836-925; the lock table part is L836-863).
pub struct LockOrder;

impl Check for LockOrder {
    fn name(&self) -> &'static str {
        "lock-order"
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(DEADLOCK_DOC) {
            return Err(Skip(format!("{DEADLOCK_DOC} not found")).into());
        }
        let statics = code_mutex_statics(repo);
        let doc_text = repo.text(DEADLOCK_DOC);
        let body = section_body(&doc_text, &TABLE_START, &TABLE_STOP);
        let mut doc_locks: BTreeMap<String, usize> = BTreeMap::new();
        let mut ranks: HashMap<String, u64> = HashMap::new();
        for (lineno, cells) in table_rows(&body) {
            let Some(name) = cells
                .iter()
                .find_map(|c| LOCK_CELL_RE.captures(c).map(|caps| caps[1].to_string()))
            else {
                continue;
            };
            doc_locks.entry(name.clone()).or_insert(lineno);
            let rank = cells
                .first()
                .map(String::as_str)
                .filter(|c| pystr::is_ascii_digits(c))
                .and_then(pystr::parse_uint);
            if let Some(rank) = rank {
                ranks.insert(name, rank);
            }
        }
        let mut out = Vec::new();
        for (name, (file, line)) in &statics {
            if doc_locks.contains_key(name) {
                continue;
            }
            out.push(
                Finding::new(
                    "lock-order",
                    DEADLOCK_DOC,
                    format!("undocumented:{name}"),
                    format!("production lock {name} is not in §3.3/§3.4"),
                    0,
                )
                .with_detail(format!("defined at {file}:{line}")),
            );
        }
        for (name, line) in &doc_locks {
            if statics.contains_key(name) {
                continue;
            }
            out.push(Finding::new(
                "lock-order",
                DEADLOCK_DOC,
                format!("stale:{name}"),
                format!("§3.3/§3.4 lists {name}, which is not a Mutex static in kernel/src"),
                *line,
            ));
        }
        out.extend(chain_findings(repo, &statics, &ranks));
        out.extend(comment_findings(repo, &statics));
        Ok(out)
    }
}

/// The first `Lock ordering ...:` line of CLAUDE.md and its continuation lines (check.py
/// L864-897): unknown names, then pairs of groups ranked in the wrong order.
fn chain_findings(
    repo: &Repo,
    statics: &BTreeMap<String, (String, usize)>,
    ranks: &HashMap<String, u64>,
) -> Vec<Finding> {
    let text = repo.text("CLAUDE.md");
    let lines = pystr::splitlines(&text);
    let Some((first, head)) = lines.iter().copied().enumerate().find_map(|(i, line)| {
        CHAIN_RE
            .captures(line)
            .map(|caps| (i, caps.get(1).map_or("", |m| m.as_str())))
    }) else {
        return Vec::new();
    };
    // Continuation lines start with three spaces and are not blank.
    let mut chain: Vec<(usize, &str)> = vec![(first + 1, head)];
    for (j, next) in lines.iter().copied().enumerate().skip(first + 1) {
        if !next.starts_with("   ") || pystr::strip(next).is_empty() {
            break;
        }
        chain.push((j + 1, pystr::strip(next)));
    }
    // The first chain line naming each lock.
    let mut name_line: HashMap<&str, usize> = HashMap::new();
    for &(lineno, part) in &chain {
        for m in LOCK_NAME_RE.find_iter(part) {
            name_line.entry(m.as_str()).or_insert(lineno);
        }
    }
    let chain_text = chain
        .iter()
        .map(|(_, part)| *part)
        .collect::<Vec<_>>()
        .join(" ");
    if chain_text.is_empty() {
        return Vec::new();
    }
    let groups: Vec<Vec<&str>> = split_outside_braces(&chain_text)
        .into_iter()
        .map(|part| {
            LOCK_NAME_RE
                .find_iter(part)
                .map(|m| m.as_str())
                .collect::<Vec<_>>()
        })
        .filter(|group| !group.is_empty())
        .collect();
    let line_of = |name: &str| name_line.get(name).copied().unwrap_or(0);
    let mut out = Vec::new();
    for group in &groups {
        for &name in group {
            if !statics.contains_key(name) {
                out.push(Finding::new(
                    "lock-order",
                    "CLAUDE.md",
                    format!("unknown:{name}"),
                    format!(
                        "lock ordering names {name}, which is not a Mutex static in kernel/src"
                    ),
                    line_of(name),
                ));
            }
        }
    }
    for (i, earlier) in groups.iter().enumerate() {
        for later in &groups[i + 1..] {
            for &a in earlier {
                for &b in later {
                    let (Some(rank_a), Some(rank_b)) = (ranks.get(a), ranks.get(b)) else {
                        continue;
                    };
                    if rank_a > rank_b {
                        out.push(Finding::new(
                            "lock-order",
                            "CLAUDE.md",
                            format!("order:{a}>{b}"),
                            format!(
                                "CLAUDE.md orders {a} before {b}, §3.3 ranks them \
                                 {rank_a} and {rank_b}"
                            ),
                            line_of(b),
                        ));
                    }
                }
            }
        }
    }
    out
}

/// `lock ordering` comment blocks in every `kernel/src/**.rs` file, test files included
/// (check.py L898-925): a `//` line mentioning "lock ordering" (any case) starts a block that
/// runs over the following non-empty `//` lines; each lock name that is neither a static nor
/// a test lock is reported once per block.
fn comment_findings(repo: &Repo, statics: &BTreeMap<String, (String, usize)>) -> Vec<Finding> {
    let mut out = Vec::new();
    for f in repo.files() {
        if !(f.starts_with("kernel/src/") && f.ends_with(".rs")) {
            continue;
        }
        let text = repo.text(f);
        let lines = pystr::splitlines(&text);
        let mut i = 0;
        while i < lines.len() {
            let s = pystr::strip(lines[i]);
            if !(s.starts_with("//") && s.to_lowercase().contains("lock ordering")) {
                i += 1;
                continue;
            }
            let mut block = vec![(i + 1, s)];
            let mut j = i + 1;
            while j < lines.len() {
                let t = pystr::strip(lines[j]);
                if !t.starts_with("//") || pystr::strip(t.trim_end_matches(['/', '!'])).is_empty() {
                    break;
                }
                block.push((j + 1, t));
                j += 1;
            }
            let mut reported: HashSet<&str> = HashSet::new();
            for &(ln, t) in &block {
                for m in LOCK_NAME_RE.find_iter(t) {
                    let name = m.as_str();
                    if !statics.contains_key(name)
                        && !TEST_LOCKS.contains(&name)
                        && reported.insert(name)
                    {
                        out.push(Finding::new(
                            "lock-order",
                            f.as_str(),
                            format!("unknown:{name}"),
                            format!(
                                "lock-ordering comment names {name}, which is not a Mutex static"
                            ),
                            ln,
                        ));
                    }
                }
            }
            i = j;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        for re in [
            &LOCK_NAME_RE,
            &STATIC_RE,
            &TEST_MOD_RE,
            &MUTEX_RE,
            &TABLE_START,
            &TABLE_STOP,
            &LOCK_CELL_RE,
            &CHAIN_RE,
        ] {
            LazyLock::force(re);
        }
    }

    #[test]
    fn split_outside_braces_matches_the_lookahead_split() {
        // Expected values are Python's re.split(r">(?![^{]*})", s).
        assert_eq!(split_outside_braces("A > B"), ["A ", " B"]);
        assert_eq!(
            split_outside_braces("A > {C, D > E} > F"),
            ["A ", " {C, D > E} ", " F"]
        );
        assert_eq!(split_outside_braces("A>>B"), ["A", "", "B"]);
        assert_eq!(split_outside_braces("A } > B"), ["A } ", " B"]);
        assert_eq!(split_outside_braces("{A > B"), ["{A ", " B"]);
        assert_eq!(split_outside_braces("A"), ["A"]);
        assert_eq!(split_outside_braces(""), [""]);
    }

    #[test]
    fn lock_cells_and_statics_match_like_check_py() {
        let cell = |c: &str| LOCK_CELL_RE.captures(c).map(|caps| caps[1].to_string());
        assert_eq!(cell("`BETA_LOCK[cpu]`").as_deref(), Some("BETA_LOCK"));
        assert_eq!(cell("`ALPHA`").as_deref(), Some("ALPHA"));
        assert_eq!(cell("`lower_lock`"), None);
        assert_eq!(cell("`A_LOCK` x"), None);
        let caps = STATIC_RE
            .captures("pub(crate) static BETA_LOCK: [spin::Mutex<()>; 4] = x;")
            .expect("a static line");
        assert_eq!(&caps[1], "BETA_LOCK");
        assert!(MUTEX_RE.is_match(&caps[2]));
        assert!(TEST_MOD_RE.is_match("pub(crate) mod tests {"));
        assert!(!TEST_MOD_RE.is_match("fn helper() {}"));
    }
}
```

In `tools/src/cmd/docs_check/checks/mod.rs`, replace the module block from Task 8 with:

```rust
pub mod doc_map;
pub mod just_recipes;
pub mod links;
pub mod lock_order;
pub mod repo_paths;
pub mod test_count;
```

and replace the whole `pub fn registry()` item (keep its doc comment) with:

```rust
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
        Box::new(test_count::TestCount),
        Box::new(lock_order::LockOrder),
    ]
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_code`
Expected: PASS, `test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

Run: `cargo test -p aios-tools --lib lock_order && cargo test -p aios-tools --lib test_count`
Expected: PASS; the `lock_order` run ends with `test result: ok. 3 passed`, the `test_count` run with `test result: ok. 2 passed`.

Run: `cargo test -p aios-tools`
Expected: every test binary reports `test result: ok`.

- [ ] **Step 5: Differential check against check.py on this worktree**

```bash
just tools
diff <(python3 scripts/docs/check.py --all --check test-count,lock-order) \
     <(target/tools/release/aios docs-check --all --check test-count,lock-order) && echo "text identical"
diff <(python3 scripts/docs/check.py --json --all --check test-count,lock-order) \
     <(target/tools/release/aios docs-check --json --all --check test-count,lock-order) && echo "json identical"
```

Expected: no diff lines, then `text identical` and `json identical` (plan-time first line of both: `docs-check: 17 findings across 2 checks - 0 new, 17 baselined (0 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)`; the real repository's 16 lock-order findings exercise the chain, the table and the comment blocks).

- [ ] **Step 6: Format and lint**

```bash
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: no output from `cargo fmt --check`; clippy finishes with no warnings.

- [ ] **Step 7: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/test_count.rs tools/src/cmd/docs_check/checks/lock_order.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_code.rs
git commit -F - <<'EOF'
Port docs-check test-count and lock-order to Rust

Line-by-line ports of check.py check_test_count, code_mutex_statics and
check_lock_order (L787-925), registered as checks eight and nine. The
lookahead split of the CLAUDE.md lock chain, re.split(r">(?![^{]*})"),
becomes split_outside_braces, pinned by unit tests against Python's
results. The integration tests pin the production-order findings that
check.py's own functions return for the same fixture files.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 10: milestone-status (R45) and phase-count

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/milestones.rs` (checks `MilestoneStatus`, `PhaseCount`; unit tests)
- Create: `tools/tests/checks_status.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (add `pub mod milestones;`, append two registry entries)
- Test: `tools/tests/checks_status.rs`, `#[cfg(test)] mod tests` in `milestones.rs`

**Interfaces:**
- Consumes:
  - T1 `aios_tools::pystr`: `pub fn strip(s: &str) -> &str`, `pub fn splitlines(s: &str) -> Vec<&str>`, `pub fn is_ascii_digits(s: &str) -> bool`, `pub fn parse_uint(s: &str) -> Option<u64>`, `pub fn int_str(digits: &str) -> String`.
  - T3 `cmd::docs_check::model`: `Finding::new(check: &'static str, file: impl Into<String>, target: impl Into<String>, message: impl Into<String>, line: usize) -> Finding` (`Finding: Debug + Clone + PartialEq + Eq`), `pub struct Skip(pub String)` (implements `std::error::Error`).
  - T4 `cmd::docs_check::markdown`: `pub fn prose_lines(text: &str) -> Vec<(usize, &str)>`, `pub fn section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)>`, `pub fn table_rows(lines: &[(usize, &str)]) -> Vec<(usize, Vec<String>)>`, `pub fn milestone_tokens(text: &str) -> BTreeSet<u64>`.
  - T5 `cmd::docs_check::repo::Repo`: `open(root: &str) -> anyhow::Result<Repo>`, `is_file(&self, rel: &str) -> bool`, `text(&self, rel: &str) -> Rc<str>`, `merged_milestones(&self) -> anyhow::Result<Rc<BTreeMap<u64, u64>>>` (milestone to phase; `Err` downcasts to `Skip` for a shallow clone), `phase_docs(&self) -> Vec<(u64, String)>`, `milestone_sections(&self, rel: &str) -> BTreeMap<u64, (usize, usize)>`.
  - T6 `cmd::docs_check::checks`: `pub trait Check { fn name(&self) -> &'static str; fn describe(&self) -> &'static str; fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>>; }`, `pub fn registry() -> Vec<Box<dyn Check>>` (after T9: the nine checks `md-links` .. `lock-order`); `just tools` recipe and `target/tools/release/aios`.
  - T2 `tools/tests/common`: `TestRepo::with_files(label: &str, files: &[(&str, &str)]) -> TestRepo`, `TestRepo::commit(&self, subject: &str)`, `TestRepo::adopt(root: PathBuf) -> TestRepo`, `TestRepo::path_str(&self) -> &str`, `unique_dir(label: &str) -> PathBuf`, `git(dir: &Path, args: &[&str]) -> String`.
- Produces (module `aios_tools::cmd::docs_check::checks::milestones`):
  - `pub struct MilestoneStatus;` and `pub struct PhaseCount;`, both `impl Check` (names `milestone-status`, `phase-count`).
  - `pub fn is_unchecked_task(line: &str) -> bool` (R45).
  - `pub fn status_findings(rel: &str, target: &str, status: &str, ms: &BTreeSet<u64>, done: &BTreeSet<u64>, line: usize) -> Vec<Finding>`.
  - `pub fn fmt_ms(ms: &BTreeSet<u64>) -> String`.
  - `registry()` now returns eleven checks, ending `..., LockOrder, MilestoneStatus, PhaseCount`.

**Parity notes (check.py at 33c6b3d):**
- milestone-status = L928-976, `status_findings` = L979-989, `fmt_ms` = L992-993, phase-count = L996-1017. Findings are produced in check.py's order: the `Skip` check first; then per phase doc (in `phase_docs()` order) its status finding followed by its unchecked-task findings for merged milestones in ascending order; then the README `latest:` finding; then the `§8` table rows in table order; then the `§8.1` gaps in ascending order.
- Skip messages are exact: `no 'Phase N MK:' commits found on main's first-parent history` (empty merged map), `docs/project/development-plan.md not found` (phase-count). A shallow clone's `Skip` comes from `Repo::merged_milestones` and propagates unchanged through `?`.
- Status line (L938-943): first `splitlines()` line matching `^\*\*Status:\*\*\s*(.*)$`; the status is the Python-stripped group, line 0 and `""` when absent.
- R45 (L951) has a negative lookahead; `is_unchecked_task` is the contract's code replacement: match `^\s*[-*] \[ \] ` and reject when the rest starts with `~~`.
- `phase_ms` (L933-937) records every phase doc, including one with no milestone sections; L966 `phase_ms.get(phase) or {...}` falls back to the merged milestones of that phase when the entry is missing or empty (Python truthiness), which the `Some(ms) if !ms.is_empty()` arm reproduces.
- `§8` rows (L962-968): skipped unless `len(cells) >= 6` and `cells[0]` is ASCII digits; the target prints `str(int(cells[0]))` (`int_str`, so `04` gives `§8:phase-4`), the status text is `cells[5]` as `table_rows` stripped it.
- phase-count (L1000-1016): `actual` counts `§8` rows whose first cell is digits, whatever the number of cells; sources in the order plan, README (`N phases across`), rule 07, CLAUDE.md (`N phases`); raw prose lines (not masked); the comparison is numeric but the target and message keep the raw digits (`claimed:04`, `says 04 phases`).
- Expected findings in the tests below were recorded by running check.py's own `check_milestone_status`, `check_phase_count` and `status_findings` on repositories with exactly these files and commit subjects.

- [ ] **Step 1: Write the failing integration test**

Create `tools/tests/checks_status.rs`:

````rust
//! milestone-status and phase-count on small committed repositories. The
//! expected findings were recorded from check.py's `check_milestone_status` and
//! `check_phase_count` on the same files and history (production order, before
//! merging by key).

mod common;

use aios_tools::cmd::docs_check::checks::milestones::{MilestoneStatus, PhaseCount};
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::{Finding, Skip};
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const STATUS_FILES: &[(&str, &str)] = &[
    (
        "docs/phases/00-foundation.md",
        r#"# Phase 0: Foundation

**Status:** In Progress

## Milestones

| Milestone | Steps |
|---|---|
| **M1 — Boot** | 1 |

## Milestone 1 — Boot

- [x] Entry stub
- [ ] Banner
  * [ ] Nested banner detail
- [ ] ~~Legacy console~~ — deferred to M4

## Milestone 2 — UART

- [x] Driver
"#,
    ),
    (
        "docs/phases/01-memory.md",
        r#"# Phase 1: Memory

**Status:**   Planned

## Milestone 3 — Allocator

- [ ] Buddy allocator

## Milestone 4 — Paging

- [ ] Page tables
"#,
    ),
    (
        "docs/phases/02-next.md",
        r#"# Phase 2: Next

**Status:** Complete

## Milestone 5 — Later

- [ ] Future work
"#,
    ),
    (
        "docs/phases/03-extra.md",
        r#"# Phase 3: Extra

## Milestone 6 — Extra

- [x] Done
"#,
    ),
    (
        "docs/phases/06-empty.md",
        r#"# Phase 6: Empty

**Status:** Planned
"#,
    ),
    (
        "README.md",
        r#"# Fixture

Status: M1–M3 merged.
"#,
    ),
    (
        "docs/project/development-plan.md",
        r#"# Development Plan

## 8. Phase Detail Reference

| Phase | Name | Tier | Weeks | Deliverable | Status |
|---|---|---|---|---|---|
| 0 | Foundation | 1 | 2 | Boots | Complete |
| 1 | Memory | 1 | 2 | Allocates | Planned |
| 04 | Later | 1 | 2 | Something | Complete |
| 5 | Orphan | 1 | 2 | Nothing | In Progress |
| 6 | Empty | 1 | 2 | Nothing | Planned |
| 7 | Short | 1 |
| x | Bad | 1 | 2 | Nothing | Complete |

## 8.1 Actual Progress

### Velocity Summary

| Phase | Planned | Actual | Speedup | Milestones |
|---|---|---|---|---|
| 0 (Foundation) | 2 weeks | 1 day | 10x | M1–M2 |
| 1 (Memory) | 2 weeks | 1 day | 10x | M3 |
"#,
    ),
];

const PHASE_FILES: &[(&str, &str)] = &[
    (
        "docs/project/development-plan.md",
        r#"# Development Plan

3 phases across 1 tier.

## 8. Phase Detail Reference

| Phase | Name | Status |
|---|---|---|
| 0 | Foundation | Complete |
| 1 | Memory | Complete |
| 02 | Next | Planned |
| — | Later | Planned |

The plan lists 03 phases across two tiers and 04 phases across one.

## 9. Other

| 3 | Not counted | x |
"#,
    ),
    (
        "README.md",
        r#"# Readme

We plan 5 phases across 2 tiers and v2 phases across none.

```text
7 phases across 1 tier in a fence.
```
"#,
    ),
    (
        ".claude/rules/07-milestone-numbering.md",
        r#"# Milestone Numbering

47 phases with variable milestones; 3 phases are done.
"#,
    ),
    (
        "CLAUDE.md",
        r#"# Project

Plan: 3 phases, then 12 phases later.
"#,
    ),
];

/// Subjects committed after "Initial", oldest first. "Docs: ..." is not a
/// milestone subject; merged milestones: M1, M2 (phase 0), M3 (1), M6 (3),
/// M9 (5, no phase doc) and M10 (6, a phase doc without milestone sections).
const STATUS_COMMITS: [&str; 7] = [
    "Phase 0 M1: Step 1 — boot stub",
    "Docs: tidy the phase docs",
    "Phase 0 M2: Step 2 — uart console",
    "Phase 1 M3: Step 1 — buddy allocator",
    "Phase 3 M6: Step 1 — extra",
    "Phase 5 M9: Step 1 — orphan",
    "Phase 6 M10: Step 1 — empty",
];

fn status_repo(label: &str) -> TestRepo {
    let repo = TestRepo::with_files(label, STATUS_FILES);
    for subject in STATUS_COMMITS {
        repo.commit(subject);
    }
    repo
}

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

fn skip_message(result: anyhow::Result<Vec<Finding>>) -> String {
    let err = result.expect_err("the check should be skipped");
    match err.downcast_ref::<Skip>() {
        Some(skip) => skip.0.clone(),
        None => panic!("expected a Skip, got: {err:#}"),
    }
}

#[test]
fn check_names() {
    assert_eq!(MilestoneStatus.name(), "milestone-status");
    assert_eq!(PhaseCount.name(), "phase-count");
}

#[test]
fn milestone_status_matches_check_py() {
    let repo = status_repo("ms-status");
    let found = MilestoneStatus
        .run(&open(&repo))
        .expect("milestone-status runs");
    let expected = vec![
        Finding::new(
            "milestone-status",
            "docs/phases/00-foundation.md",
            "status",
            "all milestones (M1, M2) are merged but status is 'In Progress'",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/00-foundation.md",
            "M1:unchecked",
            "merged milestone M1 still has 2 unchecked task(s)",
            14,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/01-memory.md",
            "status",
            "M3 merged but status is 'Planned'",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/01-memory.md",
            "M3:unchecked",
            "merged milestone M3 still has 1 unchecked task(s)",
            7,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/02-next.md",
            "status",
            "status is 'Complete' but no milestone is merged",
            3,
        ),
        Finding::new(
            "milestone-status",
            "docs/phases/03-extra.md",
            "status",
            "all milestones (M6) are merged but status is ''",
            0,
        ),
        Finding::new(
            "milestone-status",
            "README.md",
            "latest:M10",
            "README status does not mention the latest merged milestone M10",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-1",
            "M3 merged but status is 'Planned'",
            8,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-4",
            "status is 'Complete' but no milestone is merged",
            9,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-5",
            "all milestones (M9) are merged but status is 'In Progress'",
            10,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8:phase-6",
            "all milestones (M10) are merged but status is 'Planned'",
            11,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M6",
            "§8.1 Velocity Summary has no row covering merged milestone M6",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M9",
            "§8.1 Velocity Summary has no row covering merged milestone M9",
            0,
        ),
        Finding::new(
            "milestone-status",
            "docs/project/development-plan.md",
            "§8.1:M10",
            "§8.1 Velocity Summary has no row covering merged milestone M10",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn milestone_status_skips_without_phase_commits() {
    let repo = TestRepo::with_files("ms-nophase", STATUS_FILES);
    repo.commit("Docs: no phase subjects");
    assert_eq!(
        skip_message(MilestoneStatus.run(&open(&repo))),
        "no 'Phase N MK:' commits found on main's first-parent history"
    );
}

#[test]
fn milestone_status_skips_a_shallow_clone() {
    let source = status_repo("ms-shallow-src");
    let dir = common::unique_dir("ms-shallow");
    let dest = dir.to_str().expect("UTF-8 temp path").to_string();
    let url = format!("file://{}", source.path_str());
    common::git(&dir, &["clone", "-q", "--depth", "1", &url, &dest]);
    let clone = TestRepo::adopt(dir);
    assert_eq!(
        skip_message(MilestoneStatus.run(&open(&clone))),
        "shallow clone: git history unavailable (use fetch-depth: 0)"
    );
}

#[test]
fn phase_count_matches_check_py() {
    let repo = TestRepo::with_files("phase-count", PHASE_FILES);
    let found = PhaseCount.run(&open(&repo)).expect("phase-count runs");
    let expected = vec![
        Finding::new(
            "phase-count",
            "docs/project/development-plan.md",
            "claimed:04",
            "says 04 phases; development-plan §8 lists 3",
            14,
        ),
        Finding::new(
            "phase-count",
            "README.md",
            "claimed:5",
            "says 5 phases; development-plan §8 lists 3",
            3,
        ),
        Finding::new(
            "phase-count",
            ".claude/rules/07-milestone-numbering.md",
            "claimed:47",
            "says 47 phases; development-plan §8 lists 3",
            3,
        ),
        Finding::new(
            "phase-count",
            "CLAUDE.md",
            "claimed:12",
            "says 12 phases; development-plan §8 lists 3",
            3,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn phase_count_skips_without_plan() {
    let repo = TestRepo::with_files(
        "phase-noplan",
        &[("README.md", "5 phases across 2 tiers.\n")],
    );
    assert_eq!(
        skip_message(PhaseCount.run(&open(&repo))),
        "docs/project/development-plan.md not found"
    );
}
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_status`
Expected: FAIL to compile: `error[E0432]: unresolved import` for `aios_tools::cmd::docs_check::checks::milestones` (could not find `milestones` in `checks`).

- [ ] **Step 3: Implement the checks**

Create `tools/src/cmd/docs_check/checks/milestones.rs`:

````rust
//! `milestone-status` and `phase-count` (check.py L928-1017; line references are to
//! `scripts/docs/check.py` at 33c6b3d).
//!
//! milestone-status compares the milestones merged on main's first-parent
//! history (`Phase N MK:` subjects, see [`Repo::merged_milestones`]) with each
//! phase doc's `**Status:**` line and task checkboxes, the README status and the
//! development plan's §8 and §8.1 tables. phase-count compares "N phases" claims
//! in prose with the number of §8 table rows.
//!
//! Accepted divergences from check.py (no tracked file exercises them): `\d` is
//! `[0-9]` and `str.isdigit()` is ASCII-only, so non-ASCII decimal digits are not
//! numbers here; `\s` and `\b` follow the `regex` crate's Unicode classes; a §8
//! phase number that does not fit `u64` matches no phase doc or merged milestone
//! (check.py compares arbitrary-precision ints), while its `§8:phase-N` target
//! is still printed exactly (`int_str`).

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::{milestone_tokens, prose_lines, section_body, table_rows};
use crate::cmd::docs_check::model::{Finding, Skip};
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{int_str, is_ascii_digits, parse_uint, splitlines, strip};

const MILESTONE_STATUS: &str = "milestone-status";
const PHASE_COUNT: &str = "phase-count";
const PLAN: &str = "docs/project/development-plan.md";
const README: &str = "README.md";
const RULE_07: &str = ".claude/rules/07-milestone-numbering.md";
const CLAUDE_MD: &str = "CLAUDE.md";

/// check.py L940.
static STATUS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\*Status:\*\*\s*(.*)$").expect("valid regex"));
/// R45 without its `(?!~~)` lookahead; [`is_unchecked_task`] applies it.
static UNCHECKED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*[-*] \[ \] ").expect("valid regex"));
/// R46: the development plan's §8 table.
static PLAN_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## 8\. ").expect("valid regex"));
static PLAN_STOP_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
/// R47: the §8.1 Velocity Summary table.
static VELOCITY_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^### Velocity Summary").expect("valid regex"));
static VELOCITY_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^#{2,3} ").expect("valid regex"));
/// R48.
static PHASES_ACROSS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9]+) phases across\b").expect("valid regex"));
static PHASES_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b([0-9]+) phases\b").expect("valid regex"));

/// Merged `Phase N MK:` milestones vs phase docs, README and development plan.
pub struct MilestoneStatus;

/// "N phases" claims vs the development plan's §8 table.
pub struct PhaseCount;

/// check.py L951, R45 `^\s*[-*] \[ \] (?!~~)`: an open task checkbox that is not a
/// struck-through ("deferred") item. `\s*` has no alternative that lets `[-*]`
/// match elsewhere, so testing the text after the one possible match is exact.
pub fn is_unchecked_task(line: &str) -> bool {
    UNCHECKED_RE
        .find(line)
        .is_some_and(|m| !line[m.end()..].starts_with("~~"))
}

/// check.py L979-989: at most one finding comparing a status text with the
/// milestones of a phase (`ms`) and the merged ones among them (`done`).
pub fn status_findings(
    rel: &str,
    target: &str,
    status: &str,
    ms: &BTreeSet<u64>,
    done: &BTreeSet<u64>,
    line: usize,
) -> Vec<Finding> {
    let s = status.to_lowercase();
    let message = if !ms.is_empty() && done == ms && !s.starts_with("complete") {
        format!(
            "all milestones ({}) are merged but status is '{status}'",
            fmt_ms(ms)
        )
    } else if !done.is_empty() && done != ms && s.starts_with("planned") {
        format!("{} merged but status is '{status}'", fmt_ms(done))
    } else if done.is_empty() && s.starts_with("complete") {
        format!("status is '{status}' but no milestone is merged")
    } else {
        return Vec::new();
    };
    vec![Finding::new(MILESTONE_STATUS, rel, target, message, line)]
}

/// check.py L992-993: `M1, M2, M10` in numeric order.
pub fn fmt_ms(ms: &BTreeSet<u64>) -> String {
    ms.iter()
        .map(|m| format!("M{m}"))
        .collect::<Vec<_>>()
        .join(", ")
}

/// The first `**Status:**` line: (1-based line, stripped status), or (0, "").
fn status_line<'a>(lines: &[&'a str]) -> (usize, &'a str) {
    for (i, &line) in lines.iter().enumerate() {
        if let Some(caps) = STATUS_RE.captures(line) {
            return (i + 1, strip(caps.get(1).map_or("", |m| m.as_str())));
        }
    }
    (0, "")
}

impl Check for MilestoneStatus {
    fn name(&self) -> &'static str {
        MILESTONE_STATUS
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let merged = repo.merged_milestones()?;
        let Some(&latest) = merged.keys().next_back() else {
            return Err(Skip(
                "no 'Phase N MK:' commits found on main's first-parent history".to_string(),
            )
            .into());
        };
        let merged_set: BTreeSet<u64> = merged.keys().copied().collect();
        let mut out = Vec::new();
        let mut phase_ms: BTreeMap<u64, BTreeSet<u64>> = BTreeMap::new();
        for (phase, rel) in repo.phase_docs() {
            let sections = repo.milestone_sections(&rel);
            let ms: BTreeSet<u64> = sections.keys().copied().collect();
            phase_ms.insert(phase, ms.clone());
            let text = repo.text(&rel);
            let lines = splitlines(&text);
            let (status_lineno, status) = status_line(&lines);
            let done: BTreeSet<u64> = ms.intersection(&merged_set).copied().collect();
            out.extend(status_findings(
                &rel,
                "status",
                status,
                &ms,
                &done,
                status_lineno,
            ));
            for num in &done {
                let Some(&(lo, hi)) = sections.get(num) else {
                    continue;
                };
                // "- [ ] ~~task~~ — deferred to MK" is closed here, as in brief.sh.
                let unchecked: Vec<usize> = (lo..=hi)
                    .filter(|&n| n >= 1 && lines.get(n - 1).copied().is_some_and(is_unchecked_task))
                    .collect();
                if let Some(&first) = unchecked.first() {
                    out.push(Finding::new(
                        MILESTONE_STATUS,
                        rel.as_str(),
                        format!("M{num}:unchecked"),
                        format!(
                            "merged milestone M{num} still has {} unchecked task(s)",
                            unchecked.len()
                        ),
                        first,
                    ));
                }
            }
        }
        if repo.is_file(README) && !milestone_tokens(&repo.text(README)).contains(&latest) {
            out.push(Finding::new(
                MILESTONE_STATUS,
                README,
                format!("latest:M{latest}"),
                format!("README status does not mention the latest merged milestone M{latest}"),
                0,
            ));
        }
        if repo.is_file(PLAN) {
            let text = repo.text(PLAN);
            for (lineno, cells) in table_rows(&section_body(&text, &PLAN_START_RE, &PLAN_STOP_RE)) {
                if cells.len() < 6 || !is_ascii_digits(&cells[0]) {
                    continue;
                }
                // check.py: `phase_ms.get(phase) or {m for m, p in merged.items() if p == phase}`.
                let phase = parse_uint(&cells[0]);
                let ms: BTreeSet<u64> = match phase.and_then(|p| phase_ms.get(&p)) {
                    Some(ms) if !ms.is_empty() => ms.clone(),
                    _ => merged
                        .iter()
                        .filter(|&(_, &p)| Some(p) == phase)
                        .map(|(&m, _)| m)
                        .collect(),
                };
                let done: BTreeSet<u64> = ms.intersection(&merged_set).copied().collect();
                out.extend(status_findings(
                    PLAN,
                    &format!("§8:phase-{}", int_str(&cells[0])),
                    &cells[5],
                    &ms,
                    &done,
                    lineno,
                ));
            }
            let mut covered: BTreeSet<u64> = BTreeSet::new();
            for (_, cells) in
                table_rows(&section_body(&text, &VELOCITY_START_RE, &VELOCITY_STOP_RE))
            {
                if cells.len() >= 5 {
                    covered.extend(milestone_tokens(&cells[4]));
                }
            }
            for num in merged_set.difference(&covered) {
                out.push(Finding::new(
                    MILESTONE_STATUS,
                    PLAN,
                    format!("§8.1:M{num}"),
                    format!("§8.1 Velocity Summary has no row covering merged milestone M{num}"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

/// check.py L1009-1016 for one source file: every `rx` claim on a prose line
/// whose number differs from `actual` (a claim too large for u64 differs too).
fn phase_claims(repo: &Repo, rel: &str, rx: &Regex, actual: usize, out: &mut Vec<Finding>) {
    if !repo.is_file(rel) {
        return;
    }
    let text = repo.text(rel);
    for (lineno, line) in prose_lines(&text) {
        for caps in rx.captures_iter(line) {
            let claimed = caps.get(1).map_or("", |m| m.as_str());
            if parse_uint(claimed) != u64::try_from(actual).ok() {
                out.push(Finding::new(
                    PHASE_COUNT,
                    rel,
                    format!("claimed:{claimed}"),
                    format!("says {claimed} phases; development-plan §8 lists {actual}"),
                    lineno,
                ));
            }
        }
    }
}

impl Check for PhaseCount {
    fn name(&self) -> &'static str {
        PHASE_COUNT
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        if !repo.is_file(PLAN) {
            return Err(Skip(format!("{PLAN} not found")).into());
        }
        let plan = repo.text(PLAN);
        let actual = table_rows(&section_body(&plan, &PLAN_START_RE, &PLAN_STOP_RE))
            .iter()
            .filter(|(_, cells)| cells.first().is_some_and(|c| is_ascii_digits(c)))
            .count();
        let mut out = Vec::new();
        phase_claims(repo, PLAN, &PHASES_ACROSS_RE, actual, &mut out);
        phase_claims(repo, README, &PHASES_ACROSS_RE, actual, &mut out);
        phase_claims(repo, RULE_07, &PHASES_RE, actual, &mut out);
        phase_claims(repo, CLAUDE_MD, &PHASES_RE, actual, &mut out);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(ms: &[u64]) -> BTreeSet<u64> {
        ms.iter().copied().collect()
    }

    #[test]
    fn regexes_compile() {
        for re in [
            &STATUS_RE,
            &UNCHECKED_RE,
            &PLAN_START_RE,
            &PLAN_STOP_RE,
            &VELOCITY_START_RE,
            &VELOCITY_STOP_RE,
            &PHASES_ACROSS_RE,
            &PHASES_RE,
        ] {
            LazyLock::force(re);
        }
    }

    #[test]
    fn unchecked_task_skips_struck_through_items() {
        assert!(is_unchecked_task("- [ ] task"));
        assert!(is_unchecked_task("  * [ ] x"));
        assert!(!is_unchecked_task("- [ ] ~~old~~ — deferred"));
        assert!(!is_unchecked_task("- [x] y"));
        assert!(!is_unchecked_task("+ [ ] plus is not a task marker here"));
        assert!(!is_unchecked_task("- [ ]no space"));
        assert!(is_unchecked_task("\t- [ ] ~ single tilde"));
    }

    #[test]
    fn status_findings_match_check_py() {
        let rel = "docs/phases/00-a.md";
        let one = |target: &str, message: &str, line: usize| {
            vec![Finding::new("milestone-status", rel, target, message, line)]
        };
        assert_eq!(
            status_findings(
                rel,
                "status",
                "In Progress",
                &set(&[1, 2]),
                &set(&[1, 2]),
                3
            ),
            one(
                "status",
                "all milestones (M1, M2) are merged but status is 'In Progress'",
                3
            )
        );
        assert_eq!(
            status_findings(
                rel,
                "status",
                "Complete (2026-01-01)",
                &set(&[1, 2]),
                &set(&[1, 2]),
                3
            ),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "PLANNED", &set(&[1, 2]), &set(&[2]), 3),
            one("status", "M2 merged but status is 'PLANNED'", 3)
        );
        assert_eq!(
            status_findings(rel, "status", "In Progress", &set(&[1, 2]), &set(&[2]), 3),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "complete", &set(&[1, 2]), &set(&[]), 3),
            one(
                "status",
                "status is 'complete' but no milestone is merged",
                3
            )
        );
        assert_eq!(
            status_findings(rel, "status", "Planned", &set(&[]), &set(&[]), 0),
            Vec::new()
        );
        assert_eq!(
            status_findings(rel, "status", "", &set(&[7]), &set(&[7]), 0),
            one(
                "status",
                "all milestones (M7) are merged but status is ''",
                0
            )
        );
    }

    #[test]
    fn fmt_ms_sorts_numerically() {
        assert_eq!(fmt_ms(&set(&[12, 3, 7])), "M3, M7, M12");
        assert_eq!(fmt_ms(&set(&[])), "");
    }
}
````

- [ ] **Step 4: Register the checks**

In `tools/src/cmd/docs_check/checks/mod.rs`, make the `pub mod` block read exactly as follows (alphabetical: rustfmt's `reorder_modules` sorts consecutive `mod` items, so `milestones` goes between `lock_order` and `repo_paths`):

```rust
pub mod doc_map;
pub mod just_recipes;
pub mod links;
pub mod lock_order;
pub mod milestones;
pub mod repo_paths;
pub mod test_count;
```

Then replace the whole `pub fn registry()` item (keep its doc comment above it) with:

```rust
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
        Box::new(test_count::TestCount),
        Box::new(lock_order::LockOrder),
        Box::new(milestones::MilestoneStatus),
        Box::new(milestones::PhaseCount),
    ]
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_status`
Expected: PASS, `test result: ok. 6 passed; 0 failed`.

Run: `cargo test -p aios-tools --lib checks::milestones`
Expected: PASS, `test result: ok. 4 passed; 0 failed` (`regexes_compile`, `unchecked_task_skips_struck_through_items`, `status_findings_match_check_py`, `fmt_ms_sorts_numerically`).

- [ ] **Step 6: Compare with check.py on the real repository**

```bash
just tools
d=$(mktemp -d)
python3 scripts/docs/check.py --all --check milestone-status,phase-count >"$d/py.txt"; echo "check.py exit $?"
target/tools/release/aios docs-check --all --check milestone-status,phase-count >"$d/aios.txt"; echo "aios exit $?"
cmp "$d/py.txt" "$d/aios.txt" && echo IDENTICAL-TEXT
python3 scripts/docs/check.py --json --all --check milestone-status,phase-count >"$d/py.json"
target/tools/release/aios docs-check --json --all --check milestone-status,phase-count >"$d/aios.json"
cmp "$d/py.json" "$d/aios.json" && echo IDENTICAL-JSON
rm -rf "$d"
```

Expected: `check.py exit 0`, `aios exit 0` (5 milestone-status and 1 phase-count findings, all baselined, at the branch point), `IDENTICAL-TEXT`, `IDENTICAL-JSON`. The exit codes must be equal whatever main's history adds later. If `cmp` reports a difference, run `diff "$d/py.txt" "$d/aios.txt"` before the `rm`, fix the port, and add the differing input to `checks_status.rs`.

- [ ] **Step 7: Format and lint**

```bash
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: `cargo fmt --check` prints nothing and exits 0 (the code above is already rustfmt-clean, so the first command changes nothing); clippy ends with `Finished` and reports no warnings.

- [ ] **Step 8: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/milestones.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_status.rs
git commit -F - <<'EOF'
Port docs-check milestone-status and phase-count to Rust

milestone-status compares the merged 'Phase N MK:' milestones with the
phase docs (status line, open tasks), the README status and the
development plan's section 8 tables; phase-count compares "N phases"
claims with the section 8 rows. The R45 lookahead becomes
is_unchecked_task. The expected findings in tests/checks_status.rs were
recorded from check.py's own check functions on the same repositories.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 11: layout and harness-tables

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/layout.rs` (check `Layout`, `layout_block`, `tree_entries`; unit tests)
- Create: `tools/src/cmd/docs_check/checks/harness.rs` (check `HarnessTables`, `SKILL_NAME`, `project_skills`, `project_agents`, `claude_table_names`, `layout_list`; unit tests)
- Create: `tools/tests/checks_harness.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (add `pub mod harness;`, `pub mod layout;`, append two registry entries)
- Test: `tools/tests/checks_harness.rs`, `#[cfg(test)] mod tests` in `layout.rs` and `harness.rs`

**Interfaces:**
- Consumes:
  - T1 `aios_tools::pystr`: `pub fn strip(s: &str) -> &str`, `pub fn lstrip(s: &str) -> &str`; `aios_tools::paths`: `pub fn basename(path: &str) -> &str`.
  - T3 `model`: `Finding::new(...)` as in Task 10.
  - T4 `markdown`: `pub fn section_body<'a>(text: &'a str, start: &Regex, stop: &Regex) -> Vec<(usize, &'a str)>`.
  - T5 `Repo`: `open`, `files(&self) -> &[String]`, `is_file(&self, rel: &str) -> bool`, `text(&self, rel: &str) -> Rc<str>`.
  - T6 `checks::Check`, `registry()` (after Task 10: eleven checks); `just tools`.
  - T2 `tools/tests/common`: `TestRepo::with_files`, `TestRepo::path_str`.
- Produces:
  - `checks::layout`: `pub struct Layout;` (`impl Check`, name `layout`), `pub fn layout_block(repo: &Repo) -> Vec<String>`, `pub fn tree_entries(lines: &[String], start: &str, stop: &str) -> (BTreeSet<String>, BTreeSet<String>)`, and `pub(crate) static TREE_PREFIX_RE: LazyLock<Regex>` (R52, shared with harness-tables).
  - `checks::harness`: `pub struct HarnessTables;` (`impl Check`, name `harness-tables`), `pub const SKILL_NAME: &str`, `pub fn project_skills(repo: &Repo) -> (BTreeSet<String>, BTreeSet<String>)` (skills, plugin names), `pub fn project_agents(repo: &Repo) -> BTreeSet<String>`, `pub fn claude_table_names(repo: &Repo, marker: &str, rx: &Regex) -> BTreeSet<String>` (`rx` must start with `^`, as check.py's `re.match`), `pub fn layout_list(block: &[String], label: &str) -> Option<BTreeSet<String>>`. Task 12 uses `SKILL_NAME`, `project_skills` and `project_agents`.
  - `registry()` now returns thirteen checks, ending `..., PhaseCount, Layout, HarnessTables`.

**Parity notes (check.py at 33c6b3d):**
- layout = L1020-1082. `layout_block` = the lines of CLAUDE.md's `## Workspace Layout` section (R49, fences included). `tree_entries` (L1025-1051): a tree entry is any line containing `──`; the start and stop entries are found by substring; inside, an entry resets collecting, adds its R50 directory, and when it contains `(top-level)` adds the R51 words after it and starts collecting; a non-entry line while collecting adds the R51 words after removing the R52 indent; a trailing `.rs` is dropped.
- Actual modules (L1055-1058): directory = `f.split("/")[2]` for files under the prefix with at least 3 slashes; module = basename without `.rs` for `.rs` files with exactly 2 slashes. For each of kernel dir, kernel module, shared dir, shared module: sorted missing, then sorted stale, all on `CLAUDE.md` at line 0; then rule 05 (only when tracked): R53 multi-line on the raw text, missing then stale.
- harness-tables = L1086-1166. `project_skills` (L1116-1142): first pass maps each skills-dir plugin directory to its plugin.json `name` when that is a non-empty JSON string, else the directory name (invalid JSON, a non-object, or a missing, empty or non-string name all fall back); second pass adds `plugin:skill` for `skills/<skill>/SKILL.md` of a plugin directory, else `<name>` for `.claude/skills/<name>` (a tracked symlink such as `obsidian`) or `.claude/skills/<name>/SKILL.md` when `<name>` is not a plugin directory.
- `claude_table_names` (L1089-1097): section from the first line containing the literal marker to the next line starting `**` or `## `; rows are lines whose left-stripped text starts with `|`; the first cell is Python-stripped after trimming `|` from both ends of the stripped line.
- `layout_list` (L1100-1113): after the matching `── <label>/` entry, every following line up to the next `──` line adds SKILL_NAME words from the text before its first `(`, even lines past the tree fence when the entry is the last one; `None` when the entry is absent, and then that comparison is skipped.
- Output order (L1152-1166): skills-table, agents-table, layout-skills, layout-agents; each sorted missing then sorted stale; all on `CLAUDE.md`, line 0.
- plugin.json parsing uses serde_json; the divergences from Python's `json` (NaN/Infinity literals, lone surrogates, nesting beyond 128) are listed in the module docs. Duplicate keys: both take the last value.
- Expected findings in the tests below were recorded by running check.py's `check_layout`, `check_harness_tables`, `project_skills`, `tree_entries` and `layout_list` on repositories with exactly these files.

- [ ] **Step 1: Write the failing integration test**

Create `tools/tests/checks_harness.rs`:

````rust
//! layout and harness-tables on small committed repositories. The expected
//! findings were recorded from check.py's `check_layout` and
//! `check_harness_tables` on the same files (production order).

mod common;

use aios_tools::cmd::docs_check::checks::harness::{project_agents, project_skills, HarnessTables};
use aios_tools::cmd::docs_check::checks::layout::{layout_block, tree_entries, Layout};
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::Finding;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;
use std::collections::BTreeSet;

/// `.claude/skills/linked` is a plain file standing in for a tracked symlink
/// (the real repository tracks `.claude/skills/obsidian` as one); both are a
/// single tracked path that names the skill.
const FILES: &[(&str, &str)] = &[
    (
        "CLAUDE.md",
        r#"# Project

## Workspace Layout

```text
proj/
├── .claude/
│   ├── agents/           worker, helper
│   ├── skills/           alpha, linked, kit:go,
│   │                     bad:run, retired (plugin skills as plugin:skill)
│   └── rules/            01-code (auto-loaded)
├── kernel/src/           kernel
│   ├── mm/               memory
│   ├── ipc/              channels
│   └── (top-level)       main.rs, boot_phase,
│                         dtb
├── shared/src/           shared types
│   ├── kits/             kit traits
│   └── (top-level)       lib, boot, cap
├── uefi-stub/src/        stub
└── docs/                 docs
```

## Team

**Agents** (defined in `.claude/agents/`):

| Agent | Role |
| --- | --- |
| `worker` | Works |
| `ghost-agent` | Gone |

**Skills** (defined in `.claude/skills/`):

| Skill | Purpose |
| --- | --- |
| `/alpha` | First |
| `/kit:go` | Plugin skill |
| `/stale-skill` | Gone |

## Other

Nothing else.
"#,
    ),
    (
        ".claude/rules/05-file-placement.md",
        r#"# File Placement Rules

```
kernel/src/mm/                 Memory
kernel/src/old/                Removed
kernel/src/                    Entry
```
"#,
    ),
    (
        "kernel/src/main.rs",
        r#"fn main() {}
"#,
    ),
    (
        "kernel/src/boot_phase.rs",
        r#"//! Boot phases.
"#,
    ),
    (
        "kernel/src/mm/mod.rs",
        r#"//! Memory.
"#,
    ),
    (
        "kernel/src/sched/mod.rs",
        r#"//! Scheduler.
"#,
    ),
    (
        "kernel/src/sched/rq/mod.rs",
        r#"//! Run queues.
"#,
    ),
    (
        "shared/src/lib.rs",
        r#"//! Shared.
"#,
    ),
    (
        "shared/src/boot.rs",
        r#"//! Boot info.
"#,
    ),
    (
        "shared/src/kits/mod.rs",
        r#"//! Kits.
"#,
    ),
    (
        "shared/src/ipc/mod.rs",
        r#"//! IPC types.
"#,
    ),
    (
        ".claude/agents/worker.md",
        r#"# Worker
"#,
    ),
    (
        ".claude/agents/helper.md",
        r#"# Helper
"#,
    ),
    (
        ".claude/skills/alpha/SKILL.md",
        r#"# Alpha
"#,
    ),
    (
        ".claude/skills/alpha/reference.md",
        r#"# Alpha reference
"#,
    ),
    (
        ".claude/skills/linked",
        r#"../../elsewhere/linked
"#,
    ),
    (
        ".claude/skills/pack/.claude-plugin/plugin.json",
        r#"{"name": "kit"}
"#,
    ),
    (
        ".claude/skills/pack/skills/go/SKILL.md",
        r#"# Go
"#,
    ),
    (
        ".claude/skills/bad/.claude-plugin/plugin.json",
        r#"not json
"#,
    ),
    (
        ".claude/skills/bad/skills/run/SKILL.md",
        r#"# Run
"#,
    ),
    (
        ".claude/skills/unnamed/.claude-plugin/plugin.json",
        r#"{"name": ""}
"#,
    ),
    (
        ".claude/skills/unnamed/skills/x/SKILL.md",
        r#"# X
"#,
    ),
];

/// A repository without CLAUDE.md: every table is empty and the layout lists
/// are absent, so only the table comparisons report.
const BARE_FILES: &[(&str, &str)] = &[
    ("kernel/src/main.rs", "fn main() {}\n"),
    (".claude/agents/solo.md", "# Solo\n"),
    (".claude/skills/only/SKILL.md", "# Only\n"),
];

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

fn names(items: &[&str]) -> BTreeSet<String> {
    items.iter().map(|s| s.to_string()).collect()
}

#[test]
fn check_names() {
    assert_eq!(Layout.name(), "layout");
    assert_eq!(HarnessTables.name(), "harness-tables");
}

#[test]
fn layout_tree_reads_the_workspace_layout_section() {
    let repo = TestRepo::with_files("layout-tree", FILES);
    let block = layout_block(&open(&repo));
    assert_eq!(
        tree_entries(&block, "kernel/src/", "shared/src/"),
        (names(&["ipc", "mm"]), names(&["boot_phase", "dtb", "main"]))
    );
    assert_eq!(
        tree_entries(&block, "shared/src/", "uefi-stub/"),
        (names(&["kits"]), names(&["boot", "cap", "lib"]))
    );
}

#[test]
fn layout_matches_check_py() {
    let repo = TestRepo::with_files("layout", FILES);
    let found = Layout.run(&open(&repo)).expect("layout runs");
    let expected = vec![
        Finding::new(
            "layout",
            "CLAUDE.md",
            "missing:kernel/src/sched/",
            "Workspace Layout does not list kernel dir kernel/src/sched/",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:kernel/src/ipc/",
            "Workspace Layout lists kernel/src/ipc/, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:kernel/src/dtb.rs",
            "Workspace Layout lists kernel/src/dtb.rs, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "missing:shared/src/ipc/",
            "Workspace Layout does not list shared dir shared/src/ipc/",
            0,
        ),
        Finding::new(
            "layout",
            "CLAUDE.md",
            "stale:shared/src/cap.rs",
            "Workspace Layout lists shared/src/cap.rs, which does not exist",
            0,
        ),
        Finding::new(
            "layout",
            ".claude/rules/05-file-placement.md",
            "missing:kernel/src/sched/",
            "rule 05 does not list kernel/src/sched/",
            0,
        ),
        Finding::new(
            "layout",
            ".claude/rules/05-file-placement.md",
            "stale:kernel/src/old/",
            "rule 05 lists kernel/src/old/, which does not exist",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn layout_without_claude_md_lists_every_module_as_missing() {
    let repo = TestRepo::with_files("layout-bare", BARE_FILES);
    let found = Layout.run(&open(&repo)).expect("layout runs");
    let expected = vec![Finding::new(
        "layout",
        "CLAUDE.md",
        "missing:kernel/src/main.rs",
        "Workspace Layout does not list kernel module kernel/src/main.rs",
        0,
    )];
    assert_eq!(found, expected);
}

#[test]
fn project_skills_and_agents_follow_the_skills_dir_rules() {
    let test_repo = TestRepo::with_files("harness-skills", FILES);
    let repo = open(&test_repo);
    assert_eq!(
        project_skills(&repo),
        (
            names(&["alpha", "bad:run", "kit:go", "linked", "unnamed:x"]),
            names(&["bad", "kit", "unnamed"])
        )
    );
    assert_eq!(project_agents(&repo), names(&["helper", "worker"]));
}

#[test]
fn harness_tables_matches_check_py() {
    let repo = TestRepo::with_files("harness", FILES);
    let found = HarnessTables
        .run(&open(&repo))
        .expect("harness-tables runs");
    let expected = vec![
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:bad:run",
            "CLAUDE.md skills-table omits skill bad:run",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:linked",
            "CLAUDE.md skills-table omits skill linked",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:unnamed:x",
            "CLAUDE.md skills-table omits skill unnamed:x",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-stale:stale-skill",
            "CLAUDE.md skills-table lists skill stale-skill, which is not in .claude/",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-missing:helper",
            "CLAUDE.md agents-table omits agent helper",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-stale:ghost-agent",
            "CLAUDE.md agents-table lists agent ghost-agent, which is not in .claude/",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "layout-skills-missing:unnamed:x",
            "CLAUDE.md layout-skills omits skill unnamed:x",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "layout-skills-stale:retired",
            "CLAUDE.md layout-skills lists skill retired, which is not in .claude/",
            0,
        ),
    ];
    assert_eq!(found, expected);
}

#[test]
fn harness_tables_without_claude_md_skips_the_layout_lists() {
    let repo = TestRepo::with_files("harness-bare", BARE_FILES);
    let found = HarnessTables
        .run(&open(&repo))
        .expect("harness-tables runs");
    let expected = vec![
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "skills-table-missing:only",
            "CLAUDE.md skills-table omits skill only",
            0,
        ),
        Finding::new(
            "harness-tables",
            "CLAUDE.md",
            "agents-table-missing:solo",
            "CLAUDE.md agents-table omits agent solo",
            0,
        ),
    ];
    assert_eq!(found, expected);
}
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_harness`
Expected: FAIL to compile: `error[E0432]: unresolved import` for `aios_tools::cmd::docs_check::checks::harness` and `aios_tools::cmd::docs_check::checks::layout`.

- [ ] **Step 3: Implement layout**

Create `tools/src/cmd/docs_check/checks/layout.rs`:

````rust
//! `layout` (check.py L1020-1082 at 33c6b3d): the `kernel/src` and `shared/src` directories
//! and top-level modules against the CLAUDE.md Workspace Layout tree and the
//! `kernel/src/<dir>/` lines of rule 05.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::section_body;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::paths::basename;

const CHECK: &str = "layout";
const CLAUDE_MD: &str = "CLAUDE.md";
const RULE_05: &str = ".claude/rules/05-file-placement.md";
/// A tree entry line (`├── `, `└── `) contains this box-drawing run.
const TREE_MARK: &str = "──";

/// R49: the Workspace Layout section of CLAUDE.md.
static LAYOUT_START_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## Workspace Layout").expect("valid regex"));
static LAYOUT_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^## ").expect("valid regex"));
/// R50: a directory entry `── name/`.
static TREE_DIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"──\s+([A-Za-z0-9_.-]+)/").expect("valid regex"));
/// R51: module names after `(top-level)` and on its continuation lines.
static MODULE_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[a-z_][a-z0-9_.]*").expect("valid regex"));
/// R52: the tree-drawing indent of a continuation line (also used by harness-tables).
pub(crate) static TREE_PREFIX_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[│\s]+").expect("valid regex"));
/// R53: `kernel/src/<dir>/` at the start of a line of rule 05.
static RULE_DIR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^kernel/src/([a-z0-9_]+)/").expect("valid regex"));

/// kernel/src and shared/src modules vs CLAUDE.md layout and rule 05.
pub struct Layout;

/// check.py L1020-1022: the lines of CLAUDE.md's `## Workspace Layout` section
/// (fences included), without line numbers.
pub fn layout_block(repo: &Repo) -> Vec<String> {
    let text = repo.text(CLAUDE_MD);
    section_body(&text, &LAYOUT_START_RE, &LAYOUT_STOP_RE)
        .into_iter()
        .map(|(_, line)| line.to_string())
        .collect()
}

/// check.py L1025-1051: (directory names, top-level module names) listed
/// between the tree entry naming `start` and the next tree entry naming `stop`.
pub fn tree_entries(
    lines: &[String],
    start: &str,
    stop: &str,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut dirs = BTreeSet::new();
    let mut mods = BTreeSet::new();
    let mut inside = false;
    let mut collecting = false;
    for line in lines {
        let entry = line.contains(TREE_MARK);
        if line.contains(start) && entry {
            inside = true;
            continue;
        }
        if inside && line.contains(stop) && entry {
            break;
        }
        if !inside {
            continue;
        }
        if entry {
            collecting = false;
            if let Some(caps) = TREE_DIR_RE.captures(line) {
                dirs.insert(caps[1].to_string());
            }
            if let Some((_, rest)) = line.split_once("(top-level)") {
                collecting = true;
                add_module_names(&mut mods, rest);
            }
        } else if collecting {
            add_module_names(&mut mods, &TREE_PREFIX_RE.replace(line, ""));
        }
    }
    (dirs, mods)
}

/// Every R51 name in `text`, with a trailing `.rs` removed.
fn add_module_names(mods: &mut BTreeSet<String>, text: &str) {
    for m in MODULE_NAME_RE.find_iter(text) {
        let name = m.as_str();
        mods.insert(name.strip_suffix(".rs").unwrap_or(name).to_string());
    }
}

/// `f.split("/")[2]` for tracked files at least one directory below `prefix`.
fn source_dirs(files: &[String], prefix: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|f| f.starts_with(prefix) && f.matches('/').count() >= 3)
        .filter_map(|f| f.split('/').nth(2))
        .map(str::to_string)
        .collect()
}

/// Basenames without `.rs` of the `.rs` files directly in `prefix`.
fn source_modules(files: &[String], prefix: &str) -> BTreeSet<String> {
    files
        .iter()
        .filter(|f| f.starts_with(prefix) && f.matches('/').count() == 2)
        .filter_map(|f| basename(f).strip_suffix(".rs"))
        .map(str::to_string)
        .collect()
}

impl Check for Layout {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let files = repo.files();
        let kdirs = source_dirs(files, "kernel/src/");
        let kmods = source_modules(files, "kernel/src/");
        let sdirs = source_dirs(files, "shared/src/");
        let smods = source_modules(files, "shared/src/");
        let block = layout_block(repo);
        let (ldirs, lmods) = tree_entries(&block, "kernel/src/", "shared/src/");
        let (sd, sm) = tree_entries(&block, "shared/src/", "uefi-stub/");
        let groups = [
            ("kernel dir", &kdirs, &ldirs, "kernel/src/", "/"),
            ("kernel module", &kmods, &lmods, "kernel/src/", ".rs"),
            ("shared dir", &sdirs, &sd, "shared/src/", "/"),
            ("shared module", &smods, &sm, "shared/src/", ".rs"),
        ];
        let mut out = Vec::new();
        for (label, actual, listed, prefix, suffix) in groups {
            for name in actual.difference(listed) {
                let path = format!("{prefix}{name}{suffix}");
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("missing:{path}"),
                    format!("Workspace Layout does not list {label} {path}"),
                    0,
                ));
            }
            for name in listed.difference(actual) {
                let path = format!("{prefix}{name}{suffix}");
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("stale:{path}"),
                    format!("Workspace Layout lists {path}, which does not exist"),
                    0,
                ));
            }
        }
        if repo.is_file(RULE_05) {
            let text = repo.text(RULE_05);
            let rdirs: BTreeSet<String> = RULE_DIR_RE
                .captures_iter(&text)
                .map(|caps| caps[1].to_string())
                .collect();
            for name in kdirs.difference(&rdirs) {
                out.push(Finding::new(
                    CHECK,
                    RULE_05,
                    format!("missing:kernel/src/{name}/"),
                    format!("rule 05 does not list kernel/src/{name}/"),
                    0,
                ));
            }
            for name in rdirs.difference(&kdirs) {
                out.push(Finding::new(
                    CHECK,
                    RULE_05,
                    format!("stale:kernel/src/{name}/"),
                    format!("rule 05 lists kernel/src/{name}/, which does not exist"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn regexes_compile() {
        for re in [
            &LAYOUT_START_RE,
            &LAYOUT_STOP_RE,
            &TREE_DIR_RE,
            &MODULE_NAME_RE,
            &TREE_PREFIX_RE,
            &RULE_DIR_RE,
        ] {
            LazyLock::force(re);
        }
    }

    #[test]
    fn tree_entries_reads_dirs_and_top_level_modules() {
        let block: Vec<String> = [
            "```text",
            "├── kernel/src/           kernel",
            "│   ├── mm/               memory",
            "│   └── (top-level)       main.rs, boot_phase,",
            "│                         dtb",
            "├── shared/src/           shared types",
            "│   ├── kits/             kit traits",
            "│   └── (top-level)       lib, boot",
            "├── uefi-stub/src/        stub",
            "│   └── (top-level)       never read",
            "```",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            tree_entries(&block, "kernel/src/", "shared/src/"),
            (names(&["mm"]), names(&["boot_phase", "dtb", "main"]))
        );
        assert_eq!(
            tree_entries(&block, "shared/src/", "uefi-stub/"),
            (names(&["kits"]), names(&["boot", "lib"]))
        );
        assert_eq!(
            tree_entries(&block, "absent/", "uefi-stub/"),
            (names(&[]), names(&[]))
        );
    }
}
````

- [ ] **Step 4: Implement harness-tables**

Create `tools/src/cmd/docs_check/checks/harness.rs`:

````rust
//! `harness-tables` (check.py L1086-1166 at 33c6b3d): the CLAUDE.md **Skills** and
//! **Agents** tables and the `skills/` and `agents/` lists of its Workspace
//! Layout tree against `.claude/skills` and `.claude/agents`.
//!
//! Accepted divergence from check.py: a skills-dir `plugin.json` is parsed with
//! `serde_json`, which rejects `NaN`/`Infinity` literals, lone surrogate escapes
//! and nesting deeper than 128 levels that Python's `json` module accepts; such a
//! file falls back to the directory name as the plugin name. No tracked
//! plugin.json uses them.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::layout::{layout_block, TREE_PREFIX_RE};
use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::section_body;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{lstrip, strip};

const CHECK: &str = "harness-tables";
const CLAUDE_MD: &str = "CLAUDE.md";

/// R54: a skill as a slash command without the slash: `name` or `plugin:name`.
pub const SKILL_NAME: &str = r"[a-z][a-z0-9-]*(?::[a-z][a-z0-9-]*)?";

static SKILL_NAME_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(SKILL_NAME).expect("valid regex"));
/// R56: first cell of a Skills table row (anchored, as check.py's `re.match`).
static SKILL_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&["^`/(", SKILL_NAME, ")"].concat()).expect("valid regex"));
/// R56: first cell of an Agents table row.
static AGENT_CELL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^`([a-z0-9-]+)`").expect("valid regex"));
/// R55: a table section ends at the next bold label or `## ` heading.
static TABLE_STOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\*\*|^## ").expect("valid regex"));
/// R59.
static PLUGIN_JSON_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\.claude/skills/([^/]+)/\.claude-plugin/plugin\.json$").expect("valid regex")
});
static PLUGIN_SKILL_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\.claude/skills/([^/]+)/skills/([^/]+)/SKILL\.md$").expect("valid regex")
});
static SKILL_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.claude/skills/([^/]+)(?:/SKILL\.md)?$").expect("valid regex"));
static AGENT_FILE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\.claude/agents/([^/]+)\.md$").expect("valid regex"));

/// CLAUDE.md skills/agents tables and layout lists vs .claude/.
pub struct HarnessTables;

/// check.py L1116-1142: (skill names, plugin names) as slash commands without
/// the slash. `.claude/skills/<name>/SKILL.md` (or a tracked symlink
/// `.claude/skills/<name>`) is `<name>`; a skills-dir plugin
/// `.claude/skills/<dir>/.claude-plugin/plugin.json` plus
/// `.claude/skills/<dir>/skills/<skill>/SKILL.md` is `<plugin>:<skill>`, where
/// `<plugin>` is plugin.json's non-empty string `name`, else `<dir>`.
pub fn project_skills(repo: &Repo) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut plugins: BTreeMap<String, String> = BTreeMap::new();
    for f in repo.files() {
        if let Some(caps) = PLUGIN_JSON_RE.captures(f) {
            let dir = caps[1].to_string();
            let name = plugin_name(&repo.text(f)).unwrap_or_else(|| dir.clone());
            plugins.insert(dir, name);
        }
    }
    let mut skills = BTreeSet::new();
    for f in repo.files() {
        if let Some(caps) = PLUGIN_SKILL_RE.captures(f) {
            if let Some(plugin) = plugins.get(&caps[1]) {
                skills.insert(format!("{plugin}:{}", &caps[2]));
                continue;
            }
        }
        if let Some(caps) = SKILL_FILE_RE.captures(f) {
            if !plugins.contains_key(&caps[1]) {
                skills.insert(caps[1].to_string());
            }
        }
    }
    (skills, plugins.into_values().collect())
}

/// `json.loads(text).get("name")` when it is a non-empty string; `None` for
/// invalid JSON, a non-object, or a missing, empty or non-string name.
fn plugin_name(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    match value.get("name") {
        Some(serde_json::Value::String(name)) if !name.is_empty() => Some(name.clone()),
        _ => None,
    }
}

/// check.py L1147: `.claude/agents/<name>.md` names.
pub fn project_agents(repo: &Repo) -> BTreeSet<String> {
    repo.files()
        .iter()
        .filter_map(|f| AGENT_FILE_RE.captures(f))
        .map(|caps| caps[1].to_string())
        .collect()
}

/// check.py L1089-1097: group 1 of `rx` (anchored at the cell start) on the
/// first cell of each table row between the line containing `marker` and the
/// next `**` label or `## ` heading of CLAUDE.md.
pub fn claude_table_names(repo: &Repo, marker: &str, rx: &Regex) -> BTreeSet<String> {
    let start = Regex::new(&regex::escape(marker)).expect("an escaped literal is a valid regex");
    let text = repo.text(CLAUDE_MD);
    let mut names = BTreeSet::new();
    for (_, line) in section_body(&text, &start, &TABLE_STOP_RE) {
        if !lstrip(line).starts_with('|') {
            continue;
        }
        let first = strip(line)
            .trim_matches('|')
            .split('|')
            .next()
            .map_or("", strip);
        if let Some(group) = rx.captures(first).and_then(|caps| caps.get(1)) {
            names.insert(group.as_str().to_string());
        }
    }
    names
}

/// check.py L1100-1113: the names on the Workspace Layout tree entry
/// `── <label>/` and its continuation lines (text before the first `(` only),
/// or `None` when the tree has no such entry.
pub fn layout_list(block: &[String], label: &str) -> Option<BTreeSet<String>> {
    let entry = Regex::new(&[r"──\s+", &regex::escape(label), r"/\s+(.*)$"].concat())
        .expect("an escaped label forms a valid regex");
    let mut names = BTreeSet::new();
    let mut found = false;
    for line in block {
        if line.contains("──") {
            if found {
                break;
            }
            if let Some(caps) = entry.captures(line) {
                found = true;
                add_skill_names(&mut names, &caps[1]);
            }
        } else if found {
            add_skill_names(&mut names, &TREE_PREFIX_RE.replace(line, ""));
        }
    }
    found.then_some(names)
}

/// Every SKILL_NAME match in `text` before its first `(`.
fn add_skill_names(names: &mut BTreeSet<String>, text: &str) {
    let head = text.split('(').next().unwrap_or(text);
    names.extend(
        SKILL_NAME_RE
            .find_iter(head)
            .map(|m| m.as_str().to_string()),
    );
}

impl Check for HarnessTables {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let (skills, _) = project_skills(repo);
        let agents = project_agents(repo);
        let table_skills = claude_table_names(repo, "**Skills**", &SKILL_CELL_RE);
        let table_agents = claude_table_names(repo, "**Agents**", &AGENT_CELL_RE);
        let block = layout_block(repo);
        let comparisons = [
            ("skill", &skills, Some(table_skills), "skills-table"),
            ("agent", &agents, Some(table_agents), "agents-table"),
            (
                "skill",
                &skills,
                layout_list(&block, "skills"),
                "layout-skills",
            ),
            (
                "agent",
                &agents,
                layout_list(&block, "agents"),
                "layout-agents",
            ),
        ];
        let mut out = Vec::new();
        for (kind, actual, listed, place) in comparisons {
            let Some(listed) = listed else {
                continue;
            };
            for name in actual.difference(&listed) {
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("{place}-missing:{name}"),
                    format!("CLAUDE.md {place} omits {kind} {name}"),
                    0,
                ));
            }
            for name in listed.difference(actual) {
                out.push(Finding::new(
                    CHECK,
                    CLAUDE_MD,
                    format!("{place}-stale:{name}"),
                    format!("CLAUDE.md {place} lists {kind} {name}, which is not in .claude/"),
                    0,
                ));
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(items: &[&str]) -> BTreeSet<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn regexes_compile() {
        for re in [
            &SKILL_NAME_RE,
            &SKILL_CELL_RE,
            &AGENT_CELL_RE,
            &TABLE_STOP_RE,
            &PLUGIN_JSON_RE,
            &PLUGIN_SKILL_RE,
            &SKILL_FILE_RE,
            &AGENT_FILE_RE,
        ] {
            LazyLock::force(re);
        }
    }

    #[test]
    fn plugin_name_follows_json_get_name() {
        assert_eq!(plugin_name(r#"{"name": "kit"}"#), Some("kit".to_string()));
        assert_eq!(plugin_name(r#"{"name": ""}"#), None);
        assert_eq!(plugin_name(r#"{"name": 5}"#), None);
        assert_eq!(plugin_name(r#"["name"]"#), None);
        assert_eq!(plugin_name("not json"), None);
        assert_eq!(
            plugin_name("{\"name\": \"a\", \"name\": \"b\"}\n"),
            Some("b".to_string())
        );
    }

    #[test]
    fn layout_list_reads_entry_and_continuations() {
        let block: Vec<String> = [
            "├── .claude/",
            "│   ├── agents/           worker, helper",
            "│   ├── skills/           alpha, kit:go,",
            "│   │                     bad:run, retired (plugin skills as plugin:skill)",
            "│   └── rules/            01-code (auto-loaded)",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        assert_eq!(
            layout_list(&block, "skills"),
            Some(names(&["alpha", "bad:run", "kit:go", "retired"]))
        );
        assert_eq!(
            layout_list(&block, "agents"),
            Some(names(&["helper", "worker"]))
        );
        assert_eq!(layout_list(&block, "hooks"), None);
    }
}
````

- [ ] **Step 5: Register the checks**

In `tools/src/cmd/docs_check/checks/mod.rs`, make the `pub mod` block read exactly:

```rust
pub mod doc_map;
pub mod harness;
pub mod just_recipes;
pub mod layout;
pub mod links;
pub mod lock_order;
pub mod milestones;
pub mod repo_paths;
pub mod test_count;
```

Then replace the whole `pub fn registry()` item (keep its doc comment) with:

```rust
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
        Box::new(test_count::TestCount),
        Box::new(lock_order::LockOrder),
        Box::new(milestones::MilestoneStatus),
        Box::new(milestones::PhaseCount),
        Box::new(layout::Layout),
        Box::new(harness::HarnessTables),
    ]
}
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_harness`
Expected: PASS, `test result: ok. 7 passed; 0 failed`.

Run: `cargo test -p aios-tools --lib checks::layout` and `cargo test -p aios-tools --lib checks::harness`
Expected: PASS, `test result: ok. 2 passed; 0 failed` and `test result: ok. 3 passed; 0 failed`.

- [ ] **Step 7: Compare with check.py on the real repository**

```bash
just tools
d=$(mktemp -d)
python3 scripts/docs/check.py --all --check layout,harness-tables >"$d/py.txt"; echo "check.py exit $?"
target/tools/release/aios docs-check --all --check layout,harness-tables >"$d/aios.txt"; echo "aios exit $?"
cmp "$d/py.txt" "$d/aios.txt" && echo IDENTICAL-TEXT
python3 scripts/docs/check.py --json --all --check layout,harness-tables >"$d/py.json"
target/tools/release/aios docs-check --json --all --check layout,harness-tables >"$d/aios.json"
cmp "$d/py.json" "$d/aios.json" && echo IDENTICAL-JSON
rm -rf "$d"
```

Expected: `check.py exit 0`, `aios exit 0` (3 layout findings for rule 05, all baselined; 0 harness-tables findings at the branch point), `IDENTICAL-TEXT`, `IDENTICAL-JSON`. On a difference, `diff` the two files before the `rm`, fix the port and add the case to `checks_harness.rs`.

- [ ] **Step 8: Format and lint**

```bash
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: `cargo fmt --check` prints nothing and exits 0 (the code above is already rustfmt-clean, so the first command changes nothing); clippy ends with `Finished` and reports no warnings.

- [ ] **Step 9: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/layout.rs tools/src/cmd/docs_check/checks/harness.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_harness.rs
git commit -F - <<'EOF'
Port docs-check layout and harness-tables to Rust

layout compares kernel/src and shared/src directories and top-level
modules with the CLAUDE.md Workspace Layout tree and rule 05;
harness-tables compares the CLAUDE.md Skills and Agents tables and the
layout's skills/ and agents/ lists with .claude/ (skills-dir plugins as
plugin:skill). The expected findings in tests/checks_harness.rs were
recorded from check.py's own functions on the same repositories.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 12: pointer-doctor, knowledge-hygiene; registry complete

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/src/cmd/docs_check/checks/pointer_doctor.rs` (check `PointerDoctor`, constants, `norm_section`, `Phrase`, `Resolution`, `resolve_phrase`; unit tests)
- Create: `tools/src/cmd/docs_check/checks/knowledge.rs` (check `KnowledgeHygiene`; unit tests)
- Create: `tools/tests/checks_pointer.rs`
- Modify: `tools/src/cmd/docs_check/checks/mod.rs` (add `pub mod knowledge;`, `pub mod pointer_doctor;`, the final `registry()`, and `mod registry_tests`)
- Test: `tools/tests/checks_pointer.rs`, `#[cfg(test)]` modules in `pointer_doctor.rs`, `knowledge.rs` and `checks/mod.rs`

**Interfaces:**
- Consumes:
  - T1 `aios_tools::pystr`: `pub fn strip(s: &str) -> &str`, `pub fn split_ws(s: &str) -> Vec<&str>`, `pub fn splitlines(s: &str) -> Vec<&str>`; `aios_tools::paths`: `pub fn basename(path: &str) -> &str`.
  - T3 `model`: `Finding::new(...)`, `Finding::key(&self) -> String`, `pub const CHECK_ORDER: [&str; 15]`.
  - T4 `markdown`: `pub static HEADING_RE: LazyLock<Regex>` (R3), `pub fn prose_lines(text: &str) -> Vec<(usize, &str)>`, `pub fn code_spans(line: &str) -> Vec<String>`, `pub fn clean_repo_path(token: &str) -> String`, `pub fn is_path_placeholder(path: &str) -> bool`, `pub fn parse_frontmatter(text: &str) -> Option<HashMap<String, String>>`, `pub struct Heading { pub line: usize, pub level: usize, pub text: String }`.
  - T5 `Repo`: `open`, `md_files(&self) -> &[String]`, `is_file`, `text`, `headings(&self, rel: &str) -> Rc<Vec<Heading>>`, `exists(&self, rel: &str) -> bool`.
  - T6 `checks::Check`, `registry()`, `output::render_list_checks(checks: &[Box<dyn Check>]) -> String` (one `"{name:<18} {description}\n"` line per check); `just tools`.
  - Task 11 `checks::harness`: `SKILL_NAME`, `project_skills(repo) -> (skills, plugins)`, `project_agents(repo)`.
  - T2 `tools/tests/common`: `TestRepo::with_files`, `TestRepo::path_str`.
- Produces:
  - `checks::pointer_doctor`: `pub struct PointerDoctor;` (`impl Check`, name `pointer-doctor`), `pub const KNOWN_TOOLS: [&str; 34]`, `pub const BUILTIN_COMMANDS: [&str; 29]`, `pub const BUILTIN_AGENTS: [&str; 5]`, `pub fn norm_section(name: &str) -> String`, `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Phrase { Before(Vec<String>), After(Vec<String>) }`, `#[derive(Debug, Clone, PartialEq, Eq)] pub enum Resolution { Ok(String), Stub(String), Moved { section: String, rule: String }, Missing(String), Ignore }`, `pub fn resolve_phrase(p: &Phrase, sections: &HashMap<String, (String, bool)>, rules: &HashMap<String, String>) -> Resolution`.
  - `checks::knowledge`: `pub struct KnowledgeHygiene;` (`impl Check`, name `knowledge-hygiene`).
  - `registry()` returns all fifteen checks in CHECK_ORDER; `aios docs-check` is feature-complete (Tasks 13-14 prove parity with goldens; `just docs-check` keeps calling check.py until Task 15).

**Parity notes (check.py at 33c6b3d):**
- pointer-doctor = L1273-1335 with helpers L1169-1270 and constants L70-88. Harness files are the md files under `.claude/agents/`, `.claude/skills/`, `.claude/rules/` in `md_files` order. Per file: first the `tools:` findings (L1282-1290: frontmatter `(?s)^---\n(.*?)\n---`, LF only; its `splitlines()` numbered from 2; comma parts Python-stripped, empty parts dropped; unknown unless in KNOWN_TOOLS or starting `mcp__`), then per prose line (raw line, not masked; L1292-1334):
  1. A heading line sets "under a CLAUDE.md heading" to whether its R3 text contains `CLAUDE.md`.
  2. Phrases: `section_candidates(line)` when the line contains `CLAUDE.md`, plus `labelled_item_candidates(line)` when under a CLAUDE.md heading and the line is not a heading.
  3. Each phrase through `resolve_phrase`: stub, moved and missing give a `claude-md:` finding; one finding per key per line (a later phrase with the same key on the same line is dropped; the same key on another line is kept and merges later by key).
  4. R66 rule references whose `.claude/rules/<file>` is not tracked.
  5. Code spans: a span starting `docs/` gives `clean_repo_path(first whitespace-separated word)`, reported when not a path placeholder and not `exists`; then R67 on the same span: reported when not a project skill, not a built-in command, and either unqualified or qualified with one of this repository's plugin names.
  6. R68 agents on the raw line: group 1 or group 2; reported unless a project agent or built-in.
- R62: the four patterns are concatenated from the same parts as check.py (no `format!`, so `{0,6}` and `{0,30}` stay literal); `\d` becomes `[0-9]` in LABELLED_ITEM_RE. The `regex` crate's leftmost-first semantics give the same matches and captures for these patterns as Python's backtracking.
- `norm_section` (L1169-1173): lowercase, `&` and every char outside `[a-z0-9 ]` to a space, Python split, drop `and`/`the`, `doc` to `document`. `claude_sections` (L1176-1192): per level-2 CLAUDE.md heading, the body is the following lines up to the next line starting `## `, excluding blank and `---` lines; a stub has at most 2 body lines and matches R61 on the space-joined body; a later heading with the same key overwrites. `rule_titles` (L1195-1203): level at most 2 headings of `.claude/rules/` md files, first file wins.
- `resolve_phrase` (L1252-1270): options are suffixes for `Before` and prefixes for `After`, longest first; all options are tried against CLAUDE.md sections before any is tried against rule headings; check.py returns `"name|rule"` for a moved section and its caller splits at the first `|`, which `Resolution::Moved` reproduces (a phrase containing `|` keeps only its part before the `|` as the section).
- knowledge-hygiene = L1350-1376 with constants L90-94: md files under `docs/knowledge/` except basenames `README.md` and `_template.md`; a file under `plans/` gives only `plans-not-empty`; otherwise `name` (R1 on the basename), then either `frontmatter` (and nothing more) or `missing:<key>` for author, date, tags, status in that order, then `status:<value>` when the status is non-empty and outside the allowed set (`draft`, `final`, `in-progress`, plus `active`, `graduated` under `discussions/`), listed sorted and joined by `, `. `parse_frontmatter` (T4, R69/R70) strips quotes from values.
- The `--list-checks` literal in `registry_tests` is check.py's output (L1597-1600) byte for byte.
- Expected findings in the tests below were recorded by running check.py's `check_pointer_doctor`, `check_knowledge_hygiene`, `norm_section`, `section_candidates`, `labelled_item_candidates`, `split_title_list` and `resolve_phrase` on the same inputs.

- [ ] **Step 1: Write the failing integration test**

Create `tools/tests/checks_pointer.rs`:

````rust
//! pointer-doctor and knowledge-hygiene on small committed repositories. The
//! expected findings were recorded from check.py's `check_pointer_doctor` and
//! `check_knowledge_hygiene` on the same files (production order, before
//! merging by key: "claude-md:Build Matrix" appears on two lines).

mod common;

use aios_tools::cmd::docs_check::checks::knowledge::KnowledgeHygiene;
use aios_tools::cmd::docs_check::checks::pointer_doctor::PointerDoctor;
use aios_tools::cmd::docs_check::checks::Check;
use aios_tools::cmd::docs_check::model::Finding;
use aios_tools::cmd::docs_check::repo::Repo;
use common::TestRepo;

const POINTER_FILES: &[(&str, &str)] = &[
    (
        "CLAUDE.md",
        r#"# Project

## Project Identity

Name: Fixture

## Architecture Document Map

Topic index lives in `docs/project/doc-map.md`.

## Key Technical Facts

- Fact one.
- Fact two.
- Fact three.

## Workspace Layout

Tree here.
"#,
    ),
    (
        ".claude/rules/01-code-conventions.md",
        r#"# Code Conventions

## Rust

- Use snake_case.
"#,
    ),
    (
        ".claude/rules/02-quality-gates.md",
        r#"# Quality Gates

Run the gates in CLAUDE.md order.
"#,
    ),
    (
        ".claude/agents/worker.md",
        r#"---
name: worker
description: Fixture worker.
tools: Read, Grep
---

# Worker

Does the work.
"#,
    ),
    (
        ".claude/agents/dev.md",
        r#"---
name: dev
description: Fixture agent.
tools: Read, Edit, MultiEdit, mcp__docs__search, Bash
---

# Dev

Follow the Code Conventions in CLAUDE.md and the Quality Gates from CLAUDE.md.
See the Architecture Document Map in CLAUDE.md for docs.
Read the Key Facts in CLAUDE.md, then the Glossary in CLAUDE.md.
Code Conventions in CLAUDE.md and Code Conventions from CLAUDE.md say the same.
Check `CLAUDE.md`: Workspace Layout, Key Technical Facts and Build Matrix.

```text
Old Section in CLAUDE.md is inside a fence.
```

## Update CLAUDE.md

1. Update: Workspace Layout, Build Matrix
- **Also**: Architecture Doc Map (the index)

## Other

1. Update: Nothing Here

Rules live in rules/03-git.md and `.claude/rules/01-code-conventions.md`.
Docs: `docs/missing/guide.md:12`, `docs/phases/NN-name.md`, `docs/project/doc-map.md`.
Skills: `/alpha`, `/help`, `/ghost`, `/other:thing`, `/kit:nope`, `/alpha --flag`.
Ask the `worker` agent, the `ghost` subagent, subagent_type: `Explore` or subagent_type: nobody.
"#,
    ),
    (
        ".claude/skills/alpha/SKILL.md",
        r#"---
name: alpha
description: Alpha skill.
---

# Alpha

Does alpha things.
"#,
    ),
    (
        ".claude/skills/pack/.claude-plugin/plugin.json",
        r#"{"name": "kit"}
"#,
    ),
    (
        ".claude/skills/pack/skills/go/SKILL.md",
        r#"---
name: go
description: Plugin skill.
---

# Go

Run `/kit:go` or `/kit:stop`.
"#,
    ),
    (
        "docs/project/doc-map.md",
        r#"# Doc Map
"#,
    ),
];

const KNOWLEDGE_FILES: &[(&str, &str)] = &[
    (
        "docs/knowledge/README.md",
        r#"# Knowledge
"#,
    ),
    (
        "docs/knowledge/plans/_template.md",
        r#"---
author: claude
---
"#,
    ),
    (
        "docs/knowledge/plans/2026-01-01-ab-plan.md",
        r#"# Plan without frontmatter
"#,
    ),
    (
        "docs/knowledge/lessons/2026-01-01-ab-good.md",
        r#"---
author: ab
date: 2026-01-01
tags: [kernel]
status: "final"
---

# Lesson
"#,
    ),
    (
        "docs/knowledge/lessons/Bad_Name.md",
        r#"---
author: ab
date: 2026-01-01
tags: [kernel]
status: final
---
"#,
    ),
    (
        "docs/knowledge/lessons/2026-01-02-ab-nofm.md",
        r#"# No frontmatter
"#,
    ),
    (
        "docs/knowledge/decisions/2026-01-03-ab-partial.md",
        r#"---
author: ab
status: wip
---
"#,
    ),
    (
        "docs/knowledge/discussions/2026-01-04-ab-talk.md",
        r#"---
author: ab
date: 2026-01-04
tags: [ipc]
status: active
---
"#,
    ),
    (
        "docs/knowledge/discussions/2026-01-05-ab-old.md",
        r#"---
author: ab
date: 2026-01-05
tags: [ipc]
status: 'archived'
---
"#,
    ),
    (
        "docs/knowledge/research/2026-01-06-abcd-long.md",
        r#"---
author: ab
date: 2026-01-06
tags: []
status:
---
"#,
    ),
    (
        "docs/other/notes.md",
        r#"# Not knowledge
"#,
    ),
];

fn open(repo: &TestRepo) -> Repo {
    Repo::open(repo.path_str()).expect("open the test repository")
}

#[test]
fn check_names() {
    assert_eq!(PointerDoctor.name(), "pointer-doctor");
    assert_eq!(KnowledgeHygiene.name(), "knowledge-hygiene");
}

#[test]
fn pointer_doctor_matches_check_py() {
    let repo = TestRepo::with_files("pointer-doctor", POINTER_FILES);
    let found = PointerDoctor
        .run(&open(&repo))
        .expect("pointer-doctor runs");
    let expected = vec![
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "tool:MultiEdit", "tools: lists unknown tool MultiEdit", 4),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Code Conventions", "points to CLAUDE.md 'Code Conventions', which now lives in .claude/rules/01-code-conventions.md", 9),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Quality Gates", "points to CLAUDE.md 'Quality Gates', which now lives in .claude/rules/02-quality-gates.md", 9),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Architecture Document Map", "points to CLAUDE.md 'Architecture Document Map', which is only a pointer stub now", 10),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Key Facts", "points to CLAUDE.md 'Key Facts', which is not a section of CLAUDE.md", 11),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Code Conventions", "points to CLAUDE.md 'Code Conventions', which now lives in .claude/rules/01-code-conventions.md", 12),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Build Matrix", "points to CLAUDE.md 'Build Matrix', which is not a section of CLAUDE.md", 13),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Build Matrix", "points to CLAUDE.md 'Build Matrix', which is not a section of CLAUDE.md", 21),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "claude-md:Architecture Document Map", "points to CLAUDE.md 'Architecture Document Map', which is only a pointer stub now", 22),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "rules:03-git.md", "rule file 03-git.md does not exist", 28),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "path:docs/missing/guide.md", "path does not exist: docs/missing/guide.md", 29),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "skill:/ghost", "/ghost is not a project skill or built-in command", 30),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "skill:/kit:nope", "/kit:nope is not a project skill or built-in command", 30),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "agent:ghost", "agent ghost is not defined in .claude/agents", 31),
        Finding::new("pointer-doctor", ".claude/agents/dev.md", "agent:nobody", "agent nobody is not defined in .claude/agents", 31),
        Finding::new("pointer-doctor", ".claude/skills/pack/skills/go/SKILL.md", "skill:/kit:stop", "/kit:stop is not a project skill or built-in command", 8),
    ];
    assert_eq!(found, expected);
}

#[test]
fn knowledge_hygiene_matches_check_py() {
    let repo = TestRepo::with_files("knowledge-hygiene", KNOWLEDGE_FILES);
    let found = KnowledgeHygiene
        .run(&open(&repo))
        .expect("knowledge-hygiene runs");
    let expected = vec![
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "missing:date",
            "frontmatter lacks 'date'",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "missing:tags",
            "frontmatter lacks 'tags'",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/decisions/2026-01-03-ab-partial.md",
            "status:wip",
            "status 'wip' is not one of draft, final, in-progress",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/discussions/2026-01-05-ab-old.md",
            "status:archived",
            "status 'archived' is not one of active, draft, final, graduated, in-progress",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/lessons/2026-01-02-ab-nofm.md",
            "frontmatter",
            "no YAML frontmatter",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/lessons/Bad_Name.md",
            "name",
            "file name is not YYYY-MM-DD-initials-short-description.md",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/plans/2026-01-01-ab-plan.md",
            "plans-not-empty",
            "working plan present; distill it and remove it before the PR is ready",
            0,
        ),
        Finding::new(
            "knowledge-hygiene",
            "docs/knowledge/research/2026-01-06-abcd-long.md",
            "name",
            "file name is not YYYY-MM-DD-initials-short-description.md",
            0,
        ),
    ];
    assert_eq!(found, expected);
}
````

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test checks_pointer`
Expected: FAIL to compile: `error[E0432]: unresolved import` for `aios_tools::cmd::docs_check::checks::knowledge` and `aios_tools::cmd::docs_check::checks::pointer_doctor`.

- [ ] **Step 3: Write the failing registry test**

Append to the end of `tools/src/cmd/docs_check/checks/mod.rs` (the module name `registry_tests` keeps it apart from any `mod tests` added in Task 6):

````rust
#[cfg(test)]
mod registry_tests {
    use super::registry;
    use crate::cmd::docs_check::model::CHECK_ORDER;
    use crate::cmd::docs_check::output::render_list_checks;

    /// `python3 scripts/docs/check.py --list-checks` output (check.py L1601-1604), byte for byte.
    const CHECK_PY_LIST_CHECKS: &str = "\
md-links           relative [text](path) links resolve to a tracked file or directory
section-refs       [x.md](path) §N resolves to a numbered heading (hub subfolders included)
anchors            #fragment links resolve to a GitHub-style heading slug
wiki-links         [[Note]] links resolve to a note in the docs/ vault
doc-map            doc-map.md paths exist and every architecture doc is listed
repo-paths         backticked kernel/ shared/ uefi-stub/ scripts/ paths exist (current-state docs)
just-recipes       backticked `just X` recipes exist; public recipes are documented
test-count         stated host test counts match #[test] in shared/src
lock-order         production Mutex statics vs deadlock-prevention.md §3.3-3.4 and CLAUDE.md
milestone-status   merged 'Phase N MK:' milestones vs phase docs, README, development-plan
phase-count        phase counts in prose match the development-plan §8 table
layout             kernel/src and shared/src modules vs CLAUDE.md layout and rule 05
harness-tables     CLAUDE.md skills/agents tables and layout lists vs .claude/ (plugin skills as plugin:skill)
pointer-doctor     CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/
knowledge-hygiene  docs/knowledge naming, frontmatter, and an empty plans/ dir
";

    #[test]
    fn registry_follows_check_order() {
        let names: Vec<&str> = registry().iter().map(|check| check.name()).collect();
        assert_eq!(names, CHECK_ORDER);
    }

    #[test]
    fn list_checks_matches_check_py() {
        assert_eq!(render_list_checks(&registry()), CHECK_PY_LIST_CHECKS);
    }
}
````

Run: `cargo test -p aios-tools --lib registry_tests`
Expected: FAIL, both tests: `registry_follows_check_order` panics with ``assertion `left == right` failed`` (left lists 13 names ending `"harness-tables"`), and `list_checks_matches_check_py` fails the same way because the pointer-doctor and knowledge-hygiene lines are missing.

- [ ] **Step 4: Implement pointer-doctor**

Create `tools/src/cmd/docs_check/checks/pointer_doctor.rs`:

````rust
//! `pointer-doctor` (check.py L1169-1335 at 33c6b3d): what the harness files under
//! `.claude/agents/`, `.claude/skills/` and `.claude/rules/` point at: CLAUDE.md
//! sections (missing, reduced to a pointer stub, or moved into a rule file),
//! rule files, `docs/` paths, `/skill` commands, agents and `tools:` entries.
//!
//! The section-name patterns (R62) are built from the same parts as check.py's
//! `TITLE_WORD`, `TITLE_LIST`, `BEFORE_CLAUDE_RE`, `AFTER_CLAUDE_RE` and
//! `LABELLED_ITEM_RE`; the `regex` crate's leftmost-first semantics give the
//! same matches and captures as Python's backtracking for them. Accepted
//! divergences: `\s` lacks U+001C..U+001F and `\b` uses the crate's Unicode word
//! definition (no tracked harness file contains either difference).

use std::collections::{BTreeSet, HashMap, HashSet};
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::harness::{project_agents, project_skills, SKILL_NAME};
use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::{
    clean_repo_path, code_spans, is_path_placeholder, prose_lines, HEADING_RE,
};
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::pystr::{split_ws, splitlines, strip};

const CHECK: &str = "pointer-doctor";
const CLAUDE_MD: &str = "CLAUDE.md";
const HARNESS_PREFIXES: [&str; 3] = [".claude/agents/", ".claude/skills/", ".claude/rules/"];

/// Claude Code tool names accepted in agent `tools:` frontmatter (check.py
/// L70-77). `mcp__*` names are accepted as-is. MultiEdit is deliberately absent:
/// it is not a tool in current Claude Code releases.
pub const KNOWN_TOOLS: [&str; 34] = [
    "Agent",
    "AskUserQuestion",
    "Bash",
    "BashOutput",
    "CronCreate",
    "CronDelete",
    "CronList",
    "Edit",
    "EnterPlanMode",
    "EnterWorktree",
    "ExitPlanMode",
    "ExitWorktree",
    "Glob",
    "Grep",
    "KillShell",
    "LSP",
    "ListMcpResourcesTool",
    "Monitor",
    "NotebookEdit",
    "PushNotification",
    "Read",
    "ReadMcpResourceTool",
    "RemoteTrigger",
    "SendMessage",
    "Skill",
    "Task",
    "TaskOutput",
    "TaskStop",
    "TodoWrite",
    "ToolSearch",
    "WebFetch",
    "WebSearch",
    "Workflow",
    "Write",
];

/// Slash commands built into Claude Code rather than project skills (check.py L80-85).
pub const BUILTIN_COMMANDS: [&str; 29] = [
    "add-dir",
    "agents",
    "clear",
    "compact",
    "config",
    "context",
    "cost",
    "doctor",
    "exit",
    "goal",
    "help",
    "hooks",
    "init",
    "loop",
    "mcp",
    "memory",
    "model",
    "permissions",
    "plan",
    "resume",
    "review",
    "schedule",
    "security-review",
    "simplify",
    "code-review",
    "status",
    "statusline",
    "tasks",
    "todos",
];

/// Built-in subagent types that need no `.claude/agents/` definition (check.py L88).
pub const BUILTIN_AGENTS: [&str; 5] = [
    "general-purpose",
    "Explore",
    "Plan",
    "statusline-setup",
    "output-style-setup",
];

/// R62 `TITLE_WORD`.
const TITLE_WORD: &str = r"[A-Z][A-Za-z0-9&/-]*";

/// R62 `TITLE_LIST`: a run of Title Case section names joined by commas, "and",
/// or a parenthetical aside: "Code Conventions (`.claude/rules/`) and Quality Gates".
fn title_list() -> String {
    [
        "(",
        TITLE_WORD,
        r"(?:\s*\([^)]*\)|\s*,\s*|\s+and\s+|\s+|",
        TITLE_WORD,
        ")*)",
    ]
    .concat()
}

/// R62 `BEFORE_CLAUDE_RE`: "<Title Words> in CLAUDE.md".
static BEFORE_CLAUDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [
        "((?:",
        TITLE_WORD,
        r"\s+){0,6}",
        TITLE_WORD,
        r#")["'”*]*\s*\(?\s*(?:in|from)\s+`?CLAUDE\.md\b"#,
    ]
    .concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R62 `AFTER_CLAUDE_RE`: "CLAUDE.md: <title list>".
static AFTER_CLAUDE_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [r#"`?CLAUDE\.md`?\s*(:)?\s*["“]?"#, title_list().as_str()].concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R62 `LABELLED_ITEM_RE`: "2. Update: Workspace Layout, Key Technical Facts"
/// under a heading that names CLAUDE.md.
static LABELLED_ITEM_RE: LazyLock<Regex> = LazyLock::new(|| {
    let pattern = [
        r"^\s*(?:[-*+]|[0-9]+[.)])\s+(?:\*\*)?[A-Za-z][A-Za-z ]{0,30}?(?:\*\*)?:\s*",
        title_list().as_str(),
    ]
    .concat();
    Regex::new(&pattern).expect("valid regex")
});
/// R63.
static TITLE_SPLIT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\([^)]*\)|\band\b").expect("valid regex"));
/// R61: a CLAUDE.md section body that only points elsewhere.
static STUB_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\b(lives in|moved to|are in|is in|see)\b").expect("valid regex")
});
/// R64: the frontmatter block (LF only, as check.py L1283).
static FRONTMATTER_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?s)^---\n(.*?)\n---").expect("valid regex"));
/// R65.
static TOOLS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^tools:\s*(.*)$").expect("valid regex"));
/// R66.
static RULE_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?:\.claude/)?rules/([0-9]{2}-[a-z0-9-]+\.md)").expect("valid regex")
});
/// R67: a code span that is a slash command.
static SKILL_SPAN_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&["^/(", SKILL_NAME, r")(?:\s|$)"].concat()).expect("valid regex"));
/// R68: "`name` agent", "`name` subagent" or "subagent_type: name".
static AGENT_REF_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"`([a-z][a-z0-9-]*)`\s+(?:agent|subagent)\b|subagent_type:\s*`?([A-Za-z][A-Za-z0-9-]*)",
    )
    .expect("valid regex")
});

/// CLAUDE.md sections, rules, paths, skills, agents, tools named by .claude/.
pub struct PointerDoctor;

/// A candidate CLAUDE.md section name, as words. `Before` came from
/// "<words> in CLAUDE.md" (try dropping leading words), `After` from
/// "CLAUDE.md <words>" or a labelled list item (try dropping trailing words).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phrase {
    Before(Vec<String>),
    After(Vec<String>),
}

/// check.py `resolve_phrase` statuses: `Ok`/`Stub` carry the CLAUDE.md heading,
/// `Moved` the phrase and the rule file, `Missing` the whole phrase.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resolution {
    Ok(String),
    Stub(String),
    Moved { section: String, rule: String },
    Missing(String),
    Ignore,
}

/// check.py L1169-1173: lowercase, `&` and every char outside `[a-z0-9 ]`
/// become spaces, drop "and"/"the", "doc" → "document", single spaces.
pub fn norm_section(name: &str) -> String {
    let mapped: String = name
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_lowercase() || c.is_ascii_digit() || c == ' ' {
                c
            } else {
                ' '
            }
        })
        .collect();
    split_ws(&mapped)
        .into_iter()
        .filter(|w| *w != "and" && *w != "the")
        .map(|w| if w == "doc" { "document" } else { w })
        .collect::<Vec<_>>()
        .join(" ")
}

/// check.py L1176-1192: normalized `## ` heading → (heading, is a pointer stub).
/// A stub has at most two non-blank, non-`---` body lines and says where the
/// content lives now. Later headings with the same key win.
fn claude_sections(repo: &Repo) -> HashMap<String, (String, bool)> {
    let text = repo.text(CLAUDE_MD);
    let lines = splitlines(&text);
    let mut out = HashMap::new();
    for heading in repo.headings(CLAUDE_MD).iter() {
        if heading.level != 2 {
            continue;
        }
        let body: Vec<&str> = lines
            .iter()
            .skip(heading.line)
            .take_while(|line| !line.starts_with("## "))
            .filter(|line| {
                let s = strip(line);
                !s.is_empty() && s != "---"
            })
            .copied()
            .collect();
        let stub = body.len() <= 2 && STUB_RE.is_match(&body.join(" "));
        out.insert(norm_section(&heading.text), (heading.text.clone(), stub));
    }
    out
}

/// check.py L1195-1203: normalized `# `/`## ` heading → the first rule file
/// under `.claude/rules/` that has it.
fn rule_titles(repo: &Repo) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for f in repo.md_files() {
        if !f.starts_with(".claude/rules/") {
            continue;
        }
        for heading in repo.headings(f).iter() {
            if heading.level <= 2 {
                out.entry(norm_section(&heading.text))
                    .or_insert_with(|| f.clone());
            }
        }
    }
    out
}

/// check.py L1218-1234: split a TITLE_LIST match into section-name word lists.
/// "and" and parenthetical asides always separate names; commas only when
/// `all_parts` (after a colon), otherwise the list ends at the first comma.
fn split_title_list(text: &str, all_parts: bool) -> Vec<Vec<String>> {
    let chunks = text.split(',').take(if all_parts { usize::MAX } else { 1 });
    let mut out = Vec::new();
    for chunk in chunks {
        for part in TITLE_SPLIT_RE.split(chunk) {
            let words = split_ws(part);
            if !words.is_empty() {
                out.push(words.into_iter().map(str::to_string).collect());
            }
        }
    }
    out
}

/// check.py L1237-1244: phrases before and after each `CLAUDE.md` on a line.
fn section_candidates(line: &str) -> Vec<Phrase> {
    let mut phrases = Vec::new();
    for caps in BEFORE_CLAUDE_RE.captures_iter(line) {
        let words = split_ws(caps.get(1).map_or("", |m| m.as_str()));
        phrases.push(Phrase::Before(
            words.into_iter().map(str::to_string).collect(),
        ));
    }
    for caps in AFTER_CLAUDE_RE.captures_iter(line) {
        let list = caps.get(2).map_or("", |m| m.as_str());
        for words in split_title_list(list, caps.get(1).is_some()) {
            phrases.push(Phrase::After(words));
        }
    }
    phrases
}

/// check.py L1247-1249: the names of a labelled list item, as `After` phrases.
fn labelled_item_candidates(line: &str) -> Vec<Phrase> {
    match LABELLED_ITEM_RE.captures(line).and_then(|caps| caps.get(1)) {
        Some(list) => split_title_list(list.as_str(), true)
            .into_iter()
            .map(Phrase::After)
            .collect(),
        None => Vec::new(),
    }
}

/// check.py L1252-1270. Options are the suffixes (`Before`) or prefixes
/// (`After`) of the phrase, longest first; a CLAUDE.md section wins over a rule
/// heading; an unresolved single word is ignored.
pub fn resolve_phrase(
    p: &Phrase,
    sections: &HashMap<String, (String, bool)>,
    rules: &HashMap<String, String>,
) -> Resolution {
    let (words, before) = match p {
        Phrase::Before(words) => (words, true),
        Phrase::After(words) => (words, false),
    };
    let n = words.len();
    let options: Vec<&[String]> = if before {
        (0..n).map(|i| &words[i..]).collect()
    } else {
        (1..=n).rev().map(|j| &words[..j]).collect()
    };
    for opt in &options {
        if let Some((heading, stub)) = sections.get(&norm_section(&opt.join(" "))) {
            return if *stub {
                Resolution::Stub(heading.clone())
            } else {
                Resolution::Ok(heading.clone())
            };
        }
    }
    for opt in &options {
        let name = opt.join(" ");
        if let Some(rule) = rules.get(&norm_section(&name)) {
            // check.py returns "name|rule" and its caller splits at the first '|'.
            return match name.split_once('|') {
                Some((section, rest)) => Resolution::Moved {
                    section: section.to_string(),
                    rule: format!("{rest}|{rule}"),
                },
                None => Resolution::Moved {
                    section: name.clone(),
                    rule: rule.clone(),
                },
            };
        }
    }
    if n < 2 {
        Resolution::Ignore
    } else {
        Resolution::Missing(words.join(" "))
    }
}

/// check.py L1282-1290: unknown tools in the `tools:` frontmatter line; line
/// numbers count the frontmatter body from 2.
fn tool_findings(rel: &str, text: &str, out: &mut Vec<Finding>) {
    let Some(body) = FRONTMATTER_RE.captures(text).and_then(|caps| caps.get(1)) else {
        return;
    };
    for (i, line) in splitlines(body.as_str()).into_iter().enumerate() {
        let Some(list) = TOOLS_RE.captures(line).and_then(|caps| caps.get(1)) else {
            continue;
        };
        for tool in list
            .as_str()
            .split(',')
            .map(strip)
            .filter(|t| !t.is_empty())
        {
            if !KNOWN_TOOLS.contains(&tool) && !tool.starts_with("mcp__") {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("tool:{tool}"),
                    format!("tools: lists unknown tool {tool}"),
                    i + 2,
                ));
            }
        }
    }
}

/// Facts about `.claude/` and CLAUDE.md shared by every harness file.
struct Context<'a> {
    repo: &'a Repo,
    sections: HashMap<String, (String, bool)>,
    rules: HashMap<String, String>,
    skills: BTreeSet<String>,
    plugins: BTreeSet<String>,
    agents: BTreeSet<String>,
}

/// check.py L1299-1334 for one prose line of `rel`.
fn line_findings(
    ctx: &Context<'_>,
    rel: &str,
    lineno: usize,
    line: &str,
    phrases: &[Phrase],
    out: &mut Vec<Finding>,
) {
    let mut line_keys: HashSet<String> = HashSet::new();
    for phrase in phrases {
        let (target, message) = match resolve_phrase(phrase, &ctx.sections, &ctx.rules) {
            Resolution::Stub(name) => (
                format!("claude-md:{name}"),
                format!("points to CLAUDE.md '{name}', which is only a pointer stub now"),
            ),
            Resolution::Moved { section, rule } => (
                format!("claude-md:{section}"),
                format!("points to CLAUDE.md '{section}', which now lives in {rule}"),
            ),
            Resolution::Missing(name) => (
                format!("claude-md:{name}"),
                format!("points to CLAUDE.md '{name}', which is not a section of CLAUDE.md"),
            ),
            Resolution::Ok(_) | Resolution::Ignore => continue,
        };
        let finding = Finding::new(CHECK, rel, target, message, lineno);
        if line_keys.insert(finding.key()) {
            out.push(finding);
        }
    }
    for caps in RULE_REF_RE.captures_iter(line) {
        let file = caps.get(1).map_or("", |m| m.as_str());
        if !ctx.repo.is_file(&format!(".claude/rules/{file}")) {
            out.push(Finding::new(
                CHECK,
                rel,
                format!("rules:{file}"),
                format!("rule file {file} does not exist"),
                lineno,
            ));
        }
    }
    for span in code_spans(line) {
        if span.starts_with("docs/") {
            let path = clean_repo_path(split_ws(&span).first().copied().unwrap_or(""));
            if !is_path_placeholder(&path) && !ctx.repo.exists(&path) {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("path:{path}"),
                    format!("path does not exist: {path}"),
                    lineno,
                ));
            }
        }
        if let Some(name) = SKILL_SPAN_RE.captures(&span).and_then(|caps| caps.get(1)) {
            let name = name.as_str();
            // /<plugin>:<skill> is checked only for this repo's plugins; others are installed per user.
            let checked = match name.split_once(':') {
                Some((plugin, _)) => ctx.plugins.contains(plugin),
                None => true,
            };
            if checked && !ctx.skills.contains(name) && !BUILTIN_COMMANDS.contains(&name) {
                out.push(Finding::new(
                    CHECK,
                    rel,
                    format!("skill:/{name}"),
                    format!("/{name} is not a project skill or built-in command"),
                    lineno,
                ));
            }
        }
    }
    for caps in AGENT_REF_RE.captures_iter(line) {
        let Some(name) = caps.get(1).or_else(|| caps.get(2)).map(|m| m.as_str()) else {
            continue;
        };
        if !ctx.agents.contains(name) && !BUILTIN_AGENTS.contains(&name) {
            out.push(Finding::new(
                CHECK,
                rel,
                format!("agent:{name}"),
                format!("agent {name} is not defined in .claude/agents"),
                lineno,
            ));
        }
    }
}

impl Check for PointerDoctor {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let (skills, plugins) = project_skills(repo);
        let ctx = Context {
            repo,
            sections: claude_sections(repo),
            rules: rule_titles(repo),
            skills,
            plugins,
            agents: project_agents(repo),
        };
        let mut out = Vec::new();
        for rel in repo.md_files() {
            if !HARNESS_PREFIXES
                .iter()
                .any(|&prefix| rel.starts_with(prefix))
            {
                continue;
            }
            let text = repo.text(rel);
            tool_findings(rel, &text, &mut out);
            let mut under_claude_heading = false;
            for (lineno, line) in prose_lines(&text) {
                let heading = HEADING_RE.captures(line);
                if let Some(caps) = &heading {
                    under_claude_heading =
                        caps.get(2).is_some_and(|m| m.as_str().contains(CLAUDE_MD));
                }
                let mut phrases = if line.contains(CLAUDE_MD) {
                    section_candidates(line)
                } else {
                    Vec::new()
                };
                if under_claude_heading && heading.is_none() {
                    phrases.extend(labelled_item_candidates(line));
                }
                line_findings(&ctx, rel, lineno, line, &phrases, &mut out);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    fn before(items: &[&str]) -> Phrase {
        Phrase::Before(words(items))
    }

    fn after(items: &[&str]) -> Phrase {
        Phrase::After(words(items))
    }

    #[test]
    fn regexes_compile() {
        for re in [
            &BEFORE_CLAUDE_RE,
            &AFTER_CLAUDE_RE,
            &LABELLED_ITEM_RE,
            &TITLE_SPLIT_RE,
            &STUB_RE,
            &FRONTMATTER_RE,
            &TOOLS_RE,
            &RULE_REF_RE,
            &SKILL_SPAN_RE,
            &AGENT_REF_RE,
        ] {
            LazyLock::force(re);
        }
    }

    #[test]
    fn norm_section_matches_check_py() {
        assert_eq!(
            norm_section("Architecture Doc Map"),
            "architecture document map"
        );
        assert_eq!(norm_section("The Code & Conventions"), "code conventions");
        assert_eq!(norm_section("Key-Technical_Facts!"), "key technical facts");
        assert_eq!(norm_section("and the"), "");
        assert_eq!(norm_section("ÉTAT Doc"), "tat document");
    }

    #[test]
    fn section_candidates_match_check_py() {
        assert_eq!(
            section_candidates(
                "Follow the Code Conventions in CLAUDE.md and the Quality Gates from CLAUDE.md."
            ),
            vec![
                before(&["Code", "Conventions"]),
                before(&["Quality", "Gates"])
            ]
        );
        assert_eq!(
            section_candidates(
                "Check `CLAUDE.md`: Workspace Layout, Key Technical Facts and Build Matrix."
            ),
            vec![
                after(&["Workspace", "Layout"]),
                after(&["Key", "Technical", "Facts"]),
                after(&["Build", "Matrix"])
            ]
        );
        assert_eq!(
            section_candidates("See CLAUDE.md Workspace Layout, then the rest."),
            vec![after(&["Workspace", "Layout"])]
        );
        assert_eq!(
            section_candidates("Read \"Key Facts\" (in CLAUDE.md) today."),
            vec![before(&["Key", "Facts"])]
        );
        assert_eq!(
            section_candidates(
                "The CLAUDE.md “Code Conventions (`.claude/rules/`) and Quality Gates” sections."
            ),
            vec![
                after(&["Code", "Conventions"]),
                after(&["Quality", "Gates"])
            ]
        );
    }

    #[test]
    fn labelled_item_candidates_match_check_py() {
        assert_eq!(
            labelled_item_candidates("1. Update: Workspace Layout, Key Technical Facts"),
            vec![
                after(&["Workspace", "Layout"]),
                after(&["Key", "Technical", "Facts"])
            ]
        );
        assert_eq!(
            labelled_item_candidates("- **Also**: Architecture Doc Map (the index)"),
            vec![after(&["Architecture", "Doc", "Map"])]
        );
        assert_eq!(
            labelled_item_candidates("   * Note:Build Matrix and Test Plan"),
            vec![after(&["Build", "Matrix"]), after(&["Test", "Plan"])]
        );
        assert_eq!(
            labelled_item_candidates("Update: Workspace Layout"),
            Vec::new()
        );
    }

    #[test]
    fn split_title_list_matches_check_py() {
        assert_eq!(
            split_title_list("Workspace Layout, Key Facts and Build (x, y) Matrix", true),
            vec![
                words(&["Workspace", "Layout"]),
                words(&["Key", "Facts"]),
                words(&["Build", "(x"]),
                words(&["y)", "Matrix"])
            ]
        );
        assert_eq!(
            split_title_list("Workspace Layout, then more", false),
            vec![words(&["Workspace", "Layout"])]
        );
    }

    #[test]
    fn resolve_phrase_matches_check_py() {
        let sections: HashMap<String, (String, bool)> = [
            ("workspace layout", "Workspace Layout", false),
            (
                "architecture document map",
                "Architecture Document Map",
                true,
            ),
        ]
        .into_iter()
        .map(|(key, heading, stub)| (key.to_string(), (heading.to_string(), stub)))
        .collect();
        let rules: HashMap<String, String> = [
            ("code conventions", ".claude/rules/01-code-conventions.md"),
            ("b c", ".claude/rules/09-x.md"),
        ]
        .into_iter()
        .map(|(key, file)| (key.to_string(), file.to_string()))
        .collect();
        let resolve = |p: Phrase| resolve_phrase(&p, &sections, &rules);
        assert_eq!(
            resolve(before(&["Follow", "Workspace", "Layout"])),
            Resolution::Ok("Workspace Layout".to_string())
        );
        assert_eq!(
            resolve(after(&["Workspace", "Layout", "Rules"])),
            Resolution::Ok("Workspace Layout".to_string())
        );
        assert_eq!(
            resolve(before(&["Architecture", "Doc", "Map"])),
            Resolution::Stub("Architecture Document Map".to_string())
        );
        assert_eq!(
            resolve(after(&["Code", "Conventions", "Now"])),
            Resolution::Moved {
                section: "Code Conventions".to_string(),
                rule: ".claude/rules/01-code-conventions.md".to_string()
            }
        );
        assert_eq!(
            resolve(after(&["Build", "Matrix"])),
            Resolution::Missing("Build Matrix".to_string())
        );
        assert_eq!(resolve(before(&["Glossary"])), Resolution::Ignore);
        // check.py splits "b|c)|<rule>" at the first '|'.
        assert_eq!(
            resolve(after(&["b|c)"])),
            Resolution::Moved {
                section: "b".to_string(),
                rule: "c)|.claude/rules/09-x.md".to_string()
            }
        );
    }
}
````

- [ ] **Step 5: Implement knowledge-hygiene**

Create `tools/src/cmd/docs_check/checks/knowledge.rs`:

````rust
//! `knowledge-hygiene` (check.py L1350-1376 at 33c6b3d): `docs/knowledge/` file names
//! (`YYYY-MM-DD-initials-short-description.md`), YAML frontmatter keys and
//! status values, and an empty `plans/` directory.

use std::collections::BTreeSet;
use std::sync::LazyLock;

use regex::Regex;

use crate::cmd::docs_check::checks::Check;
use crate::cmd::docs_check::markdown::parse_frontmatter;
use crate::cmd::docs_check::model::Finding;
use crate::cmd::docs_check::repo::Repo;
use crate::paths::basename;

const CHECK: &str = "knowledge-hygiene";
const KNOWLEDGE_DIR: &str = "docs/knowledge/";
const PLANS_DIR: &str = "docs/knowledge/plans/";
/// Frontmatter keys every note needs (check.py L93).
const KNOWLEDGE_KEYS: [&str; 4] = ["author", "date", "tags", "status"];
/// check.py L90.
const KNOWLEDGE_STATUSES: [&str; 3] = ["draft", "in-progress", "final"];
/// check.py L92: discussions are "draft or active", then "graduated".
const DISCUSSION_STATUSES: [&str; 2] = ["active", "graduated"];

/// R1 (check.py L94).
static KNOWLEDGE_NAME_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[0-9]{4}-[0-9]{2}-[0-9]{2}-[a-z]{2,3}-[a-z0-9][a-z0-9-]*\.md$")
        .expect("valid regex")
});

/// docs/knowledge naming, frontmatter, and an empty plans/ dir.
pub struct KnowledgeHygiene;

impl Check for KnowledgeHygiene {
    fn name(&self) -> &'static str {
        CHECK
    }

    fn run(&self, repo: &Repo) -> anyhow::Result<Vec<Finding>> {
        let mut out = Vec::new();
        for f in repo.md_files() {
            if !f.starts_with(KNOWLEDGE_DIR) {
                continue;
            }
            let base = basename(f);
            if base == "README.md" || base == "_template.md" {
                continue;
            }
            if f.starts_with(PLANS_DIR) {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "plans-not-empty",
                    "working plan present; distill it and remove it before the PR is ready",
                    0,
                ));
                continue;
            }
            if !KNOWLEDGE_NAME_RE.is_match(base) {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "name",
                    "file name is not YYYY-MM-DD-initials-short-description.md",
                    0,
                ));
            }
            let Some(frontmatter) = parse_frontmatter(&repo.text(f)) else {
                out.push(Finding::new(
                    CHECK,
                    f.as_str(),
                    "frontmatter",
                    "no YAML frontmatter",
                    0,
                ));
                continue;
            };
            for key in KNOWLEDGE_KEYS {
                if !frontmatter.contains_key(key) {
                    out.push(Finding::new(
                        CHECK,
                        f.as_str(),
                        format!("missing:{key}"),
                        format!("frontmatter lacks '{key}'"),
                        0,
                    ));
                }
            }
            let mut allowed: BTreeSet<&str> = KNOWLEDGE_STATUSES.into_iter().collect();
            if f.split('/').nth(2) == Some("discussions") {
                allowed.extend(DISCUSSION_STATUSES);
            }
            if let Some(status) = frontmatter.get("status") {
                if !status.is_empty() && !allowed.contains(status.as_str()) {
                    let listed = allowed.into_iter().collect::<Vec<_>>().join(", ");
                    out.push(Finding::new(
                        CHECK,
                        f.as_str(),
                        format!("status:{status}"),
                        format!("status '{status}' is not one of {listed}"),
                        0,
                    ));
                }
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn regexes_compile() {
        LazyLock::force(&KNOWLEDGE_NAME_RE);
    }

    #[test]
    fn knowledge_name_pattern() {
        assert!(KNOWLEDGE_NAME_RE.is_match("2026-09-22-jl-rust-agent-tools.md"));
        assert!(KNOWLEDGE_NAME_RE.is_match("2026-01-01-abc-x.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-06-abcd-long.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("Bad_Name.md"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-01-ab--x.md.txt"));
        assert!(!KNOWLEDGE_NAME_RE.is_match("2026-01-01-ab-X.md"));
    }
}
````

- [ ] **Step 6: Complete the registry**

In `tools/src/cmd/docs_check/checks/mod.rs`, make the `pub mod` block read exactly:

```rust
pub mod doc_map;
pub mod harness;
pub mod just_recipes;
pub mod knowledge;
pub mod layout;
pub mod links;
pub mod lock_order;
pub mod milestones;
pub mod pointer_doctor;
pub mod repo_paths;
pub mod test_count;
```

Then replace the whole `pub fn registry()` item including its doc comment (the per-task note no longer applies) with:

```rust
/// Every docs-check check, in CHECK_ORDER (check.py `CHECK_FUNCS`, L1379-1395 at 33c6b3d).
pub fn registry() -> Vec<Box<dyn Check>> {
    vec![
        Box::new(links::MdLinks),
        Box::new(links::SectionRefs),
        Box::new(links::Anchors),
        Box::new(links::WikiLinks),
        Box::new(doc_map::DocMap),
        Box::new(repo_paths::RepoPaths),
        Box::new(just_recipes::JustRecipes),
        Box::new(test_count::TestCount),
        Box::new(lock_order::LockOrder),
        Box::new(milestones::MilestoneStatus),
        Box::new(milestones::PhaseCount),
        Box::new(layout::Layout),
        Box::new(harness::HarnessTables),
        Box::new(pointer_doctor::PointerDoctor),
        Box::new(knowledge::KnowledgeHygiene),
    ]
}
```

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test checks_pointer`
Expected: PASS, `test result: ok. 3 passed; 0 failed`.

Run: `cargo test -p aios-tools --lib checks::pointer_doctor`, `cargo test -p aios-tools --lib checks::knowledge`, `cargo test -p aios-tools --lib registry_tests`
Expected: PASS with 6, 2 and 2 tests.

Run: `cargo test -p aios-tools`
Expected: every test binary ends `test result: ok.` with 0 failed (unit tests, `shim`, `repo`, `cli`, `checks_links`, `checks_paths`, `checks_code`, `checks_status`, `checks_harness`, `checks_pointer`, doc-tests).

- [ ] **Step 8: Compare with check.py on the real repository**

```bash
just tools
diff <(python3 scripts/docs/check.py --list-checks) <(target/tools/release/aios docs-check --list-checks) && echo IDENTICAL-LIST
d=$(mktemp -d)
python3 scripts/docs/check.py --all >"$d/py-all.txt"; echo "check.py exit $?"
target/tools/release/aios docs-check --all >"$d/aios-all.txt"; echo "aios exit $?"
cmp "$d/py-all.txt" "$d/aios-all.txt" && echo IDENTICAL-ALL
python3 scripts/docs/check.py --json --all >"$d/py.json"
target/tools/release/aios docs-check --json --all >"$d/aios.json"
cmp "$d/py.json" "$d/aios.json" && echo IDENTICAL-JSON
python3 scripts/docs/check.py --markdown >"$d/py.md.txt"
target/tools/release/aios docs-check --markdown >"$d/aios.md.txt"
cmp "$d/py.md.txt" "$d/aios.md.txt" && echo IDENTICAL-MARKDOWN
rm -rf "$d"
```

Expected: `IDENTICAL-LIST`; `check.py exit 1` and `aios exit 1` (this branch carries new drift until Tasks 15-16: at least the plan file's `knowledge-hygiene` `plans-not-empty` and `just-recipes` `undocumented:tools` for the Task 6 recipe; whatever the set, both sides must report the same); `IDENTICAL-ALL`, `IDENTICAL-JSON`, `IDENTICAL-MARKDOWN`. Never pass `--update-baseline` here: it rewrites `scripts/docs/baseline.json`. This is an early smoke check; Task 14's goldens and differential test are the parity proof. On a difference, keep `$d`, rerun both sides with `--all --check <name>` for each of the 15 checks to find the diverging check, fix it in its module and add the input to that check's test file.

- [ ] **Step 9: Format and lint**

```bash
cargo fmt -p aios-tools
cargo fmt --check -p aios-tools
cargo clippy -p aios-tools -- -D warnings
```

Expected: `cargo fmt --check` prints nothing and exits 0 (the code above is already rustfmt-clean, so the first command changes nothing); clippy ends with `Finished` and reports no warnings.

- [ ] **Step 10: Commit and push**

```bash
git add tools/src/cmd/docs_check/checks/pointer_doctor.rs tools/src/cmd/docs_check/checks/knowledge.rs tools/src/cmd/docs_check/checks/mod.rs tools/tests/checks_pointer.rs
git commit -F - <<'EOF'
Port docs-check pointer-doctor and knowledge-hygiene to Rust

pointer-doctor checks what the .claude/ harness files point at (CLAUDE.md
sections, rule files, docs/ paths, skills, agents, tools: entries);
knowledge-hygiene checks docs/knowledge naming, frontmatter and the empty
plans/ directory. The registry now holds all 15 checks in CHECK_ORDER,
and a unit test pins --list-checks to check.py's output byte for byte.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 13: Fixture repository and drift-injection test

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Create: `tools/tests/fixtures/docs-check/base.txt`
- Create: `tools/tests/fixtures/docs-check/variants/<name>.txt` for the 18 names `anchors`, `doc-map`, `grown`, `harness-tables`, `just-recipes`, `knowledge-hygiene`, `layout`, `line-shift`, `lock-order`, `md-links`, `milestone-status`, `phase-count`, `pointer-doctor`, `repo-paths`, `section-refs`, `skip`, `test-count`, `wiki-links`
- Create: `tools/tests/common/fixture.rs`
- Modify: `tools/tests/common/mod.rs` (one new line, `pub mod fixture;`)
- Test: `tools/tests/docs_check_fixture.rs`

**Interfaces:**
- Consumes (Task 2, `tools/tests/common/mod.rs`): `pub struct TestRepo` with `TestRepo::new(label: &str) -> TestRepo` (fresh directory under `CARGO_TARGET_TMPDIR` plus `git init -q -b main`), `fn write(&self, rel: &str, content: &str)` (creates parent directories), `fn commit(&self, subject: &str)` (`git add -A`, then `git commit -q --allow-empty -m <subject>`), `fn path(&self) -> &std::path::Path`; dropping it removes the directory. (Task 6) `pub struct Run { pub code: i32, pub stdout: Vec<u8>, pub stderr: Vec<u8> }` and `pub fn run_aios(cwd: &std::path::Path, args: &[&str]) -> Run`. (Task 3) `aios_tools::cmd::docs_check::model::CHECK_ORDER: [&str; 15]`. The complete `aios docs-check` binary (Tasks 6-12).
- Produces (module `common::fixture`, extended by Task 14): `pub enum Op { File(String, String), Append(String, String), Prepend(String, String), Delete(String), Commit(String), Flag(String) }`; `pub struct Variant { pub name: &'static str, pub expect_new: &'static [&'static str] }`; `pub const VARIANTS: &[Variant]` (19 entries in this order: `base`, the 15 check names in `CHECK_ORDER`, `line-shift`, `skip`, `grown`); `pub fn fixtures_dir() -> PathBuf`; `pub fn parse_bundle(text: &str) -> Vec<Op>`; `pub fn read_bundle(rel: &str) -> Vec<Op>`; `pub fn materialize_fixture(variant: &str) -> TestRepo`.

**Notes (check.py parity):**
- Bundle format: a line starting with `@@@ ` is a directive `@@@ <kind> <arg>` (`file`, `append`, `prepend`, `delete`, `commit` in the base only, `option no-phase-history` in a variant only); every other line is content of the preceding `file`/`append`/`prepend`. One final `"\n"` of the bundle is stripped, the rest split on `"\n"`; content is the lines joined with `"\n"` plus a final `"\n"`, or `""` for no lines. Content after `delete`, `commit` or `option` is a parse error (panic).
- `materialize_fixture` writes the base files, commits `Initial fixture`, adds one empty commit per base `commit` subject (skipped by `option no-phase-history`), applies the variant's operations and commits `Fixture variant: <name>`. Each variant's expected new keys were verified by materializing it this way and running check.py (`--json`).
- Keys are `check|file|target` without line numbers (check.py L120-149). A key is new when it is missing from the baseline or its occurrence count grew past the baselined `count` (`compare`, L1468-1485); `--json` lists only new findings unless `--all` (L1629-1660); the exit status is 1 exactly when a key is new (L1662).
- `base` has three findings, one resolved baseline entry (`anchors|docs/kernel/alpha.md|#gone`), one reduced entry (`older-spec.md`, count 2 to 1), one accepted false positive (a `reason`), and an entry of a check that no longer exists (`retired-check`), which `--update-baseline` keeps verbatim (L1618-1625).
- `line-shift` prepends three blank lines to `docs/kernel/alpha.md`: nothing is new, and the merged `old-spec.md` finding moves from line 12 (also 16) to line 15 (also 19) (merge by key, L1442-1456).
- `skip` omits the `Phase N MK:` commits. `merged_milestones` then uses the fixture's `main` branch as the history base (L440-462), finds no subjects, and `milestone-status` is skipped with check.py's message (L928-931).
- `grown` adds a third `old-spec.md` link: count 3 against a baselined 2, so the key is new and JSON shows `baseline_count` 2.
- Bundles are `.txt`, never `.md`, so the real repository's docs-check never scans the fixture's Markdown. They must stay byte-exact (UTF-8 `—`, `–`, `§`, `├──`, `│`; LF; no trailing whitespace; `line-shift.txt` ends with three empty lines). The heredocs in Step 3 reproduce them exactly and the `shasum` check proves it.

- [ ] **Step 1: Write the failing test**

Add this line to `tools/tests/common/mod.rs`, directly below its `#![allow(dead_code)]` line:

```rust
pub mod fixture;
```

Create `tools/tests/docs_check_fixture.rs`:

```rust
//! The docs-check fixture repository: every drift variant injects exactly its own new
//! finding, and the base, line-shift, skip and grown variants behave as check.py did.

mod common;

use aios_tools::cmd::docs_check::model::CHECK_ORDER;
use common::fixture::{fixtures_dir, materialize_fixture, parse_bundle, Op, VARIANTS};
use common::{run_aios, TestRepo};
use serde_json::{json, Value};
use std::collections::BTreeSet;
use std::fs;

/// The three findings of the base fixture (`--all`).
const BASE_KEYS: [&str; 3] = [
    "md-links|docs/kernel/alpha.md|old-spec.md",
    "md-links|docs/kernel/beta.md|older-spec.md",
    "pointer-doctor|.claude/agents/worker.md|path:docs/retired/guide.md",
];

/// Run `aios docs-check --json <extra>` in `repo`; return the exit code and the JSON.
fn docs_check_json(repo: &TestRepo, extra: &[&str]) -> (i32, Value) {
    let mut args = vec!["docs-check", "--json"];
    args.extend_from_slice(extra);
    let run = run_aios(repo.path(), &args);
    let json = serde_json::from_slice(&run.stdout).unwrap_or_else(|e| {
        panic!(
            "aios {} printed invalid JSON ({e}); stderr: {}",
            args.join(" "),
            String::from_utf8_lossy(&run.stderr)
        )
    });
    (run.code, json)
}

/// The `key` of every listed finding.
fn finding_keys(json: &Value) -> BTreeSet<String> {
    json["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .map(|f| {
            f["key"]
                .as_str()
                .expect("finding key is a string")
                .to_string()
        })
        .collect()
}

/// The listed finding with this key.
fn finding<'a>(json: &'a Value, key: &str) -> &'a Value {
    json["findings"]
        .as_array()
        .expect("findings is an array")
        .iter()
        .find(|f| f["key"] == key)
        .unwrap_or_else(|| panic!("no finding {key} in {json}"))
}

#[test]
fn bundle_parser_splits_directives_and_content() {
    let ops = parse_bundle(
        "@@@ commit Phase 0 M1: Step 1 — boot\n@@@ file a/b.md\n# B\n\ntext\n@@@ append c.md\n@@@ prepend d.md\n\n\n@@@ delete e.md\n@@@ option no-phase-history\n",
    );
    assert_eq!(
        ops,
        vec![
            Op::Commit("Phase 0 M1: Step 1 — boot".to_string()),
            Op::File("a/b.md".to_string(), "# B\n\ntext\n".to_string()),
            Op::Append("c.md".to_string(), String::new()),
            Op::Prepend("d.md".to_string(), "\n\n".to_string()),
            Op::Delete("e.md".to_string()),
            Op::Flag("no-phase-history".to_string()),
        ]
    );
}

#[test]
#[should_panic(expected = "takes no content lines")]
fn bundle_parser_rejects_content_after_commit() {
    parse_bundle("@@@ commit Phase 0 M1: x\nstray content\n");
}

#[test]
fn variants_cover_every_bundle_in_check_order() {
    let mut files: Vec<String> = fs::read_dir(fixtures_dir().join("variants"))
        .expect("variants directory is readable")
        .map(|entry| {
            let name = entry.expect("readable entry").file_name();
            let name = name.to_str().expect("UTF-8 file name").to_string();
            name.strip_suffix(".txt")
                .unwrap_or_else(|| panic!("variants/{name} is not a .txt bundle"))
                .to_string()
        })
        .collect();
    files.sort();
    let mut names: Vec<&str> = VARIANTS
        .iter()
        .map(|v| v.name)
        .filter(|name| *name != "base")
        .collect();
    names.sort_unstable();
    assert_eq!(files, names);
    let drift: Vec<&str> = VARIANTS[1..16].iter().map(|v| v.name).collect();
    assert_eq!(drift, CHECK_ORDER);
}

#[test]
fn each_variant_reports_exactly_its_new_keys() {
    let mut failures = Vec::new();
    for variant in VARIANTS {
        let repo = materialize_fixture(variant.name);
        let (code, json) = docs_check_json(&repo, &[]);
        let want_code = if variant.expect_new.is_empty() { 0 } else { 1 };
        let want: BTreeSet<String> = variant.expect_new.iter().map(|k| k.to_string()).collect();
        let got = finding_keys(&json);
        if code != want_code || got != want {
            failures.push(format!(
                "{}: exit {code} (want {want_code}), new keys {got:?} (want {want:?})",
                variant.name
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[test]
fn base_lists_three_findings_one_resolved_one_reduced() {
    let repo = materialize_fixture("base");
    let (code, json) = docs_check_json(&repo, &["--all"]);
    assert_eq!(code, 0);
    let want: BTreeSet<String> = BASE_KEYS.iter().map(|k| k.to_string()).collect();
    assert_eq!(finding_keys(&json), want);
    assert_eq!(
        json["resolved"],
        json!(["anchors|docs/kernel/alpha.md|#gone"])
    );
    assert_eq!(
        json["reduced"],
        json!({"md-links|docs/kernel/beta.md|older-spec.md": {"baseline": 2, "current": 1}})
    );
    let accepted = finding(&json, BASE_KEYS[2]);
    assert_eq!(
        accepted["accepted"],
        "false positive: the agent cites the retired guide on purpose"
    );
}

#[test]
fn line_shift_moves_lines_but_keeps_keys() {
    let repo = materialize_fixture("line-shift");
    let (code, json) = docs_check_json(&repo, &["--all"]);
    assert_eq!(code, 0);
    let old = finding(&json, BASE_KEYS[0]);
    assert_eq!(old["line"], 15);
    assert_eq!(old["also"], json!([19]));
    assert_eq!(old["new"], false);
}

#[test]
fn skip_variant_skips_milestone_status() {
    let repo = materialize_fixture("skip");
    let (code, json) = docs_check_json(&repo, &[]);
    assert_eq!(code, 0);
    assert_eq!(
        json["checks"]["milestone-status"],
        json!({"skipped": "no 'Phase N MK:' commits found on main's first-parent history"})
    );
}

#[test]
fn grown_variant_reports_the_extra_occurrence() {
    let repo = materialize_fixture("grown");
    let (code, json) = docs_check_json(&repo, &[]);
    assert_eq!(code, 1);
    let grown = finding(&json, BASE_KEYS[0]);
    assert_eq!(grown["line"], 12);
    assert_eq!(grown["also"], json!([16, 17]));
    assert_eq!(grown["count"], 3);
    assert_eq!(grown["baseline_count"], 2);
    assert_eq!(grown["new"], true);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p aios-tools --test docs_check_fixture`
Expected: FAIL to compile, with ``error[E0583]: file not found for module `fixture` `` pointing at `tools/tests/common/mod.rs`.

- [ ] **Step 3: Create the fixture bundles**

Run from the worktree root. The base bundle:

````bash
mkdir -p tools/tests/fixtures/docs-check/variants
cat > tools/tests/fixtures/docs-check/base.txt <<'AIOS_BUNDLE_EOF'
@@@ commit Phase 0 M1: Step 1 — boot stub
@@@ commit Phase 0 M2: Step 2 — uart console
@@@ commit Phase 1 M3: Step 1 — buddy allocator
@@@ file CLAUDE.md
# Fixture Project

## Workspace Layout

```text
fixture/
├── .claude/
│   ├── agents/           worker
│   └── skills/           build, team:start
├── kernel/src/           fixture kernel
│   ├── sync/             locks
│   └── (top-level)       main.rs
├── shared/src/           shared types
│   └── (top-level)       lib
├── uefi-stub/            boot stub
└── docs/                 architecture, phase, knowledge docs
```

## Key Technical Facts

```text
Lock ordering (full):         ALPHA_LOCK > BETA_LOCK
```

## Architecture Document Map

Topic index lives in `docs/project/doc-map.md`.

## Team

**Agents** (defined in `.claude/agents/`):

| Agent | Role |
| --- | --- |
| `worker` | Does the work |

**Skills** (defined in `.claude/skills/`):

| Skill | Purpose |
| --- | --- |
| `/build` | Builds the fixture |
| `/team:start` | Starts a session |
@@@ file README.md
# Fixture

Status: Phase 1 M3 is complete.

3 phases across 1 tier.

## Build Commands

| Command | Description |
|---|---|
| `just build` | Build |
| `just check` | Check |
| `just test` | Test |
| `just docs-check` | Docs drift |
@@@ file justfile
# Fixture justfile

default: build

build:
    echo build

check: build
    echo check

test:
    echo test

[private]
helper:
    echo helper

_hidden:
    echo hidden

[positional-arguments]
docs-check *args:
    echo docs-check "$@"
@@@ file .claude/agents/worker.md
---
name: worker
description: Fixture agent.
tools: Read, Edit, Bash
---

# Worker

Follow the Workspace Layout in CLAUDE.md and the rules in `.claude/rules/01-code.md`.
The old guide `docs/retired/guide.md` is cited on purpose.
Ask the `worker` agent for help, or run `/build`.
@@@ file .claude/skills/build/SKILL.md
---
name: build
description: Fixture skill that builds.
---

# Build

Run `just build`, then `just check`.
@@@ file .claude/skills/team/.claude-plugin/plugin.json
{"name": "team"}
@@@ file .claude/skills/team/skills/start/SKILL.md
---
name: start
description: Fixture plugin skill.
---

# Start

Read `docs/project/doc-map.md` first.
@@@ file .claude/rules/01-code.md
# Code Conventions

- Keep modules small.
@@@ file .claude/rules/05-file-placement.md
# File Placement Rules

```
kernel/src/sync/               Locks
kernel/src/                    Kernel entry
```
@@@ file docs/project/developer-guide.md
# Developer Guide

## 5. Build

### 5.1 Just Commands

| Command | What it does |
|---|---|
| `just build` | Build |
| `just check` | Check |

### 5.2 Tests

Host test count: <!-- gen:test-count --> 4
@@@ file docs/project/doc-map.md
# Doc Map

| Topic | Doc |
|---|---|
| Kernel | `docs/kernel/{alpha,beta,deadlock-prevention}.md` |
| Kernel hub member | `docs/kernel/beta/detail.md` |
| Plan | `docs/project/development-plan.md` |
| Guide | `docs/project/developer-guide.md` |
@@@ file docs/project/development-plan.md
# Development Plan

## 8. Phase Detail Reference

| Phase | Name | Tier | Weeks | Deliverable | Status |
|---|---|---|---|---|---|
| 0 | Foundation | 1 | 2 | Boots | Complete |
| 1 | Memory | 1 | 2 | Allocates | Complete |
| 2 | Next | 1 | 2 | Something | Planned |

3 phases across 1 tier.

## 8.1 Actual Progress

### Velocity Summary

| Phase | Planned | Actual | Speedup | Milestones |
|---|---|---|---|---|
| 0 (Foundation) | 2 weeks | 1 day | 10x | M1–M2 |
| 1 (Memory) | 2 weeks | 1 day | 10x | M3 |
@@@ file docs/phases/00-foundation.md
# Phase 0: Foundation

**Status:** Complete

## Milestones

| Milestone | Steps |
|---|---|
| **M1 — Boot** | 1 |
| **M2 — UART** | 2 |

## Milestone 1 — Boot

- [x] Write the entry stub
- [x] Print a banner

## Milestone 2 — UART

- [x] Drive the UART
- [ ] ~~Legacy console~~ — deferred to M4
@@@ file docs/phases/01-memory.md
# Phase 1: Memory

**Status:** Complete

## Milestone 3 — Allocator

- [x] Buddy allocator
- [x] Slab allocator
@@@ file docs/kernel/alpha.md
# Alpha

## 1. Overview

Alpha depends on [beta](beta.md) and the [lock rules](deadlock-prevention.md#33-lock-hierarchy).
See [deadlock](deadlock-prevention.md) §3.3 and the [beta detail](beta.md) §2.1.
The notes [[beta]] and [[kernel/deadlock-prevention|locks]] are wiki links.

### 1.2 Details

Jump to [details](#12-details) or [the overview](#1-overview).
The retired spec is [old](old-spec.md).

## 2. History

The retired spec is still [old](old-spec.md) here.
@@@ file docs/kernel/beta.md
# Beta

Beta is a hub; its sections live in [beta/detail.md](beta/detail.md).

## 1. Scope

Beta once linked [older](older-spec.md).
@@@ file docs/kernel/beta/detail.md
# Beta Detail

Part of: [beta.md](../beta.md)

## 2. Detail

### 2.1 Detail

Back to [alpha](../alpha.md).
@@@ file docs/kernel/deadlock-prevention.md
# Deadlock Prevention

## 3. Locks

### 3.3 Lock Hierarchy

| Rank | Lock | Purpose |
|---|---|---|
| 1 | `ALPHA_LOCK` | First |
| 2 | `BETA_LOCK` | Second |

### 3.4 Test Locks

`TEST_CHANNEL` is excluded.

### 3.5 Notes

Nothing else.
@@@ file docs/knowledge/README.md
# Knowledge

Notes live here.
@@@ file docs/knowledge/plans/_template.md
---
author: claude
date: YYYY-MM-DD
tags: []
status: in-progress
---

# Plan: template
@@@ file docs/knowledge/decisions/2026-01-01-ab-example.md
---
author: ab
date: 2026-01-01
tags: [kernel]
status: final
---

# Decision: Example

We chose the example.
@@@ file kernel/src/main.rs
//! Fixture kernel entry.

mod sync;

fn main() {}
@@@ file kernel/src/sync/mod.rs
//! Fixture locks.

use spin::Mutex;

// Lock ordering: ALPHA_LOCK before BETA_LOCK.
// Never take BETA_LOCK first.

pub static ALPHA_LOCK: Mutex<u32> = Mutex::new(0);
pub(crate) static BETA_LOCK: spin::Mutex<()> = spin::Mutex::new(());
static COUNTER: u32 = 0;

#[cfg(test)]
mod tests {
    use super::*;
    static TEST_ONLY_LOCK: Mutex<u8> = Mutex::new(0);
}
@@@ file shared/src/lib.rs
//! Fixture shared crate.

#[cfg(test)]
mod tests {
    #[test]
    fn one() {}

    #[test]
    fn two() {}

    #[test]
    fn three() {}

    #[test]
    fn four() {}
}
@@@ file uefi-stub/src/main.rs
fn main() {}
@@@ file scripts/docs/baseline.json
{
  "comment": "Fixture baseline, hand-ordered so that --update-baseline re-sorts it.",
  "version": 1,
  "counts": {},
  "findings": [
    {
      "key": "retired-check|README.md|legacy",
      "check": "retired-check",
      "file": "README.md",
      "target": "legacy",
      "message": "kept verbatim: this check no longer exists",
      "note": "extra fields survive --update-baseline"
    },
    {
      "key": "pointer-doctor|.claude/agents/worker.md|path:docs/retired/guide.md",
      "check": "pointer-doctor",
      "file": ".claude/agents/worker.md",
      "target": "path:docs/retired/guide.md",
      "message": "path does not exist: docs/retired/guide.md",
      "reason": "false positive: the agent cites the retired guide on purpose"
    },
    {
      "key": "anchors|docs/kernel/alpha.md|#gone",
      "check": "anchors",
      "file": "docs/kernel/alpha.md",
      "target": "#gone",
      "message": "no heading for anchor #gone in docs/kernel/alpha.md"
    },
    {
      "key": "md-links|docs/kernel/beta.md|older-spec.md",
      "check": "md-links",
      "file": "docs/kernel/beta.md",
      "target": "older-spec.md",
      "message": "broken link -> older-spec.md",
      "count": 2
    },
    {
      "key": "md-links|docs/kernel/alpha.md|old-spec.md",
      "check": "md-links",
      "file": "docs/kernel/alpha.md",
      "target": "old-spec.md",
      "message": "broken link -> old-spec.md",
      "count": 2
    }
  ]
}
AIOS_BUNDLE_EOF
````

The 18 variant bundles:

````bash
V=tools/tests/fixtures/docs-check/variants
cat > "$V/anchors.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/kernel/alpha.md
Jump to [nowhere](#no-such-heading).
AIOS_BUNDLE_EOF
cat > "$V/doc-map.txt" <<'AIOS_BUNDLE_EOF'
@@@ file docs/kernel/gamma.md
# Gamma

A new architecture doc that doc-map.md does not list.
AIOS_BUNDLE_EOF
cat > "$V/grown.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/kernel/alpha.md
A third mention of the [old](old-spec.md) spec.
AIOS_BUNDLE_EOF
cat > "$V/harness-tables.txt" <<'AIOS_BUNDLE_EOF'
@@@ append CLAUDE.md
| `/ghost` | Haunts the tables |
AIOS_BUNDLE_EOF
cat > "$V/just-recipes.txt" <<'AIOS_BUNDLE_EOF'
@@@ append README.md

Run `just nope` to do nothing.
AIOS_BUNDLE_EOF
cat > "$V/knowledge-hygiene.txt" <<'AIOS_BUNDLE_EOF'
@@@ file docs/knowledge/lessons/bad-name.md
---
author: ab
date: 2026-01-02
tags: [kernel]
status: final
---

# Lesson with a bad file name
AIOS_BUNDLE_EOF
cat > "$V/layout.txt" <<'AIOS_BUNDLE_EOF'
@@@ file kernel/src/extra.rs
//! Extra top-level module.
AIOS_BUNDLE_EOF
cat > "$V/line-shift.txt" <<'AIOS_BUNDLE_EOF'
@@@ prepend docs/kernel/alpha.md



AIOS_BUNDLE_EOF
cat > "$V/lock-order.txt" <<'AIOS_BUNDLE_EOF'
@@@ append kernel/src/main.rs

pub static GAMMA_LOCK: spin::Mutex<u8> = spin::Mutex::new(0);
AIOS_BUNDLE_EOF
cat > "$V/md-links.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/kernel/alpha.md
A new [missing](missing.md) link.
AIOS_BUNDLE_EOF
cat > "$V/milestone-status.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/phases/01-memory.md
- [ ] Late task
AIOS_BUNDLE_EOF
cat > "$V/phase-count.txt" <<'AIOS_BUNDLE_EOF'
@@@ append README.md

4 phases across 1 tier, eventually.
AIOS_BUNDLE_EOF
cat > "$V/pointer-doctor.txt" <<'AIOS_BUNDLE_EOF'
@@@ append .claude/agents/worker.md
Check the Build Matrix in CLAUDE.md before editing.
AIOS_BUNDLE_EOF
cat > "$V/repo-paths.txt" <<'AIOS_BUNDLE_EOF'
@@@ append CLAUDE.md

The kernel entry point is `kernel/src/missing.rs`.
AIOS_BUNDLE_EOF
cat > "$V/section-refs.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/kernel/alpha.md
See [deadlock](deadlock-prevention.md) §9.9 as well.
AIOS_BUNDLE_EOF
cat > "$V/skip.txt" <<'AIOS_BUNDLE_EOF'
@@@ option no-phase-history
AIOS_BUNDLE_EOF
cat > "$V/test-count.txt" <<'AIOS_BUNDLE_EOF'
@@@ append shared/src/lib.rs

#[test]
fn five() {}
AIOS_BUNDLE_EOF
cat > "$V/wiki-links.txt" <<'AIOS_BUNDLE_EOF'
@@@ append docs/kernel/alpha.md
See [[nonexistent-note]] for more.
AIOS_BUNDLE_EOF
````

Run: `shasum -a 256 tools/tests/fixtures/docs-check/base.txt tools/tests/fixtures/docs-check/variants/*.txt`
Expected, exactly:

```text
91cfab2ec06f81d31e5f2b34853d4749e3c7f1192bd46c0591a0b64a115e0723  tools/tests/fixtures/docs-check/base.txt
570577d4de3fe0f4f6f0c5acbb10b911c10833ced5d7c2a319c79eb318f33478  tools/tests/fixtures/docs-check/variants/anchors.txt
8aee75218b4d9a598a02f0d6aed9a769390116527f3d7d9314cb68e4d36e7a25  tools/tests/fixtures/docs-check/variants/doc-map.txt
2fee4727fd588051e32262a0d13ca33b936a00148d505094323afffd475db9d1  tools/tests/fixtures/docs-check/variants/grown.txt
57427935f651747926106295ddedf2bd59c4b9bb468f99b1aeb8d7d111081c7b  tools/tests/fixtures/docs-check/variants/harness-tables.txt
9a1d6d751f5d74ce4c3fa2038940faabab907f93d5085fc160f62687d58520b0  tools/tests/fixtures/docs-check/variants/just-recipes.txt
787130e796e8a41723a3b8278d80edea8294f1140a8f413cbd0a21d51eaa14a3  tools/tests/fixtures/docs-check/variants/knowledge-hygiene.txt
145708513ea25f3746d264e6758df899d57f5f5821b5ed1762fc1358feb627b5  tools/tests/fixtures/docs-check/variants/layout.txt
42688f6178bc20c146b647567c31a355a4e0874c1793a16f4cf9541c2e01b24f  tools/tests/fixtures/docs-check/variants/line-shift.txt
40afe0eaf99be111a7c5c021598065e8f453241256001774bdf0f525f4150a53  tools/tests/fixtures/docs-check/variants/lock-order.txt
82c99f0ec5882a77fd989feb98636a5997074fd2361ac23b6a31febc93447f2e  tools/tests/fixtures/docs-check/variants/md-links.txt
f2d43eab7896488fb74eca927deaa44e486aded1c2f89403165b6631d2ca84b6  tools/tests/fixtures/docs-check/variants/milestone-status.txt
f987da71733f5975da6392939d802b215f84ba833307eea56cb7cb5a1ec7c498  tools/tests/fixtures/docs-check/variants/phase-count.txt
63053a086ba935aaf3146b4c881150b10d7e12a9cfa4a8d1b31c2a79c3314a49  tools/tests/fixtures/docs-check/variants/pointer-doctor.txt
1a7ec223b1d94217e585087c875964eab5f0053d0e87bd6f84f16ee8f26bd763  tools/tests/fixtures/docs-check/variants/repo-paths.txt
f0d5106996b56e49cb283461f4145f8a517c6e879f7c65b6d97b4e372c13f0b7  tools/tests/fixtures/docs-check/variants/section-refs.txt
d91312a9306da7efa0443ec502a85b6d63fd9a7da2dfa12c5f8c364ba7c0ff71  tools/tests/fixtures/docs-check/variants/skip.txt
14233665f91a58c27cd80beef3bd9669fced3949c4c10b86d4b715554d94f9fb  tools/tests/fixtures/docs-check/variants/test-count.txt
ea8f7911eea39b2557d7d78c1d4c8af333fa95f6694c203df5d0b2df4f488b3d  tools/tests/fixtures/docs-check/variants/wiki-links.txt
```

A different hash means the bundle changed in transit (trailing whitespace, a lost empty line, a replaced dash). Rewrite that file from the heredoc before continuing.

- [ ] **Step 4: Write the fixture module**

Create `tools/tests/common/fixture.rs`:

```rust
//! The docs-check fixture repository (R1 contract, fixture bundle format and variants).
//!
//! The fixture lives in text bundles under `tests/fixtures/docs-check/`: `base.txt`
//! and one `variants/<name>.txt` per variant. A bundle line starting with `@@@ ` is a
//! directive `@@@ <kind> <arg>`; every other line is content of the preceding `file`,
//! `append` or `prepend` directive. Bundles are `.txt` so that the real repository's
//! docs-check, Cargo and Claude Code never see the fixture's Markdown, `CLAUDE.md`,
//! `.claude/` or `.rs` files. `materialize_fixture` turns base plus one variant into a
//! throwaway git repository (removed when the returned `TestRepo` drops).

use super::TestRepo;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// One bundle directive with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `@@@ file <path>`: create or replace the file (parent directories created).
    File(String, String),
    /// `@@@ append <path>`: append to the file (created when missing).
    Append(String, String),
    /// `@@@ prepend <path>`: insert before the file's current content.
    Prepend(String, String),
    /// `@@@ delete <path>`: remove the file.
    Delete(String),
    /// `@@@ commit <subject>` (base only): an empty commit after `Initial fixture`.
    Commit(String),
    /// `@@@ option <name>` (variants only); the only option is `no-phase-history`.
    Flag(String),
}

/// A fixture variant and the finding keys `aios docs-check --json` reports as new.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub name: &'static str,
    pub expect_new: &'static [&'static str],
}

/// Every variant, verified against scripts/docs/check.py: the base repository, one
/// single-drift variant per check (in CHECK_ORDER), then line-shift, skip and grown.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "base",
        expect_new: &[],
    },
    Variant {
        name: "md-links",
        expect_new: &["md-links|docs/kernel/alpha.md|missing.md"],
    },
    Variant {
        name: "section-refs",
        expect_new: &["section-refs|docs/kernel/alpha.md|deadlock-prevention.md §9.9"],
    },
    Variant {
        name: "anchors",
        expect_new: &["anchors|docs/kernel/alpha.md|#no-such-heading"],
    },
    Variant {
        name: "wiki-links",
        expect_new: &["wiki-links|docs/kernel/alpha.md|nonexistent-note"],
    },
    Variant {
        name: "doc-map",
        expect_new: &["doc-map|docs/kernel/gamma.md|unlisted"],
    },
    Variant {
        name: "repo-paths",
        expect_new: &["repo-paths|CLAUDE.md|kernel/src/missing.rs"],
    },
    Variant {
        name: "just-recipes",
        expect_new: &["just-recipes|README.md|nope"],
    },
    Variant {
        name: "test-count",
        expect_new: &["test-count|docs/project/developer-guide.md|claimed:4"],
    },
    Variant {
        name: "lock-order",
        expect_new: &["lock-order|docs/kernel/deadlock-prevention.md|undocumented:GAMMA_LOCK"],
    },
    Variant {
        name: "milestone-status",
        expect_new: &["milestone-status|docs/phases/01-memory.md|M3:unchecked"],
    },
    Variant {
        name: "phase-count",
        expect_new: &["phase-count|README.md|claimed:4"],
    },
    Variant {
        name: "layout",
        expect_new: &["layout|CLAUDE.md|missing:kernel/src/extra.rs"],
    },
    Variant {
        name: "harness-tables",
        expect_new: &["harness-tables|CLAUDE.md|skills-table-stale:ghost"],
    },
    Variant {
        name: "pointer-doctor",
        expect_new: &["pointer-doctor|.claude/agents/worker.md|claude-md:Build Matrix"],
    },
    Variant {
        name: "knowledge-hygiene",
        expect_new: &["knowledge-hygiene|docs/knowledge/lessons/bad-name.md|name"],
    },
    Variant {
        name: "line-shift",
        expect_new: &[],
    },
    Variant {
        name: "skip",
        expect_new: &[],
    },
    Variant {
        name: "grown",
        expect_new: &["md-links|docs/kernel/alpha.md|old-spec.md"],
    },
];

/// `tools/tests/fixtures/docs-check`.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/docs-check")
}

/// Parse a bundle: strip one final `"\n"`, split on `"\n"`; content is the lines joined
/// with `"\n"` plus a final `"\n"`, or `""` when a directive has no content lines.
pub fn parse_bundle(text: &str) -> Vec<Op> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut directives: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("@@@ ") {
            let (kind, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            directives.push((kind, arg, Vec::new()));
        } else if let Some((_, _, content)) = directives.last_mut() {
            content.push(line);
        } else {
            panic!("bundle line before the first @@@ directive: {line:?}");
        }
    }
    directives
        .into_iter()
        .map(|(kind, arg, lines)| {
            let content = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            let arg = arg.to_string();
            match kind {
                "file" => Op::File(arg, content),
                "append" => Op::Append(arg, content),
                "prepend" => Op::Prepend(arg, content),
                "delete" | "commit" | "option" => {
                    assert!(
                        lines.is_empty(),
                        "@@@ {kind} {arg} takes no content lines, found {}",
                        lines.len()
                    );
                    match kind {
                        "delete" => Op::Delete(arg),
                        "commit" => Op::Commit(arg),
                        _ => Op::Flag(arg),
                    }
                }
                other => panic!("unknown bundle directive @@@ {other} {arg}"),
            }
        })
        .collect()
}

/// Parse `tests/fixtures/docs-check/<rel>`.
pub fn read_bundle(rel: &str) -> Vec<Op> {
    let path = fixtures_dir().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_bundle(&text)
}

/// Build the fixture repository for `variant` (`"base"` or a `variants/<name>.txt`):
/// write the base files, commit `Initial fixture`, add one empty commit per base
/// `@@@ commit` subject (unless the variant sets `no-phase-history`), apply the
/// variant's file operations and commit `Fixture variant: <variant>`.
pub fn materialize_fixture(variant: &str) -> TestRepo {
    let base = read_bundle("base.txt");
    let ops = if variant == "base" {
        Vec::new()
    } else {
        read_bundle(&format!("variants/{variant}.txt"))
    };
    let repo = TestRepo::new(&format!("fixture-{variant}"));
    let mut subjects = Vec::new();
    for op in &base {
        match op {
            Op::Commit(subject) => subjects.push(subject.as_str()),
            Op::Flag(name) => panic!("base.txt cannot set @@@ option {name}"),
            _ => apply(&repo, op),
        }
    }
    repo.commit("Initial fixture");
    let mut phase_history = true;
    for op in &ops {
        match op {
            Op::Flag(name) if name == "no-phase-history" => phase_history = false,
            Op::Flag(name) => panic!("variants/{variant}.txt: unknown @@@ option {name}"),
            Op::Commit(subject) => {
                panic!("variants/{variant}.txt: @@@ commit {subject} belongs in base.txt")
            }
            _ => {}
        }
    }
    if phase_history {
        for subject in subjects {
            repo.commit(subject);
        }
    }
    for op in ops.iter().filter(|op| !matches!(op, Op::Flag(_))) {
        apply(&repo, op);
    }
    repo.commit(&format!("Fixture variant: {variant}"));
    repo
}

/// Apply one file operation to the working tree (commits and options are handled by
/// `materialize_fixture`).
fn apply(repo: &TestRepo, op: &Op) {
    match op {
        Op::File(rel, content) => repo.write(rel, content),
        Op::Append(rel, content) => {
            let path = repo.path().join(rel);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
            file.write_all(content.as_bytes())
                .unwrap_or_else(|e| panic!("cannot append to {}: {e}", path.display()));
        }
        Op::Prepend(rel, content) => {
            let path = repo.path().join(rel);
            let old =
                fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let mut new = content.as_bytes().to_vec();
            new.extend_from_slice(&old);
            fs::write(&path, new)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        }
        Op::Delete(rel) => {
            let path = repo.path().join(rel);
            fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("cannot delete {}: {e}", path.display()));
        }
        Op::Commit(subject) => panic!("@@@ commit {subject} is applied by materialize_fixture"),
        Op::Flag(name) => panic!("@@@ option {name} is read by materialize_fixture"),
    }
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test docs_check_fixture`
Expected: PASS, 8 tests, ending with `test result: ok. 8 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out`.

If `each_variant_reports_exactly_its_new_keys` fails, its message lists each failing variant with the exit code and keys aios reported. The bundles and expected keys were verified against check.py, so the defect is in the check module named by the first field of the key (Tasks 7-12); fix it there, not here.

Run: `cargo test -p aios-tools`
Expected: every test binary ends `test result: ok.` (the fixture module now compiles into every test crate that declares `mod common;`; `#![allow(dead_code)]` in `common/mod.rs` covers the items a crate does not use).

- [ ] **Step 6: Format and lint**

Run: `cargo fmt -p aios-tools && cargo fmt --check -p aios-tools && cargo clippy -p aios-tools -- -D warnings`
Expected: `cargo fmt --check` prints nothing; clippy ends with `Finished` and no warning.

- [ ] **Step 7: Commit and push**

```bash
git add tools/tests/fixtures/docs-check tools/tests/common/fixture.rs tools/tests/common/mod.rs tools/tests/docs_check_fixture.rs
git commit -F - <<'EOF'
Add the docs-check fixture repository and drift-injection tests

The fixture is stored as text bundles (base plus 18 variants) that the
tests materialize into throwaway git repositories: one injected drift
per check, a pure line shift, a skipped check and a grown occurrence
count. Each variant's new keys were verified against
scripts/docs/check.py.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

-----

### Task 14: Goldens from check.py; parity and differential tests

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Modify: `tools/tests/common/fixture.rs` (whole file given in Step 1: Task 13's content plus the parity half)
- Create: `tools/tests/docs_check_parity.rs`
- Create (written by the ignored recorder test, never by hand): 74 files under `tools/tests/golden/docs-check/`: `real/<case>.golden` (22), `real/update-baseline.baseline.json`, `fixture/<variant>/<case>.golden` (48), `fixture/base/update-baseline.baseline.json`, `fixture/base/baseline-elsewhere-update.baseline.json`, `fixture/skip/update-baseline.baseline.json`

**Interfaces:**
- Consumes: Task 13's `common::fixture` items (unchanged). Task 2's `common::git(dir: &Path, args: &[&str]) -> String` (isolated, panics with stderr on failure), `common::isolated(cmd: &mut Command) -> &mut Command`, `common::unique_dir(label: &str) -> PathBuf`, `TestRepo::adopt(root: PathBuf) -> TestRepo`, `TestRepo::path_str(&self) -> &str`. Task 6's `Run` and `run_aios`. Task 3's `CHECK_ORDER`.
- Produces (module `common::fixture`): `pub const SNAPSHOT_SHA: &str = "33c6b3deabb36055d26d57fb2a60db233c4d3f6f"`; `pub fn repo_root() -> PathBuf`; `pub fn golden_root() -> PathBuf`; `#[derive(Clone, Copy, Debug, PartialEq, Eq)] pub enum Source { Real, Fixture(&'static str) }` with `pub fn key(self) -> String` (`real` or `fixture/<variant>`); `#[derive(Clone, Debug)] pub struct Case { pub source: Source, pub name: String, pub args: Vec<String>, pub cwd: Option<&'static str>, pub baseline_rel: &'static str, pub writes_baseline: bool }` with `label(&self) -> String`, `golden_path(&self) -> PathBuf`, `baseline_golden_path(&self) -> PathBuf`, `run_dir(&self, repo: &Path) -> PathBuf`, `flag_args(&self) -> Vec<&str>`, `aios_args(&self) -> Vec<&str>`; `pub fn cases() -> Vec<Case>` (70 cases); `pub fn snapshot_real() -> TestRepo`; `pub fn materialize(source: Source) -> TestRepo`; `pub fn materialize_with_check_py(source: Source) -> TestRepo`; `pub fn check_py_source() -> Option<PathBuf>`; `pub fn python3_available() -> bool`; `pub fn run_check_py(root: &Path, cwd: &Path, args: &[&str]) -> Run`. Tests in `tools/tests/docs_check_parity.rs`: `goldens_match_aios`, `record_goldens_from_check_py` (`#[ignore]`), `differential_against_check_py`.

**Notes (check.py parity):**
- A golden is `exit <code>\n` followed by the exact stdout bytes; a `.baseline.json` golden is the bytes of `scripts/docs/baseline.json` after an `--update-baseline` run (check.py writes `baseline_path` and prints `docs-check: wrote N findings to ...`, L1618-1625, `write_baseline` L1424-1439). Stderr is not recorded: argparse's usage text (L1585-1606) and aios's clap/anyhow text differ by design, and `unknown-check` pins exit 2 with empty stdout.
- check.py finds the repository from its own directory (`repo_root`, L1576-1582). So real cases run the snapshot's own tracked `scripts/docs/check.py`, and fixture cases get this repository's copy placed at `scripts/docs/check.py` and listed in `.git/info/exclude`, which keeps it out of `git ls-files --cached --others --exclude-standard` (L372-380); both tools see the same file list.
- `snapshot_real` deletes every ref, so `merged_milestones` (L440-462) finds neither `origin/main` nor `main` and uses `HEAD` = `SNAPSHOT_SHA`, whatever branches the source repository has. Verified while writing this plan: clone, checkout and ref deletion from the worktree take about 0.2 s; check.py's `--all` output on the snapshot equals the worktree's at `33c6b3d` byte for byte; `--update-baseline` on it rewrites `scripts/docs/baseline.json` to identical bytes.
- `--update-baseline` cases get a fresh materialization per tool; read-only cases share one materialization per source. Cases run on one worker thread per CPU, since each spawns a process and the 22 real-snapshot cases scan the whole repository.
- The golden set was produced while writing this plan by an equivalent Python recorder (same materialization, check.py at `33c6b3d`) for the 67 cases that existed then; the three `--baseline` cases were added after that run, so Step 5 verifies the shape of the recorded set and the individual files whose bytes are known, not an aggregate digest.
- The three `--baseline` cases are the recorded coverage of `run_with`'s baseline path handling: `baseline-alias` pins normalisation of a path with a `..` segment, `baseline-subdir` runs from `docs/project/` so the displayed path comes from `relpath` against the root while the file is opened against the working directory (and is missing, so every baselined finding becomes new), and `baseline-elsewhere-update` writes a baseline that is not `scripts/docs/baseline.json`. All three use directories the fixture base already has, so no fixture bundle changes.

- [ ] **Step 1: Extend the fixture module**

Replace the whole of `tools/tests/common/fixture.rs` with:

```rust
//! The docs-check fixture repository (R1 contract, fixture bundle format and variants).
//!
//! The fixture lives in text bundles under `tests/fixtures/docs-check/`: `base.txt`
//! and one `variants/<name>.txt` per variant. A bundle line starting with `@@@ ` is a
//! directive `@@@ <kind> <arg>`; every other line is content of the preceding `file`,
//! `append` or `prepend` directive. Bundles are `.txt` so that the real repository's
//! docs-check, Cargo and Claude Code never see the fixture's Markdown, `CLAUDE.md`,
//! `.claude/` or `.rs` files. `materialize_fixture` turns base plus one variant into a
//! throwaway git repository (removed when the returned `TestRepo` drops).
//!
//! The parity half adds the real-repository snapshot at `SNAPSHOT_SHA`, the case list
//! replayed by `tests/docs_check_parity.rs`, the golden file paths, and helpers that
//! run scripts/docs/check.py while it still exists.

use super::{git, isolated, unique_dir, Run, TestRepo};
use aios_tools::cmd::docs_check::model::CHECK_ORDER;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One bundle directive with its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    /// `@@@ file <path>`: create or replace the file (parent directories created).
    File(String, String),
    /// `@@@ append <path>`: append to the file (created when missing).
    Append(String, String),
    /// `@@@ prepend <path>`: insert before the file's current content.
    Prepend(String, String),
    /// `@@@ delete <path>`: remove the file.
    Delete(String),
    /// `@@@ commit <subject>` (base only): an empty commit after `Initial fixture`.
    Commit(String),
    /// `@@@ option <name>` (variants only); the only option is `no-phase-history`.
    Flag(String),
}

/// A fixture variant and the finding keys `aios docs-check --json` reports as new.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    pub name: &'static str,
    pub expect_new: &'static [&'static str],
}

/// Every variant, verified against scripts/docs/check.py: the base repository, one
/// single-drift variant per check (in CHECK_ORDER), then line-shift, skip and grown.
pub const VARIANTS: &[Variant] = &[
    Variant {
        name: "base",
        expect_new: &[],
    },
    Variant {
        name: "md-links",
        expect_new: &["md-links|docs/kernel/alpha.md|missing.md"],
    },
    Variant {
        name: "section-refs",
        expect_new: &["section-refs|docs/kernel/alpha.md|deadlock-prevention.md §9.9"],
    },
    Variant {
        name: "anchors",
        expect_new: &["anchors|docs/kernel/alpha.md|#no-such-heading"],
    },
    Variant {
        name: "wiki-links",
        expect_new: &["wiki-links|docs/kernel/alpha.md|nonexistent-note"],
    },
    Variant {
        name: "doc-map",
        expect_new: &["doc-map|docs/kernel/gamma.md|unlisted"],
    },
    Variant {
        name: "repo-paths",
        expect_new: &["repo-paths|CLAUDE.md|kernel/src/missing.rs"],
    },
    Variant {
        name: "just-recipes",
        expect_new: &["just-recipes|README.md|nope"],
    },
    Variant {
        name: "test-count",
        expect_new: &["test-count|docs/project/developer-guide.md|claimed:4"],
    },
    Variant {
        name: "lock-order",
        expect_new: &["lock-order|docs/kernel/deadlock-prevention.md|undocumented:GAMMA_LOCK"],
    },
    Variant {
        name: "milestone-status",
        expect_new: &["milestone-status|docs/phases/01-memory.md|M3:unchecked"],
    },
    Variant {
        name: "phase-count",
        expect_new: &["phase-count|README.md|claimed:4"],
    },
    Variant {
        name: "layout",
        expect_new: &["layout|CLAUDE.md|missing:kernel/src/extra.rs"],
    },
    Variant {
        name: "harness-tables",
        expect_new: &["harness-tables|CLAUDE.md|skills-table-stale:ghost"],
    },
    Variant {
        name: "pointer-doctor",
        expect_new: &["pointer-doctor|.claude/agents/worker.md|claude-md:Build Matrix"],
    },
    Variant {
        name: "knowledge-hygiene",
        expect_new: &["knowledge-hygiene|docs/knowledge/lessons/bad-name.md|name"],
    },
    Variant {
        name: "line-shift",
        expect_new: &[],
    },
    Variant {
        name: "skip",
        expect_new: &[],
    },
    Variant {
        name: "grown",
        expect_new: &["md-links|docs/kernel/alpha.md|old-spec.md"],
    },
];

/// `tools/tests/fixtures/docs-check`.
pub fn fixtures_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/docs-check")
}

/// Parse a bundle: strip one final `"\n"`, split on `"\n"`; content is the lines joined
/// with `"\n"` plus a final `"\n"`, or `""` when a directive has no content lines.
pub fn parse_bundle(text: &str) -> Vec<Op> {
    let body = text.strip_suffix('\n').unwrap_or(text);
    let mut directives: Vec<(&str, &str, Vec<&str>)> = Vec::new();
    for line in body.split('\n') {
        if let Some(rest) = line.strip_prefix("@@@ ") {
            let (kind, arg) = rest.split_once(' ').unwrap_or((rest, ""));
            directives.push((kind, arg, Vec::new()));
        } else if let Some((_, _, content)) = directives.last_mut() {
            content.push(line);
        } else {
            panic!("bundle line before the first @@@ directive: {line:?}");
        }
    }
    directives
        .into_iter()
        .map(|(kind, arg, lines)| {
            let content = if lines.is_empty() {
                String::new()
            } else {
                format!("{}\n", lines.join("\n"))
            };
            let arg = arg.to_string();
            match kind {
                "file" => Op::File(arg, content),
                "append" => Op::Append(arg, content),
                "prepend" => Op::Prepend(arg, content),
                "delete" | "commit" | "option" => {
                    assert!(
                        lines.is_empty(),
                        "@@@ {kind} {arg} takes no content lines, found {}",
                        lines.len()
                    );
                    match kind {
                        "delete" => Op::Delete(arg),
                        "commit" => Op::Commit(arg),
                        _ => Op::Flag(arg),
                    }
                }
                other => panic!("unknown bundle directive @@@ {other} {arg}"),
            }
        })
        .collect()
}

/// Parse `tests/fixtures/docs-check/<rel>`.
pub fn read_bundle(rel: &str) -> Vec<Op> {
    let path = fixtures_dir().join(rel);
    let text =
        fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
    parse_bundle(&text)
}

/// Build the fixture repository for `variant` (`"base"` or a `variants/<name>.txt`):
/// write the base files, commit `Initial fixture`, add one empty commit per base
/// `@@@ commit` subject (unless the variant sets `no-phase-history`), apply the
/// variant's file operations and commit `Fixture variant: <variant>`.
pub fn materialize_fixture(variant: &str) -> TestRepo {
    let base = read_bundle("base.txt");
    let ops = if variant == "base" {
        Vec::new()
    } else {
        read_bundle(&format!("variants/{variant}.txt"))
    };
    let repo = TestRepo::new(&format!("fixture-{variant}"));
    let mut subjects = Vec::new();
    for op in &base {
        match op {
            Op::Commit(subject) => subjects.push(subject.as_str()),
            Op::Flag(name) => panic!("base.txt cannot set @@@ option {name}"),
            _ => apply(&repo, op),
        }
    }
    repo.commit("Initial fixture");
    let mut phase_history = true;
    for op in &ops {
        match op {
            Op::Flag(name) if name == "no-phase-history" => phase_history = false,
            Op::Flag(name) => panic!("variants/{variant}.txt: unknown @@@ option {name}"),
            Op::Commit(subject) => {
                panic!("variants/{variant}.txt: @@@ commit {subject} belongs in base.txt")
            }
            _ => {}
        }
    }
    if phase_history {
        for subject in subjects {
            repo.commit(subject);
        }
    }
    for op in ops.iter().filter(|op| !matches!(op, Op::Flag(_))) {
        apply(&repo, op);
    }
    repo.commit(&format!("Fixture variant: {variant}"));
    repo
}

/// Apply one file operation to the working tree (commits and options are handled by
/// `materialize_fixture`).
fn apply(repo: &TestRepo, op: &Op) {
    match op {
        Op::File(rel, content) => repo.write(rel, content),
        Op::Append(rel, content) => {
            let path = repo.path().join(rel);
            let mut file = fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(&path)
                .unwrap_or_else(|e| panic!("cannot open {}: {e}", path.display()));
            file.write_all(content.as_bytes())
                .unwrap_or_else(|e| panic!("cannot append to {}: {e}", path.display()));
        }
        Op::Prepend(rel, content) => {
            let path = repo.path().join(rel);
            let old =
                fs::read(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()));
            let mut new = content.as_bytes().to_vec();
            new.extend_from_slice(&old);
            fs::write(&path, new)
                .unwrap_or_else(|e| panic!("cannot write {}: {e}", path.display()));
        }
        Op::Delete(rel) => {
            let path = repo.path().join(rel);
            fs::remove_file(&path)
                .unwrap_or_else(|e| panic!("cannot delete {}: {e}", path.display()));
        }
        Op::Commit(subject) => panic!("@@@ commit {subject} is applied by materialize_fixture"),
        Op::Flag(name) => panic!("@@@ option {name} is read by materialize_fixture"),
    }
}

/// main at the branch point of PR R1. Its scripts/docs/check.py and docs produced the
/// real-repository goldens; the snapshot replays exactly this commit.
pub const SNAPSHOT_SHA: &str = "33c6b3deabb36055d26d57fb2a60db233c4d3f6f";

/// The repository that contains `tools/` (the checkout or worktree under test).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("tools/ lives in the repository root")
        .to_path_buf()
}

/// `tools/tests/golden/docs-check`.
pub fn golden_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/docs-check")
}

/// Where a case's input repository comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A snapshot of this repository at `SNAPSHOT_SHA`.
    Real,
    /// `materialize_fixture(<variant>)`.
    Fixture(&'static str),
}

impl Source {
    /// `real` or `fixture/<variant>`: the golden subdirectory and the sharing key.
    pub fn key(self) -> String {
        match self {
            Source::Real => "real".to_string(),
            Source::Fixture(variant) => format!("fixture/{variant}"),
        }
    }
}

/// One golden case: `aios docs-check <args>` (or `check.py <args>`) in a fresh copy of
/// `source`.
#[derive(Clone, Debug)]
pub struct Case {
    pub source: Source,
    pub name: String,
    /// docs-check flags only; `docs-check` itself is prepended for aios.
    pub args: Vec<String>,
    /// Where the tool runs, relative to the repository root; `None` is the root. Both
    /// tools find the same repository from anywhere inside it, but `--baseline` is
    /// resolved against this directory (check.py L1611-1613).
    pub cwd: Option<&'static str>,
    /// The baseline this case writes, relative to the repository root. Only read when
    /// `writes_baseline`.
    pub baseline_rel: &'static str,
    /// `--update-baseline`: the written baseline is a golden too.
    pub writes_baseline: bool,
}

impl Case {
    /// `real/<name>` or `fixture/<variant>/<name>`.
    pub fn label(&self) -> String {
        format!("{}/{}", self.source.key(), self.name)
    }

    /// `exit <code>\n` followed by the exact stdout bytes.
    pub fn golden_path(&self) -> PathBuf {
        golden_root().join(format!("{}.golden", self.label()))
    }

    /// The bytes of `baseline_rel` after a `writes_baseline` case.
    pub fn baseline_golden_path(&self) -> PathBuf {
        golden_root().join(format!("{}.baseline.json", self.label()))
    }

    /// The working directory this case runs in, inside the materialized `repo`.
    pub fn run_dir(&self, repo: &Path) -> PathBuf {
        match self.cwd {
            Some(rel) => repo.join(rel),
            None => repo.to_path_buf(),
        }
    }

    /// The flags, for check.py.
    pub fn flag_args(&self) -> Vec<&str> {
        self.args.iter().map(String::as_str).collect()
    }

    /// `docs-check` plus the flags, for aios.
    pub fn aios_args(&self) -> Vec<&str> {
        std::iter::once("docs-check")
            .chain(self.args.iter().map(String::as_str))
            .collect()
    }
}

fn case(source: Source, name: &str, args: &[&str], writes_baseline: bool) -> Case {
    Case {
        source,
        name: name.to_string(),
        args: args.iter().map(|arg| arg.to_string()).collect(),
        cwd: None,
        baseline_rel: "scripts/docs/baseline.json",
        writes_baseline,
    }
}

/// `case`, run from `cwd` (relative to the repository root) and writing `baseline_rel`
/// instead of the default baseline.
fn case_at(
    source: Source,
    name: &str,
    args: &[&str],
    cwd: Option<&'static str>,
    baseline_rel: &'static str,
    writes_baseline: bool,
) -> Case {
    Case {
        cwd,
        baseline_rel,
        ..case(source, name, args, writes_baseline)
    }
}

/// Every golden case: 22 on the real snapshot and 48 on the fixture (70 in all, four
/// of which also write a baseline).
pub fn cases() -> Vec<Case> {
    let real = Source::Real;
    let mut out = vec![
        case(real, "default", &[], false),
        case(real, "all", &["--all"], false),
        case(real, "json", &["--json"], false),
        case(real, "json-all", &["--json", "--all"], false),
        case(real, "markdown", &["--markdown"], false),
        case(real, "list-checks", &["--list-checks"], false),
    ];
    for name in CHECK_ORDER {
        out.push(case(
            real,
            &format!("check-{name}"),
            &["--check", name],
            false,
        ));
    }
    out.push(case(real, "update-baseline", &["--update-baseline"], true));

    let base = Source::Fixture("base");
    out.extend([
        case(base, "default", &[], false),
        case(base, "all", &["--all"], false),
        case(base, "json-all", &["--json", "--all"], false),
        case(base, "markdown", &["--markdown"], false),
        case(base, "update-baseline", &["--update-baseline"], true),
        case(
            base,
            "check-subset",
            &["--check", "anchors,md-links"],
            false,
        ),
        case(base, "unknown-check", &["--check", "bogus"], false),
    ]);
    for name in CHECK_ORDER {
        let drift = Source::Fixture(name);
        out.push(case(drift, "default", &[], false));
        out.push(case(drift, "json", &["--json"], false));
    }
    let shift = Source::Fixture("line-shift");
    out.push(case(shift, "default", &[], false));
    out.push(case(shift, "all", &["--all"], false));
    let skip = Source::Fixture("skip");
    out.push(case(skip, "default", &[], false));
    out.push(case(skip, "markdown", &["--markdown"], false));
    out.push(case(skip, "json", &["--json"], false));
    out.push(case(skip, "update-baseline", &["--update-baseline"], true));
    let grown = Source::Fixture("grown");
    out.push(case(grown, "default", &[], false));
    out.push(case(grown, "markdown", &["--markdown"], false));
    // --baseline: the only place `run_with` reimplements CPython path semantics
    // (`os.path.relpath(baseline_path, root)` for the displayed path, the process
    // directory for the file that is opened and written; check.py L1611-1613,
    // L1618-1625). `docs/` and `docs/project/` exist in the fixture base, so no
    // fixture file changes for these.
    out.push(case(
        base,
        "baseline-alias",
        &["--baseline", "docs/../scripts/docs/baseline.json"],
        false,
    ));
    out.push(case_at(
        base,
        "baseline-subdir",
        &["--baseline", "../../scripts/docs/missing.json"],
        Some("docs/project"),
        "scripts/docs/baseline.json",
        false,
    ));
    out.push(case_at(
        base,
        "baseline-elsewhere-update",
        &[
            "--baseline",
            "scripts/docs/other-baseline.json",
            "--update-baseline",
        ],
        None,
        "scripts/docs/other-baseline.json",
        true,
    ));
    out
}

/// A clone of this repository at `SNAPSHOT_SHA` with every ref deleted, so that
/// docs-check's history base (merge-base with origin/main or main) falls back to HEAD.
pub fn snapshot_real() -> TestRepo {
    let source = repo_root();
    let commit = format!("{SNAPSHOT_SHA}^{{commit}}");
    let present = isolated(Command::new("git").arg("-C").arg(&source).args([
        "cat-file",
        "-e",
        commit.as_str(),
    ]))
    .output()
    .is_ok_and(|out| out.status.success());
    assert!(
        present,
        "real-repository goldens need full history containing {SNAPSHOT_SHA} (CI: fetch-depth: 0)"
    );
    let repo = TestRepo::adopt(unique_dir("real"));
    let source_str = source.to_str().expect("repository path is UTF-8");
    git(
        &source,
        &[
            "clone",
            "-q",
            "--shared",
            "--no-checkout",
            source_str,
            repo.path_str(),
        ],
    );
    git(repo.path(), &["checkout", "-q", "--detach", SNAPSHOT_SHA]);
    // A clone does not inherit the source repository's config (see `TestRepo::new`).
    git(repo.path(), &["config", "core.excludesFile", "/dev/null"]);
    let deletions = git(repo.path(), &["for-each-ref", "--format=delete %(refname)"]);
    let mut child = isolated(Command::new("git").arg("-C").arg(repo.path()).args([
        "update-ref",
        "--no-deref",
        "--stdin",
    ]))
    .stdin(Stdio::piped())
    .spawn()
    .expect("git update-ref starts");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(deletions.as_bytes())
        .expect("write the ref deletions");
    let status = child.wait().expect("git update-ref finishes");
    assert!(
        status.success(),
        "git update-ref --stdin failed in {}",
        repo.path_str()
    );
    assert_eq!(
        git(repo.path(), &["for-each-ref"]),
        "",
        "the snapshot keeps no refs"
    );
    repo
}

/// A fresh input repository for `source`.
pub fn materialize(source: Source) -> TestRepo {
    match source {
        Source::Real => snapshot_real(),
        Source::Fixture(variant) => materialize_fixture(variant),
    }
}

/// `materialize`, plus (for fixtures) this repository's scripts/docs/check.py copied to
/// `scripts/docs/check.py` and listed in `.git/info/exclude`, so that
/// `git ls-files --others --exclude-standard` never sees it. The real snapshot tracks
/// its own check.py.
pub fn materialize_with_check_py(source: Source) -> TestRepo {
    let repo = materialize(source);
    if let Source::Fixture(_) = source {
        let script = check_py_source().expect("scripts/docs/check.py exists in this repository");
        let dest = repo.path().join("scripts/docs/check.py");
        fs::create_dir_all(dest.parent().expect("scripts/docs has a parent"))
            .expect("create scripts/docs");
        fs::copy(&script, &dest).unwrap_or_else(|e| panic!("cannot copy check.py: {e}"));
        let info = repo.path().join(".git/info");
        fs::create_dir_all(&info).expect("create .git/info");
        let mut exclude = fs::OpenOptions::new()
            .append(true)
            .create(true)
            .open(info.join("exclude"))
            .expect("open .git/info/exclude");
        exclude
            .write_all(b"\nscripts/docs/check.py\n")
            .expect("exclude check.py");
    }
    repo
}

/// This repository's scripts/docs/check.py, while it exists (it is deleted by the
/// switch-over; the goldens keep parity afterwards).
pub fn check_py_source() -> Option<PathBuf> {
    let path = repo_root().join("scripts/docs/check.py");
    path.is_file().then_some(path)
}

/// `python3 --version` succeeds.
pub fn python3_available() -> bool {
    isolated(Command::new("python3").arg("--version"))
        .output()
        .is_ok_and(|out| out.status.success())
}

/// `python3 <root>/scripts/docs/check.py <args>` with `cwd` as the working directory
/// (isolated environment). check.py finds its repository from the script's own
/// directory, so `root` must contain the script; `cwd` only changes the paths check.py
/// resolves itself, such as a relative `--baseline`.
pub fn run_check_py(root: &Path, cwd: &Path, args: &[&str]) -> Run {
    let out = isolated(
        Command::new("python3")
            .arg(root.join("scripts/docs/check.py"))
            .args(args)
            .current_dir(cwd),
    )
    .output()
    .unwrap_or_else(|e| panic!("cannot run python3 in {}: {e}", cwd.display()));
    Run {
        code: out.status.code().unwrap_or(-1),
        stdout: out.stdout,
        stderr: out.stderr,
    }
}
```

- [ ] **Step 2: Write the parity tests**

Create `tools/tests/docs_check_parity.rs`:

```rust
//! docs-check parity with scripts/docs/check.py (R1 contract, goldens and parity tests).
//!
//! - `goldens_match_aios` replays every case of `fixture::cases()` (22 on a snapshot of
//!   main at `SNAPSHOT_SHA`, 48 on the fixture repository) and compares `exit N\n` plus
//!   stdout, and the written baseline, byte for byte with `tests/golden/docs-check/`.
//!   `AIOS_BLESS_GOLDENS=1` rewrites the goldens from aios instead (for an intentional
//!   output change; review the diff).
//! - `record_goldens_from_check_py` (ignored) recorded those goldens from check.py
//!   before the switch-over deleted it.
//! - `differential_against_check_py` runs check.py and aios side by side on every case
//!   and on the live checkout while check.py exists, and returns early once it is gone.

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

fn materialize(source: Source, with_check_py: bool) -> TestRepo {
    if with_check_py {
        fixture::materialize_with_check_py(source)
    } else {
        fixture::materialize(source)
    }
}

/// One read-only materialization per source, shared by every case that does not
/// write the baseline; the directories are removed when this drops.
struct Shared {
    repos: HashMap<String, TestRepo>,
}

impl Shared {
    fn new(cases: &[Case], with_check_py: bool) -> Shared {
        let mut seen = BTreeSet::new();
        let sources: Vec<Source> = cases
            .iter()
            .filter(|case| !case.writes_baseline)
            .map(|case| case.source)
            .filter(|source| seen.insert(source.key()))
            .collect();
        let repos = parallel_map(&sources, |source| materialize(*source, with_check_py));
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
fn run_cases(cases: &[Case], tool: Tool, with_check_py: bool) -> Vec<Outcome> {
    let shared = Shared::new(cases, with_check_py);
    parallel_map(cases, |case| {
        if case.writes_baseline {
            let fresh = materialize(case.source, with_check_py);
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
    let outcomes = run_cases(&cases, Tool::Aios, false);
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
#[ignore = "records the goldens from scripts/docs/check.py; run once, before check.py is deleted"]
fn record_goldens_from_check_py() {
    assert!(
        fixture::python3_available(),
        "python3 is required to record the goldens"
    );
    assert!(
        fixture::check_py_source().is_some(),
        "scripts/docs/check.py is required to record the goldens"
    );
    let cases = fixture::cases();
    let outcomes = run_cases(&cases, Tool::CheckPy, true);
    write_goldens(&cases, &outcomes);
}

#[test]
fn differential_against_check_py() {
    if !fixture::python3_available() {
        eprintln!("differential_against_check_py: skipped, python3 is not available");
        return;
    }
    if fixture::check_py_source().is_none() {
        eprintln!("differential_against_check_py: skipped, scripts/docs/check.py is gone (the goldens carry parity)");
        return;
    }
    let cases = fixture::cases();
    let shared = Shared::new(&cases, true);
    let mut failures: Vec<String> = parallel_map(&cases, |case| {
        let (want, got) = if case.writes_baseline {
            let for_python = fixture::materialize_with_check_py(case.source);
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
    assert!(
        failures.is_empty(),
        "{} difference(s) between check.py and aios:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
```

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p aios-tools --test docs_check_parity`
Expected: FAIL. `goldens_match_aios` panics with `70 golden mismatch(es):` followed by one `cannot read golden` line per case (no golden exists yet); `differential_against_check_py` passes; `record_goldens_from_check_py` is ignored; the summary is `test result: FAILED. 1 passed; 1 failed; 1 ignored; 0 measured; 0 filtered out`.

If `differential_against_check_py` fails too, each line names a case (for example `real/check-lock-order`, `fixture/base/markdown` or `live/json-all`) and the first line where aios differs from check.py. That is a parity bug in the port: fix it in the module that owns the output (the check module for a finding, `output.rs` for layout, `model.rs` for the baseline) and re-run until the differential passes. Do not record goldens while it fails.

- [ ] **Step 4: Record the goldens from check.py**

Run: `cargo test -p aios-tools --test docs_check_parity -- --ignored --exact record_goldens_from_check_py`
Expected: PASS, `test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 2 filtered out`.

- [ ] **Step 5: Verify the recorded goldens**

```bash
git status --short -uall tools/tests/golden | wc -l
(cd tools/tests/golden/docs-check && find . -type f | LC_ALL=C sort | wc -l)
find tools/tests/golden/docs-check -type f -empty | wc -l
git show 33c6b3d:scripts/docs/baseline.json | cmp - tools/tests/golden/docs-check/real/update-baseline.baseline.json && echo real-baseline-identical
shasum -a 256 tools/tests/golden/docs-check/fixture/base/update-baseline.baseline.json tools/tests/golden/docs-check/fixture/skip/update-baseline.baseline.json
cat tools/tests/golden/docs-check/fixture/skip/update-baseline.golden
cat tools/tests/golden/docs-check/fixture/grown/default.golden
```

Expected: `74` (70 case goldens and 4 written baselines); then `74` again; then `0` (no empty golden); then `real-baseline-identical`; then both baselines hash to `9ee36f78791ab48fddf35465d09cb978cbf4379dc81db0403c72b1255431672d`; then

```text
exit 0
docs-check: wrote 4 findings to scripts/docs/baseline.json (skipped: milestone-status)
```

and

```text
exit 1
docs-check: 3 findings across 15 checks - 1 new, 2 baselined (1 accepted false positives), 1 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links               2     1
  section-refs           0     0
  anchors                0     0
  wiki-links             0     0
  doc-map                0     0
  repo-paths             0     0
  just-recipes           0     0
  test-count             0     0
  lock-order             0     0
  milestone-status       0     0
  phase-count            0     0
  layout                 0     0
  harness-tables         0     0
  pointer-doctor         1     0
  knowledge-hygiene      0     0

New drift since baseline:

[md-links]
 + docs/kernel/alpha.md:12 (also 16, 17): broken link -> old-spec.md [3 occurrences, baseline 2]

2 baselined finding(s) no longer occur or occur less often; prune them with `just docs-check --update-baseline`:
  - anchors|docs/kernel/alpha.md|#gone
  - md-links|docs/kernel/beta.md|older-spec.md (2 -> 1 occurrences)
```

If any of these differ, run `cargo test -p aios-tools --test docs_check_parity -- --exact differential_against_check_py` first: it compares both tools live on every case, so a green differential means the goldens are right and the expectation here is stale (a renamed, added or removed case — fix the counts), while a red one names the diverging case, which is a parity bug in the port. Never edit a golden by hand; re-record it.

- [ ] **Step 6: Run the tests to verify they pass**

Run: `cargo test -p aios-tools --test docs_check_parity`
Expected: PASS, `test result: ok. 2 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out`.

Run: `cargo test -p aios-tools`
Expected: every test binary ends `test result: ok.`

- [ ] **Step 7: Format and lint**

Run: `cargo fmt -p aios-tools && cargo fmt --check -p aios-tools && cargo clippy -p aios-tools -- -D warnings`
Expected: `cargo fmt --check` prints nothing; clippy ends with `Finished` and no warning.

- [ ] **Step 8: Commit, push and watch CI**

```bash
git add tools/tests/common/fixture.rs tools/tests/docs_check_parity.rs tools/tests/golden/docs-check
git commit -F - <<'EOF'
Record docs-check goldens from check.py and add parity tests

70 cases (22 on a snapshot of main at 33c6b3d, 48 on the fixture
repository) plus 4 written baselines, recorded from
scripts/docs/check.py by the ignored recorder test. goldens_match_aios
replays them against aios byte for byte; differential_against_check_py
compares both tools live while check.py still exists.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
run_id=$(gh run list --branch claude/tools-crate-docs-check --commit "$(git rev-parse HEAD)" --workflow CI --limit 1 --json databaseId --jq '.[0].databaseId')
gh run watch "$run_id" --exit-status
```

If `run_id` comes back empty, GitHub has not registered the run yet: repeat those two commands after a few seconds.
Expected: the run succeeds, including the `Tools (host)` job (its checkout has `fetch-depth: 0`, so `snapshot_real` finds `33c6b3d`, and its runner has python3, so the differential runs too).

-----

### Task 15: Switch-over and documentation

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Modify: `justfile` (the docs section at the end)
- Modify: `.github/workflows/docs.yml` (whole file)
- Modify: `scripts/agent/brief.sh`
- Modify: `.claude/skills/justin/skills/doctor/SKILL.md`
- Modify: `CLAUDE.md` (Workspace Layout)
- Modify: `README.md` (Build Commands, Project Structure)
- Modify: `docs/project/developer-guide.md` (`§5.1` table, `.claude/hooks/` bullet)
- Modify: `.claude/rules/05-file-placement.md`
- Modify: `.claude/rules/01-code-conventions.md`
- Modify: `docs/project/agent-loop.md`
- Delete: `scripts/docs/check.py`
- Unchanged (verify): `scripts/docs/baseline.json`

**Interfaces:**
- Consumes: the `just tools` recipe (Task 6), which builds `target/tools/release/aios` and touches it; the shim `.claude/hooks/aios` (Task 2) with its `AIOS_TOOLS_BIN` override and exit 3 when the binary is missing and cannot be built; the goldens and `goldens_match_aios` (Task 14), which keep the parity proof once check.py is gone.
- Produces: `just docs-check [flags]` and `just docs-check-all` run `.claude/hooks/aios docs-check`; the Docs workflow builds the tools and runs `aios docs-check --markdown` with unchanged report-only semantics; `scripts/agent/brief.sh` and `/justin:doctor` call aios; `scripts/docs/check.py` no longer exists (read it with `git show 33c6b3d:scripts/docs/check.py`).

**Notes (check.py parity):**
- Output, exit codes and baseline bytes are unchanged (Task 14's goldens), so every consumer keeps working: `docs.yml` relies on exit 0, 1 and 2 (check.py L1662, L1665-1670), and brief.sh's jq reads `summary.*` and `checks.*.new` from `--json` (L1629-1660). New is exit 3 from the shim; `docs.yml` already treats any exit other than 0 and 1 as a checker failure.
- check.py resolved the repository from its own directory (L1576-1582); aios resolves it from the process working directory. `just` runs recipes from the justfile's directory, so `just docs-check` still checks the checkout that owns the justfile, and the shim picks the main checkout's binary.
- In this worktree the main checkout has no `tools/` yet (R1 is unmerged), so run docs-check with `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios"`. Without it the shim runs `just tools` in the main checkout, which has no such recipe, and exits 3.
- Every edit below was applied to a clone of this branch (with Task 1 and Task 6's justfile changes and stand-ins for the new files) and checked with check.py: `harness-tables`, `layout` and `pointer-doctor` stay at 0 new; documenting `just tools` in the README and developer-guide tables removes the `just-recipes|justfile|undocumented:tools` finding that Task 6's recipe introduced (`documented_recipes`, L751-784); the new `agent-loop.md` link resolves to the tracked directory `tools/src/cmd/docs_check/` (`md-links`, L575-585); the CLAUDE.md `tools/` lines sit below `uefi-stub/src/`, outside the layout check's `shared/src/` to `uefi-stub/` window (`tree_entries`, L1025-1051), and the new `hooks/` continuation line comes after the `agents/` line, whose list `layout_list` ends at the next tree line (L1100-1113). The baseline needs no change, and the only new finding left is the plan file's `plans-not-empty` (L1350-1376), which Task 16 removes.
- brief.sh's `--help` prints header lines 2-22 (`sed -n '2,22p'`), so its rewrapped comment paragraph keeps five lines.

- [ ] **Step 1: Record the state before the switch-over (the check this task must fix)**

```bash
just --show docs-check | tail -n 1
rg -c 'check\.py|python3 scripts/docs' justfile .github scripts .claude CLAUDE.md README.md CONTRIBUTING.md docs/project
just tools && target/tools/release/aios docs-check; echo "exit $?"
```

Expected: `    python3 scripts/docs/check.py "$@"`; then six files with matches (`justfile:3`, `.github/workflows/docs.yml:3`, `scripts/docs/check.py:7`, `scripts/agent/brief.sh:3`, `docs/project/agent-loop.md:1`, `.claude/skills/justin/skills/doctor/SKILL.md:1`, in any order); then exit 1 with this summary and new drift (a FAIL: the `tools` recipe is undocumented):

```text
docs-check: 80 findings across 15 checks - 2 new, 78 baselined (2 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links              23     0
  section-refs           4     0
  anchors                1     0
  wiki-links             0     0
  doc-map                4     0
  repo-paths             0     0
  just-recipes           2     1
  test-count             1     0
  lock-order            16     0
  milestone-status       5     0
  phase-count            1     0
  layout                 3     0
  harness-tables         0     0
  pointer-doctor        17     0
  knowledge-hygiene      3     1

New drift since baseline:

[just-recipes]
 + justfile: public recipe `tools` is missing from the README Build Commands and developer-guide §5.1 tables

[knowledge-hygiene]
 + docs/knowledge/plans/2026-09-22-jl-tools-r1-docs-check.md: working plan present; distill it and remove it before the PR is ready
exit 1
```

Any other new key is drift introduced on this branch (for example a Markdown link in the plan's prose): fix it before continuing.

- [ ] **Step 2: Point the justfile at the shim**

In `justfile`, replace:

```text
# ---------------------------------------------------------------------------
# Docs drift (scripts/docs/check.py: python3 stdlib, no LLM)
# ---------------------------------------------------------------------------

#   just docs-check                     new drift vs scripts/docs/baseline.json (exit 1 if any)
#   just docs-check --all               every finding; new ones marked '+'
#   just docs-check --update-baseline   accept the current findings as the new baseline
# Report docs drift that is not in the baseline
[positional-arguments]
docs-check *args:
    python3 scripts/docs/check.py "$@"

# List every docs drift finding, baselined and new
docs-check-all:
    python3 scripts/docs/check.py --all
```

with:

```text
# ---------------------------------------------------------------------------
# Docs drift (aios docs-check from tools/, no LLM)
# ---------------------------------------------------------------------------

#   just docs-check                     new drift vs scripts/docs/baseline.json (exit 1 if any)
#   just docs-check --all               every finding; new ones marked '+'
#   just docs-check --update-baseline   accept the current findings as the new baseline
# Report docs drift that is not in the baseline
[positional-arguments]
docs-check *args:
    .claude/hooks/aios docs-check "$@"

# List every docs drift finding, baselined and new
docs-check-all:
    .claude/hooks/aios docs-check --all
```

- [ ] **Step 3: Rewrite the Docs workflow**

Replace the whole of `.github/workflows/docs.yml` with:

```yaml
name: Docs

# Deterministic docs drift check (aios docs-check, built from tools/; no LLM).
# Report-only: drift never fails this check. New drift versus
# scripts/docs/baseline.json goes to the job summary and a warning annotation.
# The check fails only when the checker itself fails: the tools build, or
# aios docs-check exiting 2 (usage, git or internal error) or 3 (binary
# missing and not buildable). That is a broken checker and counts like any
# failing check.

on:
  push:
    branches: [main]
  pull_request:
    branches: [main]

permissions:
  contents: read

concurrency:
  group: docs-${{ github.ref }}
  cancel-in-progress: true

jobs:
  docs-check:
    name: Docs drift (report-only)
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
        with:
          # milestone-status reads main's first-parent history.
          fetch-depth: 0
      - uses: dtolnay/rust-toolchain@master
        with:
          toolchain: nightly
      - uses: Swatinem/rust-cache@v2
        with:
          workspaces: ". -> target/tools"
      - uses: extractions/setup-just@v4
      - name: Build aios tools
        run: just tools
      - name: Check docs drift against baseline
        shell: bash
        run: |
          rc=0
          just docs-check --markdown | tee -a "$GITHUB_STEP_SUMMARY" || rc=$?
          if [ "$rc" = 1 ]; then
            echo "::warning title=Docs drift::New docs drift versus scripts/docs/baseline.json; see the job summary. Report-only: drift does not fail this check."
          elif [ "$rc" != 0 ]; then
            echo "::error title=docs-check failed::aios docs-check exited $rc (checker error, not drift)"
            exit "$rc"
          fi
```

- [ ] **Step 4: Switch brief.sh and /justin:doctor**

In `scripts/agent/brief.sh`, replace:

```text
# Every section degrades to a one-line notice when git, gh, jq, python3 or the
# network is unavailable. Text from GitHub (titles, branch names) is printed as
# data with control characters replaced; it is never executed. Side effects:
# `git fetch --prune origin` (skip with --no-fetch) and a timestamp marker in
# the git common dir ($GIT_COMMON_DIR/aios-agent/last-brief).
```

with:

```text
# Every section degrades to a one-line notice when git, gh, jq, the aios tools
# binary or the network is unavailable. Text from GitHub (titles, branch names)
# is printed as data with control characters replaced; it is never executed.
# Side effects: `git fetch --prune origin` (skip with --no-fetch) and a timestamp
# marker in the git common dir ($GIT_COMMON_DIR/aios-agent/last-brief).
```

In `scripts/agent/brief.sh`, replace:

```text
# Start docs-check early; it is the slowest local step.
if [ -f scripts/docs/check.py ] && command -v python3 >/dev/null 2>&1; then
    (
        python3 scripts/docs/check.py --json >"$TMP/docs.json" 2>"$TMP/docs.err"
        echo $? >"$TMP/docs.rc"
    ) &
    DOCS_PID=$!
else
    DOCS_PID=""
fi
```

with:

```text
# Start docs-check early; it is the slowest local step.
if [ -x .claude/hooks/aios ]; then
    (
        .claude/hooks/aios docs-check --json >"$TMP/docs.json" 2>"$TMP/docs.err"
        echo $? >"$TMP/docs.rc"
    ) &
    DOCS_PID=$!
else
    DOCS_PID=""
fi
```

In `scripts/agent/brief.sh`, replace:

```text
    echo "- docs-check unavailable (needs python3 and scripts/docs/check.py on this checkout)"
```

with:

```text
    echo "- docs-check unavailable (needs .claude/hooks/aios on this checkout)"
```

In `.claude/skills/justin/skills/doctor/SKILL.md`, replace:

```text
   python3 scripts/docs/check.py --all --check pointer-doctor,harness-tables
```

with:

```text
   just docs-check --all --check pointer-doctor,harness-tables
```

- [ ] **Step 5: Update CLAUDE.md, README and the developer guide**

In `CLAUDE.md`, replace:

```text
Cargo workspace, three members. Run `ls kernel/src` for current per-file breakdown.
```

with:

```text
Cargo workspace, four members (`tools` is host-only and not a default member). Run `ls kernel/src` for current per-file breakdown.
```

In `CLAUDE.md`, replace:

```text
├── Cargo.toml            workspace root (resolver = "2"; members: kernel, shared, uefi-stub)
```

with:

```text
├── Cargo.toml            workspace root (resolver = "2"; members: kernel, shared, uefi-stub, tools; default: kernel, shared)
```

In `CLAUDE.md`, replace:

```text
├── justfile              build / build-stub / disk / run* / soak / check / test / clean
```

with:

```text
├── justfile              build / build-stub / disk / run* / soak / check / test / tools / docs-check / clean
```

In `CLAUDE.md`, replace:

```text
│   ├── hooks/            git-push-guard.py (PreToolUse), precompact-save.sh (PreCompact), tests/
```

with:

```text
│   ├── hooks/            git-push-guard.py (PreToolUse), precompact-save.sh (PreCompact),
│   │                     setup-dev-env.sh (SessionStart), aios (shim for the tools binary), tests/
```

In `CLAUDE.md`, replace:

```text
├── uefi-stub/src/        UEFI stub: BootInfo assembly, ELF loader, ExitBootServices, kernel jump
├── scripts/              setup-dev-env.sh, soak-qemu.sh (`just soak` boot soak harness)
```

with:

```text
├── uefi-stub/src/        UEFI stub: BootInfo assembly, ELF loader, ExitBootServices, kernel jump
├── tools/                host-only std crate aios-tools, binary aios (`just tools`):
│                         src/cmd/docs_check/ (docs drift checker), tests/ (goldens, fixtures)
├── scripts/              soak-qemu.sh (`just soak` boot soak harness), agent/ (brief, checkpoint),
│                         docs/baseline.json (accepted docs drift)
```

In `README.md`, replace:

```text
| `just test` | Run unit tests |
```

with:

```text
| `just test` | Run unit tests |
| `just tools` | Build the host tools binary `aios` (`target/tools/release/aios`), which `.claude/hooks/aios` runs |
```

In `README.md`, replace:

```text
└── uefi-stub/            # UEFI boot stub (aarch64-unknown-uefi)
```

with:

```text
├── uefi-stub/            # UEFI boot stub (aarch64-unknown-uefi)
└── tools/                # Host tools crate: the aios binary (docs-check), built with just tools
```

In `docs/project/developer-guide.md`, replace:

```text
| `just test` | Run host-side unit tests (shared crate) |
| `just clippy` | Run clippy on kernel and stub targets with `-D warnings` |
```

with:

```text
| `just test` | Run host-side unit tests (shared crate; the tools crate runs `cargo test -p aios-tools`) |
| `just tools` | Build the host tools binary `target/tools/release/aios` (`cargo build --release -p aios-tools --target-dir target/tools`); `.claude/hooks/aios` runs it |
| `just clippy` | Run clippy on kernel and stub targets with `-D warnings`, plus host clippy on the tools crate |
```

In `docs/project/developer-guide.md` (inside the long `.claude/hooks/` bullet of the Configuration list), replace:

```text
) and `precompact-save.sh` (flushes Remember memory before compaction). They live under
```

with:

```text
), `precompact-save.sh` (flushes Remember memory before compaction), `setup-dev-env.sh` (SessionStart: installs tools in web sessions and starts a background `just tools` build when the `aios` binary is missing or stale) and `aios` (the POSIX sh shim that runs `target/tools/release/aios` from the main checkout). They live under
```

- [ ] **Step 6: Update rules 01 and 05 and the agent-loop runbook**

In `.claude/rules/05-file-placement.md`, replace:

```text
uefi-stub/src/                 UEFI stub code
```

with:

```text
uefi-stub/src/                 UEFI stub code
tools/                         Host tooling crate aios-tools (binary aios): src/cmd/<subcommand>/, tests/ (goldens, fixtures)
```

In `.claude/rules/01-code-conventions.md`, replace:

```text
- All dependencies: must be `no_std` compatible
```

with:

```text
- All dependencies of `kernel/`, `shared/` and `uefi-stub/`: must be `no_std` compatible
- Host tools (`tools/`, package `aios-tools`, binary `aios`) are a std crate built for the host: the `no_std`/`no_main` rules and the `no_std` dependency rule do not apply there. Its dependencies are the fixed set clap, anyhow, serde, serde_json, regex and time, and it forbids `unsafe`.
```

In `docs/project/agent-loop.md`, replace:

```text
(git, gh, jq, python3; no LLM)
```

with:

```text
(git, gh, jq and the `aios` tools binary; no LLM)
```

In `docs/project/agent-loop.md`, replace:

```text
`just docs-check` compares [check.py](../../scripts/docs/check.py) findings with the baseline
```

with:

```text
`just docs-check` compares the findings of `aios docs-check` ([source](../../tools/src/cmd/docs_check/)) with the baseline
```

In `docs/project/agent-loop.md`, replace:

```text
The check fails only when check.py itself errors (exit 2), and
```

with:

```text
The check fails only when the checker itself errors (exit 2, or exit 3 when the `aios` binary cannot be built), and
```

- [ ] **Step 7: Delete check.py**

```bash
git rm -q scripts/docs/check.py
rm -rf scripts/docs/__pycache__
```

(`scripts/docs/__pycache__` is untracked, ignored bytecode of the deleted script.)

- [ ] **Step 8: Verify the switch-over**

```bash
just --show docs-check | tail -n 1
rg -n 'check\.py|python3 scripts/docs' justfile .github scripts .claude CLAUDE.md README.md CONTRIBUTING.md docs/project; echo "rg exit $?"
bash -n scripts/agent/brief.sh && echo brief-syntax-ok
ruby -ryaml -e 'YAML.load_file(".github/workflows/docs.yml")' && echo docs-yml-ok
git diff --exit-code 33c6b3d -- scripts/docs/baseline.json && echo baseline-unchanged
```

Expected: `    .claude/hooks/aios docs-check "$@"`; `rg exit 1` with no match listed; `brief-syntax-ok`; `docs-yml-ok`; `baseline-unchanged`.

Run: `just tools && AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check --all --check harness-tables,layout,pointer-doctor; echo "exit $?"`
Expected: exit 0, and the output starts with

```text
docs-check: 20 findings across 3 checks - 0 new, 20 baselined (1 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  layout                 3     0
  harness-tables         0     0
  pointer-doctor        17     0

All findings ('+' = new since baseline, '~' = accepted false positive):
```

followed by the 20 baselined findings (3 layout, 17 pointer-doctor, one of them marked `~`) and no line marked `+`.

Run: `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check; echo "exit $?"`
Expected: stdout is exactly

```text
docs-check: 79 findings across 15 checks - 1 new, 78 baselined (2 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links              23     0
  section-refs           4     0
  anchors                1     0
  wiki-links             0     0
  doc-map                4     0
  repo-paths             0     0
  just-recipes           1     0
  test-count             1     0
  lock-order            16     0
  milestone-status       5     0
  phase-count            1     0
  layout                 3     0
  harness-tables         0     0
  pointer-doctor        17     0
  knowledge-hygiene      3     1

New drift since baseline:

[knowledge-hygiene]
 + docs/knowledge/plans/2026-09-22-jl-tools-r1-docs-check.md: working plan present; distill it and remove it before the PR is ready
```

then just reports the failed recipe on stderr and the echo prints `exit 1`: the plan file is the only new finding, 0 resolved.

Run: `cargo test -p aios-tools`
Expected: every test binary ends `test result: ok.`; `differential_against_check_py` passes by returning early (check.py is gone) and `goldens_match_aios` still passes.

- [ ] **Step 9: Commit and push**

```bash
git add justfile .github/workflows/docs.yml scripts/agent/brief.sh .claude/skills/justin/skills/doctor/SKILL.md CLAUDE.md README.md docs/project/developer-guide.md .claude/rules/05-file-placement.md .claude/rules/01-code-conventions.md docs/project/agent-loop.md
git status --short
git commit -F - <<'EOF'
Switch docs-check to the aios binary and delete check.py

just docs-check and docs-check-all, the Docs workflow, brief.sh and
/justin:doctor now run aios docs-check through .claude/hooks/aios, with
the same output, exit codes and baseline format. The goldens under
tools/tests/golden/docs-check/ keep the parity proof now that
scripts/docs/check.py is deleted; scripts/docs/baseline.json is
unchanged. CLAUDE.md, the README, the developer guide, agent-loop.md
and rules 01 and 05 describe the tools crate and the shim, and the
stale scripts/setup-dev-env.sh layout entry is gone.

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

Expected: `git status --short` before the commit lists exactly the ten modified files and `D  scripts/docs/check.py`, all staged.

-----

### Task 16: Permission rules (main session), final gates, distill, PR

All commands run in `/Users/juslee/Documents/workspace/juslee/aios/.claude/worktrees/tools-r1`.

**Files:**
- Modify (MAIN SESSION, owner approval): `.claude/settings.json`
- Modify (MAIN SESSION): `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md` (Open Questions)
- Create: `docs/knowledge/lessons/2026-09-22-jl-rust-port-parity-gotchas.md`
- Delete: `docs/knowledge/plans/2026-09-22-jl-tools-r1-docs-check.md` (this plan)

**Interfaces:**
- Consumes: everything from Tasks 1-15; the committed state of `.claude/settings.json` after Step 2 (the PR body reads the rule form from it).
- Produces: a clean branch (0 new docs drift, all gates green) and the PR `Tools R1: aios crate, shim, CI, docs-check in Rust`, handed to the owner to merge.

**Notes (check.py parity):**
- `knowledge-hygiene` reports every file under `docs/knowledge/plans/` except `README.md` and `_template.md` as `plans-not-empty` (check.py L1350-1376); deleting this plan is what brings docs-check to 0 new. The lesson file must match `YYYY-MM-DD-initials-short-description.md` (L94) and carry `author`, `date`, `tags` and a `status` of `draft`, `in-progress` or `final` (L1365-1376). Both were checked against check.py on a clone with Task 15 applied: 78 findings, 0 new, 0 resolved.
- Steps 1-3 and Step 12 run in the MAIN SESSION only, with the owner's approval for the `.claude/settings.json` change and for the `main` ruleset change; a subagent executing this plan stops before Step 1 and hands back. This plan never scripts the write of `.claude/settings.json` and never changes the ruleset itself.
- The probe in Step 1 lets the model `Read` each file before editing it: `Edit` requires a prior read, and an edit that fails that precondition says nothing about whether the ask rule matched. A control edit outside the guarded path shows that `acceptEdits` approves edits the rules do not cover, so a refusal on the guarded path can only come from the ask rule.
- `main` moved to `0e067b5` after this branch started (`54d3acd` ADR #174, `8dca63e` a research note under `docs/knowledge/research/`, `0e067b5` the ipc channel-id range check, touching `kernel/src/cap/mod.rs`, `kernel/src/ipc/` and `shared/src/ipc.rs`); check.py on `0e067b5` reports 78 findings, 0 new, 0 resolved, and `scripts/docs/baseline.json` is byte-identical to `33c6b3d`, so rebasing onto it changes no finding. The real-snapshot goldens replay `33c6b3d` and do not change either.
- Read this whole task before Step 6 deletes the plan file (or copy the plan to your scratchpad first).
- Step 12 changes GitHub repository settings, not a file in this repository, so it leaves no commit; it is listed as a step so the hand-off in Step 14 cannot forget it.

- [ ] **Step 1 (MAIN SESSION): Probe the ask-rule syntax**

With `SCRATCH` set to the session's scratchpad directory:

```bash
SCRATCH="${SCRATCH:?set SCRATCH to the scratchpad directory of this session}"
P="$SCRATCH/permcheck"
rm -rf "$P"
mkdir -p "$P/tools/src/cmd/guard" "$P/tools/src/cmd/free" "$P/.claude"
git -C "$P" init -q
printf 'old\n' > "$P/tools/src/cmd/guard/probe.rs"
printf 'old\n' > "$P/tools/src/cmd/free/control.rs"
printf '%s\n' '{"permissions":{"ask":["Edit(/tools/src/cmd/guard/**)","Write(/tools/src/cmd/guard/**)"]}}' > "$P/.claude/settings.json"
(cd "$P" && claude -p --model haiku --settings "$P/.claude/settings.json" --permission-mode acceptEdits --output-format json \
    "Do exactly these steps, using no other tool and never retrying a denied tool call. 1. Read tools/src/cmd/free/control.rs, then use Edit on it to replace the word old with new. 2. Read tools/src/cmd/guard/probe.rs, then use Edit on it to replace the word old with new. 3. Use Write to create tools/src/cmd/guard/created.rs containing the single line new. Then report which steps were denied.") > "$P/result.json"
printf 'control:%s\n' "$(cat "$P/tools/src/cmd/free/control.rs")"
printf 'probe:%s\n' "$(cat "$P/tools/src/cmd/guard/probe.rs")"
if [ -e "$P/tools/src/cmd/guard/created.rs" ]; then echo "created.rs exists"; else echo "created.rs absent"; fi
jq -r '.permission_denials[] | "\(.tool_name) \(.tool_input.file_path)"' "$P/result.json"
```

Expected (the anchored form works): `control:new`, `probe:old`, `created.rs absent`, and the denials `Edit` and `Write` (one each, both on `tools/src/cmd/guard/`).

If `probe` reads `new` or `created.rs` exists, run the same block once more with both rules in the `printf` line written as `Edit(**/tools/src/cmd/guard/**)` and `Write(**/tools/src/cmd/guard/**)`, and expect the same four results. If `control` is not `new`, the probe is inconclusive (acceptEdits did not apply): fix the invocation before judging either form. If neither form is refused, do not guess: open the issue below, skip the ask rules in Step 2, and record the result in Step 3.

```bash
gh issue create --label needs-human --title "Ask rules for tools/src/cmd/guard and tools/src/cmd/loop: syntax unresolved" --body "The R1 probe (headless claude -p, acceptEdits, a Read before each Edit) did not refuse edits under tools/src/cmd/guard/ with either Edit(/tools/src/cmd/guard/**) or Edit(**/tools/src/cmd/guard/**) as an ask rule in .claude/settings.json. R1 therefore ships without these ask rules. Decide the rule form (or another protection) before R2 adds tools/src/cmd/loop/."
```

Finally `rm -rf "$SCRATCH/permcheck"`.

- [ ] **Step 2 (MAIN SESSION, owner approval): Edit `.claude/settings.json`**

Show the owner the change and apply it by hand after approval:
- `permissions.allow`: add `"Bash(.claude/hooks/aios *)"` (next to `"Bash(just *)"`).
- `permissions.ask`, when Step 1 confirmed the anchored form: add `"Edit(/tools/src/cmd/guard/**)"`, `"Write(/tools/src/cmd/guard/**)"`, `"Edit(/tools/src/cmd/loop/**)"`, `"Write(/tools/src/cmd/loop/**)"`.
- `permissions.ask`, when only the `**/` form was refused: add `"Edit(**/tools/src/cmd/guard/**)"`, `"Write(**/tools/src/cmd/guard/**)"`, `"Edit(**/tools/src/cmd/loop/**)"`, `"Write(**/tools/src/cmd/loop/**)"`.
- `permissions.ask`, when neither form was refused: add nothing.

Nothing wires `aios guard` into a hook in R1.

```bash
jq empty .claude/settings.json && echo settings-json-ok
jq -r '.permissions.allow[] | select(. == "Bash(.claude/hooks/aios *)")' .claude/settings.json
jq -r '.permissions.ask[] | select(test("tools/src/cmd/(guard|loop)/"))' .claude/settings.json
```

Expected: `settings-json-ok`; `Bash(.claude/hooks/aios *)`; the four ask rules just added (none when neither form was refused).

```bash
git add .claude/settings.json
git commit -F - <<'EOF'
Allow the aios shim; ask before edits to guard and loop sources

Owner-approved. Bash(.claude/hooks/aios *) is allowed; Edit and Write
under tools/src/cmd/guard/ and tools/src/cmd/loop/ ask, and headless
claude -p turns the ask into a refusal (checked with a one-call probe
in R1).

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
```

When no ask rule was added, use the subject `Allow the aios shim` and the body `Owner-approved. Bash(.claude/hooks/aios *) is allowed. The ask rules for tools/src/cmd/guard/ and tools/src/cmd/loop/ wait for the needs-human issue on their syntax.` followed by the same `Co-Authored-By` line.

- [ ] **Step 3 (MAIN SESSION): Record the answers in the spec's Open Questions**

In `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md`, replace:

```text
- The exact permission-rule syntax that anchors `Edit(...)`/`Write(...)` ask rules to `tools/src/cmd/guard/**` in the project settings, and whether headless `claude -p` turns those asks into refusals. To be verified in R1 with a one-call check.
- Whether `default-members` exclusion keeps `cargo build --target aarch64-unknown-none` and the kernel CI jobs from trying to build the host crate for the bare-metal target. To be verified in R1.
```

with (keep exactly one of the three `R1 answer` lines under the first question, the one matching Step 1):

```text
- The exact permission-rule syntax that anchors `Edit(...)`/`Write(...)` ask rules to `tools/src/cmd/guard/**` in the project settings, and whether headless `claude -p` turns those asks into refusals. To be verified in R1 with a one-call check.
  - **R1 answer (anchored form):** `Edit(/tools/src/cmd/guard/**)` and `Write(/tools/src/cmd/guard/**)` match, a leading `/` being relative to the project root (the directory that holds `.claude/`), and headless `claude -p` turns the ask into a refusal (probe: guarded file unchanged, one denial each for `Edit` and `Write`, an unguarded control edit applied). `.claude/settings.json` has these rules for `guard` and `loop`.
  - **R1 answer (`**/` form):** the anchored `/tools/src/cmd/guard/**` form was not refused; `Edit(**/tools/src/cmd/guard/**)` and `Write(**/tools/src/cmd/guard/**)` are, and headless `claude -p` turns the ask into a refusal (probe: guarded file unchanged, one denial each for `Edit` and `Write`, an unguarded control edit applied). `.claude/settings.json` has these rules for `guard` and `loop`.
  - **R1 answer (unresolved):** neither the `/tools/...` nor the `**/tools/...` form was refused by headless `claude -p` in the probe. R1 ships without these ask rules, and a `needs-human` issue tracks the decision.
- Whether `default-members` exclusion keeps `cargo build --target aarch64-unknown-none` and the kernel CI jobs from trying to build the host crate for the bare-metal target. To be verified in R1.
  - **R1 answer:** yes. With `default-members = ["kernel", "shared"]`, `cargo build --target aarch64-unknown-none -v` compiles nothing from `aios-tools`; the kernel CI jobs call recipes that build the default members or name a package with `-p`; `just test`, the only `--workspace` recipe, excludes `aios-tools`.
```

```bash
git add docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md
git commit -F - <<'EOF'
Record the R1 answers to the Rust tooling open questions

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

- [ ] **Step 4: Confirm the plan file is the only drift left (expected FAIL)**

Run: `just tools && AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check; echo "exit $?"`
Expected: exit 1 with exactly one new finding, `docs/knowledge/plans/2026-09-22-jl-tools-r1-docs-check.md: working plan present; distill it and remove it before the PR is ready`.

- [ ] **Step 5: Distill the lessons**

Read this plan for anything recorded while executing it (issues, decisions, surprises). Create `docs/knowledge/lessons/2026-09-22-jl-rust-port-parity-gotchas.md` with the content below, folding in what you recorded; delete any bullet that did not hold in this PR (keep the frontmatter and the four headings).

```markdown
---
author: jl + claude
date: 2026-09-22
tags: [tooling, agent-loop]
status: final
---

# Lesson: Porting a Python tool to Rust with byte-identical output

## What happened

PR R1 ported the docs drift checker (`scripts/docs/check.py`, 1,670 lines of Python) to `aios docs-check` in the host crate `tools/`, and proved parity with goldens recorded from the Python tool before deleting it. Almost none of the effort went into the checks themselves. It went into Python string semantics, into regex features that the Rust `regex` crate lacks, and into the shim's freshness test.

## Why it happened

- Python `str.splitlines()` breaks at `\n`, `\r`, `\r\n`, `\x0b`, `\x0c`, `\x1c`, `\x1d`, `\x1e`, `\x85`, U+2028 and U+2029, and Python text mode turns `\r\n` and `\r` into `\n` before that. Rust `str::lines()` breaks only at `\n` and `\r\n`, so line numbers and even line contents differ on files that contain the other breaks.
- Python `str.strip()` and `str.split()` use `str.isspace()`, which also covers U+001C to U+001F. Rust `char::is_whitespace` does not.
- Five `check.py` patterns used lookaround (`(?=...)`, `(?!...)`, `(?<!...)`). The `regex` crate has no lookaround and no backreferences.
- A `cargo build` with nothing to do leaves the binary's mtime unchanged. A `find -newer` freshness test then reports the binary as stale after every edit that does not change it, for example an edit under `tools/tests/`, and the shim rebuilds on every call.
- The shim runs the main checkout's binary on purpose, so a PR that adds or changes a subcommand cannot run its own build through the shim until it merges.

## What we learned

- Port the string primitives first (`tools/src/pystr.rs`, `tools/src/paths.rs`) and use them everywhere. Never mix in `str::lines()`, `trim()` or `split_whitespace()`.
- Rewrite each lookaround as a plain pattern plus a check on the text after the match, and write down why backtracking could not have produced a different match. Unit-test the cases of that argument.
- Record the goldens from the old tool before deleting it: on a pinned commit (`git clone --shared`, a detached checkout, every ref deleted so the history base is the pinned commit) and on a fixture repository with exactly one injected drift per check. Keep a differential test that runs both tools while the old one exists.
- Settle permission-rule syntax with a one-call headless probe instead of reading it off the docs. Let the probe `Read` the file before it edits: `Edit` requires a prior read, and an edit that fails that precondition says nothing about whether the ask rule matched. The answer for `Edit(...)`/`Write(...)` path anchoring is recorded under Open Questions in `docs/knowledge/discussions/2026-09-22-jl-rust-agent-tools.md`.

## How to avoid next time

- `just tools` ends with `touch target/tools/release/aios`, so a no-op build still makes the binary fresh.
- Test a PR's own build with `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios"`.
- Later ports (R4, R2, R3, R5) follow the same order: fixture bundle and drift variants, an ignored recorder test, a differential test, then the switch-over and the deletion in the same PR.
```

- [ ] **Step 6: Delete the plan**

```bash
git rm -q docs/knowledge/plans/2026-09-22-jl-tools-r1-docs-check.md
```

- [ ] **Step 7: Verify 0 new drift**

Run: `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check; echo "exit $?"`
Expected: exactly

```text
docs-check: 78 findings across 15 checks - 0 new, 78 baselined (2 accepted false positives), 0 resolved (baseline scripts/docs/baseline.json)

  check              total   new
  md-links              23     0
  section-refs           4     0
  anchors                1     0
  wiki-links             0     0
  doc-map                4     0
  repo-paths             0     0
  just-recipes           1     0
  test-count             1     0
  lock-order            16     0
  milestone-status       5     0
  phase-count            1     0
  layout                 3     0
  harness-tables         0     0
  pointer-doctor        17     0
  knowledge-hygiene      2     0

No new drift since baseline.
exit 0
```

- [ ] **Step 8: Commit and push**

```bash
git add docs/knowledge/lessons/2026-09-22-jl-rust-port-parity-gotchas.md
git commit -F - <<'EOF'
Distill the Tools R1 plan into a lesson and remove it

Co-Authored-By: Claude Opus 5 (1M context) <noreply@anthropic.com>
EOF
git push -u origin claude/tools-crate-docs-check
```

- [ ] **Step 9: Rebase onto main if it moved**

```bash
git fetch origin main
if git merge-base --is-ancestor origin/main HEAD; then
    echo "already on top of origin/main"
else
    git rebase origin/main && git push --force-with-lease origin claude/tools-crate-docs-check
fi
git log --oneline -1 origin/main
```

Expected: either `already on top of origin/main`, or a clean rebase (at the time of writing `origin/main` is `0e067b5`, three commits ahead of the branch point `33c6b3d`) followed by a lease push. Never push to `main`; a plain `--force` is denied by the push guard.

- [ ] **Step 10: Run the final gates**

```bash
just check
just test
cargo test -p aios-tools
just build-release
cargo metadata --no-deps --format-version 1 | jq -r '.workspace_default_members[]' | sed 's|.*/||'
cargo build --target aarch64-unknown-none -v 2>&1 | grep -c 'aios[-_]tools' || true
just tools && AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check; echo "docs-check exit $?"
if command -v cargo-deny >/dev/null 2>&1; then just deny; else echo "cargo-deny not installed; the CI Security job checks it"; fi
if command -v cargo-audit >/dev/null 2>&1; then just audit; else echo "cargo-audit not installed; the CI Security job checks it"; fi
```

Expected: `just check`, `just test`, `just build-release` succeed with zero warnings; `cargo test -p aios-tools` shows every test binary `ok` (`goldens_match_aios` passes, `differential_against_check_py` returns early); the default members are `kernel#0.1.0` and `shared#0.1.0`; the `grep -c` prints `0`; docs-check prints the Step 7 output and `docs-check exit 0`; `just deny` and `just audit` pass where installed (the CI Security job runs both on the PR in any case).

- [ ] **Step 11: Audit loop**

Run `/audit-loop` (rule 02) and repeat it until a full round reports 0 issues. Commit each round's fixes with a descriptive subject and the `Co-Authored-By` line, push, and re-run Step 10 when code or docs changed.

- [ ] **Step 12 (MAIN SESSION, owner approval): Require `Tools (host)` on `main`**

The `main` ruleset (id `23812283`) requires five status checks and has no bypass; the new `Tools (host)` job is not one of them. Once Task 15 deletes `scripts/docs/check.py`, `goldens_match_aios` is the only parity gate left and it runs only in that job, so a failing or missing `Tools (host)` would not block a merge and every later PR would silently lose parity.

Read the current list:

```bash
gh api repos/:owner/:repo/rulesets/23812283 --jq '[.rules[]|select(.type=="required_status_checks").parameters.required_status_checks[].context]'
```

Expected before the change: `["Check (fmt + clippy + build)","Build (release)","Test (host)","Security (audit + deny)","Miri (unsafe UB detection)"]`.

Add the context `Tools (host)` (no `integration_id`, as for the existing five) in Settings -> Rules -> the `main` ruleset, or with `gh api -X PUT repos/:owner/:repo/rulesets/23812283 --input <patched.json>`, then verify with the same query.

Expected after the change: six contexts, including `Tools (host)`.

A required check that a branch cannot report blocks its PR, and only branches carrying this PR's CI change have a `Tools (host)` job. So make the change immediately before or immediately after merging this PR, and rebase any other open PR onto `main` afterwards.

- [ ] **Step 13: Open the PR**

With `SCRATCH` set to the session's scratchpad directory, in one shell call:

````bash
SCRATCH="${SCRATCH:?set SCRATCH to the scratchpad directory of this session}"
guard_rule=$(jq -r '[.permissions.ask[]? | select(startswith("Edit(") and endswith("tools/src/cmd/guard/**)"))][0] // ""' .claude/settings.json)
case "$guard_rule" in
"Edit(/tools/src/cmd/guard/**)")
    rule_line='1. Permission-rule syntax: `Edit(/tools/src/cmd/guard/**)` and `Write(/tools/src/cmd/guard/**)` (a leading `/` anchors to the project root) match, and headless `claude -p` turns the ask into a refusal (one-call probe: guarded file unchanged, one denial each for Edit and Write, an unguarded control edit applied).'
    settings_line='`.claude/settings.json` (owner-approved): allow `Bash(.claude/hooks/aios *)`; ask on `Edit` and `Write` for `/tools/src/cmd/guard/**` and `/tools/src/cmd/loop/**`.'
    ;;
"Edit(**/tools/src/cmd/guard/**)")
    rule_line='1. Permission-rule syntax: the anchored `/tools/src/cmd/guard/**` form was not refused; `Edit(**/tools/src/cmd/guard/**)` and `Write(**/tools/src/cmd/guard/**)` are, and headless `claude -p` turns the ask into a refusal (one-call probe per form: guarded file unchanged, one denial each for Edit and Write, an unguarded control edit applied).'
    settings_line='`.claude/settings.json` (owner-approved): allow `Bash(.claude/hooks/aios *)`; ask on `Edit` and `Write` for `**/tools/src/cmd/guard/**` and `**/tools/src/cmd/loop/**`.'
    ;;
"")
    rule_line='1. Permission-rule syntax: still open. Neither `/tools/...` nor `**/tools/...` ask rules were refused by headless `claude -p` in the probe, so no ask rules were added; a `needs-human` issue tracks it.'
    settings_line='`.claude/settings.json` (owner-approved): allow `Bash(.claude/hooks/aios *)`. No ask rules yet (open question 1).'
    ;;
*)
    echo "unexpected guard ask rule in .claude/settings.json: $guard_rule" >&2
    exit 1
    ;;
esac
body="$SCRATCH/pr-body.md"
{
cat <<'EOF'
## Summary

- New host-only crate `tools/` (package `aios-tools`, binary `aios`, edition 2021, BSD-2-Clause). It is a workspace member but not a default member, so bare-metal builds never compile it. Dependencies: clap, anyhow, serde, serde_json (`preserve_order`), regex, and time (declared now for the PR loop in R2). `#![forbid(unsafe_code)]`.
- `aios docs-check`: a line-by-line Rust port of `scripts/docs/check.py` (15 checks, baseline comparison, text, Markdown and JSON output, `--update-baseline`), byte-identical in stdout, exit codes and the written baseline. The five `check.py` patterns that used lookaround are rewritten as plain patterns plus code.
- `.claude/hooks/aios`: a POSIX sh shim that runs the main checkout's `target/tools/release/aios`. Fresh: run. Stale: rebuild in the foreground (for `guard`, from R5 on: run, and rebuild in the background). Missing: build, and exit 3 naming `just tools` if that fails (for `guard`: a PreToolUse `ask`). `AIOS_TOOLS_BIN` overrides the binary. `just tools` builds it, and `.claude/hooks/setup-dev-env.sh` starts a background build at session start.
- CI: a new `Tools (host)` job runs `cargo fmt --check`, `cargo clippy -- -D warnings` and `cargo test` for `aios-tools`, including the parity goldens, with full history. `just check` runs host clippy on the crate; `just test` leaves its tests to that job. The Docs workflow builds the tools and runs `aios docs-check`, still report-only.
- Switch-over: `just docs-check` and `docs-check-all`, `scripts/agent/brief.sh` and `/justin:doctor` call `aios`; `scripts/docs/check.py` is deleted; `scripts/docs/baseline.json` is byte-identical.
- Docs: the `CLAUDE.md` workspace layout (adds `tools/` and the shim, fixes the stale `scripts/setup-dev-env.sh` entry), the README and developer-guide build tables, rules 01 and 05, and `docs/project/agent-loop.md`; a lesson in `docs/knowledge/lessons/2026-09-22-jl-rust-port-parity-gotchas.md`.

## Parity evidence

- Goldens recorded from `check.py` before its deletion: `tools/tests/golden/docs-check/`, 70 cases plus 4 written baselines.
  - 22 cases on a snapshot of `main` at `33c6b3d`: default, `--all`, `--json`, `--json --all`, `--markdown`, `--list-checks`, `--check <name>` for each of the 15 checks, and `--update-baseline`.
  - 48 cases on a fixture repository (`tools/tests/fixtures/docs-check/`): the base repository, one single-drift variant per check (15), a pure line shift, a skipped check, a grown occurrence count, and three `--baseline` cases (a path with a `..` segment, a run from a subdirectory with a missing baseline, and a write to a baseline outside `scripts/docs/baseline.json`).
- `differential_against_check_py` ran `check.py` and `aios` side by side on every case and on the live worktree before the deletion, with 0 differences. It now returns early; `goldens_match_aios` carries parity in CI.

## Behaviour changes

None in stdout, exit codes (0 no new drift, 1 new drift, 2 checker error) or the baseline format. The checker now needs the `aios` binary: the shim builds it on demand and exits 3 naming `just tools` when it cannot. Until this PR merges the main checkout has no binary, so a worktree tests its own build with `AIOS_TOOLS_BIN`.

## Open questions from the spec

EOF
printf '%s\n' "$rule_line"
cat <<'EOF'
2. Default members: `default-members = ["kernel", "shared"]` keeps bare-metal builds off the host crate. `cargo build --target aarch64-unknown-none -v` compiles nothing from `aios-tools`, the kernel CI jobs call recipes that build the default members or name a package with `-p`, and `just test` (the only `--workspace` recipe) excludes `aios-tools`.

## Settings

EOF
printf '%s\n' "$settings_line"
cat <<'EOF'

The `main` ruleset (`23812283`) still needs the new `Tools (host)` job in its required status checks: with `scripts/docs/check.py` deleted, `goldens_match_aios` in that job is the only parity gate. Owner action, immediately before or after the merge.

## Test plan

- [x] `just check`
- [x] `just test`
- [x] `cargo test -p aios-tools` (goldens pass; the differential returns early now that `check.py` is gone)
- [x] `just build-release`
- [x] `AIOS_TOOLS_BIN="$PWD/target/tools/release/aios" just docs-check`: 0 new, 0 resolved
- [x] `just deny` and `just audit` where installed (the Security job runs both)
- [x] `/audit-loop`: a clean round

🤖 Generated with [Claude Code](https://claude.com/claude-code)
EOF
} > "$body"
gh pr create --base main --head claude/tools-crate-docs-check --title "Tools R1: aios crate, shim, CI, docs-check in Rust" --body-file "$body"
````

Expected: `gh pr create` prints the PR URL.

- [ ] **Step 14: Checks, review comments, hand-off**

```bash
gh pr checks claude/tools-crate-docs-check --watch
```

Expected: every check passes, including `Tools (host)` and `Docs drift (report-only)` (its job summary shows `**0 new** finding(s) since baseline`). Then run `/review-pr-comments` (rule 03: wait for the automated reviewers, fix, reply, resolve). Finish by reporting the PR URL, the final `gh pr checks` state and the outstanding ruleset change to the owner (Step 12: `Tools (host)` must become a required status check on `main`, or the parity gate is advisory once check.py is gone), who merges the PR (agents never merge or push to `main`).
