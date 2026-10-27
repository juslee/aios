//! The loop's git calls (R2a, Task 4a): the changed-paths list, the PR-head
//! fetch and the round worktree, against real repositories.

mod common;

use aios_tools::cmd::pr_loop::git::{
    changed_paths, fetch_pr_head, git_in, merge_base, worktree_add_detached, worktree_discard,
    ChangedPath,
};
use common::{git, unique_dir, TestRepo};
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

fn sha(repo: &TestRepo, rev: &str) -> String {
    git(repo.path(), &["rev-parse", rev]).trim().to_string()
}

fn listed(repo: &TestRepo) -> String {
    git(repo.path(), &["worktree", "list", "--porcelain"])
}

/// A directory to hold round worktrees, apart from the repository.
fn rounds_dir() -> PathBuf {
    unique_dir("loop-rounds")
}

fn cp(status: char, path: &str) -> ChangedPath {
    ChangedPath {
        status,
        path: PathBuf::from(path),
    }
}

#[test]
fn changed_paths_lists_both_sides_of_a_rename() {
    let repo = TestRepo::with_files(
        "loop-git-rename",
        &[("docs/a.md", "hello\n".repeat(50).as_str())],
    );
    std::fs::create_dir_all(repo.path().join("tools")).expect("create tools/");
    git(repo.path(), &["mv", "docs/a.md", "tools/a.md"]);
    repo.commit("rename");
    let got = changed_paths(repo.path(), "HEAD~1", "HEAD").expect("changed paths");
    assert_eq!(got, [cp('D', "docs/a.md"), cp('A', "tools/a.md")]);
}

#[test]
fn changed_paths_keeps_unusual_names_whole() {
    let repo = TestRepo::with_files("loop-git-names", &[("seed", "x\n")]);
    repo.write("docs/ünï cödé.md", "x\n");
    repo.write("docs/with space.md", "x\n");
    repo.commit("odd names");
    let got = changed_paths(repo.path(), "HEAD~1", "HEAD").expect("changed paths");
    assert_eq!(
        got,
        [cp('A', "docs/with space.md"), cp('A', "docs/ünï cödé.md")]
    );
}

#[test]
fn changed_paths_against_the_merge_base_excludes_a_merge_of_main() {
    let repo = TestRepo::with_files(
        "loop-git-merge",
        &[("docs/x.md", "x\n"), ("Cargo.lock", "1\n")],
    );
    git(repo.path(), &["checkout", "-q", "-b", "feature"]);
    repo.write("docs/feature.md", "f\n");
    repo.commit("feature");
    git(repo.path(), &["checkout", "-q", "main"]);
    repo.write("Cargo.lock", "2\n");
    repo.commit("main moves");
    git(repo.path(), &["checkout", "-q", "feature"]);
    git(
        repo.path(),
        &["merge", "-q", "--no-ff", "-m", "merge main", "main"],
    );

    let base = merge_base(repo.path(), "main", "feature").expect("merge base");
    assert_eq!(base, sha(&repo, "main"));
    let got = changed_paths(repo.path(), &base, "feature").expect("changed paths");
    assert_eq!(got, [cp('A', "docs/feature.md")]);
}

#[test]
fn detached_worktree_round_trip() {
    let repo = TestRepo::with_files("loop-git-wt", &[("a.txt", "a\n")]);
    let head = sha(&repo, "HEAD");
    let path = rounds_dir().join("loop-pr7");
    worktree_add_detached(repo.path(), &path, &head).expect("add");
    assert!(path.join("a.txt").is_file());
    assert_eq!(git(&path, &["rev-parse", "HEAD"]).trim(), head);
    assert_eq!(
        git(&path, &["rev-parse", "--abbrev-ref", "HEAD"]).trim(),
        "HEAD"
    );
    assert!(listed(&repo).contains("loop-pr7"));

    worktree_discard(repo.path(), &path, std::slice::from_ref(&path)).expect("discard");
    assert!(!path.exists());
    assert!(!listed(&repo).contains("loop-pr7"));
}

#[test]
fn dirty_round_worktree_is_cleaned_then_removed() {
    let repo = TestRepo::with_files("loop-git-dirty", &[("a.txt", "a\n")]);
    let path = rounds_dir().join("loop-pr8");
    worktree_add_detached(repo.path(), &path, &sha(&repo, "HEAD")).expect("add");
    std::fs::write(path.join("a.txt"), "modified\n").expect("modify");
    std::fs::write(path.join("untracked.txt"), "u\n").expect("untracked");
    std::fs::create_dir_all(path.join("sub")).expect("subdir");
    std::fs::write(path.join("sub/deep.txt"), "u\n").expect("untracked dir");

    worktree_discard(repo.path(), &path, std::slice::from_ref(&path)).expect("discard");
    assert!(!path.exists());
    assert!(!listed(&repo).contains("loop-pr8"), "{}", listed(&repo));
}

#[test]
fn unrecorded_or_misnamed_path_is_refused() {
    let repo = TestRepo::with_files("loop-git-refuse", &[("a.txt", "a\n")]);
    let head = sha(&repo, "HEAD");
    let dir = rounds_dir();

    // Not recorded.
    let unrecorded = dir.join("loop-pr1");
    worktree_add_detached(repo.path(), &unrecorded, &head).expect("add");
    let err = worktree_discard(repo.path(), &unrecorded, &[]).expect_err("unrecorded");
    assert!(err.to_string().contains("not recorded"), "{err}");
    assert!(unrecorded.is_dir());

    // Recorded but misnamed.
    for name in ["wt1", "loop-pr", "loop-prx", "loop-pr1x", "xloop-pr1"] {
        let misnamed = dir.join(name);
        worktree_add_detached(repo.path(), &misnamed, &head).expect("add");
        let err = worktree_discard(repo.path(), &misnamed, std::slice::from_ref(&misnamed))
            .expect_err("misnamed");
        assert!(err.to_string().contains("not named"), "{name}: {err}");
        assert!(misnamed.is_dir(), "{name}");
    }

    // Recorded and named right, but not a worktree of this repository.
    let plain = dir.join("loop-pr2");
    std::fs::create_dir_all(&plain).expect("plain dir");
    std::fs::write(plain.join("keep.txt"), "k\n").expect("file");
    let err = worktree_discard(repo.path(), &plain, std::slice::from_ref(&plain))
        .expect_err("not listed");
    assert!(err.to_string().contains("not a listed worktree"), "{err}");
    assert!(plain.join("keep.txt").is_file());
}

#[test]
fn missing_round_worktree_is_pruned() {
    let repo = TestRepo::with_files("loop-git-prune", &[("a.txt", "a\n")]);
    let path = rounds_dir().join("loop-pr9");
    worktree_add_detached(repo.path(), &path, &sha(&repo, "HEAD")).expect("add");
    std::fs::remove_dir_all(&path).expect("remove the directory behind git's back");
    assert!(listed(&repo).contains("loop-pr9"));

    worktree_discard(repo.path(), &path, std::slice::from_ref(&path)).expect("discard");
    assert!(!listed(&repo).contains("loop-pr9"), "{}", listed(&repo));
}

#[test]
fn git_commands_set_the_hardened_env() {
    let cmd = git_in(Path::new("."));
    let env = |name: &str| {
        cmd.get_envs()
            .find(|(k, _)| *k == OsStr::new(name))
            .and_then(|(_, v)| v)
            .map(|v| v.to_string_lossy().into_owned())
    };
    assert_eq!(env("LC_ALL").as_deref(), Some("C"));
    assert_eq!(env("GIT_NO_REPLACE_OBJECTS").as_deref(), Some("1"));
    assert_eq!(
        env("GIT_GRAFT_FILE").as_deref(),
        Some("/dev/null/no-grafts")
    );
    assert_eq!(env("GIT_TERMINAL_PROMPT").as_deref(), Some("0"));
}

#[test]
fn fetch_pr_head_reads_refs_pull() {
    let origin = unique_dir("loop-git-origin");
    git(&origin, &["init", "-q", "--bare", "-b", "main"]);
    let repo = TestRepo::with_files("loop-git-fetch", &[("a.txt", "a\n")]);
    git(
        repo.path(),
        &["remote", "add", "origin", origin.to_str().expect("utf-8")],
    );
    git(repo.path(), &["push", "-q", "origin", "main"]);
    git(repo.path(), &["checkout", "-q", "-b", "pr"]);
    repo.write("b.txt", "b\n");
    repo.commit("the PR");
    let pr_head = sha(&repo, "HEAD");
    git(
        repo.path(),
        &["push", "-q", "origin", "HEAD:refs/pull/1/head"],
    );
    git(repo.path(), &["checkout", "-q", "main"]);
    git(repo.path(), &["branch", "-q", "-D", "pr"]);

    assert_eq!(fetch_pr_head(repo.path(), 1).expect("fetch"), pr_head);
    let err = fetch_pr_head(repo.path(), 2).expect_err("no such PR");
    assert!(err.to_string().contains("fetch failed"), "{err}");
    let _ = std::fs::remove_dir_all(&origin);
}
