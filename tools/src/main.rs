//! `aios`: AIOS host tooling. R1 ships `aios docs-check`; later PRs add subcommands.
#![forbid(unsafe_code)]

use std::io::Write;
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
    /// Deterministic docs drift check against scripts/docs/baseline.json (exit 1 on new drift)
    #[command(name = "docs-check", infer_long_args = true, args_override_self = true)]
    DocsCheck(aios_tools::cmd::docs_check::Args),
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
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
    }
}
