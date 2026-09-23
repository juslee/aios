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
