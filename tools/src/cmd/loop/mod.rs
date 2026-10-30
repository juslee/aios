//! `aios loop`, the PR loop.
//!
//! R2a is `aios loop review`, read-only: every gh call goes through
//! `crate::gh::GhCli`, which runs `gh::is_read`, no stage gets a write tool
//! (not even a shell), and nothing is committed or pushed. R2b adds the write
//! side.
//!
//! `aios loop review <PR>... [--tree DIR] [--budget-usd N]` reviews each PR in
//! order, one round each:
//! 1. preflight: the kill switch, the PR (team owner, fork), the QEMU lock and
//!    the per-PR lock, all before any `claude` call;
//! 2. gather ([`gather`]): a detached round worktree of the PR head, and the
//!    facts that need no model;
//! 3. review ([`review`]): three lenses in parallel, then verify;
//! 4. verdict ([`verdict`]): `ready`, `pending` or `needs-human`, reported only;
//! 5. report: a block on stdout, `report.json` and `report.md` in the run
//!    directory, one `findings.jsonl` record per verified finding. The round
//!    worktree is discarded.
//!
//! Exit codes: 0 every PR is `clean` and `ready`; 1 any finding, `incomplete`,
//! `pending`, `needs-human` or refusal; 2 a usage or internal error; 4 stopped
//! (kill switch or quiet window); 129, 130, 131, 143 a signal (the stage's
//! process group is stopped and the run directory is kept). The binary never
//! exits 3: that is the shim's.

pub mod claude;
pub mod config;
pub mod gate;
pub mod gather;
pub mod git;
pub mod github;
pub mod hive;
pub mod review;
pub mod schema;
pub mod settings;
pub mod state;
pub mod verdict;

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use serde_json::{json, Value};

use self::claude::ClaudeCli;
use self::config::Config;
use self::gather::{gather, one_line, show_path, token, Facts, Fence, GatherCtx, Source};
use self::git::{fetch_pr_head, worktree_add_detached, worktree_discard};
use self::github::{pr_view, repo_slug, LOOP_GH_ENV};
use self::review::{run_review, Abort, Prompts, Review, ReviewCtx, ReviewStatus, LENSES, VERIFY};
use self::state::{common_dir, fork_check, quiet_check, team_owner_check, LockError, State};
use self::verdict::{verdict, Inputs, Verdict};
use crate::cmd::hook::write_atomic;
use crate::cmd::soak::signals::Interrupts;
use crate::gh::GhCli;

/// The directory under the main checkout that holds round worktrees.
const ROUND_ROOT: &str = ".claude/worktrees";

/// The exit code of a stopped run (kill switch, quiet window).
const EXIT_STOPPED: u8 = 4;

#[derive(Parser, Debug)]
pub struct Args {
    #[command(subcommand)]
    pub command: Cmd,
}

#[derive(Subcommand, Debug)]
pub enum Cmd {
    /// Review pull requests, read-only: findings and a merge verdict, reported and never written to GitHub
    Review(ReviewArgs),
}

#[derive(clap::Args, Debug)]
pub struct ReviewArgs {
    /// The pull request numbers, reviewed in this order
    #[arg(required = true, value_name = "PR")]
    pub prs: Vec<u64>,
    /// Read the prompts, config and notes from DIR instead of origin/main, and run from any build
    #[arg(long, value_name = "DIR")]
    pub tree: Option<PathBuf>,
    /// Spend at most this many USD per PR (default: per_pr_usd of the config)
    #[arg(long, value_name = "N")]
    pub budget_usd: Option<f64>,
}

/// Run `aios loop`. `Ok` carries the exit code; `Err` is a usage or internal
/// error (exit 2).
pub fn run(args: &Args, cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    match &args.command {
        Cmd::Review(review) => run_review_command(review, cwd, out, err),
    }
}

/// Everything one run shares across its PRs.
struct Run {
    main: PathBuf,
    config: Config,
    prompts: Prompts,
    source: Source,
    state: State,
    gh: GhCli,
    claude: ClaudeCli,
    interrupts: Arc<Interrupts>,
    /// `AIOS_TEAM`.
    team: Option<String>,
}

/// How one PR ended.
enum PrEnd {
    /// Reviewed; the flag is "clean and ready".
    Reviewed(bool),
    /// Not reviewed; the run goes on with the next PR (exit 1).
    Refused,
    /// The run ends with this exit code.
    Stop(u8),
}

fn run_review_command(
    args: &ReviewArgs,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let main = main_checkout(cwd)?;
    state::self_location(&main, args.tree.is_some())?;
    let source = match &args.tree {
        Some(dir) => Source::Tree(
            dir.canonicalize()
                .with_context(|| format!("cannot resolve --tree {}", dir.display()))?,
        ),
        None => Source::OriginMain(main.clone()),
    };
    let mut config = Config::from_json(&source.read("scripts/agent/loop-config.json")?)?;
    if let Some(budget) = args.budget_usd {
        if !budget.is_finite() || budget < config.per_call_usd {
            bail!(
                "--budget-usd {budget} must be a number of at least per_call_usd ({})",
                config.per_call_usd
            );
        }
        config.per_pr_usd = budget;
    }
    let prompts = Prompts::load(&source)?;
    let state = State::for_repo(&main)?;
    let interrupts = Arc::new(Interrupts::install()?);
    let claude = ClaudeCli::from_env()?.with_interrupts(Arc::clone(&interrupts));
    let run = Run {
        gh: GhCli {
            cwd: main.clone(),
            env: LOOP_GH_ENV,
        },
        main,
        config,
        prompts,
        source,
        state,
        claude,
        interrupts,
        team: std::env::var("AIOS_TEAM").ok().filter(|t| !t.is_empty()),
    };
    let mut all_ready = true;
    for &n in &args.prs {
        if let Some(code) = run.interrupts.pending() {
            return Ok(code);
        }
        match review_one(&run, n, out, err).with_context(|| format!("pr #{n}"))? {
            PrEnd::Reviewed(ready) => all_ready &= ready,
            PrEnd::Refused => all_ready = false,
            PrEnd::Stop(code) => return Ok(code),
        }
    }
    Ok(u8::from(!all_ready))
}

/// The main checkout of the repository `cwd` belongs to: the parent of the git
/// common directory.
fn main_checkout(cwd: &Path) -> Result<PathBuf> {
    let common = common_dir(cwd)?;
    match (common.file_name(), common.parent()) {
        (Some(name), Some(parent)) if name == ".git" => Ok(parent.to_path_buf()),
        _ => bail!(
            "{} is not the .git directory of a checkout; run aios loop from the main checkout",
            common.display()
        ),
    }
}

/// A detached worktree of the PR head, thrown away when dropped (also on an
/// error or a signal).
struct RoundWorktree {
    main: PathBuf,
    path: PathBuf,
}

impl RoundWorktree {
    fn create(main: &Path, n: u64, sha: &str) -> Result<RoundWorktree> {
        let path = main.join(ROUND_ROOT).join(format!("loop-pr{n}"));
        if path.exists() {
            // A round that was killed leaves its worktree behind.
            worktree_discard(main, &path, std::slice::from_ref(&path))
                .context("a worktree of an earlier round is in the way")?;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("cannot create {}", parent.display()))?;
        }
        worktree_add_detached(main, &path, sha)?;
        Ok(RoundWorktree {
            main: main.to_path_buf(),
            path,
        })
    }
}

impl Drop for RoundWorktree {
    fn drop(&mut self) {
        if let Err(e) = worktree_discard(&self.main, &self.path, std::slice::from_ref(&self.path)) {
            eprintln!(
                "aios loop: warning: cannot discard {}: {e:#}",
                self.path.display()
            );
        }
    }
}

fn review_one(run: &Run, n: u64, out: &mut dyn Write, err: &mut dyn Write) -> Result<PrEnd> {
    if let Err(stop) = run.state.kill_switch(&run.gh) {
        writeln!(err, "aios loop: pr #{n}: {stop}")?;
        return Ok(PrEnd::Stop(EXIT_STOPPED));
    }
    let pr = pr_view(&run.gh, n)?;
    if let Err(refusal) = team_owner_check(&pr, n, run.team.as_deref()) {
        writeln!(err, "{refusal}")?;
        return Ok(PrEnd::Refused);
    }
    if let Some(why) = fork_check(&pr) {
        writeln!(out, "pr #{n}: needs-human: {why}")?;
        return Ok(PrEnd::Refused);
    }
    if let Err(stop) = quiet_check(&run.main) {
        writeln!(err, "aios loop: pr #{n}: {stop}")?;
        return Ok(PrEnd::Stop(EXIT_STOPPED));
    }
    let _lock = match run.state.lock(n) {
        Ok(lock) => lock,
        Err(held @ LockError::Held { .. }) => {
            writeln!(err, "pr #{n}: {held}")?;
            return Ok(PrEnd::Refused);
        }
        Err(LockError::Failed(e)) => return Err(e),
    };
    let slug = repo_slug(&run.gh)?;
    let head = fetch_pr_head(&run.main, n)?;
    let round = RoundWorktree::create(&run.main, n, &head)?;
    let settings = settings::materialise(&run.main, &run.state, n)?;
    let run_dir = run.state.new_run_dir(n)?;

    let facts = gather(&GatherCtx {
        round: &round.path,
        n,
        pr: &pr,
        config: &run.config,
        source: &run.source,
        gh: &run.gh,
        slug: &slug,
        state: &run.state,
        fence: Fence::random()?,
    })?;
    if let Some(code) = run.interrupts.pending() {
        return Ok(PrEnd::Stop(code));
    }

    let kill_switch = || run.state.kill_switch(&run.gh);
    let reviewed = run_review(&ReviewCtx {
        config: &run.config,
        claude: &run.claude,
        prompts: &run.prompts,
        facts: &facts,
        hard: pr.has_label("hard"),
        cwd: &round.path,
        settings: &settings,
        run_dir: &run_dir,
        kill_switch: &kill_switch,
    });
    let review = match reviewed {
        Ok(review) => review,
        Err(Abort::Interrupted(code)) => return Ok(PrEnd::Stop(code)),
        Err(Abort::Stopped(stop)) => {
            writeln!(err, "aios loop: pr #{n}: {stop}")?;
            return Ok(PrEnd::Stop(EXIT_STOPPED));
        }
    };

    // The verdict reads the PR as it is now, not as it was when the round began,
    // and the kill switch once more: a review can take minutes.
    if let Err(stop) = kill_switch() {
        writeln!(err, "aios loop: pr #{n}: {stop}")?;
        return Ok(PrEnd::Stop(EXIT_STOPPED));
    }
    let fresh = pr_view(&run.gh, n)?;
    let moved = facts.head_moved
        || (!fresh.facts.head_ref_oid.is_empty() && fresh.facts.head_ref_oid != facts.head);
    let verdict = verdict(&Inputs {
        review: review.status,
        pr: &fresh.raw,
        issues: facts.issues.as_deref(),
        open_threads: facts.open_threads,
        gated: &facts.gated,
        cross_check: facts.cross_check.as_deref(),
        head_moved: moved,
    });

    for record in review.records(n, &facts.head, &run.prompts) {
        run.state.append_finding(&record)?;
    }
    let report = Report {
        n,
        facts: &facts,
        review: &review,
        verdict: &verdict,
        prompts: &run.prompts,
    };
    let json_path = run_dir.join("report.json");
    let md_path = run_dir.join("report.md");
    let mut json = serde_json::to_string_pretty(&report.json()).context("cannot encode report")?;
    json.push('\n');
    write_atomic(&json_path, json.as_bytes())?;
    write_atomic(&md_path, report.markdown().as_bytes())?;
    write!(out, "{}", report.block())?;
    writeln!(out, "report: {}", json_path.display())?;
    writeln!(out, "report: {}", md_path.display())?;
    Ok(PrEnd::Reviewed(
        review.status == ReviewStatus::Clean && verdict.is_ready(),
    ))
}

/// What the report files and the stdout block are written from.
struct Report<'a> {
    n: u64,
    facts: &'a Facts,
    review: &'a Review,
    verdict: &'a Verdict,
    prompts: &'a Prompts,
}

impl Report<'_> {
    fn short_head(&self) -> &str {
        &self.facts.head[..self.facts.head.len().min(7)]
    }

    /// The per-PR block of stdout (the report paths follow it).
    fn block(&self) -> String {
        let review = self.review;
        let mut out = format!(
            "pr #{} @ {}: {} ({} confirmed, {} pre-existing; ${:.2})\n",
            self.n,
            self.short_head(),
            review.status.as_str(),
            review.confirmed().len(),
            review.pre_existing().len(),
            review.cost,
        );
        for r in review.confirmed() {
            out.push_str(&finding_line(r, ""));
        }
        for r in review.pre_existing() {
            out.push_str(&finding_line(r, " (pre-existing)"));
        }
        for why in &review.incomplete {
            out.push_str(&format!("  incomplete: {}\n", one_line(why, 300)));
        }
        if self.facts.plan_present {
            out.push_str("  note: the branch carries its working plan (plans-not-empty)\n");
        }
        for note in &self.facts.notes {
            out.push_str(&format!("  note: {}\n", one_line(note, 300)));
        }
        out.push_str(&format!(
            "merge: {}\n",
            one_line(&self.verdict.line(), 1000)
        ));
        out
    }

    fn json(&self) -> Value {
        let lessons = &self.facts.lessons;
        let versions: serde_json::Map<String, Value> = LENSES
            .iter()
            .copied()
            .chain([VERIFY])
            .map(|stage| (stage.to_string(), json!(self.prompts.version(stage))))
            .collect();
        json!({
            "pr": self.n,
            "head": self.facts.head,
            "base": self.facts.base,
            "review": self.review.to_json(),
            "merge": {
                "verdict": self.verdict.class(),
                "reasons": self.verdict.reasons(),
            },
            "gated": self.facts.gated,
            "cross_check": self.facts.cross_check,
            "plan_present": self.facts.plan_present,
            "notes": self.facts.notes,
            "lessons": {
                "tags": lessons.tags,
                "notes": lessons.notes.iter().map(|n| &n.path).collect::<Vec<_>>(),
                "dropped": lessons.dropped,
            },
            "prompt_versions": versions,
        })
    }

    fn markdown(&self) -> String {
        let mut out = format!("# Review of PR #{} at {}\n\n", self.n, self.short_head());
        out.push_str("```text\n");
        out.push_str(&self.block());
        out.push_str("```\n");
        if !self.review.reported.is_empty() {
            out.push_str("\n## Findings\n");
        }
        for r in &self.review.reported {
            let f = &r.finding;
            out.push_str(&format!(
                "\n- **{}** `{}:{}` [{}] ({}, {}) {}\n  - evidence: {}\n  - verdict: {}\n",
                token(&f.id),
                show_path(&f.file),
                f.line,
                f.severity.as_str(),
                token(&f.category),
                token(&f.lens),
                one_line(&f.claim, 400),
                one_line(&f.evidence, 600),
                one_line(&r.reason, 400),
            ));
        }
        out.push_str("\n## Stages\n\n");
        for s in &self.review.stages {
            out.push_str(&format!(
                "- {}: {} on {}, {} attempt(s), ${:.2}{}\n",
                s.stage,
                one_line(&s.result, 200),
                s.model,
                s.attempts,
                s.cost,
                if s.fallback { " (fable fallback)" } else { "" },
            ));
        }
        out
    }
}

fn finding_line(r: &review::Reported, suffix: &str) -> String {
    let f = &r.finding;
    format!(
        "  - [{}] {}:{} {}: {}{suffix}\n",
        f.severity.as_str(),
        show_path(&f.file),
        f.line,
        token(&f.category),
        one_line(&f.claim, 300),
    )
}
