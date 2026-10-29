//! The gh level (git-push-guard.py L2419-2563, with `scan_gh_flags` L916-951,
//! `normalize_repo` L1176-1181, `check_download_dir` L2036-2046 and
//! `routine_post` L2589-2603): merges, aliases and extensions, text published
//! to GitHub, artifact downloads and API writes.

use std::sync::LazyLock;

use regex::Regex;

use super::analyzer::{partition, unresolved, Analyzer};
use super::pattern::{ci, re};
use super::pyos::{self, Fault};
use super::shell::Command;
use super::verdict;
use crate::pystr;

pub const HOME_REPO: &str = "juslee/aios";

const GH_RISKY_ENV: [&str; 3] = ["GH_REPO", "GH_HOST", "GH_CONFIG_DIR"];

/// gh top-level commands (plus the default `co` alias). Anything else is a user
/// alias or an extension.
const GH_COMMANDS: &str = "
accessibility agent-task alias api attestation auth browse cache co codespace
completion config copilot extension gist gpg-key help issue label licenses
org pr preview project release repo ruleset run search secret ssh-key status
variable version workflow
";

/// gh commands that publish text to GitHub, with the shorthand flags that take a
/// value. pflag lets boolean shorthands share a cluster with one value flag.
const GH_PUBLISH: [((&str, &str), &str); 7] = [
    (("issue", "create"), "abFlmpRtT"),
    (("issue", "comment"), "bFR"),
    (("issue", "edit"), "bFmRt"),
    (("pr", "create"), "abBFHlmprRtT"),
    (("pr", "comment"), "bFR"),
    (("pr", "edit"), "bBFmRt"),
    (("pr", "review"), "bFR"),
];
const PUBLISH_VALUE_LONGS: [&str; 12] = [
    "repo",
    "body-file",
    "body",
    "title",
    "base",
    "head",
    "assignee",
    "label",
    "milestone",
    "project",
    "reviewer",
    "template",
];
const API_VALUE_LONGS: [&str; 10] = [
    "method",
    "field",
    "raw-field",
    "header",
    "input",
    "jq",
    "template",
    "preview",
    "hostname",
    "cache",
];
const API_VALUE_SHORTS: [(char, &str); 7] = [
    ('X', "method"),
    ('F', "field"),
    ('f', "raw-field"),
    ('H', "header"),
    ('q', "jq"),
    ('t', "template"),
    ('p', "preview"),
];
/// Directories whose contents run as code or configure the harness.
const PROTECTED_DIRS: [&str; 4] = [".git", ".claude", ".github", ".cargo"];

/// `GQL_DANGEROUS` (`re.I`): GraphQL mutations that change branches, merges or
/// repository settings.
static GQL_DANGEROUS: LazyLock<Regex> = LazyLock::new(|| {
    let names = [
        "mergePullRequest",
        "PullRequestAutoMerge",
        "mergeBranch",
        "updateRef",
        "deleteRef",
        "createRef",
        "createCommitOnBranch",
        "BranchProtectionRule",
        "RepositoryRuleset",
        "updateRepository",
        "deleteRepository",
        "archiveRepository",
        "transferRepository",
        "updatePullRequestBranch",
        "dismissPullRequestReview",
    ];
    let alternatives: Vec<String> = names.iter().map(|n| ci(n)).collect();
    re(&alternatives.join("|"))
});

/// A flag `scan_gh_flags` found: `--long` or `-X`, and its value.
pub type Flag = (String, Option<String>);

/// `scan_gh_flags`: a pflag-style scan for gh (`--name value`, `--name=value`,
/// `-X value`, `-Xvalue`, `-X=value`, and boolean shorthands clustered before
/// one value shorthand; no abbreviations). Returns the flags and the
/// positionals.
pub fn scan_gh_flags(
    args: &[String],
    value_shorts: &str,
    value_longs: &[&str],
) -> (Vec<Flag>, Vec<String>) {
    let mut flags = Vec::new();
    let mut positionals = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].as_str();
        i += 1;
        if a == "--" {
            positionals.extend_from_slice(&args[i..]);
            break;
        }
        if a.starts_with("--") && a.chars().count() > 2 {
            let (name, eq, value) = partition(&a[2..], '=');
            let takes = value_longs.contains(&name);
            let value = if eq {
                Some(value.to_string())
            } else if takes {
                let v = args.get(i).cloned();
                i += 1;
                v
            } else {
                None
            };
            flags.push((format!("--{name}"), value));
        } else if a.starts_with('-') && a.chars().count() > 1 {
            let cluster: Vec<char> = a.chars().skip(1).collect();
            for (pos, &ch) in cluster.iter().enumerate() {
                if value_shorts.contains(ch) {
                    let rest: String = cluster[pos + 1..].iter().collect();
                    let rest = rest.strip_prefix('=').map(str::to_string).unwrap_or(rest);
                    let value = if rest.is_empty() {
                        let v = args.get(i).cloned();
                        i += 1;
                        v
                    } else {
                        Some(rest)
                    };
                    flags.push((format!("-{ch}"), value));
                    break;
                }
                flags.push((format!("-{ch}"), None));
            }
        } else {
            positionals.push(a.to_string());
        }
    }
    (flags, positionals)
}

/// `gh_env_risky`: a variable that can point gh at another host or repository.
pub fn gh_env_risky(name: &str) -> bool {
    GH_RISKY_ENV.contains(&name) || name == "HOME" || name == "XDG_CONFIG_HOME"
}

/// `normalize_repo`: owner/repo, lowercased, from a `--repo` value or URL.
pub fn normalize_repo(value: &str) -> String {
    static SCHEME: LazyLock<Regex> = LazyLock::new(|| re(r"^[a-z]+://"));
    static GIT_SUFFIX: LazyLock<Regex> = LazyLock::new(|| re(r"\.git/?$"));
    let lowered = pystr::strip(value).to_lowercase();
    let v = SCHEME.replace(&lowered, "");
    let v = GIT_SUFFIX.replace(&v, "");
    let v = v.trim_matches('/');
    v.strip_prefix("github.com/").unwrap_or(v).to_string()
}

/// `routine_post`: POST endpoints the review workflow uses on this repository.
pub fn routine_post(endpoint: &str) -> bool {
    static ROUTINE: LazyLock<Regex> = LazyLock::new(|| {
        let repo = format!(
            "(?:{}|{}|{})",
            ci(HOME_REPO),
            ci("{owner}/{repo}"),
            ci(":owner/:repo")
        );
        let repos = format!("{}{repo}/", ci("repos/"));
        let issues = ci("issues");
        let pulls = ci("pulls");
        let comments = ci("comments");
        let reactions = ci("reactions");
        let patterns = [
            format!(r"{repos}(?:{issues}|{pulls})/\d+/{comments}"),
            format!(r"{repos}{pulls}/\d+/{comments}/\d+/{}", ci("replies")),
            format!(r"{repos}{pulls}/\d+/{}", ci("reviews")),
            format!(r"{repos}{pulls}/\d+/{}", ci("requested_reviewers")),
            format!(r"{repos}{issues}/\d+/{}", ci("labels")),
            format!(r"{repos}{issues}/(?:{comments}/)?\d+/{reactions}"),
            format!(r"{repos}{pulls}/{comments}/\d+/{reactions}"),
            format!(r"{repos}(?:{issues}|{pulls})"),
            format!(r"{}(?:/{})?", ci("markdown"), ci("raw")),
        ];
        re(&format!("^(?:{})$", patterns.join("|")))
    });
    ROUTINE.is_match(endpoint.trim_end_matches('/'))
}

impl Analyzer<'_> {
    // -- gh level -------------------------------------------------------------

    /// `runs` is false when `gh` is only an argument of some other command
    /// (`ln -s x/gh y`); then only recognised gh commands are checked.
    pub fn analyze_gh(
        &mut self,
        args: &[String],
        cmd: &Command,
        cwd: &str,
        runs: bool,
    ) -> Result<(), Fault> {
        let tag = verdict::GH;
        let Some(top) = args.first().map(String::as_str) else {
            return Ok(());
        };
        if !GH_COMMANDS.split_whitespace().any(|c| c == top) {
            if runs && unresolved(top) {
                self.verdict.ask(
                    tag,
                    format!("gh command '{top}' comes from a shell expansion"),
                );
            } else if runs && !top.starts_with('-') {
                self.verdict.ask(
                    tag,
                    format!("gh {top} is an alias or extension; the guard cannot see what it runs"),
                );
            }
            return Ok(());
        }
        let sub = args.get(1).map_or("", String::as_str);
        let env: Vec<String> = self
            .env_names()
            .into_iter()
            .filter(|n| gh_env_risky(n))
            .collect();
        let rest = args.get(2..).unwrap_or(&[]);
        if top == "alias" && ["set", "import", "delete"].contains(&sub) {
            self.verdict.ask(
                tag,
                "gh alias changes can hide a merge or an API write behind a new name",
            );
        } else if top == "pr" && sub == "merge" {
            if rest
                .iter()
                .any(|a| a == "--admin" || a.starts_with("--admin="))
            {
                self.verdict.deny(
                    tag,
                    "gh pr merge --admin bypasses branch protection; merging is reserved for the user",
                );
            } else {
                self.verdict
                    .ask(tag, "gh pr merge: merging is reserved for the user");
            }
        } else if let Some(&(_, shorts)) =
            GH_PUBLISH.iter().find(|((t, s), _)| *t == top && *s == sub)
        {
            self.check_gh_publish(top, sub, shorts, rest, cmd, cwd, &env)?;
        } else if top == "run" && sub == "download" {
            let (flags, _) = scan_gh_flags(rest, "Dnp", &["dir", "name", "pattern", "repo"]);
            let dirs = flags
                .into_iter()
                .filter(|(name, _)| name == "-D" || name == "--dir")
                .filter_map(|(_, value)| value);
            for dir in dirs {
                self.check_download_dir(&dir, cwd)?;
            }
        } else if top == "api" {
            self.check_gh_api(&args[1..], cmd, cwd, &env)?;
        }
        Ok(())
    }

    fn check_home_repo(
        &mut self,
        what: &str,
        repo_flag: Option<&str>,
        cwd: &str,
        env: &[String],
    ) -> Result<(), Fault> {
        let tag = verdict::GH;
        if !env.is_empty() {
            self.verdict.ask(
                tag,
                format!(
                    "{} set for {what} can point it at another repository",
                    env.join(", ")
                ),
            );
        }
        if let Some(flag) = repo_flag {
            if unresolved(flag) {
                self.verdict.ask(
                    tag,
                    format!("{what} targets a repository from a shell expansion"),
                );
            } else if normalize_repo(flag) != HOME_REPO.to_lowercase() {
                self.verdict
                    .ask(tag, format!("{what} targets {flag}, not {HOME_REPO}"));
            }
            return Ok(());
        }
        let repo = self.repo(cwd, &[]);
        if repo.is_repo()? {
            let found = repo.github_repos()?;
            if !found.is_empty() && !found.contains(&HOME_REPO.to_lowercase()) {
                let found: Vec<&str> = found.iter().map(String::as_str).collect();
                self.verdict.ask(
                    tag,
                    format!(
                        "{what} runs in a checkout of {}, not {HOME_REPO}",
                        found.join(", ")
                    ),
                );
            }
        }
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn check_gh_publish(
        &mut self,
        top: &str,
        sub: &str,
        shorts: &str,
        args: &[String],
        cmd: &Command,
        cwd: &str,
        env: &[String],
    ) -> Result<(), Fault> {
        let tag = verdict::GH;
        let what = format!("gh {top} {sub}");
        let (flags, _) = scan_gh_flags(args, shorts, &PUBLISH_VALUE_LONGS);
        let mut repo_flag: Option<String> = None;
        for (name, value) in flags {
            if name == "-R" || name == "--repo" {
                repo_flag = Some(value.unwrap_or_default());
            } else if name == "-F" || name == "--body-file" {
                self.check_input(tag, value.as_deref(), cmd, cwd, &what)?;
                let content = match value.as_deref() {
                    Some(v) if !v.is_empty() => self.read_repo_file(v, cwd)?,
                    _ => None,
                };
                if content.is_some_and(|c| c.to_lowercase().contains("@claude")) {
                    self.verdict.ask(
                        tag,
                        format!(
                            "{what} body file mentions @claude, which starts the Claude workflow"
                        ),
                    );
                }
            }
        }
        self.check_home_repo(&what, repo_flag.as_deref(), cwd, env)?;
        if args.iter().any(|a| a.to_lowercase().contains("@claude")) {
            self.verdict.ask(
                tag,
                format!("{what} mentions @claude, which starts the Claude workflow"),
            );
        }
        Ok(())
    }

    fn check_gh_api(
        &mut self,
        args: &[String],
        cmd: &Command,
        cwd: &str,
        env: &[String],
    ) -> Result<(), Fault> {
        static OVERRIDE: LazyLock<Regex> =
            LazyLock::new(|| re(&format!(r"^\s*{}\s*:", ci("x-http-method-override"))));
        static API_HOST: LazyLock<Regex> = LazyLock::new(|| re(r"^https?://api\.github\.com/"));
        let tag = verdict::GH;
        let shorts: String = API_VALUE_SHORTS.iter().map(|&(c, _)| c).collect();
        let (flags, positionals) = scan_gh_flags(args, &shorts, &API_VALUE_LONGS);
        let mut method: Option<String> = None;
        let mut fields: Vec<(&'static str, String)> = Vec::new();
        let mut inputs: Vec<String> = Vec::new();
        let mut headers: Vec<String> = Vec::new();
        for (name, value) in flags {
            let key: Option<&'static str> = match name.strip_prefix("--") {
                Some(long) => API_VALUE_LONGS.iter().copied().find(|k| *k == long),
                None => {
                    let short = name.chars().nth(1);
                    API_VALUE_SHORTS
                        .iter()
                        .find(|&&(c, _)| Some(c) == short)
                        .map(|&(_, k)| k)
                }
            };
            let Some(value) = value else { continue };
            match key {
                Some("method") => method = Some(value),
                Some(k @ ("field" | "raw-field")) => fields.push((k, value)),
                Some("input") => inputs.push(value),
                Some("header") => headers.push(value),
                _ => {}
            }
        }
        let endpoint = positionals.first().cloned().unwrap_or_default();
        if method.as_deref().is_some_and(unresolved) {
            self.verdict
                .ask(tag, "gh api method comes from a shell expansion");
            return Ok(());
        }
        let method = match method.filter(|m| !m.is_empty()) {
            Some(m) => m,
            None if !fields.is_empty() || !inputs.is_empty() => "POST".to_string(),
            None => "GET".to_string(),
        }
        .to_uppercase();
        if headers.iter().any(|h| OVERRIDE.is_match(h)) {
            self.verdict
                .ask(tag, "gh api overrides the HTTP method with a header");
        }
        for (key, value) in &fields {
            let (name, _, val) = partition(value, '=');
            if *key == "field" {
                if let Some(path) = val.strip_prefix('@') {
                    self.check_input(tag, Some(path), cmd, cwd, &format!("gh api field {name}"))?;
                }
            }
        }
        let stripped = API_HOST.replace(&endpoint, "");
        let ep = stripped
            .trim_start_matches('/')
            .split('?')
            .next()
            .unwrap_or("")
            .to_string();
        if !env.is_empty() {
            self.verdict.ask(
                tag,
                format!(
                    "{} set for gh api can point it at another host or repository",
                    env.join(", ")
                ),
            );
        }
        if ep == "graphql" {
            return self.check_graphql(&fields, &inputs, cmd, cwd);
        }
        for path in &inputs {
            self.check_input(tag, Some(path), cmd, cwd, "gh api --input")?;
        }
        if method == "GET" || method == "HEAD" {
            return Ok(());
        }
        let shown = if endpoint.is_empty() {
            "(no endpoint)"
        } else {
            endpoint.as_str()
        };
        if ["PUT", "PATCH", "DELETE"].contains(&method.as_str()) {
            self.verdict
                .ask(tag, format!("gh api {method} {shown} changes GitHub state"));
            return Ok(());
        }
        if unresolved(&ep) || !routine_post(&ep) {
            self.verdict.ask(
                tag,
                format!(
                    "gh api {method} {shown} is not a routine comment, reply, review or reaction on this repository"
                ),
            );
        } else if ep.contains("{owner}") || ep.contains(":owner") {
            self.check_home_repo(&format!("gh api {method}"), None, cwd, &[])?;
        }
        Ok(())
    }

    fn check_graphql(
        &mut self,
        fields: &[(&'static str, String)],
        inputs: &[String],
        cmd: &Command,
        cwd: &str,
    ) -> Result<(), Fault> {
        let tag = verdict::GH;
        let mut texts: Vec<String> = Vec::new();
        for (key, value) in fields {
            let (name, _, val) = partition(value, '=');
            match val.strip_prefix('@').filter(|_| *key == "field") {
                Some(path) => match self.request_text(path, cmd, cwd)? {
                    Some(content) => texts.push(content),
                    None => self.verdict.ask(
                        tag,
                        format!(
                            "gh api graphql reads {name} from {path}, which the guard cannot read"
                        ),
                    ),
                },
                None => texts.push(val.to_string()),
            }
        }
        for path in inputs {
            match self.request_text(path, cmd, cwd)? {
                Some(content) => texts.push(content),
                None => self.verdict.ask(
                    tag,
                    format!(
                        "gh api graphql reads its request from {path}, which the guard cannot read"
                    ),
                ),
            }
        }
        for text in &texts {
            if unresolved(text) && text.contains("mutation") {
                self.verdict.ask(
                    tag,
                    "gh api graphql mutation text comes from a shell expansion",
                );
            }
            if let Some(m) = GQL_DANGEROUS.find(text) {
                self.verdict.ask(
                    tag,
                    format!(
                        "gh api graphql calls {}, which changes branches, merges or repository settings",
                        m.as_str()
                    ),
                );
            }
        }
        Ok(())
    }

    /// A GraphQL request read from `path`: the command's first heredoc for `-`,
    /// otherwise the file.
    fn request_text(&self, path: &str, cmd: &Command, cwd: &str) -> Result<Option<String>, Fault> {
        match cmd.heredocs.first() {
            Some(body) if path == "-" => Ok(Some(body.clone())),
            _ => self.read_file(path, cwd),
        }
    }

    /// Artifacts can come from any workflow run, including fork PRs, so they must
    /// not land where files run as code or configure tools.
    fn check_download_dir(&mut self, path: &str, cwd: &str) -> Result<(), Fault> {
        let tag = verdict::GH;
        if unresolved(path) {
            self.verdict.ask(
                tag,
                "gh run download writes to a directory from a shell expansion",
            );
            return Ok(());
        }
        let full = pyos::realpath(&pyos::join(cwd, &pyos::expanduser(path)?))?;
        let protected = full.split('/').any(|p| PROTECTED_DIRS.contains(&p));
        if !self.inside_repo(path, cwd)? || protected {
            self.verdict.ask(
                tag,
                format!(
                    "gh run download writes artifacts to {path}, outside the working tree or into a directory whose files run as code"
                ),
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn scan_gh_flags_matches_pflag() {
        let (flags, pos) = scan_gh_flags(
            &strings(&[
                "-bFx",
                "--repo",
                "a/b",
                "--title=t",
                "-R=c/d",
                "e",
                "--",
                "-z",
            ]),
            "FR",
            &["repo", "title"],
        );
        assert_eq!(
            flags,
            vec![
                ("-b".to_string(), None),
                ("-F".to_string(), Some("x".to_string())),
                ("--repo".to_string(), Some("a/b".to_string())),
                ("--title".to_string(), Some("t".to_string())),
                ("-R".to_string(), Some("c/d".to_string())),
            ]
        );
        assert_eq!(pos, ["e", "-z"]);
        let (flags, _) = scan_gh_flags(&strings(&["-F", "--draft", "--repo"]), "F", &["repo"]);
        assert_eq!(
            flags,
            vec![
                ("-F".to_string(), Some("--draft".to_string())),
                ("--repo".to_string(), None),
            ]
        );
    }

    #[test]
    fn repo_and_endpoint_helpers_match_python() {
        // normalize_repo and routine_post on CPython 3.13.
        assert_eq!(
            normalize_repo(" HTTPS://GitHub.com/JusLee/AIOS.git/ "),
            "juslee/aios"
        );
        assert_eq!(
            normalize_repo("git@github.com:juslee/aios"),
            "git@github.com:juslee/aios"
        );
        assert_eq!(normalize_repo("/juslee/aios/"), "juslee/aios");
        for (ep, want) in [
            ("repos/juslee/aios/issues/12/comments", true),
            ("repos/JUSLEE/AIOS/pulls/1/comments/2/replies/", true),
            ("repos/{owner}/{repo}/pulls/3/reviews", true),
            ("repos/:owner/:repo/issues/comments/4/reactions", true),
            ("repos/juslee/aios/pulls", true),
            ("markdown/raw", true),
            ("repos/juslee/aios/merges", false),
            ("repos/other/aios/issues/1/comments", false),
            ("repos/juslee/aios/issues/x/comments", false),
            ("repos/juslee/aios/issues/1/comments\n", false),
            ("repos/juslee/a\u{130}os/pulls", true),
        ] {
            assert_eq!(routine_post(ep), want, "routine_post({ep:?})");
        }
        assert!(GQL_DANGEROUS.is_match("mutation { MERGEPULLREQUEST }"));
        assert!(!GQL_DANGEROUS.is_match("query { repository }"));
    }
}
