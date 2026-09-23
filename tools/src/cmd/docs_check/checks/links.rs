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
