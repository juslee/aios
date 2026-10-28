//! The git level (git-push-guard.py L2055-2415, with `scan_git_options`
//! L879-913, the push helpers L1126-1185, `remote_is_local` and
//! `glob_hits_protected` L2606-2633): global options and aliases, the options of
//! git subcommands that discard work or run commands, what an earlier command
//! changes for a later push, and where a push goes.

use std::sync::LazyLock;

use regex::Regex;

use super::analyzer::{partition, unresolved, Analyzer, Flow, ARG_WRAPPERS};
use super::pattern::re;
use super::pyos::{self, Fault};
use super::repo::Repo;
use super::shell::{Command, MAX_DEPTH};
use super::shlex;
use super::verdict::{self, Level};
use crate::pystr;

pub const PROTECTED_BRANCH: &str = "main";
pub const AGENT_PREFIX: &str = "claude/";

/// Environment variables that do not change which repository, config or binary
/// git uses (`HARMLESS_GIT_ENV`).
const HARMLESS_GIT_ENV: [&str; 17] = [
    "GIT_PAGER",
    "GIT_TERMINAL_PROMPT",
    "GIT_OPTIONAL_LOCKS",
    "GIT_EDITOR",
    "GIT_SEQUENCE_EDITOR",
    "GIT_MERGE_AUTOEDIT",
    "GIT_MERGE_VERBOSITY",
    "GIT_AUTHOR_NAME",
    "GIT_AUTHOR_EMAIL",
    "GIT_AUTHOR_DATE",
    "GIT_COMMITTER_NAME",
    "GIT_COMMITTER_EMAIL",
    "GIT_COMMITTER_DATE",
    "GIT_PROGRESS_DELAY",
    "GIT_FLUSH",
    "GIT_REDACT_COOKIES",
    "GIT_ADVICE",
];
pub const SHARED_RISKY_ENV: [&str; 3] = ["HOME", "XDG_CONFIG_HOME", "PATH"];

/// Builtins never shadowed by aliases; anything else gets an alias lookup.
const GIT_BUILTINS: &str = "
add am annotate apply archive backfill bisect blame branch bugreport bundle
cat-file check-attr check-ignore check-mailmap check-ref-format checkout
checkout-index cherry cherry-pick citool clean clone column commit
commit-graph commit-tree config count-objects credential describe diff
diff-files diff-index diff-tree difftool fast-export fast-import fetch
fetch-pack filter-branch fmt-merge-msg for-each-ref for-each-repo
format-patch fsck gc get-tar-commit-id grep gui hash-object help
hook index-pack init instaweb interpret-trailers log ls-files ls-remote
ls-tree mailinfo mailsplit maintenance merge merge-base merge-file
merge-index merge-tree mergetool mktag mktree multi-pack-index mv
name-rev notes pack-objects pack-refs patch-id prune prune-packed pull
range-diff read-tree rebase reflog refs remote repack replace replay
request-pull rerere reset restore rev-list rev-parse revert rm
send-email shortlog show show-branch show-index show-ref sparse-checkout
stash status stripspace submodule switch symbolic-ref tag unpack-file
unpack-objects update-index update-ref update-server-info var verify-commit
verify-pack verify-tag version whatchanged worktree write-tree lfs
";

/// Git subcommands that neither change refs, config nor the working tree.
const GIT_READ_ONLY: &str = "
blame cat-file check-attr check-ignore check-mailmap check-ref-format cherry
count-objects describe diff diff-files diff-index diff-tree fetch
for-each-ref fsck grep help log ls-files ls-remote ls-tree merge-base
name-rev range-diff rev-list rev-parse shortlog show show-branch show-ref
status var verify-commit verify-pack verify-tag version whatchanged
";

/// Git subcommands that can change which branch is checked out.
const GIT_HEAD_CHANGERS: [&str; 8] = [
    "checkout",
    "switch",
    "symbolic-ref",
    "update-ref",
    "bisect",
    "rebase",
    "worktree",
    "branch",
];

const PUSH_LONG_OPTS: [&str; 27] = [
    "all",
    "branches",
    "mirror",
    "delete",
    "tags",
    "follow-tags",
    "dry-run",
    "porcelain",
    "force",
    "force-with-lease",
    "force-if-includes",
    "repo",
    "set-upstream",
    "thin",
    "quiet",
    "verbose",
    "progress",
    "recurse-submodules",
    "verify",
    "atomic",
    "push-option",
    "receive-pack",
    "exec",
    "prune",
    "signed",
    "ipv4",
    "ipv6",
];
const PUSH_OPTS_WITH_ARG: [&str; 5] = [
    "repo",
    "push-option",
    "receive-pack",
    "exec",
    "recurse-submodules",
];
const GIT_GLOBAL_WITH_ARG: [&str; 6] = [
    "--git-dir",
    "--work-tree",
    "--namespace",
    "--super-prefix",
    "--config-env",
    "--attr-source",
];

fn word_in(list: &str, word: &str) -> bool {
    list.split_whitespace().any(|w| w == word)
}

/// An option `scan_git_options` found: its name (`force`, or `-f` for a short
/// one) and its value, `None` when it takes none or the value is missing.
pub type Hit = (String, Option<String>);

/// `scan_git_options`: options the way git's parse-options reads them, up to
/// `--`. A long option matches every name in `longs` (name, takes a value) it is
/// a non-empty prefix of, so an abbreviation git accepts is never missed. A short
/// option can sit anywhere in a cluster; a letter in `takes_arg` or
/// `optional_arg` ends the cluster, because the rest of it is that letter's value.
pub fn scan_git_options(
    args: &[String],
    longs: &[(&str, bool)],
    shorts: &str,
    takes_arg: &str,
    optional_arg: &str,
) -> Vec<Hit> {
    let mut hits = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--" {
            break;
        }
        let nxt = args.get(i + 1);
        if a.starts_with("--") && a.chars().count() > 2 {
            let (name, eq, value) = partition(&a[2..], '=');
            if name.is_empty() || name.starts_with("no-") {
                continue;
            }
            for &(full, wants) in longs {
                if full.starts_with(name) {
                    let v = if eq {
                        Some(value.to_string())
                    } else if wants {
                        nxt.cloned()
                    } else {
                        None
                    };
                    hits.push((full.to_string(), v));
                }
            }
        } else if a.starts_with('-') && a.chars().count() > 1 {
            let cluster: Vec<char> = a.chars().skip(1).collect();
            for (pos, &ch) in cluster.iter().enumerate() {
                if shorts.contains(ch) {
                    let mut value = None;
                    if takes_arg.contains(ch) {
                        let rest: String = cluster[pos + 1..].iter().collect();
                        value = if rest.is_empty() {
                            nxt.cloned()
                        } else {
                            Some(rest)
                        };
                    }
                    hits.push((format!("-{ch}"), value));
                }
                if takes_arg.contains(ch) || optional_arg.contains(ch) {
                    break;
                }
            }
        }
    }
    hits
}

/// `is_protected`: `main`, `HEAD` or `@`, in any case.
pub fn is_protected(name: &str) -> bool {
    let lower = name.to_lowercase();
    [PROTECTED_BRANCH, "head", "@"].contains(&lower.as_str())
}

fn squeeze_slashes(s: &str) -> String {
    static SLASHES: LazyLock<Regex> = LazyLock::new(|| re("/+"));
    SLASHES.replace_all(s, "/").into_owned()
}

/// `dst_candidates`: branch names on the remote that a destination can
/// resolve to.
pub fn dst_candidates(dst: &str) -> Vec<String> {
    let d = squeeze_slashes(dst);
    if let Some(rest) = d.strip_prefix("refs/") {
        return rest
            .strip_prefix("heads/")
            .map(|b| vec![b.to_string()])
            .unwrap_or_default();
    }
    let mut names = vec![d.clone()];
    if let Some(b) = d.strip_prefix("heads/") {
        names.push(b.to_string());
    }
    names
}

/// `src_branch`: the remote branch name a `<src>` refspec (no colon) updates.
pub fn src_branch(src: &str) -> Option<String> {
    let d = squeeze_slashes(src);
    if let Some(rest) = d.strip_prefix("refs/") {
        return rest.strip_prefix("heads/").map(str::to_string);
    }
    Some(d.strip_prefix("heads/").unwrap_or(&d).to_string())
}

/// `resolve_long`: git parse-options' exact match or unique prefix, or `None`
/// for an unknown or ambiguous name.
fn resolve_long(name: &str) -> Option<&'static str> {
    if let Some(&opt) = PUSH_LONG_OPTS.iter().find(|&&o| o == name) {
        return Some(opt);
    }
    let hits: Vec<&'static str> = PUSH_LONG_OPTS
        .iter()
        .copied()
        .filter(|o| o.starts_with(name))
        .collect();
    (hits.len() == 1).then(|| hits[0])
}

/// `git_env_risky`: a variable that can change which repository, config or
/// binary git uses.
pub fn git_env_risky(name: &str) -> bool {
    if SHARED_RISKY_ENV.contains(&name) {
        return true;
    }
    name.starts_with("GIT_") && !HARMLESS_GIT_ENV.contains(&name) && !name.starts_with("GIT_TRACE")
}

/// `opt_label`: an option name as typed (`-f`) or with its `--`.
fn opt_label(name: &str) -> String {
    if name.starts_with('-') {
        name.to_string()
    } else {
        format!("--{name}")
    }
}

/// `remote_is_local`: true when every URL the push goes to is a local path, so
/// it cannot reach GitHub. Unknown state (not a repo yet, URL rewriting) is
/// remote.
pub fn remote_is_local(repo: &Repo, remote: Option<&str>) -> Result<bool, Fault> {
    static REWRITE: LazyLock<Regex> =
        LazyLock::new(|| re(r"^url\..+\.(?:insteadof|pushinsteadof)$"));
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Za-z][A-Za-z0-9+.-]*::?"));
    static SCP: LazyLock<Regex> = LazyLock::new(|| re(r"^[^/]*:"));
    if !repo.is_repo()? {
        return Ok(false);
    }
    let config = repo.config()?;
    if config.keys().any(|k| REWRITE.is_match(k)) {
        return Ok(false);
    }
    let remote = match remote {
        Some(r) => r.to_string(),
        None => {
            let cur = repo.current_branch()?.filter(|c| !c.is_empty());
            let truthy = |v: Option<String>| v.filter(|s| !s.is_empty());
            let mut found = None;
            if let Some(cur) = &cur {
                found = truthy(repo.config_last(&format!("branch.{cur}.pushremote"))?);
            }
            if found.is_none() {
                found = truthy(repo.config_last("remote.pushdefault")?);
            }
            if found.is_none() {
                if let Some(cur) = &cur {
                    found = truthy(repo.config_last(&format!("branch.{cur}.remote"))?);
                }
            }
            found.unwrap_or_else(|| "origin".to_string())
        }
    };
    let fallback = vec![remote.clone()];
    let urls = config
        .get(&format!("remote.{remote}.pushurl"))
        .or_else(|| config.get(&format!("remote.{remote}.url")))
        .unwrap_or(&fallback);
    for url in urls {
        if url.starts_with("file://") {
            continue;
        }
        if SCHEME.is_match(url) || SCP.is_match(url) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// `glob_hits_protected`: a destination that is `main`, or a glob that matches
/// it (in either case).
pub fn glob_hits_protected(name: &str) -> bool {
    if name.contains('*') {
        return fnmatch_case(PROTECTED_BRANCH, name)
            || fnmatch_case(&PROTECTED_BRANCH.to_uppercase(), &name.to_uppercase());
    }
    is_protected(name)
}

/// One element of a translated glob (`fnmatch._translate`).
#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobPart {
    Star,
    Any,
    Never,
    Literal(char),
    Class {
        negate: bool,
        items: Vec<(char, char)>,
    },
}

/// `fnmatch.fnmatchcase(name, pattern)` (CPython 3.13): `*` any run, `?` any
/// character, `[...]` a set (`!` negates; an unclosed `[` is literal), every
/// other character itself.
pub fn fnmatch_case(name: &str, pattern: &str) -> bool {
    let parts = glob_parts(pattern);
    let chars: Vec<char> = name.chars().collect();
    glob_match(&parts, &chars)
}

fn glob_match(parts: &[GlobPart], s: &[char]) -> bool {
    let Some((first, rest)) = parts.split_first() else {
        return s.is_empty();
    };
    if *first == GlobPart::Star {
        return (0..=s.len()).any(|k| glob_match(rest, &s[k..]));
    }
    let Some((&c, tail)) = s.split_first() else {
        return false;
    };
    let hit = match first {
        GlobPart::Any => true,
        GlobPart::Never => false,
        GlobPart::Literal(l) => *l == c,
        GlobPart::Class { negate, items } => {
            items.iter().any(|&(lo, hi)| lo <= c && c <= hi) != *negate
        }
        GlobPart::Star => unreachable!("handled above"),
    };
    hit && glob_match(rest, tail)
}

/// `fnmatch._translate`, keeping each set as the class body Python builds and
/// reading it back as `re` reads it ([`class_items`]).
fn glob_parts(pattern: &str) -> Vec<GlobPart> {
    let pat: Vec<char> = pattern.chars().collect();
    let n = pat.len();
    let mut res = Vec::new();
    let mut i = 0;
    while i < n {
        let c = pat[i];
        i += 1;
        match c {
            '*' => {
                if res.last() != Some(&GlobPart::Star) {
                    res.push(GlobPart::Star);
                }
            }
            '?' => res.push(GlobPart::Any),
            '[' => {
                let mut j = i;
                if j < n && pat[j] == '!' {
                    j += 1;
                }
                if j < n && pat[j] == ']' {
                    j += 1;
                }
                while j < n && pat[j] != ']' {
                    j += 1;
                }
                if j >= n {
                    res.push(GlobPart::Literal('['));
                    continue;
                }
                let stuff = class_body(&pat, i, j);
                i = j + 1;
                if stuff.is_empty() {
                    res.push(GlobPart::Never);
                } else if stuff == ['!'] {
                    res.push(GlobPart::Any);
                } else {
                    res.push(class_items(&stuff));
                }
            }
            _ => res.push(GlobPart::Literal(c)),
        }
    }
    res
}

/// The class body `fnmatch._translate` builds for `pat[i..j]`, before its `!`
/// becomes `^`: backslashes escaped, hyphens that do not make a range escaped,
/// empty ranges removed, and `&`, `~` and `|` escaped.
fn class_body(pat: &[char], start: usize, j: usize) -> Vec<char> {
    let escape = |chunk: &[char], hyphens: bool| -> Vec<char> {
        let mut out = Vec::new();
        for &ch in chunk {
            if ch == '\\' || (hyphens && ch == '-') {
                out.push('\\');
            }
            out.push(ch);
        }
        out
    };
    let raw = &pat[start..j];
    let joined: Vec<char> = if !raw.contains(&'-') {
        escape(raw, false)
    } else {
        let mut i = start;
        let mut chunks: Vec<Vec<char>> = Vec::new();
        let mut k = if pat[i] == '!' { i + 2 } else { i + 1 };
        loop {
            let found = (k..j).find(|&p| pat[p] == '-');
            let Some(h) = found else { break };
            chunks.push(pat[i..h].to_vec());
            i = h + 1;
            k = h + 3;
        }
        let chunk = pat[i..j].to_vec();
        if !chunk.is_empty() {
            chunks.push(chunk);
        } else if let Some(last) = chunks.last_mut() {
            last.push('-');
        }
        let mut k = chunks.len().saturating_sub(1);
        while k > 0 {
            let prev_last = *chunks[k - 1].last().expect("a non-empty chunk");
            if prev_last > chunks[k][0] {
                let mut merged = chunks[k - 1][..chunks[k - 1].len() - 1].to_vec();
                merged.extend_from_slice(&chunks[k][1..]);
                chunks[k - 1] = merged;
                chunks.remove(k);
            }
            k -= 1;
        }
        let mut out = Vec::new();
        for (n, chunk) in chunks.iter().enumerate() {
            if n > 0 {
                out.push('-');
            }
            out.extend(escape(chunk, true));
        }
        out
    };
    let mut stuff = Vec::new();
    for ch in joined {
        if "&~|".contains(ch) {
            stuff.push('\\');
        }
        stuff.push(ch);
    }
    stuff
}

/// A class body read the way `re` reads `[...]`: a leading `!` (Python's `^`)
/// negates; `\x` is the character x; `a-b` is a range.
fn class_items(stuff: &[char]) -> GlobPart {
    let (negate, body) = match stuff.split_first() {
        Some(('!', rest)) => (true, rest),
        _ => (false, stuff),
    };
    let mut items = Vec::new();
    let mut p = 0;
    let item = |p: &mut usize| -> Option<(char, bool)> {
        let c = *body.get(*p)?;
        *p += 1;
        if c == '\\' {
            let e = *body.get(*p)?;
            *p += 1;
            return Some((e, true));
        }
        Some((c, false))
    };
    while let Some((lo, _)) = item(&mut p) {
        if body.get(p) == Some(&'-') {
            p += 1;
            match item(&mut p) {
                // A range whose end is past the body ends at Python's `]`: the
                // hyphen is literal.
                None => {
                    items.push((lo, lo));
                    items.push(('-', '-'));
                }
                // translate removes ranges whose start is past their end, so
                // every range here is ordered.
                Some((hi, _)) => items.push((lo, hi)),
            }
        } else {
            items.push((lo, lo));
        }
    }
    GlobPart::Class { negate, items }
}

impl Analyzer<'_> {
    // -- git level ----------------------------------------------------------

    pub fn analyze_git(
        &mut self,
        args: &[String],
        cmd: &Command,
        cwd: &str,
        wrapper: Option<&str>,
        depth: usize,
    ) -> Flow {
        let mut i = 0;
        let mut workdir = cwd.to_string();
        let mut extra: Vec<String> = Vec::new();
        let mut inline_config: Vec<String> = Vec::new();
        while i < args.len() {
            let a = args[i].as_str();
            if a == "-C" && i + 1 < args.len() {
                workdir = pyos::normpath(&pyos::join(&workdir, &pyos::expanduser(&args[i + 1])?));
                i += 2;
                continue;
            }
            if a == "-c" && i + 1 < args.len() {
                inline_config.push(args[i + 1].clone());
                i += 2;
                continue;
            }
            let (name, eq, value) = partition(a, '=');
            if GIT_GLOBAL_WITH_ARG.contains(&name) {
                let mut value = value.to_string();
                if !eq {
                    value = args.get(i + 1).cloned().unwrap_or_default();
                    i += 1;
                }
                i += 1;
                if name == "--config-env" {
                    inline_config.push(value);
                } else if name == "--git-dir" || name == "--work-tree" {
                    extra.push(format!("{name}={value}"));
                }
                continue;
            }
            if a.starts_with('-') {
                i += 1;
                continue;
            }
            break;
        }
        if i >= args.len() {
            return Ok(());
        }
        let sub = args[i].as_str();
        let rest = &args[i + 1..];
        if unresolved(sub) {
            self.verdict.ask(
                verdict::GIT,
                format!("git subcommand '{sub}' comes from a shell expansion"),
            );
            return Ok(());
        }
        let repo = self.repo(&workdir, &extra);
        if sub == "push" && unresolved(&workdir) {
            self.verdict.ask(
                verdict::PUSH,
                "git push runs in a directory that comes from a shell expansion",
            );
        }
        if !word_in(GIT_BUILTINS, sub)
            && !["push", "send-pack", "http-push", "subtree"].contains(&sub)
        {
            return self.expand_git_alias(
                sub,
                rest,
                &args[..i],
                &inline_config,
                cmd,
                cwd,
                &repo,
                wrapper,
                depth,
            );
        }
        if rest
            .iter()
            .any(|a| a == "--output" || a.starts_with("--output="))
        {
            self.verdict.ask(
                verdict::GIT,
                format!("git {sub} --output writes a file at any path, including protected ones"),
            );
        }
        if sub == "push" {
            self.analyze_push(rest, &repo, wrapper, &inline_config)?;
        } else if sub == "send-pack" || sub == "http-push" {
            self.verdict.ask(
                verdict::PUSH,
                format!("git {sub} updates remote refs directly"),
            );
        } else if sub == "subtree" && rest.iter().any(|a| a == "push") {
            self.verdict
                .ask(verdict::PUSH, "git subtree push updates a remote branch");
        } else {
            self.check_git_options(sub, rest, cmd, &workdir)?;
        }
        self.record_git_effects(sub, rest);
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn expand_git_alias(
        &mut self,
        sub: &str,
        rest: &[String],
        prefix: &[String],
        inline_config: &[String],
        cmd: &Command,
        cwd: &str,
        repo: &Repo,
        wrapper: Option<&str>,
        depth: usize,
    ) -> Flow {
        let key = format!("alias.{}", sub.to_lowercase());
        let mut alias = None;
        for item in inline_config {
            let (k, _, value) = partition(item, '=');
            if k.to_lowercase() == key {
                alias = Some(value.to_string());
            }
        }
        if alias.is_none() {
            alias = repo.config_last(&key)?;
        }
        let alias = match alias {
            Some(a) if !a.is_empty() => a,
            _ => {
                if !self.config_changed.is_empty()
                    || self.env_names().iter().any(|n| git_env_risky(n))
                {
                    self.verdict.ask(
                        verdict::GIT,
                        format!(
                            "git {sub} is not a git command; it may be an alias defined earlier in this command"
                        ),
                    );
                }
                return Ok(());
            }
        };
        if depth >= MAX_DEPTH {
            return Ok(());
        }
        if let Some(body) = alias.strip_prefix('!') {
            let mut text = body.to_string();
            for r in rest {
                text.push(' ');
                text.push_str(&shlex::quote(r));
            }
            let repo_cwd = repo.cwd.clone();
            return self.analyze_text(&text, &repo_cwd, depth + 1, false);
        }
        let Ok(expanded) = shlex::split(&alias) else {
            self.verdict.ask(
                verdict::GIT,
                format!("git alias '{sub}' could not be parsed"),
            );
            return Ok(());
        };
        let mut argv = prefix.to_vec();
        argv.extend(expanded);
        argv.extend_from_slice(rest);
        self.analyze_git(&argv, cmd, cwd, wrapper, depth + 1)
    }

    /// Dangerous options of git subcommands other than push, read with git's
    /// abbreviation and cluster rules.
    fn check_git_options(
        &mut self,
        sub: &str,
        args: &[String],
        cmd: &Command,
        cwd: &str,
    ) -> Result<(), Fault> {
        let tag = verdict::GIT;
        match sub {
            "rebase" => {
                if !scan_git_options(args, &[("exec", true)], "x", "sXxC", "S").is_empty() {
                    self.verdict.ask(
                        tag,
                        "git rebase --exec runs a shell command after each commit",
                    );
                }
            }
            "fetch" | "pull" => {
                if !scan_git_options(args, &[("upload-pack", true)], "", "", "").is_empty() {
                    self.verdict.ask(
                        tag,
                        format!("git {sub} --upload-pack runs a command for local and ssh remotes"),
                    );
                }
                let takes = if sub == "fetch" { "jo" } else { "sXjo" };
                let forced = !scan_git_options(
                    args,
                    &[("force", false), ("update-head-ok", false)],
                    "fu",
                    takes,
                    "S",
                )
                .is_empty();
                let reset = args.iter().filter(|a| !a.starts_with('-')).any(|p| {
                    p.split_once(':')
                        .is_some_and(|(_, dst)| !dst.is_empty() && (forced || p.starts_with('+')))
                });
                if reset {
                    self.verdict.ask(
                        tag,
                        format!("git {sub} with a forced refspec can reset local branches"),
                    );
                }
            }
            "checkout" | "switch" => {
                let hits = if sub == "checkout" {
                    scan_git_options(
                        args,
                        &[("force", false), ("merge", false), ("conflict", true)],
                        "fmB",
                        "bB",
                        "",
                    )
                } else {
                    scan_git_options(
                        args,
                        &[
                            ("force", false),
                            ("discard-changes", false),
                            ("merge", false),
                            ("conflict", true),
                            ("force-create", true),
                        ],
                        "fmC",
                        "cC",
                        "",
                    )
                };
                if let Some((name, _)) = hits.first() {
                    self.verdict.ask(
                        tag,
                        format!(
                            "git {sub} {} can discard uncommitted changes or reset a branch",
                            opt_label(name)
                        ),
                    );
                }
            }
            "add" => {
                if !scan_git_options(args, &[("force", false)], "f", "", "").is_empty() {
                    self.verdict.ask(
                        tag,
                        "git add --force stages files that .gitignore excludes, such as secrets",
                    );
                }
            }
            "commit" => {
                let hits = scan_git_options(
                    args,
                    &[("file", true), ("template", true)],
                    "Ft",
                    "mFcCt",
                    "uS",
                );
                for (name, value) in hits {
                    let what = if name == "template" || name == "-t" {
                        "git commit --template"
                    } else {
                        "git commit --file"
                    };
                    self.check_input(tag, value.as_deref(), cmd, cwd, what)?;
                }
            }
            "branch" => {
                if !scan_git_options(args, &[("force", false)], "DfMC", "u", "").is_empty() {
                    self.verdict.ask(
                        tag,
                        "git branch -D/-f/-M/-C can drop commits or overwrite a branch",
                    );
                }
            }
            "worktree" if !args.is_empty() => {
                let (action, opts) = (args[0].as_str(), &args[1..]);
                if action == "remove"
                    && !scan_git_options(opts, &[("force", false)], "f", "", "").is_empty()
                {
                    self.verdict.ask(
                        tag,
                        "git worktree remove --force deletes uncommitted changes in that worktree",
                    );
                }
                if action == "add" && !scan_git_options(opts, &[], "B", "bB", "").is_empty() {
                    self.verdict
                        .ask(tag, "git worktree add -B resets an existing branch");
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Remember what this git command changes, for later pushes in the same
    /// text: the guard reads the repository as it is before the whole command
    /// runs.
    fn record_git_effects(&mut self, sub: &str, args: &[String]) {
        let positional: Vec<&str> = args
            .iter()
            .filter(|a| !a.starts_with('-'))
            .map(String::as_str)
            .collect();
        let first = positional.first().copied();
        match sub {
            "config" => {
                const READS: [&str; 8] = [
                    "--get",
                    "--get-all",
                    "--get-regexp",
                    "--get-urlmatch",
                    "--list",
                    "-l",
                    "--get-color",
                    "--get-colorbool",
                ];
                const QUIET: [&str; 8] = [
                    "--global",
                    "--local",
                    "--system",
                    "--worktree",
                    "--show-origin",
                    "--show-scope",
                    "--null",
                    "-z",
                ];
                if matches!(first, Some("get" | "list"))
                    || args.iter().any(|a| READS.contains(&a.as_str()))
                {
                    return;
                }
                if positional.len() <= 1
                    && !args
                        .iter()
                        .any(|a| a.starts_with("--") && !QUIET.contains(&a.as_str()))
                {
                    return;
                }
                self.config_changed.insert("config".to_string());
            }
            "remote" => {
                if matches!(first, None | Some("show" | "get-url")) {
                    return;
                }
                self.config_changed.insert("remote".to_string());
            }
            "branch" => {
                let changes = scan_git_options(
                    args,
                    &[
                        ("delete", false),
                        ("move", false),
                        ("copy", false),
                        ("force", false),
                        ("set-upstream-to", true),
                        ("unset-upstream", false),
                        ("track", false),
                        ("edit-description", false),
                    ],
                    "dDmMcCfut",
                    "u",
                    "",
                );
                const FILTERS: [&str; 10] = [
                    "--contains",
                    "--no-contains",
                    "--merged",
                    "--no-merged",
                    "--points-at",
                    "--format",
                    "--sort",
                    "--list",
                    "-l",
                    "--column",
                ];
                let creates = !positional.is_empty()
                    && !args
                        .iter()
                        .any(|a| FILTERS.iter().any(|f| a.starts_with(f)));
                if changes.is_empty() && !creates {
                    return; // a listing
                }
                // Can rename the current branch or set its upstream.
                self.head_changed.insert("branch".to_string());
                self.commits_changed = true;
            }
            "worktree" => {
                let Some(action) = first else { return };
                if action == "list" || action == "prune" {
                    return;
                }
                // A new branch with an upstream; matters for push.default=upstream.
                self.head_changed.insert(format!("worktree {action}"));
            }
            _ if GIT_HEAD_CHANGERS.contains(&sub) => {
                self.head_changed.insert(sub.to_string());
                self.commits_changed = true;
            }
            _ => {
                let stash_read = sub == "stash" && matches!(first, Some("list" | "show"));
                if !word_in(GIT_READ_ONLY, sub) && !stash_read {
                    self.commits_changed = true;
                }
            }
        }
    }

    fn analyze_push(
        &mut self,
        args: &[String],
        repo: &Repo,
        wrapper: Option<&str>,
        inline_config: &[String],
    ) -> Result<(), Fault> {
        let tag = verdict::PUSH;
        if let Some(w) = wrapper.filter(|w| ARG_WRAPPERS.contains(w)) {
            self.verdict.ask(
                tag,
                format!(
                    "git push arguments are supplied by {w}; the guard cannot see the destinations"
                ),
            );
        }
        if !inline_config.is_empty() {
            self.verdict.ask(
                tag,
                "inline -c/--config-env on git push can change where it pushes",
            );
        }
        let env: Vec<String> = self
            .env_names()
            .into_iter()
            .filter(|n| git_env_risky(n))
            .collect();
        if !env.is_empty() {
            self.verdict.ask(
                tag,
                format!(
                    "{} set in this command can change where git pushes",
                    env.join(", ")
                ),
            );
        }
        if !self.config_changed.is_empty() {
            let changed: Vec<&str> = self.config_changed.iter().map(String::as_str).collect();
            self.verdict.ask(
                tag,
                format!(
                    "an earlier command in this text changes git config or branches ({}); the guard resolves pushes against the repository as it was before the command",
                    changed.join(", ")
                ),
            );
        }

        let mut force = false;
        let mut lease = false;
        let mut delete = false;
        let mut dry = false;
        let mut tags = false;
        let mut all = false;
        let mut prune = false;
        let mut receive = false;
        let mut positionals: Vec<String> = Vec::new();
        let mut i = 0;
        while i < args.len() {
            let a = args[i].as_str();
            i += 1;
            if a == "--" {
                positionals.extend_from_slice(&args[i..]);
                break;
            }
            if let Some(long) = a.strip_prefix("--") {
                let (name, eq, _) = partition(long, '=');
                let negated = name.starts_with("no-");
                let Some(opt) = resolve_long(if negated { &name[3..] } else { name }) else {
                    if !negated {
                        self.verdict.ask(
                            tag,
                            format!("unrecognised or ambiguous git push option '{a}'"),
                        );
                    }
                    continue;
                };
                if PUSH_OPTS_WITH_ARG.contains(&opt) && !eq && !negated {
                    i += 1;
                }
                match opt {
                    "force" => force = !negated,
                    "force-with-lease" => lease = !negated,
                    "delete" => delete = !negated,
                    "dry-run" => dry = !negated,
                    "tags" => tags = !negated,
                    "all" | "branches" | "mirror" => all = all || !negated,
                    "prune" => prune = !negated,
                    "receive-pack" | "exec" => receive = true,
                    _ => {}
                }
                continue;
            }
            if a.starts_with('-') && a.chars().count() > 1 {
                let cluster: Vec<char> = a.chars().skip(1).collect();
                for (pos, &ch) in cluster.iter().enumerate() {
                    match ch {
                        'f' => force = true,
                        'd' => delete = true,
                        'n' => dry = true,
                        'o' => {
                            if pos == cluster.len() - 1 {
                                i += 1;
                            }
                            break;
                        }
                        _ => {}
                    }
                }
                continue;
            }
            positionals.push(a.to_string());
        }

        if dry {
            return Ok(());
        }
        if positionals.iter().any(|p| unresolved(p)) {
            self.verdict.ask(
                tag,
                "a push remote or refspec comes from a shell expansion the guard cannot resolve",
            );
        }
        if receive {
            self.verdict.ask(
                tag,
                "--receive-pack/--exec runs a command for local remotes",
            );
        }
        if inline_config.is_empty()
            && env.is_empty()
            && remote_is_local(repo, positionals.first().map(String::as_str))?
        {
            return Ok(());
        }
        if all {
            self.verdict.deny(
                tag,
                "--all/--branches/--mirror pushes every branch, including main",
            );
        }
        if prune {
            self.verdict.ask(
                tag,
                "--prune deletes remote branches that have no local counterpart",
            );
        }
        if force {
            self.verdict.deny(
                tag,
                "plain force push (--force/-f); use --force-with-lease on a claude/* branch",
            );
        }

        static PUSH_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^remote\..+\.push$"));
        static MIRROR_KEY: LazyLock<Regex> = LazyLock::new(|| re(r"^remote\..+\.mirror$"));
        let config = repo.config()?;
        if config.keys().any(|k| PUSH_KEY.is_match(k)) {
            self.verdict.ask(
                tag,
                "remote.<name>.push is configured and can remap push destinations",
            );
        }
        let mirrored = config.iter().any(|(k, vals)| {
            MIRROR_KEY.is_match(k)
                && vals.last().is_some_and(|v| {
                    ["true", "yes", "on", "1"].contains(&v.to_lowercase().as_str())
                })
        });
        if mirrored {
            self.verdict
                .ask(tag, "a remote is configured with mirror=true");
        }

        let refspecs = positionals.get(1..).unwrap_or(&[]);
        let push_default = repo
            .config_last("push.default")?
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| "simple".to_string());
        let push_default = pystr::strip(&push_default).to_string();
        let follow_upstream = push_default == "upstream" || push_default == "tracking";
        let mut head_dependent = positionals.is_empty();
        let mut dests: Vec<(String, Vec<String>)> = Vec::new();
        let mut deletes: Vec<String> = Vec::new();
        let mut srcs: Vec<String> = Vec::new();

        if delete {
            deletes.extend(
                refspecs
                    .iter()
                    .map(|r| r.trim_start_matches('+').to_string()),
            );
        } else if refspecs.is_empty() && !tags {
            head_dependent = true;
            if push_default == "matching" {
                self.verdict.deny(
                    tag,
                    "implicit push with push.default=matching pushes every matching branch, including main",
                );
            } else if push_default != "nothing" {
                if let Some(cur) = self.push_current(repo)? {
                    dests.push((cur.clone(), vec![cur.clone()]));
                    srcs.push("HEAD".to_string());
                    if follow_upstream {
                        if let Some(up) = upstream_of(repo, &cur)? {
                            dests.push((up.clone(), vec![up]));
                        }
                    }
                }
            }
        } else {
            let mut it = refspecs.iter();
            while let Some(spec) = it.next() {
                if spec == "tag" {
                    it.next();
                    continue;
                }
                let mut spec = spec.as_str();
                if let Some(stripped) = spec.strip_prefix('+') {
                    self.verdict
                        .deny(tag, format!("'+' refspec force-pushes ({spec})"));
                    spec = stripped;
                }
                if spec == ":" || spec.is_empty() {
                    self.verdict.deny(
                        tag,
                        "the ':' refspec pushes every matching branch, including main",
                    );
                    continue;
                }
                if let Some((src, dst)) = spec.split_once(':') {
                    if src.is_empty() {
                        deletes.push(dst.to_string());
                        continue;
                    }
                    let target = if dst.is_empty() { src } else { dst };
                    dests.push((dst.to_string(), dst_candidates(target)));
                    srcs.push(src.to_string());
                    continue;
                }
                srcs.push(spec.to_string());
                if spec == "HEAD" || spec == "@" {
                    head_dependent = true;
                    if let Some(cur) = self.push_current(repo)? {
                        dests.push((cur.clone(), vec![cur]));
                    } else if repo.is_repo()? {
                        self.verdict
                            .ask(tag, "HEAD is detached; cannot resolve the push destination");
                    }
                    continue;
                }
                let Some(branch) = src_branch(spec) else {
                    continue;
                };
                dests.push((spec.to_string(), vec![branch.clone()]));
                if follow_upstream {
                    if let Some(up) = upstream_of(repo, &branch)? {
                        dests.push((up.clone(), vec![up]));
                    }
                }
            }
        }

        if !self.head_changed.is_empty() && (head_dependent || follow_upstream) {
            let changed: Vec<&str> = self.head_changed.iter().map(String::as_str).collect();
            self.verdict.ask(
                tag,
                format!(
                    "an earlier command in this text can switch branches ({}); the guard resolves HEAD and upstreams as they were before the command",
                    changed.join(", ")
                ),
            );
        }
        for (shown, names) in &dests {
            if names.iter().any(|n| glob_hits_protected(n)) {
                self.verdict.deny(
                    tag,
                    format!("push would update {PROTECTED_BRANCH} (destination '{shown}')"),
                );
            }
        }
        for shown in &deletes {
            if dst_candidates(shown).iter().any(|n| glob_hits_protected(n)) {
                self.verdict.deny(
                    tag,
                    format!("push would delete {PROTECTED_BRANCH} ('{shown}')"),
                );
            } else {
                self.verdict
                    .ask(tag, format!("push deletes remote ref '{shown}'"));
            }
        }
        if lease {
            for (shown, names) in &dests {
                if names.is_empty() || !names.iter().all(|n| n.starts_with(AGENT_PREFIX)) {
                    self.verdict.ask(
                        tag,
                        format!("--force-with-lease rewrites '{shown}', which is outside {AGENT_PREFIX}*"),
                    );
                }
            }
        }
        if self.verdict.level != Level::Deny
            && !srcs.is_empty()
            && repo.is_repo()?
            && repo.unpushed_github(&srcs, self.commits_changed)?
        {
            self.verdict.ask(
                tag,
                "commits for this push change .github/; CI runs those workflows with repository secrets",
            );
        }
        Ok(())
    }

    /// `current()` in `analyze_push`: the checked-out branch, or `None` (with an
    /// ask when the directory is not a repository yet).
    fn push_current(&mut self, repo: &Repo) -> Result<Option<String>, Fault> {
        if !repo.is_repo()? {
            self.verdict.ask(
                verdict::PUSH,
                "cannot resolve the current branch for this push (directory is not a git repository yet)",
            );
            return Ok(None);
        }
        Ok(repo.current_branch()?.filter(|c| !c.is_empty()))
    }
}

/// `upstream_of(branch)` in `analyze_push`: the branch its `merge` config names.
fn upstream_of(repo: &Repo, branch: &str) -> Result<Option<String>, Fault> {
    let merge = repo
        .config_last(&format!("branch.{branch}.merge"))?
        .unwrap_or_default();
    Ok(merge
        .strip_prefix("refs/heads/")
        .filter(|up| !up.is_empty())
        .map(str::to_string))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn fnmatch_matches_python() {
        // fnmatch.fnmatchcase(name, pattern) on CPython 3.13.
        for (name, pattern, want) in [
            ("main", "*", true),
            ("main", "m*", true),
            ("main", "*n", true),
            ("main", "ma?n", true),
            ("main", "m[a-c]in", true),
            ("main", "m[!a]in", false),
            ("main", "m[!b]in", true),
            ("main", "m[c-a]in", false),
            ("main", "[z-a-m]ain", true),
            ("main", "[]m]ain", true),
            ("main", "[!]ain", false),
            ("main", "[!]main", false),
            ("main", "[m", false),
            ("[m", "[m", true),
            ("main", "[a-]ain", false),
            ("-ain", "[a-]ain", true),
            ("main", "[l-n]a*", true),
            ("main", "m[\\]ain", false),
            ("main", "*[!]*", false),
            ("main", "*[]*", false),
            ("main", "m[b-a-z]in", false),
            ("main", "**a**", true),
            ("MAIN", "M[A-Z]IN", true),
            ("main", "m[&~|a]in", true),
        ] {
            assert_eq!(
                fnmatch_case(name, pattern),
                want,
                "fnmatchcase({name:?}, {pattern:?})"
            );
        }
    }

    #[test]
    fn scan_git_options_reads_clusters_and_prefixes() {
        let hits = scan_git_options(
            &strings(&["-qfo", "x", "--fo", "--no-force", "--"]),
            &[("force", false)],
            "f",
            "o",
            "",
        );
        assert_eq!(
            hits,
            vec![("-f".to_string(), None), ("force".to_string(), None)]
        );
        let hits = scan_git_options(
            &strings(&["-mfoo", "--file=x"]),
            &[("file", true)],
            "F",
            "mF",
            "",
        );
        assert_eq!(hits, vec![("file".to_string(), Some("x".to_string()))]);
        let hits = scan_git_options(
            &strings(&["-Fx", "--file", "y"]),
            &[("file", true)],
            "F",
            "F",
            "",
        );
        assert_eq!(
            hits,
            vec![
                ("-F".to_string(), Some("x".to_string())),
                ("file".to_string(), Some("y".to_string()))
            ]
        );
    }

    #[test]
    fn push_helpers_match_python() {
        assert_eq!(dst_candidates("refs//heads/main"), ["main"]);
        assert_eq!(dst_candidates("refs/tags/v1"), Vec::<String>::new());
        assert_eq!(dst_candidates("heads/x"), ["heads/x", "x"]);
        assert_eq!(src_branch("refs/heads/a").as_deref(), Some("a"));
        assert_eq!(src_branch("refs/tags/a"), None);
        assert_eq!(src_branch("heads//a").as_deref(), Some("a"));
        assert_eq!(resolve_long("force"), Some("force"));
        assert_eq!(resolve_long("forc"), None);
        assert_eq!(resolve_long("dry"), Some("dry-run"));
        assert_eq!(resolve_long(""), None);
        assert!(is_protected("HEAD") && is_protected("@") && is_protected("Main"));
        assert!(glob_hits_protected("ma*") && glob_hits_protected("M*N"));
        assert!(!glob_hits_protected("claude/*"));
        assert!(git_env_risky("GIT_DIR") && git_env_risky("PATH"));
        assert!(!git_env_risky("GIT_TRACE2") && !git_env_risky("GIT_PAGER"));
    }
}
