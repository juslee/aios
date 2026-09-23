//! Repository model of `aios docs-check`: check.py `Repo` (L372-526) and `slug_set`
//! (L588-604) at 33c6b3d.
//!
//! Files come from `git ls-files` (tracked plus untracked-but-not-ignored), never
//! from a filesystem walk, so linked worktrees and build output are never scanned.
//! File text is lossy UTF-8 with universal newlines (Python text mode); git output
//! is strict UTF-8 with universal newlines (Python `text=True`). Texts, headings,
//! slugs and the merged milestones are cached per `Repo`.
//!
//! Accepted divergences (contract §1.9): `\d` is `[0-9]` in `PHASE_SUBJECT_RE`,
//! `PHASE_DOC_RE` and `MILESTONE_HEADING_RE`, so a phase, milestone or recipe
//! number written with a non-ASCII Unicode decimal digit does not match here,
//! where Python's `\d` (and `int()`) would; `\b` (`MILESTONE_HEADING_RE`,
//! `PRIVATE_ATTR_RE`, `RECIPE_RE`) and `\s` (`ANCHOR_ID_RE`) use the `regex`
//! crate's Unicode word/space classes, differing from Python's `re` at the same
//! edges markdown.rs documents. A `rel` with an embedded NUL makes `exists`
//! treat the unresolvable component as simply missing, where CPython's
//! `os.path.realpath` raises `ValueError` and check.py exits 2; non-UTF-8 `git`
//! stderr is decoded lossily here, where CPython's `subprocess.run(text=True)`
//! raises `UnicodeDecodeError` for either stream regardless of the command's
//! exit status.

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

/// R19 (check.py L458).
static PHASE_SUBJECT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Phase ([0-9]+) M([0-9]+):").expect("valid regex"));
/// R20 (check.py L467). `\n?$` matches Python's non-MULTILINE `$` on a
/// tracked path ending in a trailing newline (fix round 1).
static PHASE_DOC_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^docs/phases/([0-9]+)-[^/]+\.md\n?$").expect("valid regex"));
/// R21 (check.py L478, re.match).
static MILESTONE_HEADING_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^Milestone ([0-9]+)\b").expect("valid regex"));
/// R22 (check.py L513).
static PRIVATE_ATTR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[.*\bprivate\b.*\]").expect("valid regex"));
/// R24 without its negative lookahead (see `recipe_name`).
static RECIPE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^@?([A-Za-z_][A-Za-z0-9_-]*)\b[^:=]*:").expect("valid regex"));
/// R26 (check.py L602).
static ANCHOR_ID_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<a\s+(?:name|id)="([^"]+)""#).expect("valid regex"));

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
    ///
    /// No check in this crate calls it yet; it is part of `Repo`'s public API
    /// for later subcommands (R2 consumes it).
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
        // Forcing each LazyLock runs its Regex::new(...).expect("valid regex"): a
        // bad pattern panics here, at test time, rather than in production.
        for rx in all {
            LazyLock::force(rx);
        }
    }

    #[test]
    fn phase_doc_re_matches_a_trailing_newline_path() {
        // Python: re.match(r"^docs/phases/(\d+)-[^/]+\.md$",
        //   "docs/phases/05-x.md\n").group(1) == "05" (non-MULTILINE $ matches
        // just before a trailing newline too).
        let caps = PHASE_DOC_RE
            .captures("docs/phases/05-x.md\n")
            .expect("a trailing-\\n path matches, as check.py's non-MULTILINE $ does");
        assert_eq!(&caps[1], "05");
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
