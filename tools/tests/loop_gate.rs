//! The Option-A gate (R2a, Task 4b): path normalisation, the gate over a real
//! repository's diff, and the cross-check of GitHub's file list.

mod common;

use aios_tools::cmd::pr_loop::gate::{cross_check, gated_paths, normalise};
use aios_tools::inputs::is_tools_input;
use common::{git, TestRepo};
use std::collections::BTreeSet;

#[test]
fn normalise_and_gate_path_table() {
    // (path, normalised or None when rejected, is it a tools input)
    let rows: &[(&str, Option<&str>, bool)] = &[
        ("tools/src/main.rs", Some("tools/src/main.rs"), true),
        ("./tools/src/main.rs", Some("tools/src/main.rs"), true),
        ("././tools/x", Some("tools/x"), true),
        ("tools//src///main.rs", Some("tools/src/main.rs"), true),
        ("tools/./x", Some("tools/x"), true),
        ("tools/", Some("tools"), true),
        ("tools", Some("tools"), true),
        ("shared/src/lib.rs", Some("shared/src/lib.rs"), true),
        ("Cargo.lock", Some("Cargo.lock"), true),
        ("Cargo.toml", Some("Cargo.toml"), true),
        ("rust-toolchain.toml", Some("rust-toolchain.toml"), true),
        ("rust-toolchain", Some("rust-toolchain"), true),
        (".cargo/config.toml", Some(".cargo/config.toml"), true),
        ("justfile", Some("justfile"), true),
        ("Justfile", Some("Justfile"), true),
        ("TOOLS/x", Some("TOOLS/x"), true),
        ("Tools/Src/x", Some("Tools/Src/x"), true),
        ("toolsx/a", Some("toolsx/a"), false),
        ("tools-old/a", Some("tools-old/a"), false),
        ("docs/tools/x", Some("docs/tools/x"), false),
        ("kernel/Cargo.toml", Some("kernel/Cargo.toml"), false),
        ("uefi-stub/Cargo.toml", Some("uefi-stub/Cargo.toml"), false),
        ("Cargo.toml.bak", Some("Cargo.toml.bak"), false),
        ("Cargo.lockx", Some("Cargo.lockx"), false),
        (
            "scripts/agent/loop-config.json",
            Some("scripts/agent/loop-config.json"),
            false,
        ),
        (".claude/hooks/aios", Some(".claude/hooks/aios"), false),
        ("docs/a.md", Some("docs/a.md"), false),
        ("..", None, false),
        ("../tools/x", None, false),
        ("tools/../shared/x", None, true),
        ("tools/..", None, true),
        ("/tools/x", None, false),
        ("/", None, false),
        ("", None, false),
        (".", None, false),
        ("./", None, false),
    ];
    assert!(rows.len() >= 25);
    for &(path, normalised, tools_input) in rows {
        assert_eq!(
            normalise(path).ok().as_deref(),
            normalised,
            "normalise({path:?})"
        );
        // The gate sees git's paths, which are already normal.
        let probe = normalised.unwrap_or(path);
        assert_eq!(
            is_tools_input(probe),
            tools_input,
            "is_tools_input({probe:?})"
        );
    }
}

/// A repository whose `origin/main` is its first commit, on branch `pr`.
fn pr_repo(label: &str) -> TestRepo {
    let repo = TestRepo::with_files(
        label,
        &[
            ("tools/src/main.rs", "fn main() {}\n"),
            ("tools/b.rs", "// b\n// b\n// b\n// b\n"),
            ("shared/lib.rs", "// lib\n"),
            ("justfile", "default:\n\techo\n"),
            ("Cargo.lock", "# 1\n"),
            ("docs/a.md", "# a\n# a\n# a\n# a\n"),
            ("docs/other.md", "other\n"),
        ],
    );
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(repo.path(), &["checkout", "-q", "-b", "pr"]);
    repo
}

fn gate(repo: &TestRepo) -> Vec<String> {
    gated_paths(repo.path(), "HEAD").expect("gated paths")
}

#[test]
fn edit_of_a_tools_file_is_gated() {
    let repo = pr_repo("loop-gate-edit");
    repo.write("tools/src/main.rs", "fn main() { println!(); }\n");
    repo.commit("edit");
    assert_eq!(gate(&repo), ["tools/src/main.rs"]);
}

#[test]
fn added_shared_file_is_gated() {
    let repo = pr_repo("loop-gate-add");
    repo.write("shared/x", "x\n");
    repo.commit("add");
    assert_eq!(gate(&repo), ["shared/x"]);
}

#[test]
fn deleted_justfile_is_gated() {
    let repo = pr_repo("loop-gate-delete");
    git(repo.path(), &["rm", "-q", "justfile"]);
    repo.commit("delete");
    assert_eq!(gate(&repo), ["justfile"]);
}

#[test]
fn rename_into_and_out_of_tools_is_gated() {
    let repo = pr_repo("loop-gate-rename");
    git(repo.path(), &["mv", "docs/a.md", "tools/a.md"]);
    git(repo.path(), &["mv", "tools/b.rs", "docs/b.rs"]);
    repo.commit("renames");
    // Both sides of both renames: the old docs path is not an input, the new
    // tools path is; the old tools path is, the new docs path is not.
    assert_eq!(gate(&repo), ["tools/a.md", "tools/b.rs"]);
}

#[test]
fn added_cargo_config_is_gated() {
    let repo = pr_repo("loop-gate-cargo");
    repo.write(".cargo/config.toml", "[build]\n");
    repo.commit("cargo config");
    assert_eq!(gate(&repo), [".cargo/config.toml"]);
}

#[test]
fn upper_case_tools_directory_is_gated() {
    let repo = pr_repo("loop-gate-case");
    // Straight into the index: on a case-insensitive filesystem a file written
    // as TOOLS/x would land in tools/.
    let blob = git(
        repo.path(),
        &["hash-object", "-w", "-t", "blob", "/dev/null"],
    );
    git(
        repo.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("100644,{},TOOLS/x", blob.trim()),
        ],
    );
    git(repo.path(), &["commit", "-q", "-m", "TOOLS"]);
    assert_eq!(gate(&repo), ["TOOLS/x"]);
}

#[test]
fn mode_only_change_is_gated() {
    use std::os::unix::fs::PermissionsExt;
    let repo = pr_repo("loop-gate-mode");
    let file = repo.path().join("tools/b.rs");
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o755)).expect("chmod +x");
    repo.commit("chmod +x");
    assert_eq!(gate(&repo), ["tools/b.rs"]);
}

#[test]
fn docs_only_branch_that_merged_a_main_with_a_cargo_lock_change_is_not_gated() {
    let repo = pr_repo("loop-gate-merge");
    repo.write("docs/new.md", "new\n");
    repo.commit("docs only");
    // main moves on, and changes a gated input.
    git(repo.path(), &["checkout", "-q", "main"]);
    repo.write("Cargo.lock", "# 2\n");
    repo.commit("main changes Cargo.lock");
    git(
        repo.path(),
        &["update-ref", "refs/remotes/origin/main", "HEAD"],
    );
    git(repo.path(), &["checkout", "-q", "pr"]);
    git(
        repo.path(),
        &["merge", "-q", "--no-ff", "-m", "merge main", "main"],
    );
    assert!(gate(&repo).is_empty());
}

#[test]
fn docs_only_pr_is_not_gated() {
    let repo = pr_repo("loop-gate-docs");
    repo.write("docs/other.md", "changed\n");
    repo.commit("docs");
    assert!(gate(&repo).is_empty());
}

fn set(paths: &[&str]) -> BTreeSet<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

fn files(paths: &[&str]) -> Vec<String> {
    paths.iter().map(|p| p.to_string()).collect()
}

#[test]
fn github_list_missing_a_git_path_is_fine() {
    let git_set = set(&["docs/a.md", "tools/a.md", "docs/b.md"]);
    assert!(cross_check(&git_set, &files(&["docs/b.md"]), 1).is_ok());
    // A path spelled with a leading ./ or a doubled slash is the same path.
    assert!(cross_check(&git_set, &files(&["./docs/b.md", "docs//a.md"]), 2).is_ok());
}

#[test]
fn rename_is_not_a_mismatch() {
    // git (--no-renames) lists both sides; GitHub lists only the new one.
    let git_set = set(&["docs/a.md", "tools/a.md"]);
    assert!(cross_check(&git_set, &files(&["tools/a.md"]), 1).is_ok());
}

#[test]
fn github_path_not_in_git_set_is_a_mismatch() {
    let git_set = set(&["docs/a.md"]);
    let m = cross_check(&git_set, &files(&["tools/x.rs"]), 1).expect_err("mismatch");
    assert!(m.reason.contains("tools/x.rs"), "{}", m.reason);
    // A path that cannot be normalised is a mismatch too.
    let m = cross_check(&git_set, &files(&["../docs/a.md"]), 1).expect_err("mismatch");
    assert!(m.reason.contains("a .. segment"), "{}", m.reason);
}

#[test]
fn changed_files_100_or_more_is_a_mismatch() {
    let names: Vec<String> = (0..100).map(|i| format!("docs/{i}.md")).collect();
    let git_set: BTreeSet<String> = names.iter().cloned().collect();
    let m = cross_check(&git_set, &names, 100).expect_err("mismatch");
    assert!(m.reason.contains("100"), "{}", m.reason);
    let m = cross_check(&git_set, &names[..99], 835).expect_err("mismatch");
    assert!(m.reason.contains("835"), "{}", m.reason);
    // 99 is still checkable.
    assert!(cross_check(&git_set, &names[..99], 99).is_ok());
}

#[test]
fn changed_files_ne_len_is_a_mismatch() {
    let git_set = set(&["docs/a.md", "docs/b.md"]);
    let m = cross_check(&git_set, &files(&["docs/a.md"]), 2).expect_err("mismatch");
    assert!(m.reason.contains("listed 1"), "{}", m.reason);
    let m = cross_check(&git_set, &files(&["docs/a.md", "docs/b.md"]), 1).expect_err("mismatch");
    assert!(m.reason.contains("listed 2"), "{}", m.reason);
}
