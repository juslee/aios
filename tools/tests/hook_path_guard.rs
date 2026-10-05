//! `aios hook path-guard`: edits under a denied prefix are denied, everything
//! else gets no decision, and an error is a deny.

mod hook_support;

use std::path::{Path, PathBuf};

use hook_support::{git, make_git_repo, run_hook, unique_dir, Run};
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
    let outside = unique_dir("outside");
    std::fs::create_dir_all(outside.join("kernel")).expect("create kernel");
    let path = outside.join("kernel/x.rs");
    assert_no_decision(&guard(&[], &edit(&dir, path.to_str().unwrap()), &dir));
    // Even when the outside path is reached by climbing out of the repo.
    let up = format!("{}/../{}/kernel/x.rs", dir.display(), "elsewhere");
    assert_no_decision(&guard(&[], &edit(&dir, &up), &dir));
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

#[test]
fn a_git_failure_is_denied() {
    // A directory with no repository around it.
    let bare = unique_dir("no-repo");
    let payload = edit(&bare, "docs/a.md");
    let run = run_hook(
        &["path-guard", "--deny", "kernel/"],
        payload.to_string().as_bytes(),
        &[(
            "GIT_CEILING_DIRECTORIES",
            bare.parent().unwrap().to_str().unwrap(),
        )],
        &bare,
    );
    assert_eq!(run.code, Some(0));
    assert!(
        deny_reason(&run).contains("git rev-parse"),
        "{}",
        run.stdout
    );
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
