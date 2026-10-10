//! `aios soak`'s boot loop: build the ESP, snapshot it, boot QEMU `runs` times
//! under a [`Supervisor`], and write `run-NN.log`, `summary.tsv` and
//! `summary.md`. The snapshot and what identifies it make an [`Arm`], and
//! [`boot_once`] runs one boot of it. A port of `cleanup`, `poll_log` and
//! `run_soak` in the former `scripts/soak-qemu.sh` (blob at `212df62`,
//! L522-801); `timeout(1)` is replaced by the supervisor, which reports the
//! same exit statuses.
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

use std::collections::HashMap;

use super::awk::{contains, fields};
use super::classify::{classify, preprocess, Class};
use super::host;
use super::report::{self, BootTiming, SummaryInfo, Tally};
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

/// The `[soak] meta` values of a log, as [`footer`] wrote them: every
/// `key=value` token of every footer line (a later one wins, as the
/// classifier reads them), after the classifier's NUL, CR and ANSI clean-up.
#[derive(Debug, Default)]
pub struct Footer(HashMap<Vec<u8>, Vec<u8>>);

impl Footer {
    /// The footer values of the raw log `raw`.
    pub fn parse(raw: &[u8]) -> Footer {
        let mut values = HashMap::new();
        for line in preprocess(raw).split(|&b| b == b'\n') {
            if !line.starts_with(b"[soak] meta ") {
                continue;
            }
            for field in fields(line).skip(2) {
                if let Some(eq) = field.iter().position(|&b| b == b'=').filter(|&eq| eq > 0) {
                    values.insert(field[..eq].to_vec(), field[eq + 1..].to_vec());
                }
            }
        }
        Footer(values)
    }

    /// The value of `key`, if the footer has it.
    pub fn get(&self, key: &str) -> Option<&[u8]> {
        self.0.get(key.as_bytes()).map(Vec::as_slice)
    }

    /// `key` as a whole number, if the footer has it and it is one.
    fn number<T: std::str::FromStr>(&self, key: &str) -> Option<T> {
        std::str::from_utf8(self.get(key)?).ok()?.parse().ok()
    }

    /// The `summary.tsv` timing fields the footer records: those of
    /// [`Boot::timing`] for a log this harness wrote, `None` for a value it lacks.
    pub fn timing(&self) -> BootTiming {
        BootTiming {
            elapsed: self.number("elapsed"),
            rc: self.number("qemu_rc"),
            load1: self.get("load1").map(<[u8]>::to_vec),
            kstart: self.number("kstart"),
            hb_first: self.number("hb_first"),
            bench_start: self.number("bench_start"),
            g1done: self.number("g1done"),
            hb_max_gap: self.number("hb_max_gap"),
        }
    }
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

pub(super) fn bytes(p: &Path) -> &[u8] {
    p.as_os_str().as_bytes()
}

pub(super) fn warn(err: &mut dyn Write, message: &[u8]) -> Result<()> {
    err.write_all(&[&b"soak: warning: "[..], message, b"\n"].concat())?;
    Ok(())
}

/// Append `data` to `path`.
pub(super) fn append(path: &Path, data: &[u8]) -> Result<()> {
    let mut f = OpenOptions::new()
        .append(true)
        .open(path)
        .with_context(|| format!("cannot append to {}", path.display()))?;
    f.write_all(data)
        .with_context(|| format!("cannot append to {}", path.display()))
}

pub(super) fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).with_context(|| format!("cannot read {}", path.display()))
}

/// Whole seconds since `start`.
fn secs_since(start: Instant) -> i64 {
    i64::try_from(start.elapsed().as_secs()).unwrap_or(i64::MAX)
}

/// One ESP image under test, as its boots need it: the QEMU program and the
/// firmware it loads (single mode: the justfile's `edk2_fw`, as evaluated;
/// interleave mode: that path resolved once, so every boot loads the file the
/// harness hashed), the private snapshot of the ESP that every boot uses, and
/// what the reports identify it by (the kernel ELF sha256 line and the git
/// rev, with `-dirty`). Single mode runs `qemu-system-aarch64` from `PATH`;
/// interleave mode, the one binary it resolved and checks before every boot.
pub struct Arm {
    pub qemu: OsString,
    pub firmware: Vec<u8>,
    pub esp: PathBuf,
    pub kernel_sha: String,
    pub git_rev: String,
}

/// A boot that ran to its end: QEMU exited, could not be spawned (status 127),
/// or was stopped at its time limit. `load1` is the host's 1-minute load just
/// before the boot, and `text` the whole log, footer included.
pub struct Boot {
    pub rc: i32,
    pub elapsed: i64,
    pub progress: Progress,
    pub load1: Vec<u8>,
    pub text: Vec<u8>,
}

impl Boot {
    /// The boot's `summary.tsv` timing fields.
    pub fn timing(&self) -> BootTiming {
        BootTiming {
            elapsed: Some(self.elapsed),
            rc: Some(self.rc),
            load1: Some(self.load1.clone()),
            kstart: Some(self.progress.kernel),
            hb_first: Some(self.progress.hb0),
            bench_start: Some(self.progress.bench),
            g1done: Some(self.progress.g1),
            hb_max_gap: Some(self.progress.gap),
        }
    }
}

/// How [`boot_once`] ended.
pub enum BootOutcome {
    /// The boot ran to its end, and its log carries the footer.
    Booted(Boot),
    /// A signal arrived while QEMU ran: QEMU was stopped and waited for, the
    /// log has no footer, and the soak ends with this status.
    Interrupted(u8),
}

/// Boot `arm` once: with `cfg.fresh_data`, make `data` a fresh data disk;
/// run QEMU on the arm's ESP snapshot and `data` under a [`Supervisor`],
/// with its output in `log`; poll the log for progress; then append the
/// footer. Signals and Ctrl-Z are handled as for the whole soak.
pub fn boot_once(
    arm: &Arm,
    cfg: &Config,
    log: &Path,
    data: &Path,
    interrupts: &Interrupts,
) -> Result<BootOutcome> {
    if cfg.fresh_data {
        // Sparse 256 MiB of zeros: reads the same as create-data-disk's file
        // without writing 256 MiB per boot.
        let _ = std::fs::remove_file(data);
        File::create(data)
            .and_then(|f| f.set_len(DATA_DISK_BYTES))
            .map_err(|_| anyhow::anyhow!("cannot create {}", data.display()))?;
    }
    let args = qemu_args(&cfg.mode, OsStr::from_bytes(&arm.firmware), &arm.esp, data);
    let load1 = host::load1();
    let mut progress = Progress::default();

    let log_file = File::create(log).with_context(|| format!("cannot create {}", log.display()))?;
    // QEMU's running time: a Ctrl-Z moves it forward by the time spent
    // stopped, as it moves QEMU's time limit, so every progress time and
    // the footer's elapsed share the limit's clock.
    let mut start = Instant::now();
    let mut command = host::command(&arm.qemu);
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
                    log,
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
                            return Ok(BootOutcome::Interrupted(code));
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
                    progress.poll(&read(log)?, secs_since(start));
                }
            }
        }
    };
    let elapsed = secs_since(start);
    progress.poll(&read(log)?, elapsed);
    append(log, &footer(cfg, elapsed, rc, &progress, &load1))?;
    Ok(BootOutcome::Booted(Boot {
        rc,
        elapsed,
        progress,
        load1,
        text: read(log)?,
    }))
}

/// `just disk` in `root`, its output appended to `build_log`. `who` prefixes
/// the messages (`""` in single mode, `"arm A: "` in interleave mode), and
/// `env_remove` names variables the build must not inherit. Returns the
/// signal's exit status when a signal arrived meanwhile.
pub fn build_esp(
    root: &Path,
    build_log: &Path,
    who: &str,
    env_remove: Option<&[&str]>,
    interrupts: &Interrupts,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Option<u8>> {
    out.write_all(
        &[
            format!("soak: {who}building ESP image (just disk) -> ").as_bytes(),
            bytes(build_log),
            b"\n",
        ]
        .concat(),
    )?;
    out.flush()?;
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .open(build_log)
        .with_context(|| format!("cannot create {}", build_log.display()))?;
    let mut command = host::command("just");
    command
        .arg("disk")
        .current_dir(root)
        .stdout(log.try_clone()?)
        .stderr(log);
    for var in env_remove.unwrap_or_default() {
        command.env_remove(var);
    }
    let status = command.status();
    if let Some(code) = interrupts.pending() {
        return Ok(Some(code));
    }
    if !status.is_ok_and(|s| s.success()) {
        err.write_all(host::tail_lines(&read(build_log)?, 30))?;
        bail!(
            "{who}build failed (just disk); full log: {}",
            build_log.display()
        );
    }
    Ok(None)
}

/// Snapshot the ESP image `disk_rel` of `root` to `esp`, so every boot uses
/// identical bits even if the tree is rebuilt while the soak runs.
pub fn snapshot_esp(root: &Path, disk_rel: &[u8], esp: &Path) -> Result<()> {
    let disk = root.join(OsStr::from_bytes(disk_rel));
    if !disk.is_file() {
        bail!(
            "ESP image {} missing (run without --no-build)",
            disk.display()
        );
    }
    std::fs::copy(&disk, esp)
        .with_context(|| format!("cannot copy {} to {}", disk.display(), esp.display()))?;
    Ok(())
}

/// What identifies the bits under test: the sha256 line of the kernel ELF
/// inside the ESP snapshot `esp` (extracted to `scratch_elf`, then removed),
/// which can differ from `root`'s `kernel_rel` with --no-build (a warning,
/// prefixed with `who`); or the ESP image's own sha256 when `mcopy` cannot
/// extract it.
pub fn esp_kernel_sha(
    root: &Path,
    disk_rel: &[u8],
    kernel_rel: &[u8],
    esp: &Path,
    scratch_elf: &Path,
    who: &str,
    err: &mut dyn Write,
) -> Result<String> {
    let extracted = host::find_in_path("mcopy").is_some()
        && host::command("mcopy")
            .arg("-n")
            .arg("-i")
            .arg(esp)
            .arg("::/EFI/AIOS/aios.elf")
            .arg(scratch_elf)
            .stderr(Stdio::null())
            .status()
            .is_ok_and(|s| s.success());
    if !extracted {
        return Ok(format!(
            "ESP image sha256 `{}`, kernel not extracted (mcopy)",
            host::sha256_16(esp)?
        ));
    }
    let sha = format!("kernel ELF sha256 `{}`", host::sha256_16(scratch_elf)?);
    let target_kernel = root.join(OsStr::from_bytes(kernel_rel));
    let same = target_kernel.is_file() && read(scratch_elf)? == read(&target_kernel)?;
    if !same {
        let d = String::from_utf8_lossy(disk_rel);
        let k = String::from_utf8_lossy(kernel_rel);
        warn(
            err,
            format!("{who}the kernel in {d} differs from {k}; the soak boots the one in {d}")
                .as_bytes(),
        )?;
    }
    let _ = std::fs::remove_file(scratch_elf);
    Ok(sha)
}

/// Create the output directory `out_arg` (relative to `cwd`) and return it
/// canonical. It must be new or empty, and must not be `root` (the
/// repository root, for a soak): the harness writes its files there and never
/// overwrites or deletes anything it did not create.
pub fn fresh_out_dir(cwd: &Path, out_arg: &OsStr, root: Option<&Path>) -> Result<PathBuf> {
    let out_path = cwd.join(out_arg);
    let shown = String::from_utf8_lossy(out_arg.as_bytes()).into_owned();
    if out_path.exists() && !out_path.is_dir() {
        bail!("--out {shown} exists and is not a directory");
    }
    std::fs::create_dir_all(&out_path)
        .map_err(|_| anyhow::anyhow!("cannot create output directory {shown}"))?;
    let out_dir =
        std::fs::canonicalize(&out_path).with_context(|| format!("cannot resolve {shown}"))?;
    if root == Some(out_dir.as_path()) {
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
    Ok(out_dir)
}

/// Write `summary.md` in `out_dir` (`head`, then the tables of `tally`),
/// and print the head and where the summary files are.
pub fn write_summary(
    out_dir: &Path,
    head: &[u8],
    tally: &Tally,
    out: &mut dyn Write,
) -> Result<()> {
    let md = write_summary_file(out_dir, head, tally)?;
    let tsv = out_dir.join("summary.tsv");
    out.write_all(b"\n")?;
    out.write_all(head)?;
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
    Ok(())
}

/// Write `summary.md` in `out_dir`: `head`, then the tables of `tally`.
/// Returns its path.
pub fn write_summary_file(out_dir: &Path, head: &[u8], tally: &Tally) -> Result<PathBuf> {
    let md = out_dir.join("summary.md");
    std::fs::write(&md, [head, &report::summary_tail(tally)].concat())
        .with_context(|| format!("cannot write {}", md.display()))?;
    Ok(md)
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
    let out_dir = fresh_out_dir(cwd, &out_arg, Some(&root))?;

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
        if let Some(code) = build_esp(&root, &build_log, "", None, interrupts, out, err)? {
            return Ok(code);
        }
    }
    snapshot_esp(&root, &disk_rel, &esp)?;
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
    out.flush()?;
    let kernel_sha = esp_kernel_sha(
        &root,
        &disk_rel,
        &kernel_rel,
        &esp,
        &scratch.path().join("aios.elf"),
        "",
        err,
    )?;
    let arm = Arm {
        qemu: OsString::from("qemu-system-aarch64"),
        firmware,
        esp,
        kernel_sha,
        git_rev,
    };
    let qemu_version = host::qemu_version(&arm.qemu);
    let load_start = host::loadavg();
    // A signal that ended one of the probes above (git, mcopy, QEMU's version,
    // sysctl) without failing the soak: stop before any report, as the trap did.
    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }

    let tsv = out_dir.join("summary.tsv");
    std::fs::write(&tsv, report::tsv_header())
        .with_context(|| format!("cannot write {}", tsv.display()))?;

    let data_word = if cfg.fresh_data { "fresh" } else { "reused" };
    out.write_all(
        format!(
            "soak: {} x {}s, mode={}, commit={}, data={data_word}\n",
            cfg.runs_raw, cfg.secs_raw, cfg.mode, arm.git_rev
        )
        .as_bytes(),
    )?;
    out.write_all(&[&b"soak: firmware="[..], &arm.firmware, b"\n"].concat())?;
    out.write_all(
        &[
            &b"soak: logs in "[..],
            bytes(&out_dir),
            format!(" (load average at start: {load_start})\n").as_bytes(),
        ]
        .concat(),
    )?;

    let width = cfg.runs_raw.len().max(2);
    let mut tally = Tally::default();
    let mut non_clean = false;

    for n in 1..=cfg.runs {
        if let Some(code) = interrupts.pending() {
            return Ok(code);
        }
        let idx = format!("{n:0width$}");
        let log_name = format!("run-{idx}.log");
        let log = out_dir.join(&log_name);
        let boot = match boot_once(&arm, cfg, &log, &data, interrupts)? {
            BootOutcome::Booted(boot) => boot,
            BootOutcome::Interrupted(code) => return Ok(code),
        };

        // A boot on which the UEFI stub never ran says nothing about the kernel.
        // On the first boot it means the setup is broken (QEMU failed to start,
        // or the firmware never loaded the stub), so stop.
        if n == 1 && !contains(&boot.text, b"AIOS UEFI stub") {
            err.write_all(host::tail_lines(&boot.text, 20))?;
            bail!(
                "the UEFI stub never ran on the first boot (QEMU exit status {} after {}s): check QEMU, the firmware ({}) and the ESP image; see {}",
                boot.rc,
                boot.elapsed,
                String::from_utf8_lossy(&arm.firmware),
                log.display()
            );
        }

        let c = classify(&boot.text, None);
        out.write_all(&report::format_result(
            format!("run {idx}/{}", cfg.runs_raw).as_bytes(),
            &c,
        ))?;
        non_clean |= c.class != Class::Clean;
        let timing = boot.timing();
        append(
            &tsv,
            &report::tsv_row(&idx, &cfg.mode, &c, &timing, &log_name),
        )?;
        tally.add(&idx, &c, &timing);
    }
    drop(scratch); // the ESP snapshot and the fresh data disk
    let load_end = host::loadavg();

    let host_line = [&host::uname()[..], b", ", &host::host_cpus(), b" CPUs"].concat();
    let info = SummaryInfo {
        mode: &cfg.mode,
        runs: &cfg.runs_raw,
        secs: &cfg.secs_raw,
        stall_secs: &cfg.stall_raw,
        fresh_data: Some(cfg.fresh_data),
        git_rev: &arm.git_rev,
        kernel_sha: &arm.kernel_sha,
        qemu_version: &qemu_version,
        firmware: &arm.firmware,
        host: &host_line,
        load_start: &load_start,
        load_end: &load_end,
        out: bytes(&out_dir),
    };
    let head = report::summary_head(&info, &tally);
    // A signal after the last boot's QEMU exited: the script's trap exited at
    // once, so there is no summary.md.
    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }
    write_summary(&out_dir, &head, &tally, out)?;

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
    fn the_footer_reads_back_as_the_boot_s_timing() {
        let boot = Boot {
            rc: 124,
            elapsed: 76,
            progress: Progress {
                hb: 3,
                adv: 70,
                kernel: 1,
                hb0: 2,
                bench: -1,
                g1: -1,
                gap: -1,
            },
            load1: b"1.50".to_vec(),
            text: Vec::new(),
        };
        let log = [
            &b"[heartbeat] tick=0\r\n\x1b[0m"[..],
            &footer(
                &cfg("gpu"),
                boot.elapsed,
                boot.rc,
                &boot.progress,
                &boot.load1,
            ),
        ]
        .concat();
        let f = Footer::parse(&log);
        assert_eq!(f.timing(), boot.timing());
        assert_eq!(f.get("mode"), Some(&b"gpu"[..]));
        assert_eq!(f.get("stall_limit"), Some(&b"15"[..]));
        // An empty load is kept as written; a log without a footer has nothing.
        let empty = footer(&cfg("text"), 1, 0, &boot.progress, b"");
        assert_eq!(Footer::parse(&empty).timing().load1, Some(Vec::new()));
        assert_eq!(
            Footer::parse(b"AIOS UEFI stub\n").timing(),
            BootTiming::default()
        );
        // A value that is not a number is unknown.
        let odd = Footer::parse(b"[soak] meta elapsed=7x qemu_rc=1\n").timing();
        assert_eq!((odd.elapsed, odd.rc), (None, Some(1)));
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
