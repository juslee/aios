//! `aios`: AIOS host tooling. R1 ships `aios docs-check` and R4 `aios soak`;
//! later PRs add subcommands.
#![forbid(unsafe_code)]

use std::ffi::OsString;
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
    /// Boot AIOS repeatedly under QEMU and classify every boot (see `aios soak --help`)
    #[command(name = "soak", disable_help_flag = true)]
    Soak(aios_tools::cmd::soak::Args),
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
        Command::Soak(_) => soak(),
    }
}

/// `aios soak`. clap drops a leading `--`, which the soak command line treats as
/// the end of its options, so the raw arguments after `soak` are parsed instead.
fn soak() -> ExitCode {
    let cwd = match std::env::current_dir() {
        Ok(dir) => dir,
        Err(err) => {
            eprintln!("soak: error: cannot read the current directory: {err}");
            return ExitCode::from(2);
        }
    };
    let raw: Vec<OsString> = std::env::args_os().skip(2).collect();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    let stderr = std::io::stderr();
    let mut err = stderr.lock();
    let result = aios_tools::cmd::soak::run(&raw, &cwd, &mut out, &mut err);
    let flushed = out.flush();
    match (result, flushed) {
        (Ok(code), Ok(())) => ExitCode::from(code),
        (Ok(_), Err(e)) => {
            let _ = writeln!(err, "soak: error: cannot write output: {e}");
            ExitCode::from(2)
        }
        (Err(e), _) => {
            let _ = writeln!(err, "soak: error: {e:#}");
            ExitCode::from(2)
        }
    }
}
