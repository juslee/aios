//! `aios hook path-guard`: edits under a denied prefix are denied, everything
//! else gets no decision, and an error is a deny.

mod hook_support;

use std::path::{Path, PathBuf};

use hook_support::{git, make_git_repo, outside_dir, run_hook, unique_dir, Run};
use serde_json::{json, Value};

/// A git repository with `kernel/src/` and `docs/` directories.
fn repo(label: &str) -> PathBuf {
    let dir = make_git_repo(label);
    std::fs::create_dir_all(dir.join("kernel/src")).expect("create kernel/src");
    std::fs::create_dir_all(dir.join("docs")).expect("create docs");
    std::fs::write(dir.join("kernel/src/lib.rs"), "\n").expect("write lib.rs");
    std::fs::write(dir.join("docs/a.md"), "\n").expect("write a.md");
    dir
}

fn guard(extra: &[&str], payload: &Value, cwd: &Path) -> Run {
    let mut args = vec!["path-guard", "--deny", "kernel/", "--deny", "shared/"];
    args.extend_from_slice(extra);
    let run = run_hook(&args, payload.to_string().as_bytes(), &[], cwd);
    assert_eq!(run.code, Some(0), "{}", run.stderr);
    run
}

fn edit(cwd: &Path, path: &str) -> Value {
    json!({
        "session_id": "s1",
        "cwd": cwd,
        "hook_event_name": "PreToolUse",
        "tool_name": "Edit",
        "tool_input": { "file_path": path, "old_string": "a", "new_string": "b" },
    })
}

fn deny_reason(run: &Run) -> String {
    let v: Value = serde_json::from_str(&run.stdout).expect("stdout is one JSON object");
    assert_eq!(v["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    assert_eq!(v["hookSpecificOutput"]["permissionDecision"], "deny");
    v["hookSpecificOutput"]["permissionDecisionReason"]
        .as_str()
        .expect("a reason")
        .to_string()
}

fn assert_no_decision(run: &Run) {
    assert_eq!(run.stdout, "", "{}", run.stderr);
}

#[test]
fn an_allowed_path_gets_no_decision() {
    let dir = repo("allowed");
    let path = dir.join("docs/a.md");
    assert_no_decision(&guard(&[], &edit(&dir, path.to_str().unwrap()), &dir));
}

#[test]
fn a_denied_prefix_is_denied_with_the_path_and_prefix_in_the_reason() {
    let dir = repo("denied");
    let path = dir.join("kernel/src/lib.rs");
    let run = guard(&[], &edit(&dir, path.to_str().unwrap()), &dir);
    let reason = deny_reason(&run);
    assert!(reason.contains("`kernel/src/lib.rs`"), "{reason}");
    assert!(reason.contains("`kernel/`"), "{reason}");
    assert!(reason.contains("kernel-dev"), "{reason}");
    assert_eq!(run.stderr, "");
}

#[test]
fn every_deny_prefix_applies() {
    let dir = repo("second-prefix");
    let run = guard(&[], &edit(&dir, "shared/src/new.rs"), &dir);
    assert!(deny_reason(&run).contains("`shared/`"));
}

#[test]
fn a_relative_path_is_joined_to_the_input_cwd() {
    let dir = repo("relative");
    let run = guard(&[], &edit(&dir, "kernel/src/lib.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
    // The same relative path from a subdirectory is another file.
    let sub = dir.join("docs");
    assert_no_decision(&guard(&[], &edit(&sub, "a.md"), &sub));
    let run = guard(&[], &edit(&sub, "../kernel/src/lib.rs"), &sub);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
}

#[test]
fn the_process_cwd_is_used_when_the_input_has_none() {
    let dir = repo("no-cwd");
    let payload = json!({
        "tool_name": "Write",
        "tool_input": { "file_path": "kernel/new.rs", "content": "" },
    });
    let run = guard(&[], &payload, &dir);
    assert!(deny_reason(&run).contains("`kernel/new.rs`"));
}

#[test]
fn dot_dot_traversal_into_a_denied_prefix_is_denied() {
    let dir = repo("traversal");
    let path = format!("{}/docs/../kernel/x.rs", dir.display());
    let run = guard(&[], &edit(&dir, &path), &dir);
    assert!(deny_reason(&run).contains("`kernel/x.rs`"));
    let run = guard(&[], &edit(&dir, "docs/../kernel/x.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/x.rs`"));
}

#[test]
fn dot_dot_out_of_a_denied_prefix_is_allowed() {
    let dir = repo("traversal-out");
    assert_no_decision(&guard(&[], &edit(&dir, "kernel/../docs/b.md"), &dir));
}

#[cfg(unix)]
#[test]
fn a_symlink_inside_the_repo_pointing_into_a_denied_prefix_is_denied() {
    use std::os::unix::fs::symlink;

    let dir = repo("symlink");
    symlink(dir.join("kernel/src"), dir.join("docs/linked")).expect("symlink a directory");
    let run = guard(&[], &edit(&dir, "docs/linked/lib.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
    // A file that does not exist yet beneath the link is still resolved.
    let run = guard(&[], &edit(&dir, "docs/linked/new.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/new.rs`"));
    // A symlink to a file.
    symlink(dir.join("kernel/src/lib.rs"), dir.join("docs/file-link")).expect("symlink a file");
    let run = guard(&[], &edit(&dir, "docs/file-link"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
}

#[cfg(unix)]
#[test]
fn a_dangling_symlink_into_a_denied_prefix_is_denied() {
    use std::os::unix::fs::symlink;

    let dir = repo("dangling");
    symlink("../kernel/src/not-yet.rs", dir.join("docs/dangling")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/dangling"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/not-yet.rs`"));
}

#[cfg(unix)]
#[test]
fn a_dangling_link_inside_a_symlinked_directory_resolves_against_the_real_directory() {
    use std::os::unix::fs::symlink;

    let dir = repo("dangling-nested");
    symlink(dir.join("kernel/src"), dir.join("docs/linked")).expect("symlink a directory");
    // The link physically lives in kernel/src, so `../x.rs` is kernel/x.rs.
    symlink("../x.rs", dir.join("kernel/src/dangling")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/linked/dangling"), &dir);
    assert!(deny_reason(&run).contains("`kernel/x.rs`"));
}

#[cfg(unix)]
#[test]
fn dot_dot_after_a_symlink_folds_against_the_link_target() {
    use std::os::unix::fs::symlink;

    // `docs/linked/..` is `kernel` (the link's target `kernel/src`, one up), not
    // `docs` (the link's lexical parent): the OS resolves the link first.
    let dir = repo("symlink-dotdot");
    symlink(dir.join("kernel/src"), dir.join("docs/linked")).expect("symlink a directory");
    let run = guard(&[], &edit(&dir, "docs/linked/../x.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/x.rs`"));
    let path = format!("{}/docs/linked/../x.rs", dir.display());
    let run = guard(&[], &edit(&dir, &path), &dir);
    assert!(deny_reason(&run).contains("`kernel/x.rs`"));
    // A relative link target folds the same way.
    symlink("../kernel/src", dir.join("docs/rel")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/rel/../y.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/y.rs`"));
    // The mirror image: a link out of a denied prefix, then `..`, lands outside it.
    symlink(dir.join("docs"), dir.join("kernel/to-docs")).expect("symlink");
    assert_no_decision(&guard(&[], &edit(&dir, "kernel/to-docs/../ok.md"), &dir));
}

#[cfg(unix)]
#[test]
fn dot_dot_after_a_missing_component_is_denied() {
    // The OS fails `new/../kernel/x.rs` with ENOENT when `new` does not exist, so
    // there is no file to decide on; the guard does not guess one.
    let dir = repo("missing-dotdot");
    let run = guard(&[], &edit(&dir, "docs/new/../../kernel/x.rs"), &dir);
    assert!(deny_reason(&run).contains("`..`"));
}

#[cfg(unix)]
#[test]
fn a_symlink_loop_fails_closed() {
    use std::os::unix::fs::symlink;

    let dir = repo("loop");
    symlink("b", dir.join("docs/a")).expect("symlink");
    symlink("a", dir.join("docs/b")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/a"), &dir);
    assert!(deny_reason(&run).contains("symlinks"));
}

#[cfg(unix)]
#[test]
fn the_symlink_budget_is_forty_links_in_total_per_path() {
    use std::os::unix::fs::symlink;

    // `docs/l1 -> l2 -> ... -> l40 -> kernel/src`: a path through the whole chain
    // passes 40 links, which is the budget. A per-link budget that restarts at each
    // target, or a boundary at 39, would deny it; one at 41 would let `l0` through.
    let dir = repo("link-budget");
    for n in 1..40 {
        symlink(format!("l{}", n + 1), dir.join(format!("docs/l{n}"))).expect("symlink");
    }
    symlink(dir.join("kernel/src"), dir.join("docs/l40")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/l1/new.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/new.rs`"));
    // One more link in front makes 41 on the same path.
    symlink("l1", dir.join("docs/l0")).expect("symlink");
    let run = guard(&[], &edit(&dir, "docs/l0/new.rs"), &dir);
    assert!(deny_reason(&run).contains("too many symlinks"));
    // The budget is per path, not shared: the 40-link path is still fine.
    let run = guard(&[], &edit(&dir, "docs/l1/other.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/src/other.rs`"));
}

#[cfg(unix)]
#[test]
fn a_symlink_out_of_a_denied_prefix_is_allowed() {
    use std::os::unix::fs::symlink;

    let dir = repo("symlink-out");
    symlink(dir.join("docs"), dir.join("kernel/to-docs")).expect("symlink");
    assert_no_decision(&guard(&[], &edit(&dir, "kernel/to-docs/a.md"), &dir));
}

#[test]
fn an_uppercase_path_is_denied_on_any_filesystem() {
    let dir = repo("case");
    let run = guard(&[], &edit(&dir, "Kernel/src/x.rs"), &dir);
    assert!(deny_reason(&run).contains("`kernel/"), "{}", run.stdout);
    let upper = dir.join("KERNEL/SRC/X.RS");
    let run = guard(&[], &edit(&dir, upper.to_str().unwrap()), &dir);
    assert!(deny_reason(&run)
        .to_ascii_lowercase()
        .contains("kernel/src/x.rs"));
}

#[test]
fn a_directory_that_merely_starts_with_the_prefix_name_is_allowed() {
    let dir = repo("lookalike");
    assert_no_decision(&guard(&[], &edit(&dir, "kernel-notes/a.md"), &dir));
    assert_no_decision(&guard(&[], &edit(&dir, "kernel.md"), &dir));
}

#[test]
fn a_path_outside_the_repo_gets_no_decision() {
    let dir = repo("inside");
    let outside = outside_dir("outside");
    std::fs::create_dir_all(outside.join("kernel")).expect("create kernel");
    let path = outside.join("kernel/x.rs");
    assert_no_decision(&guard(&[], &edit(&dir, path.to_str().unwrap()), &dir));
    // Even when the outside path is reached by climbing out of the repo: enough
    // `..` to reach the filesystem root, then down to the outside directory.
    let ups = "../".repeat(dir.components().count());
    let up = format!(
        "{}/{ups}{}/kernel/x.rs",
        dir.display(),
        outside.strip_prefix("/").unwrap().display()
    );
    assert_no_decision(&guard(&[], &edit(&dir, &up), &dir));
}

#[test]
fn a_root_that_does_not_contain_the_target_is_denied() {
    // `core.worktree` points git's answer at a directory the target is not in.
    let dir = repo("foreign-worktree");
    let elsewhere = unique_dir("foreign-root");
    git(
        &dir,
        &["config", "core.worktree", elsewhere.to_str().unwrap()],
    );
    let run = guard(&[], &edit(&dir, "kernel/src/lib.rs"), &dir);
    let reason = deny_reason(&run);
    assert!(reason.contains("root"), "{reason}");
    assert!(reason.contains("git"), "{reason}");
}

#[test]
fn a_worktree_is_guarded_by_its_own_root() {
    let dir = repo("worktree-main");
    git(&dir, &["add", "."]);
    git(&dir, &["commit", "-q", "-m", "tree"]);
    let wt = unique_dir("worktree-linked");
    let wt = wt.join("wt");
    git(
        &dir,
        &["worktree", "add", "-q", "-b", "other", wt.to_str().unwrap()],
    );
    let wt = std::fs::canonicalize(&wt).expect("canonicalize the worktree");
    let run = guard(&[], &edit(&wt, "kernel/src/lib.rs"), &wt);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
    assert_no_decision(&guard(&[], &edit(&wt, "docs/a.md"), &wt));
}

#[test]
fn tools_that_do_not_write_files_get_no_decision() {
    let dir = repo("other-tools");
    for tool in ["Read", "Bash", "Grep", "Agent"] {
        let payload = json!({
            "cwd": dir,
            "tool_name": tool,
            "tool_input": { "file_path": "kernel/src/lib.rs", "command": "touch kernel/x" },
        });
        assert_no_decision(&guard(&[], &payload, &dir));
    }
    assert_no_decision(&guard(&[], &json!({ "cwd": dir }), &dir));
}

#[test]
fn notebook_edit_uses_notebook_path() {
    let dir = repo("notebook");
    let payload = json!({
        "cwd": dir,
        "tool_name": "NotebookEdit",
        "tool_input": { "notebook_path": "kernel/n.ipynb", "new_source": "x" },
    });
    assert!(deny_reason(&guard(&[], &payload, &dir)).contains("`kernel/n.ipynb`"));
    let payload = json!({
        "cwd": dir,
        "tool_name": "NotebookEdit",
        "tool_input": { "notebook_path": "docs/n.ipynb", "new_source": "x" },
    });
    assert_no_decision(&guard(&[], &payload, &dir));
}

#[test]
fn multi_edit_and_write_are_checked_too() {
    let dir = repo("tools");
    for tool in ["Write", "MultiEdit"] {
        let payload = json!({
            "cwd": dir,
            "tool_name": tool,
            "tool_input": { "file_path": "kernel/x.rs" },
        });
        assert!(deny_reason(&guard(&[], &payload, &dir)).contains("`kernel/x.rs`"));
    }
}

#[test]
fn reason_text_is_appended_to_the_deny_reason() {
    let dir = repo("reason");
    let run = guard(
        &["--reason", "Ask team-lead if unsure."],
        &edit(&dir, "kernel/x.rs"),
        &dir,
    );
    let reason = deny_reason(&run);
    assert!(reason.contains("`kernel/x.rs`"), "{reason}");
    assert!(reason.ends_with(" Ask team-lead if unsure."), "{reason}");
    let plain = deny_reason(&guard(&[], &edit(&dir, "kernel/x.rs"), &dir));
    assert!(!plain.contains("Ask team-lead"), "{plain}");
}

#[test]
fn malformed_input_is_denied() {
    let cwd = unique_dir("malformed");
    for input in [&b"{not json"[..], b"", b"[1]", b"\xff"] {
        let run = run_hook(&["path-guard", "--deny", "kernel/"], input, &[], &cwd);
        assert_eq!(run.code, Some(0));
        assert!(deny_reason(&run).contains("path-guard"));
        assert!(run.stderr.contains("path-guard"), "{}", run.stderr);
    }
}

#[test]
fn a_checked_tool_without_a_path_is_denied() {
    let dir = repo("no-path");
    let payload = json!({ "cwd": dir, "tool_name": "Edit", "tool_input": {} });
    let run = guard(&[], &payload, &dir);
    assert!(deny_reason(&run).contains("file_path"));
    let payload = json!({ "cwd": dir, "tool_name": "Write", "tool_input": { "file_path": 7 } });
    assert!(deny_reason(&guard(&[], &payload, &dir)).contains("file_path"));
}

/// A main checkout with a linked worktree at `<main>/.claude/worktrees/x`, the
/// layout AIOS uses. Returns `(main, worktree)`, both canonical.
fn repo_with_nested_worktree(label: &str) -> (PathBuf, PathBuf) {
    let main = repo(label);
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "tree"]);
    let wt = main.join(".claude/worktrees/x");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "nested",
            wt.to_str().unwrap(),
        ],
    );
    let wt = std::fs::canonicalize(&wt).expect("canonicalize the worktree");
    (main, wt)
}

#[test]
fn a_target_in_no_repository_gets_no_decision() {
    // A plain directory with a `kernel/` in it and no repository around it.
    let plain = outside_dir("no-repo");
    std::fs::create_dir_all(plain.join("kernel")).expect("create kernel");
    assert_no_decision(&guard(&[], &edit(&plain, "kernel/x.rs"), &plain));
    // Absolute, and in a directory that does not exist yet.
    let path = plain.join("kernel/new/dir/x.rs");
    assert_no_decision(&guard(&[], &edit(&plain, path.to_str().unwrap()), &plain));
}

#[test]
fn a_dot_git_entry_git_cannot_use_denies_instead_of_skipping() {
    // An empty `.git` directory is not a repository, so git climbs past it and
    // reports "not a git repository"; the filesystem still shows a `.git` entry, so
    // the answer is not trusted.
    let plain = outside_dir("bad-dot-git");
    std::fs::create_dir_all(plain.join(".git")).expect("create an empty .git");
    std::fs::create_dir_all(plain.join("kernel")).expect("create kernel");
    let run = guard(&[], &edit(&plain, "kernel/x.rs"), &plain);
    let reason = deny_reason(&run);
    assert!(reason.contains("no repository"), "{reason}");
    assert!(reason.contains(".git"), "{reason}");
}

#[test]
fn a_git_failure_other_than_no_repository_is_denied() {
    // `.git` that is not a valid gitfile: git fails, but not with "not a git repository".
    let broken = outside_dir("broken-gitfile");
    std::fs::write(broken.join(".git"), "garbage\n").expect("write the gitfile");
    let run = guard(&[], &edit(&broken, "kernel/x.rs"), &broken);
    assert!(
        deny_reason(&run).contains("git rev-parse"),
        "{}",
        run.stdout
    );
}

#[test]
fn a_bare_repository_is_denied_not_skipped() {
    // git has no work tree to report for a bare repository ("this operation must be
    // run in a work tree"), which is neither "no repository" nor a root.
    let bare = outside_dir("bare");
    git(&bare, &["init", "-q", "--bare"]);
    let path = bare.join("kernel/x.rs");
    let run = guard(&[], &edit(&bare, path.to_str().unwrap()), &bare);
    let reason = deny_reason(&run);
    assert!(reason.contains("git rev-parse"), "{reason}");
    assert!(reason.contains("work tree"), "{reason}");
}

#[test]
fn a_missing_git_binary_is_denied_not_skipped() {
    // With no git to ask, "no repository" cannot be told from "unknown", and the
    // shim's promise is that a missing binary denies.
    let dir = repo("no-git");
    let empty = unique_dir("empty-path");
    let payload = edit(&dir, "kernel/src/lib.rs");
    for path in [empty.to_str().unwrap(), "/nonexistent"] {
        let run = run_hook(
            &["path-guard", "--deny", "kernel/"],
            payload.to_string().as_bytes(),
            &[("PATH", path)],
            &dir,
        );
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        let reason = deny_reason(&run);
        assert!(reason.contains("cannot run git"), "{path}: {reason}");
    }
}

#[test]
fn a_pruned_worktree_is_denied_not_skipped() {
    let (main, wt) = repo_with_nested_worktree("pruned");
    std::fs::remove_dir_all(main.join(".git/worktrees")).expect("remove the worktree admin dir");
    let run = guard(
        &[],
        &edit(&main, wt.join("kernel/src/lib.rs").to_str().unwrap()),
        &main,
    );
    assert!(
        deny_reason(&run).contains("git rev-parse"),
        "{}",
        run.stdout
    );
}

#[test]
fn an_inherited_git_ceiling_does_not_disable_the_guard() {
    // A ceiling at the repository root makes git say "not a git repository" for
    // every directory inside it; the guard clears it, so the edit is still denied.
    let main = repo("ceiling");
    let payload = edit(&main, "kernel/src/lib.rs");
    let args = ["path-guard", "--deny", "kernel/"];
    let root = main.to_str().unwrap();
    let parent = main.join("kernel").to_str().unwrap().to_string();
    for env in [
        vec![("GIT_CEILING_DIRECTORIES", root)],
        vec![("GIT_CEILING_DIRECTORIES", parent.as_str())],
        vec![
            ("GIT_CEILING_DIRECTORIES", root),
            ("GIT_DISCOVERY_ACROSS_FILESYSTEM", "0"),
        ],
    ] {
        let run = run_hook(&args, payload.to_string().as_bytes(), &env, &main);
        assert_eq!(run.code, Some(0), "{env:?}: {}", run.stderr);
        let reason = deny_reason(&run);
        assert!(reason.contains("`kernel/src/lib.rs`"), "{env:?}: {reason}");
    }
}

#[test]
fn a_nested_worktree_is_guarded_from_the_main_checkout_cwd() {
    // Worktrees live inside the main checkout, so with cwd at the main checkout
    // the root must come from the target for `kernel/` to match.
    let (main, wt) = repo_with_nested_worktree("nested");
    let path = wt.join("kernel/src/lib.rs");
    let run = guard(&[], &edit(&main, path.to_str().unwrap()), &main);
    let reason = deny_reason(&run);
    assert!(reason.contains("`kernel/src/lib.rs`"), "{reason}");
    assert!(!reason.contains(".claude"), "{reason}");
    // The same file in the main checkout is denied by the same rule.
    let run = guard(&[], &edit(&main, "kernel/src/lib.rs"), &main);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
    // A path the worktree allows stays allowed.
    let path = wt.join("docs/a.md");
    assert_no_decision(&guard(&[], &edit(&main, path.to_str().unwrap()), &main));
}

#[test]
fn a_nested_worktree_is_guarded_from_its_own_cwd_and_relative_paths() {
    let (main, wt) = repo_with_nested_worktree("nested-own");
    let run = guard(&[], &edit(&wt, "kernel/src/lib.rs"), &wt);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
    // A relative path from the main checkout reaches into the worktree.
    let run = guard(
        &[],
        &edit(&main, ".claude/worktrees/x/kernel/src/lib.rs"),
        &main,
    );
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
}

#[test]
fn a_new_file_in_a_missing_directory_of_a_nested_worktree_is_denied() {
    let (main, wt) = repo_with_nested_worktree("nested-new");
    let path = wt.join("kernel/newdir/sub/x.rs");
    let run = guard(&[], &edit(&main, path.to_str().unwrap()), &main);
    assert!(deny_reason(&run).contains("`kernel/newdir/sub/x.rs`"));
    let path = wt.join("docs/newdir/x.md");
    assert_no_decision(&guard(&[], &edit(&main, path.to_str().unwrap()), &main));
}

#[test]
fn ambient_git_location_variables_do_not_disable_the_guard() {
    // Not every row can fail: `rev-parse --show-toplevel` never reads the index, so
    // the `GIT_INDEX_FILE` row passes whether or not the guard removes it (it is
    // there to document the scrub list). The other rows were checked by dropping
    // each `env_remove` in turn: each makes this test fail. (`GIT_DISCOVERY_ACROSS_FILESYSTEM`
    // only matters at a mount boundary, which a test cannot make portably, so no
    // test covers it.)
    let main = repo("ambient-git");
    let git_dir = main.join(".git");
    let payload = edit(&main, "kernel/src/lib.rs");
    let args = ["path-guard", "--deny", "kernel/"];
    for env in [
        vec![("GIT_DIR", git_dir.to_str().unwrap())],
        vec![("GIT_WORK_TREE", "/")],
        vec![("GIT_COMMON_DIR", "/nonexistent")],
        vec![("GIT_OBJECT_DIRECTORY", "/nonexistent")],
        vec![("GIT_INDEX_FILE", "/nonexistent")],
        vec![
            ("GIT_DIR", git_dir.to_str().unwrap()),
            ("GIT_WORK_TREE", "/"),
        ],
    ] {
        let run = run_hook(&args, payload.to_string().as_bytes(), &env, &main);
        assert_eq!(run.code, Some(0), "{env:?}: {}", run.stderr);
        let reason = deny_reason(&run);
        assert!(reason.contains("`kernel/src/lib.rs`"), "{env:?}: {reason}");
    }
}

#[test]
fn a_gitfile_rewritten_to_move_the_root_up_is_denied() {
    // The worktree's `.git` file points at a fabricated git directory whose config
    // sets `core.worktree` to the main checkout: git then reports the main checkout
    // as the root, and the worktree's `kernel/` edit would match no prefix.
    let (main, wt) = repo_with_nested_worktree("gitfile-attack");
    let fake = wt.join("docs/fake.git");
    for dir in ["objects", "refs"] {
        std::fs::create_dir_all(fake.join(dir)).expect("create the fake git dir");
    }
    std::fs::write(fake.join("HEAD"), "ref: refs/heads/main\n").expect("write HEAD");
    std::fs::write(
        fake.join("config"),
        format!(
            "[core]\n\trepositoryformatversion = 0\n\tworktree = {}\n",
            main.display()
        ),
    )
    .expect("write the fake config");
    std::fs::write(wt.join(".git"), format!("gitdir: {}\n", fake.display()))
        .expect("rewrite the gitfile");
    let path = wt.join("kernel/src/lib.rs");
    let run = guard(&[], &edit(&wt, path.to_str().unwrap()), &wt);
    let reason = deny_reason(&run);
    assert!(reason.contains("root"), "{reason}");
    assert!(reason.contains("git"), "{reason}");
}

#[test]
fn git_metadata_is_denied_for_the_edit_tools() {
    let (main, wt) = repo_with_nested_worktree("git-metadata");
    // The gitfile of a linked worktree is named as git metadata.
    let run = guard(&[], &edit(&wt, ".git"), &wt);
    let reason = deny_reason(&run);
    assert!(reason.contains("`.git`"), "{reason}");
    assert!(reason.contains("git metadata"), "{reason}");
    // The git directory of the main checkout is denied too: by name where git can
    // be asked about its parent, and as an error where git refuses to run inside it.
    for path in [".git/config", ".GIT/hooks/pre-commit"] {
        deny_reason(&guard(&[], &edit(&main, path), &main));
    }
    // A name that only starts with `.git` is an ordinary file.
    assert_no_decision(&guard(&[], &edit(&main, ".gitignore"), &main));
    assert_no_decision(&guard(&[], &edit(&main, "docs/.gitkeep"), &main));
}

#[test]
fn a_dot_git_below_a_multi_component_prefix_cannot_move_the_root() {
    // With `--deny kernel/arch/`, a gitfile planted at `kernel/.git` would make git
    // report `kernel/` as the root, so `kernel/arch/boot.S` would become `arch/boot.S`
    // and match nothing. Writing the gitfile is what is denied.
    let main = repo("nested-gitfile");
    std::fs::create_dir_all(main.join("kernel/arch")).expect("create kernel/arch");
    let args = ["path-guard", "--deny", "kernel/arch/"];
    let git_dir = main.join(".git");
    let run = |payload: &Value| {
        let out = run_hook(&args, payload.to_string().as_bytes(), &[], &main);
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        out
    };
    let write = json!({
        "cwd": main,
        "tool_name": "Write",
        "tool_input": { "file_path": "kernel/.git", "content": format!("gitdir: {}\n", git_dir.display()) },
    });
    let reason = deny_reason(&run(&write));
    assert!(reason.contains("`kernel/.git`"), "{reason}");
    assert!(reason.contains("git metadata"), "{reason}");
    // A `.git` directory deeper down is denied by its component too.
    let deeper = edit(&main, "docs/sub/.git/config");
    assert!(deny_reason(&run(&deeper)).contains("git metadata"));
    // Nested-prefix behaviour is unchanged.
    let arch = edit(&main, "kernel/arch/boot.S");
    assert!(deny_reason(&run(&arch)).contains("`kernel/arch/`"));
    assert_no_decision(&run(&edit(&main, "kernel/other.rs")));
}

#[test]
fn a_sibling_worktree_outside_the_cwd_repository_is_guarded() {
    let main = repo("sibling-main");
    git(&main, &["add", "."]);
    git(&main, &["commit", "-q", "-m", "tree"]);
    let elsewhere = unique_dir("sibling-linked").join("wt");
    git(
        &main,
        &[
            "worktree",
            "add",
            "-q",
            "-b",
            "sibling",
            elsewhere.to_str().unwrap(),
        ],
    );
    let elsewhere = std::fs::canonicalize(&elsewhere).expect("canonicalize the worktree");
    // cwd is another, unrelated repository.
    let unrelated = repo("sibling-unrelated");
    let path = elsewhere.join("kernel/src/lib.rs");
    let run = guard(&[], &edit(&unrelated, path.to_str().unwrap()), &unrelated);
    assert!(deny_reason(&run).contains("`kernel/src/lib.rs`"));
}

#[test]
fn a_cwd_that_does_not_exist_is_denied() {
    let dir = repo("bad-cwd");
    let gone = dir.join("no/such/dir");
    let payload = edit(&gone, "kernel/x.rs");
    let run = guard(&[], &payload, &dir);
    assert!(deny_reason(&run).contains("path-guard"));
}

#[test]
fn a_deny_prefix_that_could_match_nothing_denies_every_checked_call() {
    let dir = repo("badprefix");
    let payload = edit(&dir, "docs/a.md");
    for prefix in ["kernel//", "./kernel/./", "docs/../kernel/"] {
        let run = run_hook(
            &["path-guard", "--deny", prefix],
            payload.to_string().as_bytes(),
            &[],
            &dir,
        );
        assert_eq!(run.code, Some(0), "{}", run.stderr);
        assert!(
            deny_reason(&run).contains("--deny"),
            "{prefix}: {}",
            run.stdout
        );
    }
}

#[test]
fn at_least_one_deny_is_required() {
    let dir = repo("usage");
    let run = run_hook(&["path-guard"], b"{}", &[], &dir);
    assert_eq!(run.code, Some(2));
    assert_eq!(run.stdout, "");
    assert!(run.stderr.contains("--deny"), "{}", run.stderr);
}

#[test]
fn a_bad_deny_prefix_denies_a_checked_call() {
    let dir = repo("bad-prefix");
    let payload = edit(&dir, "docs/a.md");
    let run = run_hook(
        &["path-guard", "--deny", "/abs/"],
        payload.to_string().as_bytes(),
        &[],
        &dir,
    );
    assert!(deny_reason(&run).contains("--deny"));
}

/// `edit` plus the subagent fields a payload from inside an agent carries.
fn edit_by(cwd: &Path, path: &str, agent_type: Option<Value>, agent_id: Option<&str>) -> Value {
    let mut v = edit(cwd, path);
    if let Some(t) = agent_type {
        v["agent_type"] = t;
    }
    if let Some(id) = agent_id {
        v["agent_id"] = json!(id);
    }
    v
}

const WORKER: &[&str] = &["--agent-type", "worker"];

#[test]
fn the_named_agent_type_is_guarded() {
    let dir = repo("at-worker");
    let p = dir.join("kernel/src/lib.rs");
    let run = guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), Some("a1")),
        &dir,
    );
    assert!(deny_reason(&run).contains("kernel/src/lib.rs"));
}

#[test]
fn another_agent_type_gets_no_decision() {
    let dir = repo("at-kernel-dev");
    let p = dir.join("kernel/src/lib.rs");
    assert_no_decision(&guard(
        WORKER,
        &edit_by(
            &dir,
            p.to_str().unwrap(),
            Some(json!("kernel-dev")),
            Some("a1"),
        ),
        &dir,
    ));
}

#[test]
fn the_main_thread_gets_no_decision() {
    let dir = repo("at-main");
    let p = dir.join("kernel/src/lib.rs");
    assert_no_decision(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), None, None),
        &dir,
    ));
}

#[test]
fn a_subagent_without_an_agent_type_is_denied_anywhere() {
    let dir = repo("at-unidentified");
    let p = dir.join("docs/a.md");
    let reason = deny_reason(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), None, Some("a1")),
        &dir,
    ));
    assert!(reason.contains("agent_type"), "{reason}");
}

#[test]
fn agent_types_are_exact_names() {
    let dir = repo("at-case");
    let p = dir.join("kernel/src/lib.rs");
    assert_no_decision(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!("Worker")), Some("a1")),
        &dir,
    ));
}

#[test]
fn the_named_agent_type_may_edit_outside_the_prefixes() {
    let dir = repo("at-docs");
    let p = dir.join("docs/a.md");
    assert_no_decision(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), Some("a1")),
        &dir,
    ));
}

#[test]
fn the_filter_runs_before_path_resolution() {
    let dir = repo("at-before-resolve");
    let mut payload = edit_by(
        &dir,
        "kernel/src/lib.rs",
        Some(json!("kernel-dev")),
        Some("a1"),
    );
    payload["cwd"] = json!(dir.join("no-such-dir"));
    assert_no_decision(&guard(WORKER, &payload, &dir));
}

#[test]
fn an_empty_agent_type_flag_denies_every_checked_call() {
    let dir = repo("at-empty-flag");
    let p = dir.join("docs/a.md");
    let reason = deny_reason(&guard(
        &["--agent-type", " "],
        &edit_by(
            &dir,
            p.to_str().unwrap(),
            Some(json!("kernel-dev")),
            Some("a1"),
        ),
        &dir,
    ));
    assert!(reason.contains("--agent-type"), "{reason}");
}

#[test]
fn every_named_agent_type_is_guarded() {
    let dir = repo("at-two");
    let p = dir.join("kernel/src/lib.rs");
    let flags = &["--agent-type", "worker", "--agent-type", "doc-writer"];
    deny_reason(&guard(
        flags,
        &edit_by(
            &dir,
            p.to_str().unwrap(),
            Some(json!("doc-writer")),
            Some("a1"),
        ),
        &dir,
    ));
}

#[test]
fn a_wrong_typed_agent_type_counts_as_absent() {
    let dir = repo("at-number");
    let p = dir.join("docs/a.md");
    deny_reason(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!(7)), Some("a1")),
        &dir,
    ));
}

#[test]
fn a_session_launched_as_the_agent_is_guarded() {
    let dir = repo("at-dash-agent");
    let p = dir.join("kernel/src/lib.rs");
    deny_reason(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!("worker")), None),
        &dir,
    ));
}

#[test]
fn unchecked_tools_are_filtered_before_the_agent() {
    let dir = repo("at-read");
    let mut payload = edit_by(
        &dir,
        dir.join("kernel/src/lib.rs").to_str().unwrap(),
        Some(json!("worker")),
        Some("a1"),
    );
    payload["tool_name"] = json!("Read");
    assert_no_decision(&guard(WORKER, &payload, &dir));
}

#[test]
fn an_empty_agent_type_with_an_agent_id_is_denied() {
    let dir = repo("at-empty-type");
    let p = dir.join("docs/a.md");
    deny_reason(&guard(
        WORKER,
        &edit_by(&dir, p.to_str().unwrap(), Some(json!("")), Some("a1")),
        &dir,
    ));
}

/// Attacks on the identity fields themselves: a payload that names a subagent in
/// any shape must not be taken for the main thread.
#[test]
fn an_unusable_agent_id_does_not_pass_for_the_main_thread() {
    let dir = repo("at-bad-id");
    let p = dir.join("kernel/src/lib.rs");
    for id in [json!(7), json!(""), json!(null), json!(["a1"]), json!({})] {
        let mut payload = edit_by(&dir, p.to_str().unwrap(), None, None);
        payload["agent_id"] = id.clone();
        let reason = deny_reason(&guard(WORKER, &payload, &dir));
        assert!(reason.contains("agent_type"), "agent_id {id}: {reason}");
    }
}

#[test]
fn an_unusable_agent_type_is_denied_even_without_an_agent_id() {
    let dir = repo("at-bad-type");
    let p = dir.join("docs/a.md");
    for t in [
        json!(7),
        json!(""),
        json!(null),
        json!(["worker"]),
        json!({"name": "worker"}),
        json!(true),
    ] {
        let payload = edit_by(&dir, p.to_str().unwrap(), Some(t.clone()), None);
        let reason = deny_reason(&guard(WORKER, &payload, &dir));
        assert!(reason.contains("agent_type"), "agent_type {t}: {reason}");
    }
}

#[test]
fn a_named_other_type_stays_free_whatever_its_agent_id_looks_like() {
    let dir = repo("at-other-bad-id");
    let p = dir.join("kernel/src/lib.rs");
    let mut payload = edit_by(&dir, p.to_str().unwrap(), Some(json!("kernel-dev")), None);
    payload["agent_id"] = json!(7);
    assert_no_decision(&guard(WORKER, &payload, &dir));
}

#[test]
fn without_the_flag_every_caller_is_still_checked() {
    let dir = repo("at-no-flag");
    let p = dir.join("kernel/src/lib.rs");
    let payload = edit_by(
        &dir,
        p.to_str().unwrap(),
        Some(json!("kernel-dev")),
        Some("a1"),
    );
    deny_reason(&guard(&[], &payload, &dir));
}
