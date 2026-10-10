//! `aios soak`: boot AIOS repeatedly under QEMU and classify every boot, or
//! classify saved serial logs (`--classify`). A port of `scripts/soak-qemu.sh`
//! (deleted in R4; the parity oracle is its blob at `212df62`).
//!
//! The command line is parsed by hand, in the script's `case` order, because
//! the script's syntax is not clap's: `key=value` aliases, options only before
//! the first log file, and a `--` that ends the options. `main` passes the raw
//! arguments, since clap drops a leading `--`.
//!
//! Accepted divergences from the script, beyond those in [`runner`]:
//! - Messages start `soak: error:` and `soak: warning:` (the script's were
//!   `soak-qemu: ...`), and the usage text names `aios soak`.
//! - A log file that exists but cannot be read ends with a `soak: error:`
//!   message and status 2 (the script's `set -e` exited 1 after bash's
//!   redirection error, `<script>: line N: <file>: Permission denied`).
//! - A `--runs`, `--secs` or `--stall-secs` value that is not valid UTF-8 is
//!   rejected as not a positive integer, as before; one in a message (`--mode`)
//!   is shown with U+FFFD for its invalid bytes.

pub mod awk;
pub mod classify;
pub mod host;
pub mod report;
pub mod runner;
pub mod signals;
pub mod stats;
pub mod tripwire;

use std::ffi::OsString;
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

use anyhow::{bail, Result};

use runner::Config;

/// Arguments of `aios soak`. clap only collects them; [`run`] parses the raw
/// argument list itself (see the module docs).
#[derive(clap::Args, Debug)]
pub struct Args {
    /// Options, key=value aliases and log files; see `aios soak --help`.
    #[arg(trailing_var_arg = true, allow_hyphen_values = true, num_args = 0..)]
    pub args: Vec<OsString>,
}

/// QEMU start to the Gate 1 bench takes about 6-8 s on current hosts. A `--secs`
/// shorter than `--stall-secs` plus this budget mostly produces INCONCLUSIVE boots.
const BOOT_BUDGET_SECS: u64 = 20;

/// `aios soak --help`.
pub const USAGE: &str = r#"Usage: aios soak [options] [key=value ...]
       aios soak --classify [--stall-secs S] [--out DIR] LOG...
       just soak [options] [key=value ...]

Boot AIOS N times under QEMU (same arguments as `just run` / `just run-gpu`),
save each boot's serial log, and classify every boot as exactly one of:

  PCZERO        an exception report with ELR=0x0000000000000000 (jump to PC 0).
                Unlocked output from several CPUs can split a report, so the
                ELR may be up to 3 lines below the "EXCEPTION[CPU n]:" prefix;
                an instruction abort with FAR=0 (EC=0x20/0x21, or the line
                "Instruction Abort at 0x0000000000000000") counts as well
  PANIC-LOCK    a PANIC whose message (the line after "PANIC: panicked at
                <file>:<line>:<col>:", joined to it) contains "lock re-entry:"
                (crash-fix step 1b's lock detector)
  PANIC         any other "PANIC: " from the kernel panic handler
  EXCEPTION     any other exception report: "EXCEPTION[CPU n]:" (EL1),
                "DATA ABORT (EL0)", "INST ABORT (EL0)", "UNKNOWN EXCEPTION
                (EL0)", or an edk2-format "Synchronous Exception at 0x..."
                report from the firmware or the UEFI stub. A report whose
                prefix was split by another CPU's output is still caught by
                its register fields ("ESR=0x.. EC=0x.." or "EC=0x.. FAR=0x..
                ELR=0x.." on EL1, "(EL0): FAR=0x" / "(EL0): EC=0x" on EL0),
                or by a "Data/Instruction Abort at 0x" line with no report
                in the 4 lines above it
  WEDGE-STUCK   no fatal report, the boot is not healthy at the end of the run
                (see CLEAN), and it had more than --stall-secs to get there:
                the CPU 0 heartbeat never printed, stayed at tick 0, or stopped
                advancing
  WEDGE-ALIVE   as WEDGE-STUCK, but the heartbeat kept running: the Gate 1
                bench never completed, or (gpu mode) a GPU marker is missing
  INCONCLUSIVE  not a result about the kernel:
                - the UEFI stub never ran: no "AIOS UEFI stub" line and no
                  kernel output, whatever QEMU's exit status (QEMU failed to
                  start, or the firmware never loaded the stub). On the first
                  boot of a soak this is a setup error instead (exit 2)
                - QEMU was killed by a signal before the time limit (exit
                  status above 128 other than the harness's own 124/137, or
                  137 before the limit) and no fatal report came first
                - the symptoms of a wedge, but the run ended no more than
                  --stall-secs after the boot's last progress (kernel start,
                  heartbeat, bench start), so the boot was cut short rather
                  than shown to be stuck
  DEGRADED      CLEAN by every other rule, but the Gate 1 IPC line
                ("[bench] IPC round-trip (same core): avg=N us, ...
                (N iters)", the first one in the log) reports fewer than
                10000 iterations, is missing, or is cut before its count.
                The count is shown where a CLEAN boot shows its detail, and
                in summary.tsv's ipc_iters column ("-" when unreadable)
  CLEAN         no fatal report; the heartbeat advanced past tick 0 and a new
                heartbeat arrived within the last --stall-secs of the run;
                "=== Gate 1 Complete ===" was printed; the Gate 1 IPC line
                reports 10000 iterations; and in gpu mode the GpuReady,
                InputReady and "display handoff complete" markers were
                printed

Precedence: stub never ran (INCONCLUSIVE) > PCZERO/PANIC-LOCK/PANIC/EXCEPTION
> QEMU killed by a signal (INCONCLUSIVE) > WEDGE-STUCK/WEDGE-ALIVE/INCONCLUSIVE
(cut short) > DEGRADED > CLEAN. When a log holds several fatal reports, the
earliest one decides the class (later ones are usually fallout, e.g. a data
abort after a panic, so a "lock re-entry:" panic after an exception stays
EXCEPTION); the count is kept in the detail. Soaks from before crash-fix step 1a report
both wedge classes as WEDGE, PANIC-LOCK as PANIC, and DEGRADED as CLEAN.

Heartbeat timing comes from the harness: it polls the log every second and
appends a "[soak] meta" line recording when the kernel started, when the first
heartbeat, the Gate 1 bench header and "=== Gate 1 Complete ===" appeared,
when the heartbeat last advanced, and the longest gap between heartbeat
advances after the bench completed (hb_max_gap; a trace only, reported in the
detail when it exceeds --stall-secs). Silence is measured to the planned end
of the boot (--secs), not to QEMU's exit after the time limit. A log without
that line (not written by this harness) is classified log-only: a heartbeat
that stops after tick 0 cannot be detected there, and a cut-short boot cannot
be told apart from a wedge.

Options:
  --runs N           number of sequential boots (default 10)
  --secs T           wall-clock seconds per boot (default 75)
  --mode text|gpu    text: `just run` devices (-nographic, ramfb)
                     gpu:  `just run-gpu` devices plus -display none
                     (default text)
  --out DIR          output directory; must be new or empty and must not be
                     the repository root (default target/soak/<timestamp>-<mode>).
                     With --classify: also write summary.tsv and summary.md
                     for the logs there (no default: without it, nothing is
                     written)
  --stall-secs S     heartbeat silence at the end that counts as a wedge
                     (default 15)
  --no-build         skip `just disk` and boot the existing ESP image
  --fresh-data       fresh zeroed 256 MiB data disk for every boot (default)
  --reuse-data       boot every run on the repository's data.img, so disk
                     state carries over between boots (like `just run`)
  --report-only      exit 0 even when some boots are not CLEAN
  --classify LOG...  classify existing log files instead of booting; with
                     --out, number them in the order given, take each one's
                     timing from its "[soak] meta" line, and give "-" for what
                     logs cannot tell (commit, QEMU, firmware, host, load)
  -h, --help         show this help

key=value aliases (so `just soak runs=5 mode=gpu` works): runs=N secs=T
mode=text|gpu out=DIR stall_secs=S report_only=1

`just soak` runs this command from the directory you invoke just in, so
relative out= and --classify paths resolve against that directory. The soak
boots the git checkout that contains that directory.

Output directory: run-NN.log (raw serial output plus a trailing "[soak] meta"
line), summary.tsv, summary.md and build.log. The ESP snapshot and the fresh
data disks live in a private .scratch.* subdirectory that is removed at exit.

summary.tsv has one row per boot, with a header line. Read it by column name:
the first 22 columns are the script-era ones (class, ticks, timing, markers,
detail, first fatal line, INFO lines, log); then ipc_avg_us (the Gate 1 IPC
average in whole us, as the kernel truncates it) and ipc_iters; reentry_lock,
reentry_ctx and reentry_holder_irqs (a PANIC-LOCK's message); ev_ph, ev_self,
ev_self_irq (ctx=irq or irq-exit) and ev_stuck ("[tripwire-ev]" lock events);
tw_v, tw_src, tw_cpu, tw_t, tw_ncpu and one tw_<key> per tripwire key, from
the last complete "[tripwire]" line (docs/kernel/observability.md 6.5: its n=
must equal its key=value count; a key it leaves out is 0; a line of another
schema version fills only tw_v and tw_line); g1_elrmm, from the last complete
src=g1 line; then the whole lines g1_line and tw_line. "-" means unreadable,
or no such line. summary.md has the settings, the class counts, the CLEAN
rate with its 95% Wilson interval, the mean Gate 1 IPC average over CLEAN
boots, the tripwire counters by class, and the per-boot table.

Environment: AIOS_EDK2_FW overrides the firmware path, as in the justfile.
Requires qemu-system-aarch64, just, mtools (for `just disk`) and the POSIX
kill utility. Each boot's QEMU runs in its own process group: the harness
sends the group SIGTERM when --secs run out and SIGKILL 10 s later, and
records QEMU's exit status as timeout(1) reported it (124 when the time limit
stopped QEMU, 137 when SIGKILL was needed). Ctrl-Z (SIGTSTP) stops the running
QEMU along with the harness; on resume, the time limit and the boot's
timings are moved back by the time spent stopped.

Exit status: 0 when every boot is CLEAN (or with --report-only), 1 when some
boot is not CLEAN, 2 on a usage or setup error (bad arguments, unusable --out,
build failure, the UEFI stub never running on the first boot) -- setup errors
exit 2 even with --report-only. 130 on SIGINT, 143 on SIGTERM, 129 on SIGHUP,
131 on SIGQUIT; QEMU is stopped first. SIGKILL cannot be caught: a harness
killed by it leaves the running QEMU behind until it is killed by hand, so
stop a soak with one of the four signals above. Likewise SIGSTOP, or SIGTTIN
or SIGTTOU stopping a background soak, stops only the harness: QEMU runs on
past its time limit until the harness is continued. A signal ignored when
the soak started is caught all the same: nohup, or a background job of a
non-interactive shell, does not protect a soak from SIGHUP, SIGINT or
SIGQUIT, so run long soaks under setsid, tmux or screen.
"#;

/// What the command line asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Request {
    Help,
    Classify {
        files: Vec<OsString>,
        stall_override: Option<u64>,
        report_only: bool,
        /// `--out DIR`: also write `summary.tsv` and `summary.md` there.
        out: Option<OsString>,
    },
    Soak(Config),
}

/// The value as a positive decimal integer, as `is_uint "$v" && [ "$v" -ge 1 ]`
/// accepts it: ASCII digits only, at least 1, and within bash's 64-bit `test`.
fn positive(value: &OsString) -> Option<u64> {
    let v = value.as_bytes();
    if v.is_empty() || !v.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let n: i64 = std::str::from_utf8(v).ok()?.parse().ok()?;
    u64::try_from(n).ok().filter(|&n| n >= 1)
}

/// `truthy`: the script's booleans for `report_only=`.
fn truthy(value: &[u8]) -> Result<bool> {
    match value {
        b"1" | b"true" | b"yes" | b"on" => Ok(true),
        b"0" | b"false" | b"no" | b"off" | b"" => Ok(false),
        _ => bail!(
            "expected a boolean (1/0, true/false), got '{}'",
            show(value)
        ),
    }
}

fn show(v: &[u8]) -> String {
    String::from_utf8_lossy(v).into_owned()
}

/// The option a `--opt=value` or `key=value` word sets, and its value.
fn assignment(arg: &[u8]) -> Option<(&'static str, &[u8])> {
    const FORMS: [(&[u8], &str); 11] = [
        (b"--runs=", "runs"),
        (b"runs=", "runs"),
        (b"--secs=", "secs"),
        (b"secs=", "secs"),
        (b"--mode=", "mode"),
        (b"mode=", "mode"),
        (b"--out=", "out"),
        (b"out=", "out"),
        (b"--stall-secs=", "stall"),
        (b"stall_secs=", "stall"),
        (b"report_only=", "report_only"),
    ];
    FORMS
        .iter()
        .find(|(prefix, _)| arg.starts_with(prefix))
        .map(|(prefix, key)| (*key, &arg[prefix.len()..]))
}

/// Parse `aios soak`'s arguments. Warnings go to `err`; usage errors are `Err`.
pub fn parse(args: &[OsString], err: &mut dyn Write) -> Result<Request> {
    let mut runs = OsString::from("10");
    let mut secs = OsString::from("75");
    let mut mode = OsString::from("text");
    let mut out: Option<OsString> = None;
    let mut stall = OsString::from("15");
    let mut stall_given = false;
    let mut build = true;
    let mut report_only = false;
    let mut fresh_data = true;
    let mut classify = false;
    let mut positional: Vec<OsString> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        let arg = args[i].as_bytes();
        let mut set = |key: &str, value: OsString| -> Result<()> {
            match key {
                "runs" => runs = value,
                "secs" => secs = value,
                "mode" => mode = value,
                // `[ -n "$OUT" ] || OUT=<default>`: an empty value selects the default.
                "out" => out = (!value.is_empty()).then_some(value),
                "stall" => {
                    stall = value;
                    stall_given = true;
                }
                _ => report_only = truthy(value.as_bytes())?,
            }
            Ok(())
        };
        match arg {
            b"--runs" | b"--secs" | b"--mode" | b"--out" | b"--stall-secs" => {
                let Some(value) = args.get(i + 1) else {
                    bail!("option {} needs a value", show(arg));
                };
                let key = match arg {
                    b"--runs" => "runs",
                    b"--secs" => "secs",
                    b"--mode" => "mode",
                    b"--out" => "out",
                    _ => "stall",
                };
                set(key, value.clone())?;
                i += 2;
                continue;
            }
            b"--report-only" => report_only = true,
            b"--no-build" => build = false,
            b"--fresh-data" => fresh_data = true,
            b"--reuse-data" => fresh_data = false,
            b"--classify" => classify = true,
            b"-h" | b"--help" => return Ok(Request::Help),
            b"--" => {
                positional = args[i + 1..].to_vec();
                break;
            }
            _ => {
                if let Some((key, value)) = assignment(arg) {
                    set(key, OsString::from(std::ffi::OsStr::from_bytes(value)))?;
                } else if arg.starts_with(b"-") {
                    bail!("unknown option: {} (see --help)", show(arg));
                } else {
                    if !classify {
                        bail!("unexpected argument: {} (see --help)", show(arg));
                    }
                    positional = args[i..].to_vec();
                    break;
                }
            }
        }
        i += 1;
    }

    let Some(stall_n) = positive(&stall) else {
        bail!("--stall-secs must be a positive integer");
    };
    if classify {
        return Ok(Request::Classify {
            files: positional,
            stall_override: stall_given.then_some(stall_n),
            report_only,
            out,
        });
    }
    if let Some(first) = positional.first() {
        bail!(
            "unexpected argument: {} (see --help)",
            show(first.as_bytes())
        );
    }
    let Some(runs_n) = positive(&runs) else {
        bail!("--runs must be a positive integer");
    };
    let Some(secs_n) = positive(&secs) else {
        bail!("--secs must be a positive integer");
    };
    let (stall_s, secs_s) = (show(stall.as_bytes()), show(secs.as_bytes()));
    if stall_n >= secs_n {
        bail!("--stall-secs ({stall_s}) must be smaller than --secs ({secs_s})");
    }
    if secs_n < stall_n.saturating_add(BOOT_BUDGET_SECS) {
        writeln!(
            err,
            "soak: warning: --secs {secs_s} leaves under {BOOT_BUDGET_SECS}s beyond --stall-secs {stall_s} for the boot itself (about 6-8 s to the Gate 1 bench); late boots will be INCONCLUSIVE, and a wedge in the last {stall_s}s of a boot is never seen"
        )?;
    }
    let mode = show(mode.as_bytes());
    if mode != "text" && mode != "gpu" {
        bail!("--mode must be text or gpu, got '{mode}'");
    }
    Ok(Request::Soak(Config {
        runs_raw: show(runs.as_bytes()),
        runs: runs_n,
        secs_raw: secs_s,
        secs: secs_n,
        stall_raw: stall_s,
        mode,
        out,
        build,
        report_only,
        fresh_data,
    }))
}

/// The one value every log's footer gives for `key`, `mixed` when they
/// differ, or `-` when none gives one.
fn common_value(footers: &[runner::Footer], key: &str) -> String {
    let mut values = footers.iter().filter_map(|f| f.get(key));
    match values.next() {
        None => "-".to_string(),
        Some(first) if values.all(|v| v == first) => show(first),
        Some(_) => "mixed".to_string(),
    }
}

/// `run_classify`: classify each log, print its summary line (and, when it is
/// not CLEAN, its first fatal line and last INFO lines). Exit 1 when some log
/// is not CLEAN, unless `report_only`.
///
/// With `out`, also write `summary.tsv` and `summary.md` there, as a soak
/// writes them: the boots are numbered in the order given, the `log` column
/// is each path as given, and the timing columns come from each log's
/// `[soak] meta` footer (`-` without one). What a set of logs cannot tell
/// (the commit, QEMU, the firmware, the host, the load before and after) is
/// `-`; the mode, `--secs` and the stall limit come from the footers when
/// they agree.
pub fn classify_files(
    files: &[OsString],
    stall_override: Option<u64>,
    report_only: bool,
    out_arg: Option<&OsString>,
    cwd: &Path,
    out: &mut dyn Write,
) -> Result<u8> {
    if files.is_empty() {
        bail!("--classify needs at least one log file");
    }
    let out_dir = out_arg
        .map(|o| runner::fresh_out_dir(cwd, o, None))
        .transpose()?;
    let width = files.len().to_string().len().max(2);
    let mut tsv = report::tsv_header();
    let mut tally = report::Tally::default();
    let mut footers = Vec::new();
    let mut parents = Vec::new();
    let mut non_clean = false;
    for (n, file) in files.iter().enumerate() {
        let path = cwd.join(file);
        if !path.is_file() {
            bail!("no such log file: {}", show(file.as_bytes()));
        }
        let raw = std::fs::read(&path)
            .map_err(|e| anyhow::anyhow!("cannot read {}: {e}", show(file.as_bytes())))?;
        let c = classify::classify(&raw, stall_override);
        out.write_all(&report::format_result(file.as_bytes(), &c))?;
        if c.class != classify::Class::Clean {
            non_clean = true;
            out.write_all(&report::classify_details(&c))?;
        }
        if out_dir.is_some() {
            let idx = format!("{:0width$}", n + 1);
            let footer = runner::Footer::parse(&raw);
            let timing = footer.timing();
            let mode = footer.get("mode").map_or_else(|| "-".to_string(), show);
            tsv.extend(report::tsv_row(
                &idx,
                &mode,
                &c,
                &timing,
                &show(file.as_bytes()),
            ));
            tally.add(&idx, &c, &timing);
            footers.push(footer);
            parents.push(path.parent().and_then(|p| std::fs::canonicalize(p).ok()));
        }
    }
    if let Some(dir) = out_dir {
        let tsv_path = dir.join("summary.tsv");
        std::fs::write(&tsv_path, &tsv)
            .map_err(|e| anyhow::anyhow!("cannot write {}: {e}", tsv_path.display()))?;
        // The directory that holds every log, when there is one.
        let logs = match parents.first() {
            Some(Some(first)) if parents.iter().all(|p| p.as_ref() == Some(first)) => {
                first.as_os_str().as_bytes().to_vec()
            }
            _ => b"-".to_vec(),
        };
        let stall =
            stall_override.map_or_else(|| common_value(&footers, "stall_limit"), |n| n.to_string());
        let runs = files.len().to_string();
        let mode = common_value(&footers, "mode");
        let secs = common_value(&footers, "secs");
        let info = report::SummaryInfo {
            mode: &mode,
            runs: &runs,
            secs: &secs,
            stall_secs: &stall,
            fresh_data: None,
            git_rev: "-",
            kernel_sha: "-",
            qemu_version: b"-",
            firmware: b"-",
            host: b"-",
            load_start: "-",
            load_end: "-",
            out: &logs,
        };
        let head = report::summary_head(&info, &tally);
        runner::write_summary(&dir, &head, &tally, out)?;
    }
    Ok(if non_clean && !report_only { 1 } else { 0 })
}

/// Run `aios soak` with its raw arguments. Returns the exit status; `Err` is a
/// usage or setup error (status 2).
pub fn run(args: &[OsString], cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    match parse(args, err)? {
        Request::Help => {
            out.write_all(USAGE.as_bytes())?;
            Ok(0)
        }
        Request::Classify {
            files,
            stall_override,
            report_only,
            out: out_arg,
        } => classify_files(
            &files,
            stall_override,
            report_only,
            out_arg.as_ref(),
            cwd,
            out,
        ),
        Request::Soak(cfg) => runner::run(&cfg, cwd, out, err),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    /// Parse `args`; returns the request (or the error text) and the warnings.
    fn parse_str(args: &[&str]) -> (std::result::Result<Request, String>, String) {
        let mut err = Vec::new();
        let r = parse(&os(args), &mut err).map_err(|e| format!("{e:#}"));
        (r, String::from_utf8(err).expect("UTF-8"))
    }

    fn soak(args: &[&str]) -> Config {
        match parse_str(args).0 {
            Ok(Request::Soak(cfg)) => cfg,
            other => panic!("{args:?}: {other:?}"),
        }
    }

    fn error(args: &[&str]) -> String {
        match parse_str(args).0 {
            Err(e) => e,
            other => panic!("{args:?}: {other:?}"),
        }
    }

    #[test]
    fn defaults() {
        assert_eq!(
            soak(&[]),
            Config {
                runs_raw: "10".into(),
                runs: 10,
                secs_raw: "75".into(),
                secs: 75,
                stall_raw: "15".into(),
                mode: "text".into(),
                out: None,
                build: true,
                report_only: false,
                fresh_data: true,
            }
        );
    }

    #[test]
    fn every_spelling_of_every_option() {
        let a = soak(&[
            "--runs",
            "5",
            "--secs=90",
            "mode=gpu",
            "--out",
            "o",
            "stall_secs=20",
            "--no-build",
            "--reuse-data",
            "--report-only",
        ]);
        assert_eq!(
            (a.runs, a.secs, a.mode.as_str(), a.stall_raw.as_str()),
            (5, 90, "gpu", "20")
        );
        assert_eq!(a.out, Some(OsString::from("o")));
        assert!(!a.build && !a.fresh_data && a.report_only);
        let b = soak(&[
            "runs=007",
            "secs=075",
            "--mode=text",
            "out=x=y",
            "--stall-secs=5",
            "--reuse-data",
            "--fresh-data",
            "report_only=on",
        ]);
        assert_eq!((b.runs_raw.as_str(), b.runs, b.secs), ("007", 7, 75));
        assert_eq!(b.out, Some(OsString::from("x=y")));
        assert!(b.fresh_data && b.report_only);
        assert!(!soak(&["report_only=1", "report_only="]).report_only);
        // An empty out= is the default directory, also after an earlier one.
        assert_eq!(soak(&["out="]).out, None);
        assert_eq!(soak(&["--out", "o", "--out", ""]).out, None);
        assert_eq!(soak(&["--out=o", "--out="]).out, None);
    }

    #[test]
    fn usage_errors_use_the_script_messages() {
        assert_eq!(error(&["--runs"]), "option --runs needs a value");
        assert_eq!(error(&["--bogus"]), "unknown option: --bogus (see --help)");
        assert_eq!(error(&["-"]), "unknown option: - (see --help)");
        assert_eq!(
            error(&["--report-only=1"]),
            "unknown option: --report-only=1 (see --help)"
        );
        assert_eq!(error(&["foo"]), "unexpected argument: foo (see --help)");
        assert_eq!(
            error(&["--", "foo"]),
            "unexpected argument: foo (see --help)"
        );
        assert_eq!(
            error(&["--", "--classify", "x.log"]),
            "unexpected argument: --classify (see --help)"
        );
        assert_eq!(error(&["runs=0"]), "--runs must be a positive integer");
        assert_eq!(
            error(&["--runs", "+5"]),
            "--runs must be a positive integer"
        );
        assert_eq!(
            error(&["--runs", "99999999999999999999"]),
            "--runs must be a positive integer"
        );
        assert_eq!(error(&["secs="]), "--secs must be a positive integer");
        assert_eq!(
            error(&["stall_secs=0"]),
            "--stall-secs must be a positive integer"
        );
        assert_eq!(
            error(&["--secs", "10", "--stall-secs", "10"]),
            "--stall-secs (10) must be smaller than --secs (10)"
        );
        assert_eq!(
            error(&["mode=fast"]),
            "--mode must be text or gpu, got 'fast'"
        );
        assert_eq!(
            error(&["report_only=maybe"]),
            "expected a boolean (1/0, true/false), got 'maybe'"
        );
        // Errors are raised in argument order: this one comes before --help.
        assert_eq!(
            error(&["report_only=maybe", "--help"]),
            "expected a boolean (1/0, true/false), got 'maybe'"
        );
    }

    #[test]
    fn help_wins_over_later_problems() {
        assert_eq!(parse_str(&["--runs", "x", "-h"]).0, Ok(Request::Help));
        assert_eq!(parse_str(&["--help", "--bogus"]).0, Ok(Request::Help));
        assert!(USAGE.starts_with("Usage: aios soak [options] [key=value ...]\n"));
    }

    #[test]
    fn classify_takes_everything_after_the_first_file() {
        assert_eq!(
            parse_str(&["--stall-secs", "5", "--classify", "a.log", "--report-only"]).0,
            Ok(Request::Classify {
                files: os(&["a.log", "--report-only"]),
                stall_override: Some(5),
                report_only: false,
                out: None,
            })
        );
        assert_eq!(
            parse_str(&["report_only=1", "--classify", "--", "-x.log"]).0,
            Ok(Request::Classify {
                files: os(&["-x.log"]),
                stall_override: None,
                report_only: true,
                out: None,
            })
        );
        // Soak-only options are not checked in --classify mode.
        assert_eq!(
            parse_str(&["--runs", "abc", "mode=fast", "--classify"]).0,
            Ok(Request::Classify {
                files: vec![],
                stall_override: None,
                report_only: false,
                out: None,
            })
        );
        assert_eq!(
            error(&["--stall-secs", "0", "--classify", "a.log"]),
            "--stall-secs must be a positive integer"
        );
    }

    #[test]
    fn a_short_secs_warns() {
        let (r, warning) = parse_str(&["--secs", "30"]);
        assert!(matches!(r, Ok(Request::Soak(_))));
        assert_eq!(
            warning,
            "soak: warning: --secs 30 leaves under 20s beyond --stall-secs 15 for the boot itself (about 6-8 s to the Gate 1 bench); late boots will be INCONCLUSIVE, and a wedge in the last 15s of a boot is never seen\n"
        );
        assert_eq!(parse_str(&["--secs", "35"]).1, "");
    }
}
