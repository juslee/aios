//! `aios soak`'s boot loop: build the ESP, snapshot it, boot QEMU `runs` times
//! under a [`Supervisor`], and write `run-NN.log`, `summary.tsv` and
//! `summary.md`. A port of `cleanup`, `poll_log` and `run_soak` in the former
//! `scripts/soak-qemu.sh` (blob at `212df62`, L522-801); `timeout(1)` is
//! replaced by the supervisor, which reports the same exit statuses.
//!
//! Accepted divergences from the script:
//! - Times come from a monotonic clock rounded down to whole seconds, where bash
//!   subtracted two whole-second `$SECONDS` readings; a reading can differ by 1 s.
//! - A step that `set -e` ended with the failing command's status (`cp`, `sha256sum`,
//!   `just --evaluate` of `disk_img`, `data_img` or `kernel_elf`) ends with a
//!   `soak: error:` message and status 2 instead.
//! - `--runs`, `--secs` and `--stall-secs` with leading zeros are decimal
//!   everywhere; bash read them as octal in `$((...))` (the CLEAN-rate
//!   denominator and the `--secs` warning).
//! - SIGHUP and SIGQUIT are caught like SIGINT and SIGTERM (exit 129 and 131),
//!   so no terminal signal leaves QEMU running after the harness ends.
//! - Ctrl-Z (SIGTSTP) during a boot stops QEMU's process group with the
//!   harness, and on resume moves the boot's time limit and its clock back by
//!   the time spent stopped, so the classifier's timings are QEMU's running
//!   time. The script's `timeout` ran in its own group, so QEMU ran on and
//!   was killed at the limit while bash was stopped.

use std::ffi::{OsStr, OsString};
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::DirBuilderExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};

use super::awk::contains;
use super::classify::{classify, CLASSES};
use super::host;
use super::report::{self, BootTiming, SummaryInfo, TSV_HEADER};
use super::signals::Interrupts;
use crate::proc::Supervisor;

/// How long QEMU gets after SIGTERM before SIGKILL (`timeout --kill-after=10`).
pub const KILL_AFTER: Duration = Duration::from_secs(10);
/// The log is polled this often, as the script's `sleep 1` loop did.
const POLL: Duration = Duration::from_secs(1);
/// Signals and deadlines are checked this often while waiting for the next poll.
const SLICE: Duration = Duration::from_millis(50);
/// Sparse size of a fresh data disk: 256 MiB of zeros, as `just create-data-disk` makes.
const DATA_DISK_BYTES: u64 = 256 * 1024 * 1024;

/// A validated `aios soak` boot request. The `*_raw` strings are the values as
/// given, which the reports print; the numbers are what the loop uses.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub runs_raw: String,
    pub runs: u64,
    pub secs_raw: String,
    pub secs: u64,
    pub stall_raw: String,
    pub mode: String,
    pub out: Option<OsString>,
    pub build: bool,
    pub report_only: bool,
    pub fresh_data: bool,
}

/// Per-boot progress seen by polling the log (`poll_log`): the heartbeat line
/// count, and the seconds into the boot at which the heartbeat last grew, the
/// kernel started, the first heartbeat, the Gate 1 bench header and
/// `=== Gate 1 Complete ===` appeared (-1: not yet). `gap` is the longest wait
/// between two heartbeat advances after the bench completed (-1: it never did).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub hb: u64,
    pub adv: i64,
    pub kernel: i64,
    pub hb0: i64,
    pub bench: i64,
    pub g1: i64,
    pub gap: i64,
}

impl Default for Progress {
    fn default() -> Progress {
        Progress {
            hb: 0,
            adv: -1,
            kernel: -1,
            hb0: -1,
            bench: -1,
            g1: -1,
            gap: -1,
        }
    }
}

impl Progress {
    /// `poll_log LOG T`: record the progress visible in `log` at `t` seconds.
    pub fn poll(&mut self, log: &[u8], t: i64) {
        // grep -c counts lines; a match never spans lines.
        let count = log
            .split(|&b| b == b'\n')
            .filter(|l| contains(l, b"[heartbeat] tick="))
            .count() as u64;
        if count > self.hb {
            if self.g1 >= 0 && self.adv >= self.g1 && t - self.adv > self.gap {
                self.gap = t - self.adv;
            }
            self.hb = count;
            self.adv = t;
            if self.hb0 < 0 {
                self.hb0 = t;
            }
        }
        if self.kernel < 0 && contains(log, b"AIOS kernel booting") {
            self.kernel = t;
        }
        if self.bench < 0 && contains(log, b"=== Gate 1 Benchmark ===") {
            self.bench = t;
        }
        if self.g1 < 0 && contains(log, b"=== Gate 1 Complete ===") {
            self.g1 = t;
            self.gap = 0;
        }
    }
}

/// The `[soak] meta` footer appended to a boot's log, with its leading blank line.
pub fn footer(cfg: &Config, elapsed: i64, rc: i32, p: &Progress, load1: &[u8]) -> Vec<u8> {
    let mut s = format!(
        "\n[soak] meta mode={} secs={} elapsed={elapsed} qemu_rc={rc} kstart={} hb_first={} bench_start={} g1done={} hb_count={} hb_last_advance={} hb_max_gap={} stall_limit={} load1=",
        cfg.mode, cfg.secs_raw, p.kernel, p.hb0, p.bench, p.g1, p.hb, p.adv, p.gap, cfg.stall_raw
    )
    .into_bytes();
    s.extend_from_slice(load1);
    s.push(b'\n');
    s
}

/// `a` + `b` + `c` as one OS string.
fn concat_os(a: &str, b: &OsStr, c: &str) -> OsString {
    let mut s = OsString::from(a);
    s.push(b);
    s.push(c);
    s
}

/// QEMU's arguments for one boot. Keep in sync with the justfile's `run` (text)
/// and `run-gpu` (gpu, plus `-display none`) recipes.
pub fn qemu_args(mode: &str, firmware: &OsStr, esp: &Path, data: &Path) -> Vec<OsString> {
    let words = |ws: &[&str]| ws.iter().map(OsString::from).collect::<Vec<_>>();
    let mut args = words(&[
        "-machine",
        "virt,gic-version=3",
        "-cpu",
        "cortex-a72",
        "-smp",
        "4",
        "-m",
        "2G",
    ]);
    if mode == "text" {
        args.extend(words(&["-nographic"]));
    } else {
        args.extend(words(&["-serial", "stdio", "-display", "none"]));
    }
    args.push("-bios".into());
    args.push(firmware.to_os_string());
    args.push("-drive".into());
    args.push(concat_os(
        "if=none,id=disk0,file=",
        esp.as_os_str(),
        ",format=raw",
    ));
    args.extend(words(&["-device", "virtio-blk-pci,drive=disk0"]));
    args.push("-drive".into());
    args.push(concat_os(
        "if=none,id=data0,file=",
        data.as_os_str(),
        ",format=raw",
    ));
    args.extend(words(&["-device", "virtio-blk-device,drive=data0"]));
    if mode == "text" {
        args.extend(words(&["-device", "ramfb"]));
    } else {
        args.extend(words(&[
            "-device",
            "virtio-gpu-device",
            "-device",
            "virtio-keyboard-device",
            "-device",
            "virtio-tablet-device",
        ]));
    }
    args
}

/// The private `.scratch.XXXXXX` directory in the output directory (`mktemp -d`,
/// mode 0700) that holds the ESP snapshot and the fresh data disks. It is removed
/// when dropped, which the script's `cleanup` did at exit.
pub struct ScratchDir {
    path: PathBuf,
}

impl ScratchDir {
    pub fn create(parent: &Path) -> std::io::Result<ScratchDir> {
        use std::collections::hash_map::RandomState;
        use std::hash::BuildHasher;
        const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
        let random = RandomState::new();
        let mut last = None;
        for attempt in 0u32..100 {
            let mut n = random.hash_one(attempt);
            let suffix: String = (0..6)
                .map(|_| {
                    let c = ALPHABET[(n % ALPHABET.len() as u64) as usize] as char;
                    n /= ALPHABET.len() as u64;
                    c
                })
                .collect();
            let path = parent.join(format!(".scratch.{suffix}"));
            match std::fs::DirBuilder::new().mode(0o700).create(&path) {
                Ok(()) => return Ok(ScratchDir { path }),
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
                Err(e) => return Err(e),
            }
        }
        Err(last.expect("100 attempts"))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for ScratchDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

fn warn(err: &mut dyn Write, message: &[u8]) -> Result<()> {
    err.write_all(&[&b"soak: warning: "[..], message, b"\n"].concat())?;
    Ok(())
}

/// Append `data` to `path`.
fn append(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("cannot append to {}", path.display()))?;
    f.write_all(data)
        .with_context(|| format!("cannot append to {}", path.display()))
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))
}

/// Whole seconds since `start`.
fn secs_since(start: Instant) -> i64 {
    i64::try_from(start.elapsed().as_secs()).unwrap_or(i64::MAX)
}

/// What [`run`] has settled before it installs the signal handlers: the
/// repository, the justfile's paths (the image paths relative to `root`), and
/// the new or empty output directory.
struct Setup {
    root: PathBuf,
    firmware: Vec<u8>,
    disk_rel: Vec<u8>,
    data_rel: Vec<u8>,
    kernel_rel: Vec<u8>,
    out_dir: PathBuf,
}

/// Run a soak. Returns the exit status: 0, 1 (a boot was not CLEAN), or 129,
/// 130, 131 or 143 when a signal interrupted it. Setup errors are `Err`.
pub fn run(cfg: &Config, cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    if host::find_in_path("qemu-system-aarch64").is_none() {
        bail!("qemu-system-aarch64 not found in PATH");
    }
    if host::find_in_path("just").is_none() {
        bail!("just not found in PATH");
    }
    let root = host::repo_root(cwd)?;

    let firmware = host::just_evaluate(&root, "edk2_fw")?;
    let disk_rel = host::just_evaluate(&root, "disk_img")?;
    let data_rel = host::just_evaluate(&root, "data_img")?;
    let kernel_rel = host::just_evaluate(&root, "kernel_elf")?;
    let firmware_os = OsStr::from_bytes(&firmware);
    if !cwd.join(firmware_os).is_file() {
        bail!(
            "UEFI firmware not found: {} (set AIOS_EDK2_FW)",
            String::from_utf8_lossy(&firmware)
        );
    }

    // The output directory must be new or empty: the harness writes run-NN.log,
    // summary.* and build.log there and never overwrites or deletes anything it
    // did not create.
    let out_arg: OsString = match &cfg.out {
        Some(o) => o.clone(),
        None => root
            .join("target/soak")
            .join(format!("{}-{}", host::timestamp()?, cfg.mode))
            .into_os_string(),
    };
    let out_path = cwd.join(&out_arg);
    let shown = String::from_utf8_lossy(out_arg.as_bytes()).into_owned();
    if out_path.exists() && !out_path.is_dir() {
        bail!("--out {shown} exists and is not a directory");
    }
    std::fs::create_dir_all(&out_path)
        .map_err(|_| anyhow::anyhow!("cannot create output directory {shown}"))?;
    let out_dir =
        std::fs::canonicalize(&out_path).with_context(|| format!("cannot resolve {shown}"))?;
    if out_dir == root {
        bail!("--out must not be the repository root (default: target/soak/<timestamp>-<mode>)");
    }
    if std::fs::read_dir(&out_dir)
        .with_context(|| format!("cannot list {}", out_dir.display()))?
        .next()
        .is_some()
    {
        bail!(
            "--out {} is not empty; choose a new or empty directory",
            out_dir.display()
        );
    }

    let interrupts = Interrupts::install()?;
    let setup = Setup {
        root,
        firmware,
        disk_rel,
        data_rel,
        kernel_rel,
        out_dir,
    };
    // From here on aios survives a terminal signal, but the children it runs sit
    // in the terminal's foreground group and die of it (`just`, `sha256sum`).
    // A step that then fails ends with the signal's status, as the script's trap
    // exited 130 or 143 before `|| die` or `set -e` could act.
    build_and_boot(cfg, setup, &interrupts, out, err).or_else(|e| interrupts.pending().ok_or(e))
}

/// Build and snapshot the ESP, boot it `cfg.runs` times and write the reports,
/// with the signal handlers installed. Returns [`run`]'s status.
fn build_and_boot(
    cfg: &Config,
    setup: Setup,
    interrupts: &Interrupts,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let Setup {
        root,
        firmware,
        disk_rel,
        data_rel,
        kernel_rel,
        out_dir,
    } = setup;
    let firmware_os = OsStr::from_bytes(&firmware);
    let scratch = ScratchDir::create(&out_dir).map_err(|_| {
        anyhow::anyhow!("cannot create a scratch directory in {}", out_dir.display())
    })?;
    let esp = scratch.path().join("esp.img");
    let data = if cfg.fresh_data {
        scratch.path().join("data.img")
    } else {
        root.join(OsStr::from_bytes(&data_rel))
    };
    let build_log = out_dir.join("build.log");

    if cfg.build {
        out.write_all(
            &[
                &b"soak: building ESP image (just disk) -> "[..],
                bytes(&build_log),
                b"\n",
            ]
            .concat(),
        )?;
        out.flush()?;
        let log = File::create(&build_log)
            .with_context(|| format!("cannot create {}", build_log.display()))?;
        let status = host::command("just")
            .arg("disk")
            .current_dir(&root)
            .stdout(log.try_clone()?)
            .stderr(log)
            .status();
        if let Some(code) = interrupts.pending() {
            return Ok(code);
        }
        if !status.is_ok_and(|s| s.success()) {
            err.write_all(host::tail_lines(&read(&build_log)?, 30))?;
            bail!(
                "build failed (just disk); full log: {}",
                build_log.display()
            );
        }
    }
    let disk = root.join(OsStr::from_bytes(&disk_rel));
    if !disk.is_file() {
        bail!(
            "ESP image {} missing (run without --no-build)",
            disk.display()
        );
    }
    // Snapshot the ESP so every boot uses identical bits even if the tree is
    // rebuilt while the soak runs.
    std::fs::copy(&disk, &esp)
        .with_context(|| format!("cannot copy {} to {}", disk.display(), esp.display()))?;
    if !cfg.fresh_data && !data.is_file() {
        out.flush()?;
        let made = host::command("just")
            .arg("create-data-disk")
            .current_dir(&root)
            .status();
        if !made.is_ok_and(|s| s.success()) {
            bail!("cannot create {}", data.display());
        }
    }

    let git_rev = host::git_rev(&root);
    // Identify the bits under test: the kernel ELF inside the ESP snapshot,
    // which can differ from target/ with --no-build.
    let esp_kernel = scratch.path().join("aios.elf");
    out.flush()?;
    let extracted = host::find_in_path("mcopy").is_some()
        && host::command("mcopy")
            .arg("-n")
            .arg("-i")
            .arg(&esp)
            .arg("::/EFI/AIOS/aios.elf")
            .arg(&esp_kernel)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    let kernel_sha = if extracted {
        let sha = format!("kernel ELF sha256 `{}`", host::sha256_16(&esp_kernel)?);
        let target_kernel = root.join(OsStr::from_bytes(&kernel_rel));
        let same = target_kernel.is_file() && read(&esp_kernel)? == read(&target_kernel)?;
        if !same {
            let d = String::from_utf8_lossy(&disk_rel);
            let k = String::from_utf8_lossy(&kernel_rel);
            warn(
                err,
                format!("the kernel in {d} differs from {k}; the soak boots the one in {d}")
                    .as_bytes(),
            )?;
        }
        let _ = std::fs::remove_file(&esp_kernel);
        sha
    } else {
        format!(
            "ESP image sha256 `{}`, kernel not extracted (mcopy)",
            host::sha256_16(&esp)?
        )
    };
    let qemu_version = host::command("qemu-system-aarch64")
        .arg("--version")
        .stdin(Stdio::null())
        .stderr(Stdio::inherit())
        .output()
        .map(|o| host::first_line(&o.stdout).to_vec())
        .unwrap_or_default();
    let load_start = host::loadavg();
    // A signal that ended one of the probes above (git, mcopy, QEMU's version,
    // sysctl) without failing the soak: stop before any report, as the trap did.
    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }

    let tsv = out_dir.join("summary.tsv");
    std::fs::write(&tsv, TSV_HEADER).with_context(|| format!("cannot write {}", tsv.display()))?;

    let data_word = if cfg.fresh_data { "fresh" } else { "reused" };
    out.write_all(
        format!(
            "soak: {} x {}s, mode={}, commit={git_rev}, data={data_word}\n",
            cfg.runs_raw, cfg.secs_raw, cfg.mode
        )
        .as_bytes(),
    )?;
    out.write_all(&[&b"soak: firmware="[..], &firmware, b"\n"].concat())?;
    out.write_all(
        &[
            &b"soak: logs in "[..],
            bytes(&out_dir),
            format!(" (load average at start: {load_start})\n").as_bytes(),
        ]
        .concat(),
    )?;

    let width = cfg.runs_raw.len().max(2);
    let mut counts = [0u64; 6];
    let mut non_clean = false;
    let mut md_rows: Vec<u8> = Vec::new();
    let mut loads: Vec<Vec<u8>> = Vec::new();

    for n in 1..=cfg.runs {
        if let Some(code) = interrupts.pending() {
            return Ok(code);
        }
        let idx = format!("{n:0width$}");
        let log_name = format!("run-{idx}.log");
        let log = out_dir.join(&log_name);
        if cfg.fresh_data {
            // Sparse 256 MiB of zeros: reads the same as create-data-disk's file
            // without writing 256 MiB per boot.
            let _ = std::fs::remove_file(&data);
            File::create(&data)
                .and_then(|f| f.set_len(DATA_DISK_BYTES))
                .map_err(|_| anyhow::anyhow!("cannot create {}", data.display()))?;
        }
        let args = qemu_args(&cfg.mode, firmware_os, &esp, &data);
        let load1 = host::load1();
        let mut progress = Progress::default();

        let log_file =
            File::create(&log).with_context(|| format!("cannot create {}", log.display()))?;
        // QEMU's running time: a Ctrl-Z moves it forward by the time spent
        // stopped, as it moves QEMU's time limit, so every progress time and
        // the footer's elapsed share the limit's clock.
        let mut start = Instant::now();
        let mut command = host::command("qemu-system-aarch64");
        // stdin from /dev/null: QEMU's stdio serial must never read the terminal.
        command
            .args(&args)
            .stdin(Stdio::null())
            .stdout(log_file.try_clone()?)
            .stderr(log_file);
        let rc = {
            // Until QEMU has exited, Ctrl-Z stops QEMU's group before the harness;
            // nothing the harness waits for meanwhile runs in the terminal's
            // foreground group. Declared before `qemu`, so it is dropped after it.
            let _deferred = interrupts.defer_suspend();
            match Supervisor::spawn(command, Duration::from_secs(cfg.secs), KILL_AFTER) {
                Err(e) => {
                    // As timeout(1) did when it could not run the command: a note
                    // in the log, and status 127.
                    append(
                        &log,
                        format!("soak: cannot run qemu-system-aarch64: {e}\n").as_bytes(),
                    )?;
                    127
                }
                Ok(mut qemu) => {
                    qemu.warn_as("soak");
                    loop {
                        qemu.service()?;
                        if let Some(rc) = qemu.exit_code() {
                            break rc;
                        }
                        let wake = Instant::now() + POLL;
                        loop {
                            if let Some(code) = interrupts.pending() {
                                qemu.terminate()?;
                                qemu.wait()?;
                                return Ok(code);
                            }
                            if interrupts.take_suspend() {
                                let paused = qemu.suspend(|| interrupts.stop_self())?;
                                start = start.checked_add(paused).unwrap_or(start);
                            }
                            qemu.service()?;
                            let now = Instant::now();
                            if now >= wake {
                                break;
                            }
                            std::thread::sleep(SLICE.min(wake - now));
                        }
                        progress.poll(&read(&log)?, secs_since(start));
                    }
                }
            }
        };
        let elapsed = secs_since(start);
        progress.poll(&read(&log)?, elapsed);
        append(&log, &footer(cfg, elapsed, rc, &progress, &load1))?;

        // A boot on which the UEFI stub never ran says nothing about the kernel.
        // On the first boot it means the setup is broken (QEMU failed to start,
        // or the firmware never loaded the stub), so stop.
        let text = read(&log)?;
        if n == 1 && !contains(&text, b"AIOS UEFI stub") {
            err.write_all(host::tail_lines(&text, 20))?;
            bail!(
                "the UEFI stub never ran on the first boot (QEMU exit status {rc} after {elapsed}s): check QEMU, the firmware ({}) and the ESP image; see {}",
                String::from_utf8_lossy(&firmware),
                log.display()
            );
        }

        let c = classify(&text, None);
        out.write_all(&report::format_result(
            format!("run {idx}/{}", cfg.runs_raw).as_bytes(),
            &c,
        ))?;
        let slot = CLASSES
            .iter()
            .position(|k| *k == c.class)
            .expect("a known class");
        counts[slot] += 1;
        non_clean |= c.class != "CLEAN";
        let timing = BootTiming {
            elapsed,
            rc,
            load1: load1.clone(),
            kstart: progress.kernel,
            hb_first: progress.hb0,
            bench_start: progress.bench,
            g1done: progress.g1,
            hb_max_gap: progress.gap,
        };
        append(
            &tsv,
            &report::tsv_row(&idx, &cfg.mode, &c, &timing, &log_name),
        )?;
        md_rows.extend(report::md_row(&idx, &c));
        loads.push(load1);
    }
    drop(scratch); // the ESP snapshot and the fresh data disk
    let load_end = host::loadavg();

    let host_line = [&host::uname()[..], b", ", &host::host_cpus(), b" CPUs"].concat();
    let load_refs: Vec<&[u8]> = loads.iter().map(Vec::as_slice).collect();
    let info = SummaryInfo {
        mode: &cfg.mode,
        runs: &cfg.runs_raw,
        secs: &cfg.secs_raw,
        stall_secs: &cfg.stall_raw,
        fresh_data: cfg.fresh_data,
        git_rev: &git_rev,
        kernel_sha: &kernel_sha,
        qemu_version: &qemu_version,
        firmware: &firmware,
        host: &host_line,
        load_start: &load_start,
        load_end: &load_end,
        out: bytes(&out_dir),
    };
    let head = report::summary_head(&info, &counts, cfg.runs, &load_refs);
    // A signal after the last boot's QEMU exited: the script's trap exited at
    // once, so there is no summary.md.
    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }
    let md = out_dir.join("summary.md");
    std::fs::write(&md, &head).with_context(|| format!("cannot write {}", md.display()))?;
    out.write_all(b"\n")?;
    out.write_all(&head)?;
    append(&md, &report::summary_table(&md_rows))?;
    out.write_all(
        &[
            &b"\nsoak: per-boot table in "[..],
            bytes(&md),
            b", machine-readable rows in ",
            bytes(&tsv),
            b"\n",
        ]
        .concat(),
    )?;

    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }
    Ok(if non_clean && !cfg.report_only { 1 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn cfg(mode: &str) -> Config {
        Config {
            runs_raw: "3".into(),
            runs: 3,
            secs_raw: "75".into(),
            secs: 75,
            stall_raw: "15".into(),
            mode: mode.into(),
            out: None,
            build: true,
            report_only: false,
            fresh_data: true,
        }
    }

    #[test]
    fn progress_follows_poll_log() {
        let mut p = Progress::default();
        p.poll(b"AIOS UEFI stub\n", 1);
        assert_eq!(p, Progress::default());
        p.poll(b"AIOS kernel booting\n[heartbeat] tick=0\n", 2);
        assert_eq!((p.hb, p.adv, p.kernel, p.hb0, p.gap), (1, 2, 2, 2, -1));
        // The bench completes: the gap trace starts at 0.
        let log = b"AIOS kernel booting\n[heartbeat] tick=0\n=== Gate 1 Benchmark ===\n=== Gate 1 Complete ===\n";
        p.poll(log, 3);
        assert_eq!((p.bench, p.g1, p.gap), (3, 3, 0));
        // A heartbeat before the bench completed does not count toward the gap.
        let log2 = [&log[..], b"[heartbeat] tick=1000\n"].concat();
        p.poll(&log2, 9);
        assert_eq!((p.hb, p.adv, p.gap), (2, 9, 0));
        // Once the last advance is at or after g1done, a longer wait is recorded.
        let log3 = [&log2[..], b"[heartbeat] tick=2000\n"].concat();
        p.poll(&log3, 30);
        assert_eq!((p.hb, p.adv, p.gap), (3, 30, 21));
        // A shorter wait later leaves the maximum.
        let log4 = [&log3[..], b"[heartbeat] tick=3000\n"].concat();
        p.poll(&log4, 32);
        assert_eq!((p.hb, p.adv, p.gap), (4, 32, 21));
        // grep -c counts lines, not matches, and a partial line is not a heartbeat yet.
        let mut q = Progress::default();
        q.poll(b"[heartbeat] tick=0 [heartbeat] tick=1\n[heartbeat] ti", 1);
        assert_eq!(q.hb, 1);
    }

    #[test]
    fn footer_matches_the_script() {
        let p = Progress {
            hb: 3,
            adv: 70,
            kernel: 1,
            hb0: 2,
            bench: 7,
            g1: 8,
            gap: 5,
        };
        assert_eq!(
            footer(&cfg("text"), 76, 124, &p, b"1.50"),
            b"\n[soak] meta mode=text secs=75 elapsed=76 qemu_rc=124 kstart=1 hb_first=2 bench_start=7 g1done=8 hb_count=3 hb_last_advance=70 hb_max_gap=5 stall_limit=15 load1=1.50\n"
        );
    }

    #[test]
    fn qemu_arguments_match_the_justfile_recipes() {
        let join = |mode: &str| {
            qemu_args(
                mode,
                OsStr::new("/fw.fd"),
                Path::new("/s/esp.img"),
                Path::new("/s/data.img"),
            )
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join(" ")
        };
        assert_eq!(
            join("text"),
            "-machine virt,gic-version=3 -cpu cortex-a72 -smp 4 -m 2G -nographic -bios /fw.fd \
             -drive if=none,id=disk0,file=/s/esp.img,format=raw -device virtio-blk-pci,drive=disk0 \
             -drive if=none,id=data0,file=/s/data.img,format=raw -device virtio-blk-device,drive=data0 -device ramfb"
        );
        assert_eq!(
            join("gpu"),
            "-machine virt,gic-version=3 -cpu cortex-a72 -smp 4 -m 2G -serial stdio -display none -bios /fw.fd \
             -drive if=none,id=disk0,file=/s/esp.img,format=raw -device virtio-blk-pci,drive=disk0 \
             -drive if=none,id=data0,file=/s/data.img,format=raw -device virtio-blk-device,drive=data0 \
             -device virtio-gpu-device -device virtio-keyboard-device -device virtio-tablet-device"
        );
    }

    #[test]
    fn scratch_dir_is_private_unique_and_removed_on_drop() {
        let parent = std::env::temp_dir().join(format!("aios-scratch-{}", std::process::id()));
        std::fs::create_dir_all(&parent).expect("parent");
        let a = ScratchDir::create(&parent).expect("first");
        let b = ScratchDir::create(&parent).expect("second");
        assert_ne!(a.path(), b.path());
        let name = a
            .path()
            .file_name()
            .expect("a name")
            .to_string_lossy()
            .into_owned();
        assert!(name.starts_with(".scratch.") && name.len() == 15, "{name}");
        assert!(
            name[9..].bytes().all(|c| c.is_ascii_alphanumeric()),
            "{name}"
        );
        let mode = std::fs::metadata(a.path())
            .expect("exists")
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o700);
        std::fs::write(a.path().join("esp.img"), b"x").expect("write");
        let path = a.path().to_path_buf();
        drop(a);
        assert!(!path.exists());
        drop(b);
        let _ = std::fs::remove_dir_all(&parent);
    }
}
