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

/// Accepted divergence (the `links` and `markdown` module docs): Rust std's Unicode 18.0
/// case tables lowercase U+A7CE (assigned in Unicode 17) to U+A7CF, where CPython 3.14
/// (Unicode 16.0) leaves both unchanged. So `#\u{A7CF}` resolves against
/// `<a id="\u{A7CE}">` and `[[\u{A7CF}]]` against `docs/\u{A7CE}.md` here, where
/// check.py reports one `anchors` and one `wiki-links` finding.
#[test]
fn newer_unicode_case_pairs_match_in_anchors_and_wiki_links() {
    let t = TestRepo::with_files(
        "links-unicode18",
        &[
            ("docs/\u{A7CE}.md", "# T\n\n<a id=\"\u{A7CE}\"></a>\n"),
            (
                "docs/a.md",
                "# A\n\n[x](\u{A7CE}.md#\u{A7CF}) and [[\u{A7CF}]]\n",
            ),
        ],
    );
    let repo = open(&t);
    assert_eq!(Anchors.run(&repo).expect("the check runs"), vec![]);
    assert_eq!(WikiLinks.run(&repo).expect("the check runs"), vec![]);
}
