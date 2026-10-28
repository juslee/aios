//! `aios`: AIOS host tooling: `aios brief` and `aios checkpoint` (R3), `aios docs-check`
//! (R1), `aios guard` (R5), `aios hook` (model routing) and `aios soak` (R4); later PRs add
//! subcommands.
#![forbid(unsafe_code)]

use std::ffi::OsString;
use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "aios", version, about = "AIOS host tooling")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deterministic session briefing for /justin:brief (see `aios brief --help`)
    #[command(name = "brief", disable_help_flag = true)]
    Brief(aios_tools::cmd::brief::Args),
    /// Commit and push the work in progress of a claude/* branch, for /justin:pause (see `aios checkpoint --help`)
    #[command(name = "checkpoint", disable_help_flag = true)]
    Checkpoint(aios_tools::cmd::checkpoint::Args),
    /// Deterministic docs drift check against scripts/docs/baseline.json (exit 1 on new drift)
    #[command(name = "docs-check", infer_long_args = true, args_override_self = true)]
    DocsCheck(aios_tools::cmd::docs_check::Args),
    /// The git and team-rules guard (PreToolUse for Bash, Monitor, Agent, Task and EnterWorktree): the port of .claude/hooks/git-push-guard.py
    Guard(aios_tools::cmd::guard::Args),
    /// Claude Code hook programs: read one JSON payload on stdin; exit 0 on every payload (a usage error exits 2)
    Hook(aios_tools::cmd::hook::Args),
    /// Boot AIOS repeatedly under QEMU and classify every boot (see `aios soak --help`)
    #[command(name = "soak", disable_help_flag = true)]
    Soak(aios_tools::cmd::soak::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Hook(args) => {
            let ctx = aios_tools::cmd::hook::Ctx::from_env();
            aios_tools::cmd::hook::run(
                &args,
                &ctx,
                std::io::stdin().lock(),
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            );
            ExitCode::SUCCESS
        }
        Command::Guard(_) => {
            let env = std::env::vars_os()
                .map(|(k, v)| {
                    (
                        k.to_string_lossy().into_owned(),
                        v.to_string_lossy().into_owned(),
                    )
                })
                .collect();
            ExitCode::from(aios_tools::cmd::guard::run(
                &mut std::io::stdin().lock(),
                &env,
                &mut std::io::stdout().lock(),
                &mut std::io::stderr().lock(),
            ))
        }
        Command::DocsCheck(args) => {
            let cwd = match std::env::current_dir() {
                Ok(dir) => dir,
                Err(err) => {
                    eprintln!("docs-check: cannot read the current directory: {err}");
                    return ExitCode::from(2);
                }
            };
            let stdout = std::io::stdout();
            let mut out = stdout.lock();
            let result = aios_tools::cmd::docs_check::run(&args, &cwd, &mut out);
            let flushed = out.flush();
            match (result, flushed) {
                (Ok(code), Ok(())) => ExitCode::from(code),
                (Ok(_), Err(err)) => {
                    eprintln!("docs-check: cannot write output: {err}");
                    ExitCode::from(2)
                }
                (Err(err), _) => {
                    eprintln!("docs-check: {err:#}");
                    ExitCode::from(2)
                }
            }
        }
        Command::Brief(_) => raw_command("brief", aios_tools::cmd::brief::run),
        Command::Checkpoint(_) => raw_command("checkpoint", aios_tools::cmd::checkpoint::run),
        Command::Soak(_) => raw_command("soak: error", aios_tools::cmd::soak::run),
    }
}

/// The signature of a subcommand that parses its raw arguments itself.
type RawRun = fn(&[OsString], &Path, &mut dyn Write, &mut dyn Write) -> anyhow::Result<u8>;

/// Run a subcommand on the raw arguments after its name, parsed in argv order by the
/// command itself: clap only has to accept them (and drops a leading `--`, which the
/// soak command line treats as the end of its options). `prefix` starts every message.
fn raw_command(prefix: &str, run: RawRun) -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("{prefix}: cannot read the current directory: {err}");
            return ExitCode::from(2);
        }
    };
    let raw: Vec<OsString> = std::env::args_os().skip(2).collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    let result = run(&raw, &cwd, &mut out, &mut err);
    let flushed = out.flush();
    match (result, flushed) {
        (Ok(code), Ok(())) => ExitCode::from(code),
        (Ok(_), Err(e)) => {
            let _ = writeln!(err, "{prefix}: cannot write output: {e}");
            ExitCode::from(2)
        }
        (Err(e), _) => {
            let _ = writeln!(err, "{prefix}: {e:#}");
            ExitCode::from(2)
        }
    }
}
