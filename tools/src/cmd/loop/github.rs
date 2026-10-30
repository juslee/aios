//! The loop's GitHub reads. Each is a typed wrapper over one `gh` read, and
//! every call goes through a [`GhRunner`] (the real one is
//! `GhCli { cwd: <main checkout>, env: LOOP_GH_ENV }`), which runs
//! [`crate::gh::is_read`] before it spawns anything. There is no gh write here.
//!
//! The structs are lenient: a missing field takes its default and an unknown
//! one is ignored, so a gh release that adds a field does not break a run.
//! A consumer that needs a field to be present checks it itself.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::gh::{GhEnv, GhOutcome, GhRunner};
use crate::merge_ready::{NEEDS_HUMAN_ISSUES_ARGS, PR_FIELDS};

/// The environment every loop `gh` call gets: no prompts, no update notice, no
/// colour, and no `GH_REPO` or `GH_HOST` to point it at another repository.
pub const LOOP_GH_ENV: GhEnv = GhEnv {
    set: &[
        ("GH_PROMPT_DISABLED", "1"),
        ("GH_NO_UPDATE_NOTIFIER", "1"),
        ("NO_COLOR", "1"),
    ],
    remove: &["GH_REPO", "GH_HOST"],
};

/// What a failed gh call says in the error: `gh <command> <subcommand>`.
fn what(args: &[&str]) -> String {
    let words: Vec<&str> = args.iter().take(2).copied().collect();
    format!("gh {}", words.join(" "))
}

/// Run one read and return stdout. `Missing` is "gh is not installed", `Failed`
/// carries the first stderr line, and `Refused` (the allowlist said no) is an
/// internal error: a loop read that `is_read` rejects is a bug in this module.
fn bytes(r: &dyn GhRunner, args: &[&str]) -> Result<Vec<u8>> {
    match r.run(args) {
        GhOutcome::Ok(out) => Ok(out),
        GhOutcome::Missing => bail!("gh is not installed"),
        failed @ GhOutcome::Failed { .. } => bail!(
            "{} failed: {}",
            what(args),
            String::from_utf8_lossy(failed.first_stderr_line())
        ),
        GhOutcome::Refused { reason } => {
            bail!("internal error: {} was refused: {reason}", what(args))
        }
    }
}

/// [`bytes`], parsed as JSON.
pub fn json(r: &dyn GhRunner, args: &[&str]) -> Result<Value> {
    let out = bytes(r, args)?;
    serde_json::from_slice(&out).with_context(|| format!("{} printed invalid JSON", what(args)))
}

fn from_value<T: for<'de> Deserialize<'de>>(v: Value, what: &str) -> Result<T> {
    serde_json::from_value(v).with_context(|| format!("unexpected {what} shape"))
}

/// The repository's owner and name, for the GraphQL reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Slug {
    pub owner: String,
    pub name: String,
}

/// `gh repo view --json owner,name`, in the runner's working directory (the
/// main checkout). `review_threads` takes the result as a parameter, so one
/// lookup serves a whole run.
pub fn repo_slug(r: &dyn GhRunner) -> Result<Slug> {
    let v = json(r, &["repo", "view", "--json", "owner,name"])?;
    let owner = v["owner"]["login"].as_str();
    let name = v["name"].as_str();
    match (owner, name) {
        (Some(owner), Some(name)) => Ok(Slug {
            owner: owner.to_string(),
            name: name.to_string(),
        }),
        _ => Err(anyhow!("gh repo view printed no owner or name")),
    }
}

/// The typed part of `gh pr view`'s output. The rest of the object stays in
/// [`PrView::raw`] for `merge_ready::blockers`, which reads it as brief does.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PrFacts {
    pub head_ref_oid: String,
    pub base_ref_name: String,
    pub head_ref_name: String,
    pub is_cross_repository: bool,
    pub is_draft: bool,
    pub changed_files: u64,
    pub files: Vec<PrFile>,
    pub labels: Vec<Named>,
}

/// One entry of `files`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PrFile {
    pub path: String,
}

/// A label, or anything else that is `{name}`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct Named {
    pub name: String,
}

/// One `gh pr view`: the whole object, and its typed facts.
#[derive(Debug, Clone)]
pub struct PrView {
    pub raw: Value,
    pub facts: PrFacts,
}

impl PrView {
    pub fn has_label(&self, name: &str) -> bool {
        self.facts.labels.iter().any(|l| l.name == name)
    }
}

/// `gh pr view <n> --json <PR_FIELDS>,headRefOid,baseRefName,isCrossRepository,
/// files,changedFiles,commits,reviews`. Consumers: preflight, gather, verdict.
pub fn pr_view(r: &dyn GhRunner, n: u64) -> Result<PrView> {
    let fields = format!(
        "{PR_FIELDS},headRefOid,baseRefName,isCrossRepository,files,changedFiles,commits,reviews"
    );
    let n = n.to_string();
    let raw = json(r, &["pr", "view", &n, "--json", &fields])?;
    let facts = from_value(raw.clone(), "gh pr view")?;
    Ok(PrView { raw, facts })
}

/// One check of `gh pr checks`, with the run id parsed out of its link.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Check {
    pub name: String,
    pub state: String,
    pub bucket: String,
    pub link: String,
    #[serde(skip)]
    pub run_id: Option<u64>,
}

/// The `<id>` of a `…/actions/runs/<id>/…` link.
pub fn run_id_of(link: &str) -> Option<u64> {
    let rest = link.split("/actions/runs/").nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

/// `gh pr checks <n> --json name,state,bucket,link`. Consumer: gather (the log
/// tails of failing checks).
///
/// `gh pr checks --json` exits 0 even when a check failed or is pending (gh
/// 2.103.3), so any non-zero exit is a real failure. [`GhOutcome::Failed`] does
/// not carry stdout, so it is an error here, not a guess.
pub fn pr_checks(r: &dyn GhRunner, n: u64) -> Result<Vec<Check>> {
    let n = n.to_string();
    let v = json(r, &["pr", "checks", &n, "--json", "name,state,bucket,link"])?;
    let mut checks: Vec<Check> = from_value(v, "gh pr checks")?;
    for c in &mut checks {
        c.run_id = run_id_of(&c.link);
    }
    Ok(checks)
}

/// One comment of a review thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadComment {
    pub database_id: Option<u64>,
    /// `None` for a deleted account.
    pub author: Option<String>,
    pub body: String,
}

/// One review thread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub id: String,
    pub is_resolved: bool,
    pub is_outdated: bool,
    pub path: Option<String>,
    pub line: Option<u64>,
    pub comments: Vec<ThreadComment>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawThread {
    id: String,
    is_resolved: bool,
    is_outdated: bool,
    path: Option<String>,
    line: Option<u64>,
    comments: RawComments,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawComments {
    nodes: Vec<RawComment>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct RawComment {
    database_id: Option<u64>,
    author: Option<RawAuthor>,
    body: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct RawAuthor {
    login: String,
}

impl From<RawThread> for Thread {
    fn from(t: RawThread) -> Thread {
        Thread {
            id: t.id,
            is_resolved: t.is_resolved,
            is_outdated: t.is_outdated,
            path: t.path,
            line: t.line,
            comments: t
                .comments
                .nodes
                .into_iter()
                .map(|c| ThreadComment {
                    database_id: c.database_id,
                    author: c.author.map(|a| a.login),
                    body: c.body,
                })
                .collect(),
        }
    }
}

/// The review-threads page query. The first 20 comments of a thread are kept.
const THREADS_QUERY: &str = "query($owner:String!,$name:String!,$n:Int!,$c:String){repository(owner:$owner,name:$name){pullRequest(number:$n){reviewThreads(first:100,after:$c){pageInfo{hasNextPage endCursor} nodes{id isResolved isOutdated path line comments(first:20){nodes{databaseId author{login} body}}}}}}}";

/// A PR with more pages than this is not read to the end (a runaway cursor).
const MAX_THREAD_PAGES: usize = 50;

/// Every review thread of PR `n`, paged until `hasNextPage` is false. The
/// owner and name come from [`repo_slug`], looked up once by the caller.
/// Consumer: gather (trusted and untrusted threads) and the verdict.
pub fn review_threads(r: &dyn GhRunner, slug: &Slug, n: u64) -> Result<Vec<Thread>> {
    let n = n.to_string();
    let query = format!("query={THREADS_QUERY}");
    let owner = format!("owner={}", slug.owner);
    let name = format!("name={}", slug.name);
    let n_field = format!("n={n}");
    let mut threads = Vec::new();
    let mut cursor: Option<String> = None;
    for _ in 0..MAX_THREAD_PAGES {
        let cursor_field = cursor.as_ref().map(|c| format!("c={c}"));
        let mut args = vec![
            "api", "graphql", "-f", &owner, "-f", &name, "-F", &n_field, "-f", &query,
        ];
        if let Some(c) = &cursor_field {
            args.extend(["-f", c]);
        }
        let v = json(r, &args)?;
        let page = &v["data"]["repository"]["pullRequest"]["reviewThreads"];
        if page.is_null() {
            bail!("gh api graphql printed no reviewThreads for PR #{n}");
        }
        let nodes: Vec<RawThread> = from_value(page["nodes"].clone(), "reviewThreads nodes")?;
        threads.extend(nodes.into_iter().map(Thread::from));
        if page["pageInfo"]["hasNextPage"].as_bool() != Some(true) {
            return Ok(threads);
        }
        match page["pageInfo"]["endCursor"].as_str() {
            Some(c) if cursor.as_deref() != Some(c) => cursor = Some(c.to_string()),
            _ => bail!("reviewThreads has a next page but no new cursor"),
        }
    }
    bail!("reviewThreads of PR #{n} has more than {MAX_THREAD_PAGES} pages")
}

/// The open `needs-human` issues, as brief reads them
/// (`merge_ready::NEEDS_HUMAN_ISSUES_ARGS`); `merge_ready::gates` takes them.
pub fn issues_needs_human(r: &dyn GhRunner) -> Result<Vec<Value>> {
    let v = json(r, &NEEDS_HUMAN_ISSUES_ARGS)?;
    from_value(v, "gh issue list")
}

/// Whether the repository has the label `name`: `gh api
/// repos/{owner}/{repo}/labels/<name>`. A 404 is `false`; any other failure is
/// an error, so a caller that treats "no label" as "stop" also stops on an
/// outage.
pub fn label_exists(r: &dyn GhRunner, name: &str) -> Result<bool> {
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    {
        bail!("label name {name:?} is not a plain name");
    }
    let endpoint = format!("repos/{{owner}}/{{repo}}/labels/{name}");
    match r.run(&["api", &endpoint]) {
        GhOutcome::Ok(_) => Ok(true),
        failed @ GhOutcome::Failed { .. }
            if String::from_utf8_lossy(failed.first_stderr_line()).contains("HTTP 404") =>
        {
            Ok(false)
        }
        GhOutcome::Missing => bail!("gh is not installed"),
        failed @ GhOutcome::Failed { .. } => bail!(
            "gh api labels/{name} failed: {}",
            String::from_utf8_lossy(failed.first_stderr_line())
        ),
        GhOutcome::Refused { reason } => {
            bail!("internal error: gh api labels/{name} was refused: {reason}")
        }
    }
}

/// The numbers of the open issues labelled `label`. Consumer: the kill switch
/// (`agent-stop`).
pub fn issues_labelled(r: &dyn GhRunner, label: &str) -> Result<Vec<u64>> {
    let v = json(
        r,
        &[
            "issue", "list", "--label", label, "--state", "open", "--json", "number",
        ],
    )?;
    #[derive(Deserialize)]
    struct Number {
        number: u64,
    }
    let items: Vec<Number> = from_value(v, "gh issue list")?;
    Ok(items.into_iter().map(|i| i.number).collect())
}

/// `gh run view <id> --log-failed`, as text (not necessarily UTF-8). Consumer:
/// gather (the log tail of a failing check).
pub fn run_log_failed(r: &dyn GhRunner, run_id: u64) -> Result<String> {
    let id = run_id.to_string();
    let out = bytes(r, &["run", "view", &id, "--log-failed"])?;
    Ok(String::from_utf8_lossy(&out).into_owned())
}
