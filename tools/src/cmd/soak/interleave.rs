//! `aios soak --arm DIR --arm DIR [--arm DIR [--arm DIR]]`: an interleaved
//! soak (crash-fix step 1a). Two to four git checkouts ("arms", labelled
//! A-D in the order given) are each built and snapshotted once, then booted in
//! rounds of one boot per arm, the arm order moving by one each round (A B,
//! B A, A B, ...), all in one host session with one QEMU binary and one
//! firmware file, by this harness, which classifies every boot. Booting the
//! arms alternately, instead of one soak after another, spreads the host's
//! load and other drift over every arm alike.
//!
//! Before any build, the preflight refuses (exit 2) a set of arms that would
//! not be comparable: a host tool missing from `PATH`, an arm that does not
//! contain [`MIN_ARM_BASE`] (#196: strict-NX firmware faults every older
//! kernel), arms whose justfiles name different firmware files, a QEMU binary
//! whose `--version` prints nothing, or (unless `--allow-mixed-toolchains`)
//! arms whose `rust-toolchain.toml` channels differ. With `--no-build` no arm
//! is built: each boots the ESP image already in its checkout, the arm-base
//! check covers the checkout's `HEAD` and the toolchain checks its pinned
//! toolchain, not that image or the compiler that built it, and `summary.md`'s
//! Arm base, Toolchains and Build rows say so. It also refuses a host whose
//! 1-minute load average is above its CPU count, unless `--ignore-load`: the
//! check runs before the builds, which raise the load themselves. After each
//! arm's build, its `rustc --version` must match the first arm's (same
//! override). The QEMU binary's version line and sha256 and the firmware's
//! sha256 are checked again before and after every boot: a change stops the
//! soak (exit 2), and a boot during which the change happened is not counted,
//! even when a signal cut another probe short.
//!
//! The run is report-only: it exits 0 when every boot ran, whatever the
//! classes, unless `--fail-on-regression` is given and some pair's regression
//! guard fails ([`pair`], exit 1). Each arm's directory `arm-X/` is a normal
//! single-run directory (`run-NN.log`, `summary.tsv` row by row, `build.log`
//! in the first arm directory of each checkout built (none with `--no-build`),
//! and `summary.md` once the soak finishes). The top level holds `arms.tsv`,
//! `boots.tsv` (every boot in boot order) and `summary.md`, rewritten after
//! every boot through a temporary file and a rename, with a status line and
//! the pair report, so a signal or a time limit still leaves a report of the
//! boots so far.

use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::awk::contains;
use super::classify::{classify, Class};
use super::host;
use super::pair::{self, Boot};
use super::report::{self, SummaryInfo, Tally};
use super::runner::{self, append, boot_once, bytes, Arm, BootOutcome, Config, ScratchDir};
use super::signals::{name_of_exit, Interrupts};
use super::stats::wilson;

/// #196 (7167d40): every arm must contain it. Strict-NX firmware (upstream
/// ArmVirt, Ubuntu 26.04) faults every kernel image built before it, so an
/// older arm's boots would say nothing about the change under test.
pub const MIN_ARM_BASE: &str = "7167d408f6a43ca9859fb6238f608ca6bdee37d6";

/// A full commit SHA that replaces [`MIN_ARM_BASE`] in builds with debug
/// assertions (the profile `cargo test -p aios-tools` uses), so the fake-arm
/// tests can name their own base. Release builds (`just tools`, so `just soak`
/// and CI's soak jobs) never read it.
pub const MIN_ARM_BASE_VAR: &str = "AIOS_SOAK_MIN_ARM_BASE";

/// Harness-error boots (the UEFI stub never ran) in a row, in boot order
/// across arms, that stop the soak.
pub const MAX_HARNESS_ERRORS_IN_A_ROW: u64 = 3;

/// The fewest and the most arms.
pub const MIN_ARMS: usize = 2;
pub const MAX_ARMS: usize = 4;

/// Arm labels, in `--arm` order.
pub const LABELS: [&str; MAX_ARMS] = ["A", "B", "C", "D"];

/// What `rustup toolchain install` needs, for its refusals.
pub const RUSTUP_HINT: &str = "installing each arm's pinned toolchain needs rustup 1.28 or later, and network access the first time";

/// Programs an interleaved soak runs, checked before anything else.
const HOST_TOOLS: [&str; 6] = [
    "git",
    "just",
    "rustup",
    "rustc",
    "qemu-system-aarch64",
    "mcopy",
];

/// Variables an arm's build must not inherit: they would pick the harness's
/// toolchain or target directory instead of the arm's own. The arm's `just
/// disk` copies the kernel from its checkout's `target/`, so a target
/// directory elsewhere (`CARGO_TARGET_DIR`, or cargo's config-variable form
/// `CARGO_BUILD_TARGET_DIR`) would package whatever stale kernel is there.
const BUILD_ENV_REMOVE: [&str; 3] = [
    "RUSTUP_TOOLCHAIN",
    "CARGO_TARGET_DIR",
    "CARGO_BUILD_TARGET_DIR",
];

/// An interleaved soak request: the boot settings (`runs` boots per arm), the
/// arm directories as given, and the interleave-only options.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub cfg: Config,
    pub arms: Vec<OsString>,
    pub allow_mixed_toolchains: bool,
    /// `--combine CLASS+CLASS...`: extra rows of every pair's tests.
    pub combine: Vec<Vec<Class>>,
    /// `--ignore-load`: soak even when the load is above the CPU count.
    pub ignore_load: bool,
    /// `--fail-on-regression`: exit 1 when some pair's regression guard fails.
    pub fail_on_regression: bool,
}

/// The start check of the host's load, before the builds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoadCheck {
    /// load1 was not above the CPU count.
    Passed { load1: String },
    /// `--ignore-load`: not checked; `above` when load1 was above the CPU count.
    Ignored { load1: String, above: bool },
    /// The load average or the CPU count could not be read.
    Unreadable,
}

impl LoadCheck {
    /// Check `loadavg` (as [`host::loadavg`] prints it) against `cpus` (as
    /// [`host::host_cpus`] prints it). Refuses (`Err`) when load1 is above
    /// the CPU count, unless `ignore`.
    pub fn new(loadavg: &str, cpus: &[u8], ignore: bool) -> Result<LoadCheck> {
        let load1 = loadavg.split(' ').next().unwrap_or("").to_string();
        let l: Option<f64> = load1.trim().parse().ok().filter(|v: &f64| v.is_finite());
        let n: Option<u64> = std::str::from_utf8(cpus)
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .filter(|&n| n > 0);
        let (Some(l), Some(n)) = (l, n) else {
            return Ok(if ignore && !load1.is_empty() {
                LoadCheck::Ignored {
                    load1,
                    above: false,
                }
            } else {
                LoadCheck::Unreadable
            });
        };
        let above = l > n as f64;
        if ignore {
            return Ok(LoadCheck::Ignored { load1, above });
        }
        if above {
            bail!(
                "the host's 1-minute load average is {load1}, above its {n} CPUs: boots on a loaded host measure the host as much as the arms. Wait for the load to drop, or pass --ignore-load to soak anyway (the loads are recorded)"
            );
        }
        Ok(LoadCheck::Passed { load1 })
    }

    /// The Settings row's text.
    fn text(&self) -> String {
        match self {
            LoadCheck::Passed { load1 } => {
                format!("passed: load1 {load1} before the builds, not above the host's CPU count")
            }
            LoadCheck::Ignored { load1, above } => format!(
                "skipped (--ignore-load): load1 {load1} before the builds{}",
                if *above {
                    ", above the host's CPU count"
                } else {
                    ""
                }
            ),
            LoadCheck::Unreadable => {
                "not checked: the load average or the CPU count could not be read".to_string()
            }
        }
    }
}

/// The commit every arm must contain, and whether [`MIN_ARM_BASE_VAR`] set it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArmBase {
    pub sha: String,
    pub overridden: bool,
}

impl ArmBase {
    /// [`MIN_ARM_BASE`], or in a debug-assertion build [`MIN_ARM_BASE_VAR`]
    /// when it is set (a warning on `err` says so).
    pub fn from_env(err: &mut dyn Write) -> Result<ArmBase> {
        if cfg!(debug_assertions) {
            if let Some(v) = std::env::var_os(MIN_ARM_BASE_VAR) {
                let sha = v.to_string_lossy().into_owned();
                if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
                    bail!("{MIN_ARM_BASE_VAR} must be a full 40-hex commit SHA, got '{sha}'");
                }
                runner::warn(
                    err,
                    format!(
                        "{MIN_ARM_BASE_VAR} overrides the arm base: every arm must contain {sha} instead of {MIN_ARM_BASE} (#196); only debug builds read it"
                    )
                    .as_bytes(),
                )?;
                return Ok(ArmBase {
                    sha,
                    overridden: true,
                });
            }
        }
        Ok(ArmBase {
            sha: MIN_ARM_BASE.to_string(),
            overridden: false,
        })
    }
}

/// The arm order of round `round` (0-based): the labels' indices moved left by
/// `round`.
pub fn round_order(arms: usize, round: u64) -> Vec<usize> {
    let shift = usize::try_from(round % arms as u64).expect("below the arm count");
    (0..arms).map(|i| (i + shift) % arms).collect()
}

/// The rotation as the report states it: the orders of the first rounds,
/// one per arm at most.
pub fn rotation_text(arms: usize, runs: u64) -> String {
    let shown = u64::try_from(arms).map_or(runs, |a| runs.min(a));
    let orders: Vec<String> = (0..shown)
        .map(|r| {
            round_order(arms, r)
                .iter()
                .map(|&i| LABELS[i])
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect();
    let repeat = if runs > shown { ", then again" } else { "" };
    format!(
        "the arm order moves by one each round: {}{repeat}",
        orders.join("; ")
    )
}

/// One git checkout under test; several labels share it in an A/A control.
struct Checkout {
    /// The directory as the first `--arm` naming it gave it.
    dir: OsString,
    root: PathBuf,
    /// The commit `HEAD` named when the preflight checked the arm base. The
    /// checkout must still be on it after its build and snapshot, or the
    /// arm-base claim in the report would not hold for what boots.
    head: String,
    channel: String,
    /// The labels (indices into [`LABELS`]) that boot it.
    labels: Vec<usize>,
}

/// A checkout once built and snapshotted.
struct Built {
    arm: Arm,
    rustc: String,
}

/// What every boot must find unchanged: the QEMU binary (its version line and
/// sha256) and the firmware (its sha256). Both are canonical paths, and every
/// boot runs exactly these files, so a symlink retargeted mid-soak (a `brew
/// upgrade qemu`, say) cannot swap what boots behind the probes.
struct Fixed {
    qemu: PathBuf,
    qemu_version: Vec<u8>,
    qemu_sha: String,
    firmware: PathBuf,
    firmware_sha: String,
}

impl Fixed {
    /// Why the soak must stop, if QEMU or the firmware changed; see
    /// [`change_of`] for how the probes combine.
    fn changed(&self, interrupts: &Interrupts) -> Option<&'static str> {
        // Each probe: whether it found the value unchanged, `None` when it could not run.
        let version = host::qemu_version(self.qemu.as_os_str());
        let probes = [
            (
                "the QEMU binary changed",
                host::sha256(&self.qemu).ok().map(|s| s == self.qemu_sha),
            ),
            (
                "the QEMU binary changed",
                (!version.is_empty()).then(|| version == self.qemu_version),
            ),
            (
                "the firmware changed",
                host::sha256(&self.firmware)
                    .ok()
                    .map(|s| s == self.firmware_sha),
            ),
        ];
        change_of(&probes, || interrupts.pending().is_some())
    }
}

/// The change, if any, that a set of probes found: each probe is a reason and
/// whether it found its value unchanged (`None` when it could not run). A probe
/// that found a change wins, whatever the others did. Otherwise a probe that
/// could not run is a change, unless a signal is pending: a terminal Ctrl-C
/// reaches the probe's child too, and the caller's pending check stops the soak
/// with the signal instead, as single mode does after its probes.
fn change_of(
    probes: &[(&'static str, Option<bool>)],
    signal_pending: impl FnOnce() -> bool,
) -> Option<&'static str> {
    if let Some((reason, _)) = probes.iter().find(|(_, same)| *same == Some(false)) {
        return Some(reason);
    }
    match probes.iter().find(|(_, same)| same.is_none()) {
        Some(_) if signal_pending() => None,
        Some((reason, _)) => Some(reason),
        None => None,
    }
}

/// The top-level `summary.md`: settings fixed at the start, and each arm's
/// class counts so far.
struct Report {
    mode: String,
    settings: Vec<(String, Vec<u8>)>,
    arms: Vec<Vec<Vec<u8>>>,
    tallies: Vec<Tally>,
    /// Every boot so far, in boot order, for the pair report.
    boots: Vec<Boot>,
    /// `--combine` groups.
    combine: Vec<Vec<Class>>,
    total: u64,
    out_dir: PathBuf,
    /// Whether a final status (`stopped` or `finished`) has been written.
    closed: bool,
}

impl Report {
    /// The boots counted so far.
    fn counted(&self) -> u64 {
        self.tallies.iter().map(Tally::boots).sum()
    }

    /// `summary.md` with `status` (`running (...)`, `stopped (...)`, `finished (...)`).
    fn render(&self, status: &str) -> Vec<u8> {
        let mut md = format!(
            "## AIOS QEMU interleaved soak ({} mode)\n\n**Status:** {status}\n\n### Settings\n\n| Setting | Value |\n|---|---|\n",
            self.mode
        )
        .into_bytes();
        for (name, value) in &self.settings {
            md.extend([format!("| {name} | ").as_bytes(), value, b" |\n"].concat());
        }
        md.extend_from_slice(
            b"\n### Arms\n\n| Arm | Checkout | Commit | Channel | rustc | Kernel |\n|---|---|---|---|---|---|\n",
        );
        for cells in &self.arms {
            md.extend_from_slice(b"|");
            for cell in cells {
                md.extend([&b" "[..], &report::md_cell(cell), b" |"].concat());
            }
            md.push(b'\n');
        }
        let labels = &LABELS[..self.tallies.len()];
        md.extend(
            format!(
                "\n### Classes per arm\n\n| Class | {} |\n|---|{}\n",
                labels.join(" | "),
                "---:|".repeat(labels.len())
            )
            .into_bytes(),
        );
        let row = |name: &str, cells: Vec<String>| format!("| {name} | {} |\n", cells.join(" | "));
        for class in Class::ALL {
            let cells = self
                .tallies
                .iter()
                .map(|t| t.count(class).to_string())
                .collect();
            md.extend(row(class.name(), cells).into_bytes());
        }
        let totals = self.tallies.iter().map(|t| t.boots().to_string()).collect();
        md.extend(row("**Total**", totals).into_bytes());
        let rates = self
            .tallies
            .iter()
            .map(|t| {
                let conclusive = t.boots() - t.count(Class::Inconclusive);
                wilson(t.count(Class::Clean), conclusive)
            })
            .collect();
        md.extend(row("CLEAN rate", rates).into_bytes());
        md.extend_from_slice(
            b"\nThe CLEAN rate is over each arm's conclusive boots (INCONCLUSIVE left out).\n",
        );
        md.extend(pair::sections(labels, &self.boots, &self.combine));
        md
    }

    /// Rewrite `summary.md` with `status`, through a temporary file and a
    /// rename, so a reader never sees a partial file.
    fn write(&self, status: &str) -> Result<()> {
        let tmp = self.out_dir.join(".summary.md.tmp");
        let md = self.out_dir.join("summary.md");
        std::fs::write(&tmp, self.render(status))
            .with_context(|| format!("cannot write {}", tmp.display()))?;
        std::fs::rename(&tmp, &md).with_context(|| format!("cannot write {}", md.display()))
    }

    fn running(&self) -> Result<()> {
        self.write(&format!(
            "running ({} of {} boots)",
            self.counted(),
            self.total
        ))
    }

    fn stopped(&mut self, reason: &str) -> Result<()> {
        self.closed = true;
        self.write(&format!(
            "stopped ({reason}) after {} of {} boots",
            self.counted(),
            self.total
        ))
    }
}

/// A cell value with tabs and newlines made spaces, for `arms.tsv`.
fn tsv_cell(v: &[u8]) -> Vec<u8> {
    v.iter()
        .map(|&b| if b == b'\t' || b == b'\n' { b' ' } else { b })
        .collect()
}

/// The harness's own commit: the git rev of the checkout holding the running
/// binary, and its path. The rev is `-dirty` when that checkout's host-tools
/// build inputs differ from its `HEAD`, untracked and ignored files included
/// ([`host::tools_inputs_dirty`]), since cargo builds from those too; a
/// checkout git cannot test is said so rather than shown clean. Read once,
/// at the start of the soak and before any arm build, so a commit or branch
/// switch in that checkout while the arms build cannot be named as the
/// classifier's.
fn harness_rev() -> Vec<u8> {
    let Ok(exe) = std::env::current_exe() else {
        return b"unknown".to_vec();
    };
    let (rev, note) = match exe.parent().and_then(|dir| host::repo_root(dir).ok()) {
        None => ("unknown".to_string(), ""),
        Some(root) => {
            let rev = host::git_rev(&root);
            match host::tools_inputs_dirty(&root) {
                Some(false) => (rev, ""),
                Some(true) if rev.ends_with("-dirty") => (rev, ""),
                Some(true) => (format!("{rev}-dirty"), ""),
                None => (rev, "; build inputs not checked"),
            }
        }
    };
    [
        format!("`{rev}` (").as_bytes(),
        b"`",
        bytes(&exe),
        b"`",
        note.as_bytes(),
        b")",
    ]
    .concat()
}

/// Run an interleaved soak. Returns the exit status: 0 when every boot ran,
/// 1 with `--fail-on-regression` when some pair's regression guard fails, or
/// 129, 130, 131 or 143 when a signal stopped it. Usage, preflight (the load
/// check included) and setup errors, and a stop for harness errors or a
/// changed QEMU or firmware, are `Err` (status 2).
pub fn run(req: &Request, cwd: &Path, out: &mut dyn Write, err: &mut dyn Write) -> Result<u8> {
    for tool in HOST_TOOLS {
        if host::find_in_path(tool).is_none() {
            if tool == "rustup" {
                bail!("rustup not found in PATH: {RUSTUP_HINT}");
            }
            bail!("{tool} not found in PATH");
        }
    }
    let base = ArmBase::from_env(err)?;
    run_with_base(req, &base, cwd, out, err)
}

/// [`run`] after the host-tool check, with the arm base given.
fn run_with_base(
    req: &Request,
    base: &ArmBase,
    cwd: &Path,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let cfg = &req.cfg;
    let shown = |d: &OsStr| String::from_utf8_lossy(d.as_bytes()).into_owned();
    // Read first, while the harness checkout is most likely still what the
    // running binary was built from (`just soak` builds it just before), and
    // never after the arm builds, which can take minutes.
    let harness = harness_rev();

    // The checkouts, each once, in label order.
    let mut checkouts: Vec<Checkout> = Vec::new();
    for (i, dir) in req.arms.iter().enumerate() {
        let label = LABELS[i];
        let path = cwd.join(dir);
        if !path.is_dir() {
            bail!("arm {label}: {} is not a directory", shown(dir));
        }
        let root = host::repo_root(&path).map_err(|_| {
            anyhow::anyhow!("arm {label}: {} is not inside a git checkout", shown(dir))
        })?;
        if let Some(c) = checkouts.iter_mut().find(|c| c.root == root) {
            c.labels.push(i);
            continue;
        }
        let channel = host::toolchain_channel(&root)
            .with_context(|| format!("arm {label} ({})", shown(dir)))?;
        // Pinned before the arm-base check, which tests this commit rather
        // than a fresh HEAD; a HEAD that differs from it after the build
        // fails the soak, and the report names this commit.
        let head = host::head_commit(&root).with_context(|| {
            format!("arm {label} ({}): cannot read its HEAD commit", shown(dir))
        })?;
        match host::contains_commit(&root, &base.sha, &head) {
            Ok(true) => {}
            Ok(false) => bail!(
                "arm {label} ({}) does not contain {} (#196: strict-NX firmware faults every older kernel); soak a later commit",
                shown(dir),
                base.sha
            ),
            Err(e) => bail!(
                "arm {label} ({}): cannot check that it contains {} ({e:#})",
                shown(dir),
                base.sha
            ),
        }
        checkouts.push(Checkout {
            dir: dir.clone(),
            root,
            head,
            channel,
            labels: vec![i],
        });
    }
    let first_label = |c: &Checkout| LABELS[c.labels[0]];
    if !req.allow_mixed_toolchains {
        let first = &checkouts[0];
        if let Some(other) = checkouts.iter().find(|c| c.channel != first.channel) {
            bail!(
                "the arms pin different toolchain channels (arm {}: {}, arm {}: {}); pass --allow-mixed-toolchains to soak a toolchain change",
                first_label(first),
                first.channel,
                first_label(other),
                other.channel
            );
        }
    }

    // One firmware file: every arm's justfile must name the same absolute path.
    let mut firmware: Option<(usize, Vec<u8>, PathBuf)> = None;
    for c in &checkouts {
        let label = first_label(c);
        let fw = host::just_evaluate(&c.root, "edk2_fw")
            .with_context(|| format!("arm {label} ({})", shown(&c.dir)))?;
        let fw_text = String::from_utf8_lossy(&fw).into_owned();
        if !fw.starts_with(b"/") {
            bail!("arm {label}: edk2_fw is {fw_text}, not an absolute path; set AIOS_EDK2_FW to the firmware file");
        }
        let resolved = std::fs::canonicalize(OsStr::from_bytes(&fw)).map_err(|_| {
            anyhow::anyhow!("arm {label}: UEFI firmware not found: {fw_text} (set AIOS_EDK2_FW)")
        })?;
        match &firmware {
            None => firmware = Some((c.labels[0], fw, resolved)),
            Some((first, first_fw, first_resolved)) if *first_resolved != resolved => bail!(
                "the arms boot different firmware (arm {}: {}, arm {label}: {fw_text}); every arm must name one firmware file",
                LABELS[*first],
                String::from_utf8_lossy(first_fw)
            ),
            Some(_) => {}
        }
    }
    let (_, firmware_raw, firmware_path) = firmware.expect("at least two arms");

    // One QEMU binary, the one PATH resolves to.
    let qemu = host::find_in_path("qemu-system-aarch64")
        .and_then(|p| std::fs::canonicalize(p).ok())
        .context("cannot resolve qemu-system-aarch64 in PATH")?;
    let qemu_version = host::qemu_version(qemu.as_os_str());
    if qemu_version.is_empty() {
        // Fixed::changed compares against this line before every boot: an
        // empty one would read as a changed binary before boot 1.
        bail!(
            "cannot run {} --version; check the QEMU install",
            qemu.display()
        );
    }
    let fixed = Fixed {
        qemu_version,
        qemu_sha: host::sha256(&qemu)?,
        firmware_sha: host::sha256(&firmware_path)?,
        qemu,
        firmware: firmware_path,
    };

    // The load, before the builds raise it.
    let load_before = host::loadavg();
    let load_check = LoadCheck::new(&load_before, &host::host_cpus(), req.ignore_load)?;
    if load_check == LoadCheck::Unreadable {
        runner::warn(
            err,
            b"cannot read the load average or the CPU count; the load check before the builds was skipped",
        )?;
    }

    // Cargo config files above an arm's checkout join its build: a worktree
    // inside another checkout gets that checkout's rustflags as well as its
    // own commit's.
    let cargo_home = host::cargo_home();
    let mut parent_configs: Vec<u8> = Vec::new();
    for c in &checkouts {
        let found = host::parent_cargo_configs(&c.root, cargo_home.as_deref());
        if found.is_empty() {
            continue;
        }
        let label = first_label(c);
        let files = found
            .iter()
            .map(|f| [&b"`"[..], bytes(f), b"`"].concat())
            .collect::<Vec<_>>()
            .join(&b", "[..]);
        runner::warn(
            err,
            &[
                format!("arm {label}: parent cargo config ").as_bytes(),
                &files,
                b" applies to the arm's build as well as its own; soak checkouts outside any other checkout",
            ]
            .concat(),
        )?;
        if !parent_configs.is_empty() {
            parent_configs.extend_from_slice(b"; ");
        }
        parent_configs.extend_from_slice(&[format!("arm {label}: ").as_bytes(), &files].concat());
    }
    if parent_configs.is_empty() {
        parent_configs.extend_from_slice(b"none above any arm's checkout");
    }

    let out_arg: OsString = match &cfg.out {
        Some(o) => o.clone(),
        None => host::repo_root(cwd)?
            .join("target/soak")
            .join(format!("{}-{}-arms", host::timestamp()?, cfg.mode))
            .into_os_string(),
    };
    // New or empty, so never an arm's checkout.
    let out_dir = runner::fresh_out_dir(cwd, &out_arg, None)?;

    let interrupts = Interrupts::install()?;
    let setup = Setup {
        checkouts,
        fixed,
        firmware_raw,
        base: base.clone(),
        load_before,
        load_check,
        parent_configs,
        harness,
        out_dir,
    };
    // As in single mode: a child that a terminal signal ended makes its step
    // fail, and the soak then ends with the signal's status.
    build_and_boot(req, setup, &interrupts, out, err).or_else(|e| interrupts.pending().ok_or(e))
}

/// What the preflight settled.
struct Setup {
    checkouts: Vec<Checkout>,
    fixed: Fixed,
    firmware_raw: Vec<u8>,
    base: ArmBase,
    /// The load average before the builds, and its check.
    load_before: String,
    load_check: LoadCheck,
    /// The cargo configs above each arm's checkout, for the summary.
    parent_configs: Vec<u8>,
    /// The Harness row ([`harness_rev`]), read before anything else.
    harness: Vec<u8>,
    out_dir: PathBuf,
}

/// `rustup toolchain install --no-self-update` in `root`, which installs the
/// toolchain `root`'s `rust-toolchain.toml` pins; output appended to `log`.
fn install_toolchain(
    root: &Path,
    log: &Path,
    label: &str,
    interrupts: &Interrupts,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Option<u8>> {
    out.write_all(
        &[
            format!(
                "soak: arm {label}: installing its pinned toolchain (rustup toolchain install) -> "
            )
            .as_bytes(),
            bytes(log),
            b"\n",
        ]
        .concat(),
    )?;
    out.flush()?;
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(log)
        .with_context(|| format!("cannot create {}", log.display()))?;
    let mut command = host::command("rustup");
    command
        .args(["toolchain", "install", "--no-self-update"])
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(file.try_clone()?)
        .stderr(file);
    for var in BUILD_ENV_REMOVE {
        command.env_remove(var);
    }
    let status = command.status();
    if let Some(code) = interrupts.pending() {
        return Ok(Some(code));
    }
    if !status.is_ok_and(|s| s.success()) {
        err.write_all(host::tail_lines(&runner::read(log)?, 30))?;
        bail!(
            "arm {label}: rustup toolchain install failed in {} ({RUSTUP_HINT}); full log: {}",
            root.display(),
            log.display()
        );
    }
    Ok(None)
}

/// Build and snapshot every checkout, boot the arms in rotating rounds, and
/// write the reports. Returns [`run`]'s status.
fn build_and_boot(
    req: &Request,
    setup: Setup,
    interrupts: &Interrupts,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<u8> {
    let cfg = &req.cfg;
    let Setup {
        checkouts,
        fixed,
        firmware_raw,
        base,
        load_before,
        load_check,
        parent_configs,
        harness,
        out_dir,
    } = setup;
    let n_arms = req.arms.len();
    let arm_dirs: Vec<PathBuf> = (0..n_arms)
        .map(|i| out_dir.join(format!("arm-{}", LABELS[i])))
        .collect();
    for dir in &arm_dirs {
        std::fs::create_dir(dir).with_context(|| format!("cannot create {}", dir.display()))?;
    }
    let scratch = ScratchDir::create(&out_dir).map_err(|_| {
        anyhow::anyhow!("cannot create a scratch directory in {}", out_dir.display())
    })?;

    // Build each checkout once, then snapshot it.
    let mut built: Vec<Built> = Vec::new();
    for c in &checkouts {
        let label = LABELS[c.labels[0]];
        let who = format!("arm {label}: ");
        let build_log = arm_dirs[c.labels[0]].join("build.log");
        if cfg.build {
            if let Some(code) = install_toolchain(&c.root, &build_log, label, interrupts, out, err)?
            {
                return Ok(code);
            }
            if let Some(code) = runner::build_esp(
                &c.root,
                &build_log,
                &who,
                Some(&BUILD_ENV_REMOVE),
                interrupts,
                out,
                err,
            )? {
                return Ok(code);
            }
        }
        let rustc = host::rustc_version(&c.root).with_context(|| format!("arm {label}"))?;
        if let Some(first) = built.first() {
            if !req.allow_mixed_toolchains && first.rustc != rustc {
                bail!(
                    "the arms' compilers differ (arm {}: {}, arm {label}: {rustc}); pass --allow-mixed-toolchains to soak a toolchain change",
                    LABELS[checkouts[0].labels[0]],
                    first.rustc
                );
            }
        }
        let disk_rel =
            host::just_evaluate(&c.root, "disk_img").with_context(|| format!("arm {label}"))?;
        let kernel_rel =
            host::just_evaluate(&c.root, "kernel_elf").with_context(|| format!("arm {label}"))?;
        let esp = scratch.path().join(format!("esp-{label}.img"));
        runner::snapshot_esp(&c.root, &disk_rel, &esp).with_context(|| format!("arm {label}"))?;
        let head = host::head_commit(&c.root);
        if head.as_deref() != Some(c.head.as_str()) {
            bail!(
                "arm {label} ({}): HEAD moved from {} to {} after the preflight checked it; soak checkouts that stay on one commit",
                String::from_utf8_lossy(c.dir.as_bytes()),
                c.head,
                head.as_deref().unwrap_or("an unreadable commit")
            );
        }
        // The pinned commit, not a fresh read of HEAD, which could have moved since the check.
        let git_rev = host::git_rev_of(&c.root, &c.head);
        out.flush()?;
        let kernel_sha = runner::esp_kernel_sha(
            &c.root,
            &disk_rel,
            &kernel_rel,
            &esp,
            &scratch.path().join("aios.elf"),
            &who,
            err,
        )?;
        built.push(Built {
            arm: Arm {
                qemu: fixed.qemu.clone().into_os_string(),
                firmware: bytes(&fixed.firmware).to_vec(),
                esp,
                kernel_sha,
                git_rev,
            },
            rustc,
        });
    }
    let load_after = host::loadavg();
    // A signal that ended a probe above without failing the soak.
    if let Some(code) = interrupts.pending() {
        return Ok(code);
    }

    // Each label's built checkout.
    let arm_of: Vec<usize> = (0..n_arms)
        .map(|i| {
            checkouts
                .iter()
                .position(|c| c.labels.contains(&i))
                .expect("every label has a checkout")
        })
        .collect();
    let data = scratch.path().join("data.img");
    let total = cfg.runs.saturating_mul(n_arms as u64);

    // arms.tsv, boots.tsv and each arm's summary.tsv header.
    let mut arms_tsv =
        b"arm\tcheckout\troot\tcommit\tchannel\trustc\tkernel\tfirmware\tqemu\tqemu_args\n"
            .to_vec();
    let mut arm_rows = Vec::new();
    for (i, &k) in arm_of.iter().enumerate() {
        let (c, b) = (&checkouts[k], &built[k]);
        let args = runner::qemu_args(
            &cfg.mode,
            OsStr::from_bytes(&b.arm.firmware),
            &b.arm.esp,
            &data,
        );
        let args: Vec<&[u8]> = args.iter().map(|a| a.as_bytes()).collect();
        let row: Vec<Vec<u8>> = vec![
            LABELS[i].as_bytes().to_vec(),
            req.arms[i].as_bytes().to_vec(),
            bytes(&c.root).to_vec(),
            b.arm.git_rev.as_bytes().to_vec(),
            c.channel.as_bytes().to_vec(),
            b.rustc.as_bytes().to_vec(),
            b.arm.kernel_sha.as_bytes().to_vec(),
            firmware_raw.clone(),
            bytes(&fixed.qemu).to_vec(),
            args.join(&b' '),
        ];
        let cells: Vec<Vec<u8>> = row.iter().map(|v| tsv_cell(v)).collect();
        arms_tsv.extend(cells.join(&b'\t'));
        arms_tsv.push(b'\n');
        arm_rows.push(vec![
            LABELS[i].as_bytes().to_vec(),
            [&b"`"[..], req.arms[i].as_bytes(), b"`"].concat(),
            format!("`{}`", b.arm.git_rev).into_bytes(),
            c.channel.as_bytes().to_vec(),
            b.rustc.as_bytes().to_vec(),
            b.arm.kernel_sha.as_bytes().to_vec(),
        ]);
    }
    let write = |path: PathBuf, data: &[u8]| -> Result<()> {
        std::fs::write(&path, data).with_context(|| format!("cannot write {}", path.display()))
    };
    write(out_dir.join("arms.tsv"), &arms_tsv)?;
    let boots_tsv = out_dir.join("boots.tsv");
    write(
        boots_tsv.clone(),
        &[&b"round\tposition\tarm\t"[..], &report::tsv_header()].concat(),
    )?;
    for dir in &arm_dirs {
        write(dir.join("summary.tsv"), &report::tsv_header())?;
    }

    let host_line = [&host::uname()[..], b", ", &host::host_cpus(), b" CPUs"].concat();
    let labels = LABELS[..n_arms].join(", ");
    // Without builds, the check covered each checkout's pinned toolchain, not
    // the compiler that built the image that boots.
    let toolchains: &[u8] = match (req.allow_mixed_toolchains, cfg.build) {
        (true, _) => {
            b"mixed allowed (--allow-mixed-toolchains): the arms' channels and compilers may differ"
        }
        (false, true) => b"one channel and one rustc for every arm",
        (false, false) => {
            b"one channel and one rustc for every arm, checked on each checkout's pinned toolchain, not on the compiler that built the ESP image that boots (--no-build)"
        }
    };
    let arm_base = if base.overridden {
        format!(
            "every arm contains `{}` ({MIN_ARM_BASE_VAR} override; the release base is `{MIN_ARM_BASE}`, #196)",
            base.sha
        )
    } else {
        format!("every arm contains `{}` (#196)", base.sha)
    };
    // Without builds, the check covered each checkout's HEAD, not the image that boots.
    let (arm_base, build) = if cfg.build {
        (
            arm_base,
            &b"each checkout's ESP image built once (`just disk`) from the commit the arm-base check saw"[..],
        )
    } else {
        (
            format!("{arm_base}; checked on each checkout's HEAD, not on the ESP image that boots (--no-build)"),
            &b"skipped (--no-build): each arm boots the ESP image already in its checkout, not one built from the checked commit"[..],
        )
    };
    let short = |s: &str| s.chars().take(16).collect::<String>();
    // The file every boot loads, when the justfile's path goes through a link.
    let firmware_via: Vec<u8> = if firmware_raw.as_slice() == bytes(&fixed.firmware) {
        Vec::new()
    } else {
        [&b"` -> `"[..], bytes(&fixed.firmware)].concat()
    };
    // A debug build's forged load, named in every summary.md it reaches
    // (report::summary_head names it in each arm's).
    let load_override = host::loadavg_override_note();
    let settings: Vec<(String, Vec<u8>)> = vec![
        (
            "Design".into(),
            format!(
                "{n_arms} arms ({labels}), {} rounds of one boot per arm: {total} boots",
                cfg.runs_raw
            )
            .into_bytes(),
        ),
        (
            "Rotation".into(),
            rotation_text(n_arms, cfg.runs).into_bytes(),
        ),
        (
            "Boots".into(),
            format!(
                "{}s each, stall limit {}s, data disk fresh per boot",
                cfg.secs_raw, cfg.stall_raw
            )
            .into_bytes(),
        ),
        (
            "QEMU".into(),
            [
                &fixed.qemu_version[..],
                b" (`",
                bytes(&fixed.qemu),
                format!("`, sha256 `{}`)", short(&fixed.qemu_sha)).as_bytes(),
            ]
            .concat(),
        ),
        (
            "Firmware".into(),
            [
                &b"`"[..],
                &firmware_raw,
                &firmware_via,
                format!("` (sha256 `{}`)", short(&fixed.firmware_sha)).as_bytes(),
            ]
            .concat(),
        ),
        ("Host".into(), host_line.clone()),
        (
            "Load average".into(),
            format!(
                "before the builds {load_before}; after the builds {load_after}{load_override}"
            )
            .into_bytes(),
        ),
        (
            "Load check".into(),
            format!("{}{load_override}", load_check.text()).into_bytes(),
        ),
        ("Toolchains".into(), toolchains.to_vec()),
        ("Build".into(), build.to_vec()),
        ("Parent cargo config".into(), parent_configs),
        ("Arm base".into(), arm_base.into_bytes()),
        ("Harness".into(), harness),
        ("Logs".into(), [&b"`"[..], bytes(&out_dir), b"`"].concat()),
    ];
    let mut report = Report {
        mode: cfg.mode.clone(),
        settings,
        arms: arm_rows,
        tallies: (0..n_arms).map(|_| Tally::default()).collect(),
        boots: Vec::new(),
        combine: req.combine.clone(),
        total,
        out_dir: out_dir.clone(),
        closed: false,
    };
    report.running()?;

    // From here on summary.md exists. An error that wrote no final status
    // (a full disk on a log or a row, say) marks it stopped before it ends
    // the soak, so the status never stays "running".
    let result = (|| -> Result<u8> {
        out.write_all(
            format!(
                "soak: interleaved, {n_arms} arms x {} rounds x {}s, mode={}, data=fresh\n",
                cfg.runs_raw, cfg.secs_raw, cfg.mode
            )
            .as_bytes(),
        )?;
        for (i, &k) in arm_of.iter().enumerate() {
            out.write_all(
                &[
                    format!("soak: arm {}: ", LABELS[i]).as_bytes(),
                    bytes(&checkouts[k].root),
                    format!(" commit={}\n", built[k].arm.git_rev).as_bytes(),
                ]
                .concat(),
            )?;
        }
        out.write_all(&[&b"soak: qemu="[..], bytes(&fixed.qemu), b"\n"].concat())?;
        out.write_all(&[&b"soak: firmware="[..], &firmware_raw, b"\n"].concat())?;
        out.write_all(
            &[
                &b"soak: logs in "[..],
                bytes(&out_dir),
                format!(" (load average after the builds: {load_after})\n").as_bytes(),
            ]
            .concat(),
        )?;

        let width = cfg.runs_raw.len().max(2);
        let mut seen_first = vec![false; n_arms];
        let mut errors_in_a_row = 0u64;
        for round in 0..cfg.runs {
            for (position, label) in round_order(n_arms, round).into_iter().enumerate() {
                let changed = match interrupts.pending() {
                    None => fixed.changed(interrupts),
                    Some(_) => None,
                };
                // A signal before the probes, or one that ended a probe.
                if let Some(code) = interrupts.pending() {
                    report.stopped(&name_of_exit(code))?;
                    return Ok(code);
                }
                if let Some(reason) = changed {
                    report.stopped(reason)?;
                    bail!(
                        "{reason} during the soak; see {}",
                        out_dir.join("summary.md").display()
                    );
                }
                let arm = &built[arm_of[label]].arm;
                let idx = format!("{:0width$}", round + 1);
                let log_name = format!("run-{idx}.log");
                let log = arm_dirs[label].join(&log_name);
                let boot = match boot_once(arm, cfg, &log, &data, interrupts)? {
                    BootOutcome::Booted(boot) => boot,
                    BootOutcome::Interrupted(code) => {
                        report.stopped(&name_of_exit(code))?;
                        return Ok(code);
                    }
                };
                // A change during the boot: the boot is not counted. A signal that
                // ended a probe is no change: the boot is counted, and the next
                // pending check stops the soak with the signal.
                if let Some(reason) = fixed.changed(interrupts) {
                    report.stopped(reason)?;
                    bail!(
                        "{reason} during boot {} of arm {} (not counted); see {}",
                        round + 1,
                        LABELS[label],
                        out_dir.join("summary.md").display()
                    );
                }
                let stub_ran = contains(&boot.text, b"AIOS UEFI stub");
                if !seen_first[label] {
                    seen_first[label] = true;
                    if !stub_ran {
                        report.stopped(&format!(
                            "the UEFI stub never ran on arm {}'s first boot",
                            LABELS[label]
                        ))?;
                        err.write_all(host::tail_lines(&boot.text, 20))?;
                        bail!(
                        "arm {}: the UEFI stub never ran on its first boot (QEMU exit status {} after {}s): check QEMU, the firmware ({}) and the ESP image; see {}",
                        LABELS[label],
                        boot.rc,
                        boot.elapsed,
                        String::from_utf8_lossy(&arm.firmware),
                        log.display()
                    );
                    }
                }

                let c = classify(&boot.text, None);
                out.write_all(&report::format_result(
                    format!("{} run {idx}/{}", LABELS[label], cfg.runs_raw).as_bytes(),
                    &c,
                ))?;
                let timing = boot.timing();
                append(
                    &arm_dirs[label].join("summary.tsv"),
                    &report::tsv_row(&idx, &cfg.mode, &c, &timing, &log_name),
                )?;
                let boots_log = format!("arm-{}/{log_name}", LABELS[label]);
                append(
                    &boots_tsv,
                    &[
                        format!("{}\t{}\t{}\t", round + 1, position + 1, LABELS[label]).as_bytes(),
                        &report::tsv_row(&idx, &cfg.mode, &c, &timing, &boots_log),
                    ]
                    .concat(),
                )?;
                report.tallies[label].add(&idx, &c, &timing);
                report
                    .boots
                    .push(Boot::new(round + 1, label, &c, &timing, boots_log));

                errors_in_a_row = if stub_ran { 0 } else { errors_in_a_row + 1 };
                if errors_in_a_row >= MAX_HARNESS_ERRORS_IN_A_ROW {
                    let reason = format!(
                    "{MAX_HARNESS_ERRORS_IN_A_ROW} boots in a row where the UEFI stub never ran"
                );
                    report.stopped(&reason)?;
                    bail!(
                        "{reason}: check QEMU, the firmware and the host; see {}",
                        out_dir.join("summary.md").display()
                    );
                }
                report.running()?;
            }
        }
        drop(scratch); // the ESP snapshots and the fresh data disk
        let load_end = host::loadavg();
        if let Some(code) = interrupts.pending() {
            report.stopped(&name_of_exit(code))?;
            return Ok(code);
        }

        // Each arm's summary.md, as a single soak writes it.
        for (i, &k) in arm_of.iter().enumerate() {
            let b = &built[k];
            let info = SummaryInfo {
                mode: &cfg.mode,
                runs: &cfg.runs_raw,
                secs: &cfg.secs_raw,
                stall_secs: &cfg.stall_raw,
                fresh_data: Some(true),
                git_rev: &b.arm.git_rev,
                kernel_sha: &b.arm.kernel_sha,
                qemu_version: &fixed.qemu_version,
                firmware: &firmware_raw,
                host: &host_line,
                load_start: &load_after,
                load_end: &load_end,
                out: bytes(&arm_dirs[i]),
            };
            let head = report::summary_head(&info, &report.tallies[i]);
            runner::write_summary_file(&arm_dirs[i], &head, &report.tallies[i])?;
        }
        report.settings.iter_mut().for_each(|(name, value)| {
        if name == "Load average" {
            *value = format!(
                "before the builds {load_before}; after the builds {load_after}; at the end {load_end}{load_override}"
            )
            .into_bytes();
        }
    });
        report.closed = true;
        report.write(&format!("finished ({} boots)", report.counted()))?;
        if let Some(code) = interrupts.pending() {
            return Ok(code);
        }
        let labels = &LABELS[..n_arms];
        out.write_all(b"\n")?;
        out.write_all(&pair::console(labels, &report.boots))?;
        out.write_all(
            &[
                &b"soak: interleaved report in "[..],
                bytes(&out_dir.join("summary.md")),
                b", rows in ",
                bytes(&boots_tsv),
                b", each arm's single-run files in arm-X/\n",
            ]
            .concat(),
        )?;
        if req.fail_on_regression && pair::regression(n_arms, &report.boots) {
            out.write_all(
                b"soak: a pair's regression guard failed (--fail-on-regression): exit 1\n",
            )?;
            return Ok(1);
        }
        Ok(0)
    })();
    match result {
        Err(e) if !report.closed => {
            let reason = match interrupts.pending() {
                Some(code) => name_of_exit(code),
                None => format!("error: {e:#}").replace('\n', " "),
            };
            // The original error is what the caller reports; a failure to
            // write the stopped status adds nothing to it.
            let _ = report.stopped(&reason);
            Err(e)
        }
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_found_change_wins_over_a_probe_a_signal_cut_short() {
        let qemu = "the QEMU binary changed";
        let fw = "the firmware changed";
        // Ctrl-C ended the QEMU digest, but the firmware probe saw a change.
        assert_eq!(
            change_of(&[(qemu, None), (fw, Some(false))], || true),
            Some(fw)
        );
        // A probe that could not run is no change while a signal is pending,
        // and a change otherwise.
        assert_eq!(change_of(&[(qemu, None), (fw, Some(true))], || true), None);
        assert_eq!(
            change_of(&[(qemu, None), (fw, Some(true))], || false),
            Some(qemu)
        );
        assert_eq!(
            change_of(&[(qemu, Some(true)), (fw, Some(true))], || false),
            None
        );
    }

    #[test]
    fn rounds_rotate_by_one() {
        let order = |arms, rounds| -> String {
            (0..rounds)
                .map(|r| {
                    round_order(arms, r)
                        .iter()
                        .map(|&i| LABELS[i])
                        .collect::<String>()
                })
                .collect::<Vec<_>>()
                .join(" ")
        };
        assert_eq!(order(2, 4), "AB BA AB BA");
        assert_eq!(order(3, 4), "ABC BCA CAB ABC");
        assert_eq!(order(4, 2), "ABCD BCDA");
        assert_eq!(
            rotation_text(2, 5),
            "the arm order moves by one each round: A B; B A, then again"
        );
        assert_eq!(
            rotation_text(3, 2),
            "the arm order moves by one each round: A B C; B C A"
        );
        assert_eq!(
            rotation_text(2, 1),
            "the arm order moves by one each round: A B"
        );
    }

    #[test]
    fn the_load_check_refuses_load1_above_the_cpu_count() {
        let passed = |l: &str| LoadCheck::Passed { load1: l.into() };
        assert_eq!(
            LoadCheck::new("3.99 9.00 9.00", b"4", false).expect("passes"),
            passed("3.99")
        );
        // Equal to the CPU count is not above it.
        assert_eq!(
            LoadCheck::new("4.00 1.00 1.00", b"4\n", false).expect("passes"),
            passed("4.00")
        );
        let refused = LoadCheck::new("4.01 1.00 1.00", b"4", false).expect_err("refused");
        assert_eq!(
            format!("{refused:#}"),
            "the host's 1-minute load average is 4.01, above its 4 CPUs: boots on a loaded host measure the host as much as the arms. Wait for the load to drop, or pass --ignore-load to soak anyway (the loads are recorded)"
        );
        let ignored = LoadCheck::new("4.01 1.00 1.00", b"4", true).expect("ignored");
        assert_eq!(
            ignored,
            LoadCheck::Ignored {
                load1: "4.01".into(),
                above: true
            }
        );
        assert_eq!(
            ignored.text(),
            "skipped (--ignore-load): load1 4.01 before the builds, above the host's CPU count"
        );
        assert_eq!(
            passed("0.50").text(),
            "passed: load1 0.50 before the builds, not above the host's CPU count"
        );
        // Unreadable: no refusal, and the report says so.
        for (load, cpus) in [
            ("", &b"4"[..]),
            ("x 1 1", b"4"),
            ("1.00 1 1", b"?"),
            ("1.00", b"0"),
        ] {
            assert_eq!(
                LoadCheck::new(load, cpus, false).expect("not refused"),
                LoadCheck::Unreadable,
                "{load:?} {cpus:?}"
            );
        }
        assert_eq!(
            LoadCheck::Unreadable.text(),
            "not checked: the load average or the CPU count could not be read"
        );
    }

    #[test]
    fn the_report_counts_classes_per_arm() {
        let report = Report {
            mode: "text".into(),
            settings: vec![("Design".into(), b"2 arms".to_vec())],
            arms: vec![vec![b"A".to_vec(), b"`a|b`".to_vec()]],
            tallies: vec![Tally::default(), Tally::default()],
            boots: Vec::new(),
            combine: Vec::new(),
            total: 4,
            out_dir: PathBuf::from("/nonexistent"),
            closed: false,
        };
        let md = String::from_utf8(report.render("running (0 of 4 boots)")).expect("UTF-8");
        assert!(md.starts_with(
            "## AIOS QEMU interleaved soak (text mode)\n\n**Status:** running (0 of 4 boots)\n\n### Settings\n\n| Setting | Value |\n|---|---|\n| Design | 2 arms |\n"
        ), "{md}");
        assert!(md.contains("| A | `a\\|b` |\n"), "{md}");
        assert!(
            md.contains("| Class | A | B |\n|---|---:|---:|\n| PCZERO | 0 | 0 |\n"),
            "{md}"
        );
        assert!(
            md.contains("| **Total** | 0 | 0 |\n| CLEAN rate | n/a | n/a |\n"),
            "{md}"
        );
        // The pair report follows the class table, in USAGE's order.
        let at = |h: &str| md.find(h).unwrap_or_else(|| panic!("no {h} in\n{md}"));
        let order = [
            "### Classes per arm",
            "### Load",
            "### Pair tests",
            "### Gate 1 IPC",
            "### Tripwire per arm",
            "### Non-CLEAN boots",
        ]
        .map(at);
        assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
    }
}
