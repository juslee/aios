//! The soak boot classifier: a port of `CLASSIFY_AWK` and `classify_log` in the
//! former `scripts/soak-qemu.sh` (blob at `212df62`, L160-417).
//!
//! `classify` takes a raw serial log, removes NUL and CR bytes and ANSI escape
//! sequences as `classify_log` did (`tr -d '\000\r' | sed "s/ESC\[[0-9;]*[A-Za-z]//g"`),
//! then runs the awk program's per-line rules and its `END` block. Every field is
//! a byte string: `clip` cuts at a byte count, so a field can end inside a UTF-8
//! sequence, exactly as the awk program's output did.
//!
//! awk semantics (numbers, `trim`, `clip`, field splitting) live in [`super::awk`].
//! Regex matches use the leftmost start, which is all the awk program reads from
//! `match()` (RSTART) except for the heartbeat, whose `[0-9]+` is greedy in both
//! engines. Accepted divergences: none known beyond those in `awk`.
//!
//! Crash-fix step 1a refines two of the script's classes after its rules ran:
//! WEDGE splits into WEDGE-STUCK (the heartbeat never printed, stayed at tick 0
//! or stopped) and WEDGE-ALIVE (the heartbeat kept running but the Gate 1 bench
//! never completed, or a gpu marker is missing), and a PANIC whose joined first
//! fatal line contains `lock re-entry:` is PANIC-LOCK. A boot the script calls
//! CLEAN is DEGRADED unless its Gate 1 IPC line reports [`IPC_ITERATIONS`]
//! iterations ([`Ipc`]). Every other field is the script's, so
//! [`Classification::base_line`], which prints the base class, is
//! byte-identical to the script's output line.

use std::collections::HashMap;
use std::sync::LazyLock;

use regex::bytes::Regex;

use super::awk::{clip, contains, fields, find, num_str, to_num, trim};

/// A boot's class. The declaration order is the summary order ([`Class::ALL`]),
/// with each subclass next to its base class.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    /// An exception report that shows a jump to PC 0.
    PcZero,
    /// A panic whose message is step 1b's `lock re-entry:`.
    PanicLock,
    /// Any other panic.
    Panic,
    /// Any other exception report.
    Exception,
    /// No fatal report; the heartbeat never printed, stayed at tick 0 or stopped.
    WedgeStuck,
    /// No fatal report; the heartbeat kept running, but the Gate 1 bench never
    /// completed or (gpu mode) a gpu marker is missing.
    WedgeAlive,
    /// Not a result about the kernel.
    Inconclusive,
    /// Healthy by every other rule, but the Gate 1 IPC line does not report
    /// [`IPC_ITERATIONS`] iterations, or cannot be read.
    Degraded,
    /// A healthy boot.
    Clean,
}

impl Class {
    /// The number of classes.
    pub const COUNT: usize = 9;

    /// Every class, in the order the summary counts them.
    pub const ALL: [Class; Class::COUNT] = [
        Class::PcZero,
        Class::PanicLock,
        Class::Panic,
        Class::Exception,
        Class::WedgeStuck,
        Class::WedgeAlive,
        Class::Inconclusive,
        Class::Degraded,
        Class::Clean,
    ];

    /// The printed name.
    pub const fn name(self) -> &'static str {
        match self {
            Class::PcZero => "PCZERO",
            Class::PanicLock => "PANIC-LOCK",
            Class::Panic => "PANIC",
            Class::Exception => "EXCEPTION",
            Class::WedgeStuck => "WEDGE-STUCK",
            Class::WedgeAlive => "WEDGE-ALIVE",
            Class::Inconclusive => "INCONCLUSIVE",
            Class::Degraded => "DEGRADED",
            Class::Clean => "CLEAN",
        }
    }

    /// The class called `name`, if any.
    pub fn from_name(name: &str) -> Option<Class> {
        Class::ALL.into_iter().find(|c| c.name() == name)
    }

    /// The position in [`Class::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The class the script reported for the same boot.
    pub const fn base(self) -> Base {
        match self {
            Class::PcZero => Base::PcZero,
            Class::PanicLock | Class::Panic => Base::Panic,
            Class::Exception => Base::Exception,
            Class::WedgeStuck | Class::WedgeAlive => Base::Wedge,
            Class::Inconclusive => Base::Inconclusive,
            Class::Degraded | Class::Clean => Base::Clean,
        }
    }
}

/// The six classes of the former `scripts/soak-qemu.sh`, which every [`Class`]
/// refines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Base {
    PcZero,
    Panic,
    Exception,
    Wedge,
    Inconclusive,
    Clean,
}

impl Base {
    /// The script's name for the class.
    pub const fn name(self) -> &'static str {
        match self {
            Base::PcZero => "PCZERO",
            Base::Panic => "PANIC",
            Base::Exception => "EXCEPTION",
            Base::Wedge => "WEDGE",
            Base::Inconclusive => "INCONCLUSIVE",
            Base::Clean => "CLEAN",
        }
    }
}

/// What a PANIC-LOCK's `lock re-entry:` message names. A field is `None` when
/// the message (cut short, or broken up by another CPU's output) does not carry it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reentry {
    /// The lock static, with its `[index]` for a per-CPU array.
    pub lock: Option<Vec<u8>>,
    /// The waiter's context (`ctx=`).
    pub ctx: Option<Vec<u8>>,
    /// Whether the holder had IRQs on (`holder_irqs=`): `on` or `off`.
    pub holder_irqs: Option<Vec<u8>>,
}

/// The Gate 1 IPC round-trip iterations a CLEAN boot reports:
/// `IPC_ITERATIONS` in `kernel/src/bench.rs`, which a test keeps in step.
pub const IPC_ITERATIONS: u64 = 10_000;

/// The first `[bench] IPC round-trip (same core):` line's figures. A field is
/// `None` when the boot printed no such line, or the line (cut short, or broken
/// up by another CPU's output) does not carry it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Ipc {
    /// `avg=`: the mean round trip in whole µs, as the kernel truncates it.
    pub avg_us: Option<u64>,
    /// `(N iters)`: the round trips the bench completed.
    pub iters: Option<u64>,
}

/// One classified boot: the eleven fields of the awk program's output line,
/// with the class refined, plus what step 1a parses beside them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Classification {
    /// The refined class; [`Class::base`] is the script's.
    pub class: Class,
    /// The last heartbeat tick, or `-`.
    pub tick: Vec<u8>,
    /// The number of heartbeat lines.
    pub hb: Vec<u8>,
    /// Seconds of heartbeat silence before the planned end, or `-` without harness timing.
    pub stall: Vec<u8>,
    /// Comma-separated boot markers, or `-`.
    pub markers: Vec<u8>,
    /// `yes`, `no` or `-`: whether the last INFO line was a load-balance migration.
    pub lb: &'static str,
    /// The `; `-joined notes, or `-`.
    pub detail: Vec<u8>,
    /// The first fatal report line (with its panic message or ELR line), or `-`.
    pub first: Vec<u8>,
    /// The last three kernel INFO lines before the first fatal report, oldest first, each or `-`.
    pub info: [Vec<u8>; 3],
    /// The `lock re-entry:` message's fields, for a PANIC-LOCK only.
    pub reentry: Option<Reentry>,
    /// The Gate 1 IPC figures, read whatever the class.
    pub ipc: Ipc,
}

impl Classification {
    /// The classifier line, without its newline: the eleven fields joined by
    /// tabs, with the refined class name first.
    pub fn line(&self) -> Vec<u8> {
        self.fields(self.class.name())
    }

    /// The awk program's output line, without its newline: [`Self::line`] with
    /// the base class name in field 1.
    pub fn base_line(&self) -> Vec<u8> {
        self.fields(self.class.base().name())
    }

    fn fields(&self, class: &str) -> Vec<u8> {
        let parts: [&[u8]; 11] = [
            class.as_bytes(),
            &self.tick,
            &self.hb,
            &self.stall,
            &self.markers,
            self.lb.as_bytes(),
            &self.detail,
            &self.first,
            &self.info[0],
            &self.info[1],
            &self.info[2],
        ];
        parts.join(&b'\t')
    }
}

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("a valid classifier regex")
}

static HEARTBEAT: LazyLock<Regex> = LazyLock::new(|| re(r"\[heartbeat\] tick=[0-9]+"));
static BOOT_EL1: LazyLock<Regex> = LazyLock::new(|| re(r"Boot +EL: 1"));
static GATE1_PASS: LazyLock<Regex> = LazyLock::new(|| re(r"Gate 1: IPC < 10 us: *PASS"));
static REPORT: LazyLock<Regex> = LazyLock::new(|| {
    re(r"EXCEPTION\[CPU [0-9]+\]:|(DATA|INST) ABORT \(EL0\):|UNKNOWN EXCEPTION \(EL0\)")
});
static EDK2: LazyLock<Regex> =
    LazyLock::new(|| re(r"(Synchronous|IRQ|FIQ|SError) Exception at 0x[0-9A-Fa-f]+"));
static REGISTER_FIELDS: LazyLock<Regex> = LazyLock::new(|| {
    re(r"ESR=0x[0-9a-f]+ EC=0x|EC=0x[0-9a-f]+ FAR=0x[0-9a-f]+ ELR=0x|\(EL0\): (FAR|EC)=0x")
});
static ABORT_AT: LazyLock<Regex> = LazyLock::new(|| re(r"(Data|Instruction) Abort at 0x"));
static INFO: LazyLock<Regex> = LazyLock::new(|| re(r"\[ *[0-9]+\.[0-9]+\] \[[0-9]+\] INFO "));
static PC0_ABORT: LazyLock<Regex> = LazyLock::new(|| re(r"EC=0x0*2[01] FAR=0x0000000000000000"));
static ANSI: LazyLock<Regex> = LazyLock::new(|| re(r"\x1b\[[0-9;]*[A-Za-z]"));
static REENTRY_LOCK: LazyLock<Regex> = LazyLock::new(|| re(r"^[A-Z][A-Z0-9_]*(\[[0-9]+\])?"));
static REENTRY_CTX: LazyLock<Regex> = LazyLock::new(|| re(r" ctx=([a-z][a-z-]*)"));
static REENTRY_IRQS: LazyLock<Regex> = LazyLock::new(|| re(r" holder_irqs=(on|off)\b"));
static IPC_AVG: LazyLock<Regex> = LazyLock::new(|| re(r"^avg=([0-9]+) us\b"));
static IPC_ITERS: LazyLock<Regex> = LazyLock::new(|| re(r" \(([0-9]+) iters\)"));

const META: &[u8] = b"[soak] meta ";
const ELR_ZERO: &[u8] = b"ELR=0x0000000000000000";
const INST_ABORT_ZERO: &[u8] = b"Instruction Abort at 0x0000000000000000";
const REENTRY: &[u8] = b"lock re-entry: ";
const IPC_LINE: &[u8] = b"[bench] IPC round-trip (same core): ";

/// The figures of an IPC line, from `msg`, the text after its
/// [`IPC_LINE`] prefix. A number too large for `u64` is unreadable.
fn ipc_of(msg: &[u8]) -> Ipc {
    let number = |r: &Regex| {
        r.captures(msg)
            .and_then(|c| c.get(1))
            .and_then(|m| std::str::from_utf8(m.as_bytes()).ok()?.parse().ok())
    };
    Ipc {
        avg_us: number(&IPC_AVG),
        iters: number(&IPC_ITERS),
    }
}

/// The fields of the `lock re-entry:` message in a PANIC's joined first fatal
/// line, or `None` when the line has no such message.
fn reentry_of(first: &[u8]) -> Option<Reentry> {
    let msg = &first[find(first, REENTRY)? + REENTRY.len()..];
    let group = |r: &Regex| {
        r.captures(msg)
            .and_then(|c| c.get(1))
            .map(|m| m.as_bytes().to_vec())
    };
    Some(Reentry {
        lock: REENTRY_LOCK.find(msg).map(|m| m.as_bytes().to_vec()),
        ctx: group(&REENTRY_CTX),
        holder_irqs: group(&REENTRY_IRQS),
    })
}

/// `pc0_of(s)`: an EL1/EL0 exception report line that shows a jump to PC 0.
fn pc0_of(line: &[u8]) -> bool {
    contains(line, ELR_ZERO) || PC0_ABORT.is_match(line)
}

/// What a line reports (`kind` in the awk program).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    PcZero,
    Exception,
    Edk2,
    Panic,
}

/// `classify_log`'s preprocessing: drop NUL and CR bytes, then ANSI escape sequences.
pub fn preprocess(raw: &[u8]) -> Vec<u8> {
    let kept: Vec<u8> = raw
        .iter()
        .copied()
        .filter(|&b| b != 0 && b != b'\r')
        .collect();
    ANSI.replace_all(&kept, &b""[..]).into_owned()
}

/// awk records: lines split at `\n`, with no empty record after a final newline.
fn records(text: &[u8]) -> Vec<&[u8]> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix(b"\n").unwrap_or(text);
    body.split(|&b| b == b'\n').collect()
}

/// Classify one raw serial log. `limit_override` is `--stall-secs` in `--classify`
/// mode (`None`: the footer's `stall_limit`).
pub fn classify(raw: &[u8], limit_override: Option<u64>) -> Classification {
    let text = preprocess(raw);
    let mut s = Scan::default();
    for (index, line) in records(&text).into_iter().enumerate() {
        s.line(i64::try_from(index).expect("line count fits i64") + 1, line);
    }
    s.finish(limit_override)
}

/// The awk program's variables (its `BEGIN` block is `Default` plus the fields
/// initialised in [`Scan::default`]).
struct Scan {
    hb: u64,
    tick: f64,
    hb_nr: i64,
    boots: u64,
    bench_nr: i64,
    stub: bool,
    fatal: Option<Class>,
    first: Vec<u8>,
    nfatal: u64,
    pend: bool,
    fatal_tick: f64,
    edk2: bool,
    cutpfx: bool,
    exwin: u32,
    exfirst: bool,
    head_nr: i64,
    info: [Vec<u8>; 3],
    have_meta: bool,
    meta: HashMap<Vec<u8>, Vec<u8>>,
    el1: bool,
    boot: bool,
    g1pass: bool,
    g1done: bool,
    gpu: bool,
    input: bool,
    handoff: bool,
    ipc: Option<Ipc>,
}

impl Default for Scan {
    fn default() -> Scan {
        Scan {
            hb: 0,
            tick: -1.0,
            hb_nr: 0,
            boots: 0,
            bench_nr: 0,
            stub: false,
            fatal: None,
            first: Vec::new(),
            nfatal: 0,
            pend: false,
            fatal_tick: -1.0,
            edk2: false,
            cutpfx: false,
            exwin: 0,
            exfirst: false,
            head_nr: -100,
            info: [Vec::new(), Vec::new(), Vec::new()],
            have_meta: false,
            meta: HashMap::new(),
            el1: false,
            boot: false,
            g1pass: false,
            g1done: false,
            gpu: false,
            input: false,
            handoff: false,
            ipc: None,
        }
    }
}

impl Scan {
    /// The awk program's per-record rules for record number `nr` (1-based, like NR).
    fn line(&mut self, nr: i64, line: &[u8]) {
        // The panic message follows "PANIC: panicked at <loc>:" on the next
        // non-empty line; a footer right after the panic ends the wait.
        if self.pend && line.starts_with(META) {
            self.pend = false;
        }
        if self.pend && !line.iter().all(|&b| b == b' ') {
            self.first.extend_from_slice(b" / ");
            self.first.extend(clip(&trim(line), 160));
            self.pend = false;
        }
        if line.starts_with(META) {
            self.have_meta = true;
            for field in fields(line).skip(2) {
                if let Some(eq) = field.iter().position(|&b| b == b'=') {
                    if eq > 0 {
                        self.meta
                            .insert(field[..eq].to_vec(), field[eq + 1..].to_vec());
                    }
                }
            }
            return;
        }

        if let Some(m) = HEARTBEAT.find(line) {
            self.tick = to_num(&line[m.start() + 17..m.end()]);
            self.hb += 1;
            self.hb_nr = nr;
        }
        if contains(line, b"AIOS UEFI stub") {
            self.stub = true;
        }
        if contains(line, b"AIOS kernel booting") {
            self.boots += 1;
        }
        if BOOT_EL1.is_match(line) {
            self.el1 = true;
        }
        if contains(line, b"Boot sequence complete") {
            self.boot = true;
        }
        if GATE1_PASS.is_match(line) {
            self.g1pass = true;
        }
        if contains(line, b"=== Gate 1 Complete ===") {
            self.g1done = true;
        }
        if contains(line, b"=== Gate 1 Benchmark ===") {
            self.bench_nr = nr;
        }
        if contains(line, b"GpuReady") {
            self.gpu = true;
        }
        if contains(line, b"InputReady") {
            self.input = true;
        }
        if contains(line, b"display handoff complete") {
            self.handoff = true;
        }
        if self.ipc.is_none() {
            if let Some(p) = find(line, IPC_LINE) {
                self.ipc = Some(ipc_of(&line[p + IPC_LINE.len()..]));
            }
        }

        // Fatal reports. The exception and panic handlers print without a lock,
        // so output from another CPU can split a report line anywhere.
        let mut kind: Option<Kind> = None;
        let mut start = 0;
        let mut head = false;
        let mut cont = false;
        let exception_or_pc0 = |line: &[u8]| {
            if pc0_of(line) {
                Kind::PcZero
            } else {
                Kind::Exception
            }
        };
        if let Some(m) = REPORT.find(line) {
            kind = Some(exception_or_pc0(line));
            start = m.start();
            head = contains(line, b"EXCEPTION[CPU");
        } else if let Some(m) = EDK2.find(line) {
            kind = Some(Kind::Edk2);
            start = m.start();
        } else if let Some(p) = find(line, b"PANIC: ") {
            kind = Some(Kind::Panic);
            start = p;
        } else if REGISTER_FIELDS.is_match(line) {
            // The register fields of an EL1 or EL0 report without its prefix:
            // the rest of a report whose prefix line was cut (see exwin), or a
            // report whose prefix itself was split by another CPU's output.
            if self.exwin > 0 {
                cont = true;
            } else {
                kind = Some(exception_or_pc0(line));
                head = !contains(line, b"(EL0)");
                self.cutpfx = self.cutpfx || self.fatal.is_none();
            }
        } else if ABORT_AT.is_match(line) {
            // sync_exception_handler's second line: part of the EL1 report just
            // above it, or, with none within 4 lines, a stand-in for a lost one.
            if self.exwin > 0 || nr - self.head_nr <= 4 {
                cont = true;
            } else {
                kind = Some(if contains(line, INST_ABORT_ZERO) {
                    Kind::PcZero
                } else {
                    Kind::Exception
                });
                self.cutpfx = self.cutpfx || self.fatal.is_none();
            }
        }
        if self.exwin > 0 {
            // An EL1 report cut before its ELR: look for the ELR, or the
            // "Instruction Abort at" line, just below it.
            self.exwin -= 1;
            if kind.is_none() && (contains(line, ELR_ZERO) || contains(line, INST_ABORT_ZERO)) {
                if self.exfirst {
                    self.fatal = Some(Class::PcZero);
                    self.first.extend_from_slice(b" / ");
                    self.first.extend(clip(&trim(line), 100));
                }
                self.exwin = 0;
            } else if kind.is_none()
                && (contains(line, b"ELR=0x")
                    || contains(line, b"Instruction Abort at")
                    || contains(line, b"Data Abort at"))
            {
                if self.exfirst && contains(line, b"ELR=0x") {
                    self.first.extend_from_slice(b" / ");
                    self.first.extend(clip(&trim(line), 100));
                }
                self.exwin = 0;
            }
        }
        if head {
            self.head_nr = nr;
            // A window still open for the first report keeps priority.
            if !contains(line, b"ELR=") && !(self.exwin > 0 && self.exfirst) {
                self.exwin = 3;
                self.exfirst = self.fatal.is_none();
            }
        }
        if let Some(kind) = kind {
            self.nfatal += 1;
            if self.fatal.is_none() {
                self.fatal = Some(match kind {
                    Kind::PcZero => Class::PcZero,
                    Kind::Exception | Kind::Edk2 => Class::Exception,
                    Kind::Panic => Class::Panic,
                });
                self.edk2 = kind == Kind::Edk2;
                self.first = clip(&trim(&line[start..]), 200);
                self.fatal_tick = self.tick;
                self.pend = kind == Kind::Panic;
            }
        } else if !cont && self.fatal.is_none() {
            // Kernel INFO lines, frozen at the first fatal report.
            if let Some(m) = INFO.find(line) {
                self.info.rotate_left(1);
                self.info[2] = clip(&trim(&line[m.start()..]), 160);
            }
        }
    }

    /// A footer time in seconds, or -1 when the footer lacks it (`secs_of`).
    fn secs_of(&self, key: &[u8]) -> f64 {
        self.meta.get(key).map_or(-1.0, |v| to_num(v))
    }

    /// The awk program's `END` block.
    fn finish(self, limit_override: Option<u64>) -> Classification {
        let mut notes: Vec<Vec<u8>> = Vec::new();
        let mut note = |s: Vec<u8>| notes.push(s);

        let mut markers: Vec<&str> = Vec::new();
        for (on, name) in [
            (self.el1, "EL1"),
            (self.boot, "BOOT"),
            (self.g1pass, "G1PASS"),
            (self.g1done, "G1DONE"),
            (self.gpu, "GPU"),
            (self.input, "INPUT"),
            (self.handoff, "HANDOFF"),
        ] {
            if on {
                markers.push(name);
            }
        }
        let markers = if markers.is_empty() {
            "-".to_string()
        } else {
            markers.join(",")
        };

        let timing = self.have_meta
            && self.meta.contains_key(&b"elapsed"[..])
            && self.meta.contains_key(&b"hb_last_advance"[..]);
        let mut stall = 0.0;
        let mut early = false;
        let mut signaled = false;
        let mut sig_noted = false;
        let mut missing: Vec<&str> = Vec::new();
        let mut run_end = 0.0;
        let mut limit = 0.0;
        let mut kst = -1.0;
        let mut hbf = -1.0;
        let mut bst = -1.0;
        let mut elapsed = 0.0;
        let mut rc: Vec<u8> = Vec::new();
        if timing {
            elapsed = to_num(&self.meta[&b"elapsed"[..]]);
            // Silence is measured up to the planned end of the run (the footer's
            // secs), not to QEMU's actual exit.
            run_end = self.meta.get(&b"secs"[..]).map_or(elapsed, |v| to_num(v));
            let adv = self.secs_of(b"hb_last_advance");
            kst = self.secs_of(b"kstart");
            hbf = self.secs_of(b"hb_first");
            bst = self.secs_of(b"bench_start");
            stall = since(
                run_end,
                if adv >= 0.0 {
                    adv
                } else if kst >= 0.0 {
                    kst
                } else {
                    0.0
                },
            );
            limit = match limit_override {
                Some(n) => n as f64,
                None => to_num(self.meta.get(&b"stall_limit"[..]).map_or(&b""[..], |v| v)),
            };
            // 124, or 137 once --kill-after fired, at the time limit means the
            // limit ran out; anything else means QEMU ended on its own.
            rc = self.meta.get(&b"qemu_rc"[..]).cloned().unwrap_or_default();
            early = !rc.is_empty() && !((rc == b"124" || rc == b"137") && elapsed >= run_end);
            signaled = early && to_num(&rc) > 128.0;
            if self.meta.get(&b"mode"[..]).map(Vec::as_slice) == Some(&b"gpu"[..]) {
                if !self.gpu {
                    missing.push("GpuReady");
                }
                if !self.input {
                    missing.push("InputReady");
                }
                if !self.handoff {
                    missing.push("display handoff");
                }
            }
        }

        let n = num_str;
        let class: Class;
        if !self.stub && self.boots == 0 && self.hb == 0 && (self.fatal.is_none() || self.edk2) {
            class = Class::Inconclusive;
            note(b"UEFI stub never ran, not a boot result".to_vec());
            if self.fatal.is_some() {
                note(b"edk2-format report from the firmware".to_vec());
            }
        } else if let Some(fatal) = self.fatal {
            // The earliest report decides, so a `lock re-entry:` after another
            // report leaves that report's class.
            class = if fatal == Class::Panic && contains(&self.first, REENTRY) {
                Class::PanicLock
            } else {
                fatal
            };
            if self.cutpfx {
                note(b"report prefix split by other output".to_vec());
            }
            if self.edk2 {
                note(b"edk2-format report from the firmware or UEFI stub".to_vec());
            }
            note(if self.fatal_tick < 0.0 {
                b"before the first heartbeat".to_vec()
            } else {
                format!("after heartbeat tick {}", n(self.fatal_tick)).into_bytes()
            });
            if self.nfatal > 1 {
                note(format!("{} fatal reports", self.nfatal).into_bytes());
            }
        } else if signaled {
            // QEMU itself crashed or was killed; the silence is not the kernel's doing.
            class = Class::Inconclusive;
            let mut s = format!(
                "QEMU killed by signal {} after {}s (rc=",
                n(to_num(&rc) - 128.0),
                n(elapsed)
            )
            .into_bytes();
            s.extend_from_slice(&rc);
            s.extend_from_slice(b"), not a boot result");
            note(s);
            sig_noted = true;
        } else if self.hb == 0 {
            let what = if self.boot {
                "no heartbeat after boot sequence complete"
            } else if self.boots > 0 {
                "no heartbeat; boot sequence incomplete"
            } else {
                "no heartbeat; kernel never started"
            };
            if timing && stall <= limit {
                class = Class::Inconclusive;
                let since_what = if kst >= 0.0 {
                    "the kernel started"
                } else {
                    "QEMU started"
                };
                note(
                    format!(
                        "cut short: {what}, only {}s since {since_what} (limit {}s)",
                        n(stall),
                        n(limit)
                    )
                    .into_bytes(),
                );
            } else {
                class = Class::WedgeStuck;
                note(what.as_bytes().to_vec());
            }
        } else if self.tick == 0.0 {
            // The bench prints its header 500 ticks after it starts, then runs its IPC
            // loop with IRQs on; no tick=1000 means CPU 0 took no timer IRQ after that
            // (typically its timer IRQ spinning on a lock the interrupted thread holds).
            let what = if self.bench_nr > self.hb_nr && !self.g1done {
                "heartbeat stuck at tick 0 after the Gate 1 bench started"
            } else {
                "heartbeat never advanced past tick 0"
            };
            if timing && stall <= limit {
                class = Class::Inconclusive;
                note(
                    format!(
                        "cut short: {what}, only {}s before the end (limit {}s)",
                        n(stall),
                        n(limit)
                    )
                    .into_bytes(),
                );
            } else {
                class = Class::WedgeStuck;
                note(what.as_bytes().to_vec());
            }
        } else if timing && stall > limit {
            class = Class::WedgeStuck;
            note(
                format!(
                    "heartbeat stopped at tick {}, silent {}s before the end (limit {}s)",
                    n(self.tick),
                    n(stall),
                    n(limit)
                )
                .into_bytes(),
            );
        } else if !self.g1done {
            let what = if self.bench_nr != 0 {
                "heartbeat alive but the Gate 1 bench never completed"
            } else {
                "heartbeat alive but the Gate 1 bench never started"
            };
            // The bench gets --stall-secs to finish, from its header (or, if it
            // never printed one, from the first heartbeat).
            let reference = if bst >= 0.0 { bst } else { hbf };
            if timing && reference >= 0.0 && since(run_end, reference) <= limit {
                class = Class::Inconclusive;
                let since_what = if bst >= 0.0 {
                    "the bench header"
                } else {
                    "the first heartbeat"
                };
                note(
                    format!(
                        "cut short: {what}, only {}s since {since_what} (limit {}s)",
                        n(since(run_end, reference)),
                        n(limit)
                    )
                    .into_bytes(),
                );
            } else {
                class = Class::WedgeAlive;
                note(what.as_bytes().to_vec());
            }
        } else if !missing.is_empty() {
            class = Class::WedgeAlive;
            note(format!("gpu markers missing: {}", missing.join(",")).into_bytes());
        } else {
            // Decided last, so it never masks a fatal report, a wedge or a cut
            // short boot, and it adds no note: `detail` stays the script's.
            let ipc = self.ipc.unwrap_or_default();
            class = if ipc.iters == Some(IPC_ITERATIONS) {
                Class::Clean
            } else {
                Class::Degraded
            };
            if !timing {
                note(
                    b"log-only: no harness timing, a late heartbeat stall is undetectable".to_vec(),
                );
            }
        }
        if early && !sig_noted {
            let mut s = b"qemu exited before the time limit (rc=".to_vec();
            s.extend_from_slice(&rc);
            if signaled {
                s.extend(format!(", signal {}", n(to_num(&rc) - 128.0)).into_bytes());
            }
            s.push(b')');
            note(s);
        }
        // Informational only: CPU 0 went quiet for a while after the bench, then recovered.
        let gap = self.secs_of(b"hb_max_gap");
        if gap > limit {
            note(format!("heartbeat paused {}s after the bench completed", n(gap)).into_bytes());
        }
        if self.boots > 1 {
            note(format!("guest booted {} times", self.boots).into_bytes());
        }

        let i3 = &self.info[2];
        let lb = if matches!(class.base(), Base::Clean | Base::Inconclusive) || i3.is_empty() {
            "-"
        } else if contains(i3, b"Load balance: migrated") {
            "yes"
        } else {
            "no"
        };
        let dash = |v: Vec<u8>| if v.is_empty() { b"-".to_vec() } else { v };
        let reentry = if class == Class::PanicLock {
            reentry_of(&self.first)
        } else {
            None
        };
        let [i1, i2, i3] = self.info;
        Classification {
            class,
            tick: if self.tick < 0.0 {
                b"-".to_vec()
            } else {
                n(self.tick).into_bytes()
            },
            hb: self.hb.to_string().into_bytes(),
            stall: if timing {
                n(stall).into_bytes()
            } else {
                b"-".to_vec()
            },
            markers: markers.into_bytes(),
            lb,
            detail: dash(notes.join(&b"; "[..])),
            first: dash(self.first),
            info: [dash(i1), dash(i2), dash(i3)],
            reentry,
            ipc: self.ipc.unwrap_or_default(),
        }
    }
}

/// `since(t)`: seconds from footer time `t` to the planned end of the boot, never negative.
fn since(run_end: f64, t: f64) -> f64 {
    if t >= run_end {
        0.0
    } else {
        run_end - t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The awk program's output line for `log` (the goldens in
    /// `tests/golden/soak/classify.golden` cover the whole corpus).
    fn line(log: &str, limit: Option<u64>) -> String {
        String::from_utf8(classify(log.as_bytes(), limit).line()).expect("ASCII in these cases")
    }

    #[test]
    fn panic_message_is_joined_and_info_lines_kept() {
        let log = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n[heartbeat] tick=0\n[heartbeat] tick=1000\n\
                   [   2.000000] [0] INFO  Mm    frame allocator ready\n\
                   PANIC: panicked at kernel/src/sched/mod.rs:120:9:\n\nassertion failed: thread.state == Ready\n";
        assert_eq!(
            line(log, None),
            "PANIC\t1000\t2\t-\t-\tno\tafter heartbeat tick 1000\t\
             PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready\t\
             -\t-\t[   2.000000] [0] INFO  Mm    frame allocator ready"
        );
    }

    #[test]
    fn elr_zero_below_a_cut_report_makes_it_pczero() {
        let log = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n[heartbeat] tick=0\n\
                   EXCEPTION[CPU 2]: Synchronous exception\n\
                   [   2.500000] [0] INFO  Ipc   interleaved output from CPU 0\n\
                   \x20 ESR=0x86000006 EC=0x21 FAR=0x0 ELR=0x0000000000000000\n";
        assert_eq!(
            line(log, None),
            "PCZERO\t0\t1\t-\t-\t-\tafter heartbeat tick 0\t\
             EXCEPTION[CPU 2]: Synchronous exception / ESR=0x86000006 EC=0x21 FAR=0x0 ELR=0x0000000000000000\t-\t-\t-"
        );
    }

    #[test]
    fn firmware_exception_before_the_stub_is_inconclusive() {
        let log = "UEFI firmware (version edk2-stable202408)\nSynchronous Exception at 0x00000000BFE01234\n\n\
                   [soak] meta mode=text secs=75 elapsed=75 qemu_rc=124 kstart=-1 hb_first=-1 bench_start=-1 g1done=-1 hb_count=0 hb_last_advance=-1 hb_max_gap=-1 stall_limit=15 load1=1.00\n";
        assert_eq!(
            line(log, None),
            "INCONCLUSIVE\t-\t0\t75\t-\t-\tUEFI stub never ran, not a boot result; edk2-format report from the firmware\t\
             Synchronous Exception at 0x00000000BFE01234\t-\t-\t-"
        );
    }

    #[test]
    fn qemu_killed_by_a_signal_is_inconclusive() {
        let log = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n[heartbeat] tick=0\n[heartbeat] tick=1000\n\n\
                   [soak] meta mode=text secs=75 elapsed=30 qemu_rc=139 kstart=1 hb_first=1 bench_start=-1 g1done=-1 hb_count=2 hb_last_advance=29 hb_max_gap=-1 stall_limit=15 load1=1.00\n";
        assert_eq!(
            line(log, None),
            "INCONCLUSIVE\t1000\t2\t46\t-\t-\tQEMU killed by signal 11 after 30s (rc=139), not a boot result\t-\t-\t-\t-"
        );
    }

    #[test]
    fn a_late_kernel_start_is_cut_short_not_a_wedge() {
        let log = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n\n\
                   [soak] meta mode=text secs=75 elapsed=75 qemu_rc=124 kstart=70 hb_first=-1 bench_start=-1 g1done=-1 hb_count=0 hb_last_advance=-1 hb_max_gap=-1 stall_limit=15 load1=1.00\n";
        assert_eq!(
            line(log, None),
            "INCONCLUSIVE\t-\t0\t5\t-\t-\tcut short: no heartbeat; boot sequence incomplete, only 5s since the kernel started (limit 15s)\t-\t-\t-\t-"
        );
        // --stall-secs 4 in --classify mode overrides the footer's limit.
        assert!(line(log, Some(4)).starts_with("WEDGE-STUCK\t-\t0\t5\t"));
    }

    const STUB: &str = "AIOS UEFI stub v0.1.0\n";
    const KERNEL: &str = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n";

    /// A harness footer: `secs=75`, the limit ran out, stall limit 15 s.
    fn footer(mode: &str, hb_last_advance: i64) -> String {
        format!(
            "[soak] meta mode={mode} secs=75 elapsed=75 qemu_rc=124 kstart=1 hb_first=2 \
             bench_start=7 g1done=-1 hb_count=2 hb_last_advance={hb_last_advance} \
             hb_max_gap=-1 stall_limit=15 load1=1.00\n"
        )
    }

    /// The class and detail of `log`, classified with the footer's limit.
    fn class_detail(log: &str) -> (Class, String) {
        let c = classify(log.as_bytes(), None);
        (c.class, String::from_utf8(c.detail).expect("ASCII"))
    }

    fn assert_class(log: &str, class: Class, detail: &str) {
        assert_eq!(class_detail(log), (class, detail.to_string()), "{log}");
    }

    #[test]
    fn no_heartbeat_is_wedge_stuck() {
        assert_class(
            &format!("{KERNEL}[   0.2] Boot sequence complete\n"),
            Class::WedgeStuck,
            "no heartbeat after boot sequence complete",
        );
        assert_class(
            KERNEL,
            Class::WedgeStuck,
            "no heartbeat; boot sequence incomplete",
        );
        assert_class(
            STUB,
            Class::WedgeStuck,
            "no heartbeat; kernel never started",
        );
    }

    #[test]
    fn a_heartbeat_stuck_at_tick_0_is_wedge_stuck() {
        assert_class(
            &format!("{KERNEL}[heartbeat] tick=0\n=== Gate 1 Benchmark ===\n"),
            Class::WedgeStuck,
            "heartbeat stuck at tick 0 after the Gate 1 bench started",
        );
        assert_class(
            &format!("{KERNEL}[heartbeat] tick=0\n"),
            Class::WedgeStuck,
            "heartbeat never advanced past tick 0",
        );
    }

    #[test]
    fn a_heartbeat_that_stopped_is_wedge_stuck() {
        assert_class(
            &format!(
                "{KERNEL}[heartbeat] tick=0\n[heartbeat] tick=1000\n{}",
                footer("text", 40)
            ),
            Class::WedgeStuck,
            "heartbeat stopped at tick 1000, silent 35s before the end (limit 15s)",
        );
    }

    #[test]
    fn a_live_heartbeat_without_a_completed_bench_is_wedge_alive() {
        assert_class(
            &format!(
                "{KERNEL}[heartbeat] tick=0\n=== Gate 1 Benchmark ===\n[heartbeat] tick=1000\n"
            ),
            Class::WedgeAlive,
            "heartbeat alive but the Gate 1 bench never completed",
        );
        assert_class(
            &format!(
                "{KERNEL}[heartbeat] tick=0\n[heartbeat] tick=1000\n{}",
                footer("text", 74)
            ),
            Class::WedgeAlive,
            "heartbeat alive but the Gate 1 bench never started",
        );
    }

    #[test]
    fn missing_gpu_markers_are_wedge_alive() {
        assert_class(
            &format!(
                "{KERNEL}[heartbeat] tick=0\nGpuReady\n=== Gate 1 Complete ===\n[heartbeat] tick=1000\n{}",
                footer("gpu", 74)
            ),
            Class::WedgeAlive,
            "gpu markers missing: InputReady,display handoff",
        );
    }

    /// Step 1b's two-line re-entry panic (B1 text run 05, CR bytes dropped).
    const REENTRY_PANIC: &str = "PANIC: panicked at kernel/src/sched/scheduler.rs:196:38:\n\
        lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-exit holder=kernel/src/cap/mod.rs:39 holder_irqs=on tid=16 gen=508894\n\
        [panic] cpu=0 tid=16 ctx=irq-exit irq_was=off t=6.629260 irq_elr=0xffff0000000c3a08\n";

    #[test]
    fn a_lock_re_entry_panic_is_panic_lock_with_its_fields() {
        let log = format!("{KERNEL}[heartbeat] tick=0\n{REENTRY_PANIC}");
        let c = classify(log.as_bytes(), None);
        assert_eq!(c.class, Class::PanicLock);
        let b = |s: &str| Some(s.as_bytes().to_vec());
        assert_eq!(
            c.reentry,
            Some(Reentry {
                lock: b("THREAD_TABLE"),
                ctx: b("irq-exit"),
                holder_irqs: b("on"),
            })
        );
        assert_eq!(c.detail, b"after heartbeat tick 0");
        // A per-CPU lock keeps its index; a holder without a site prints `?`.
        let log = format!(
            "{KERNEL}PANIC: panicked at kernel/src/sched/scheduler.rs:192:46:\n\
             lock re-entry: CURRENT_THREAD[0] on CPU 0 ctx=irq holder=? holder_irqs=off tid=? gen=356738\n"
        );
        let c = classify(log.as_bytes(), None);
        assert_eq!(c.class, Class::PanicLock);
        assert_eq!(
            c.reentry,
            Some(Reentry {
                lock: b("CURRENT_THREAD[0]"),
                ctx: b("irq"),
                holder_irqs: b("off"),
            })
        );
    }

    #[test]
    fn a_message_broken_by_other_output_keeps_what_it_can_read() {
        let log = format!(
            "{KERNEL}PANIC: panicked at kernel/src/sched/scheduler.rs:196:38:\n\
             lock re-entry: THREAD_TABLE on CPU 0 ctx=irq-e[heartbeat] tick=1000\n"
        );
        let c = classify(log.as_bytes(), None);
        assert_eq!(c.class, Class::PanicLock);
        assert_eq!(
            c.reentry,
            Some(Reentry {
                lock: Some(b"THREAD_TABLE".to_vec()),
                ctx: Some(b"irq-e".to_vec()),
                holder_irqs: None,
            })
        );
    }

    #[test]
    fn any_other_panic_stays_panic() {
        let log =
            format!("{KERNEL}PANIC: panicked at kernel/src/mm/frame.rs:51:9:\nout of frames\n");
        let c = classify(log.as_bytes(), None);
        assert_eq!(c.class, Class::Panic);
        assert_eq!(c.reentry, None);
    }

    #[test]
    fn a_lock_re_entry_after_an_earlier_report_keeps_that_report_s_class() {
        let log = format!(
            "{KERNEL}[heartbeat] tick=0\nEXCEPTION[CPU 1]: Synchronous exception\n\
             \x20 ESR=0x96000004 EC=0x25 FAR=0x10 ELR=0xffff000000091234\n{REENTRY_PANIC}"
        );
        let c = classify(log.as_bytes(), None);
        assert_eq!(c.class, Class::Exception);
        assert_eq!(c.reentry, None);
        assert_eq!(c.detail, b"after heartbeat tick 0; 2 fatal reports");
    }

    #[test]
    fn base_line_differs_from_line_in_the_class_only() {
        let log = format!("{KERNEL}[heartbeat] tick=0\n{REENTRY_PANIC}");
        let c = classify(log.as_bytes(), None);
        let line = String::from_utf8(c.line()).expect("ASCII");
        let base = String::from_utf8(c.base_line()).expect("ASCII");
        assert_eq!(line.split_once('\t').map(|p| p.0), Some("PANIC-LOCK"));
        assert_eq!(base.split_once('\t').map(|p| p.0), Some("PANIC"));
        assert_eq!(
            line.split_once('\t').map(|p| p.1),
            base.split_once('\t').map(|p| p.1)
        );
    }

    #[test]
    fn classes_are_listed_in_summary_order_beside_their_base() {
        let names: Vec<&str> = Class::ALL.iter().map(|c| c.name()).collect();
        assert_eq!(
            names,
            [
                "PCZERO",
                "PANIC-LOCK",
                "PANIC",
                "EXCEPTION",
                "WEDGE-STUCK",
                "WEDGE-ALIVE",
                "INCONCLUSIVE",
                "DEGRADED",
                "CLEAN"
            ]
        );
        for (i, class) in Class::ALL.into_iter().enumerate() {
            assert_eq!(class.index(), i);
            assert_eq!(Class::from_name(class.name()), Some(class));
        }
        let bases: Vec<&str> = Class::ALL.iter().map(|c| c.base().name()).collect();
        assert_eq!(
            bases,
            [
                "PCZERO",
                "PANIC",
                "PANIC",
                "EXCEPTION",
                "WEDGE",
                "WEDGE",
                "INCONCLUSIVE",
                "CLEAN",
                "CLEAN"
            ]
        );
        assert_eq!(Class::from_name("WEDGE"), None);
    }

    /// The kernel's IPC line for `iters` round trips, CR byte included: 6 µs
    /// on average, or the 0 the kernel prints without a round trip.
    fn ipc_line(iters: u64) -> String {
        let avg = if iters == 0 { 0 } else { 6 };
        format!(
            "[bench] IPC round-trip (same core): avg={avg} us, p99=8 us, min=4992 ns, max=754000 ns ({iters} iters)\r\n"
        )
    }

    /// A boot that is healthy by every rule but rule 3's IPC count, with
    /// `ipc` in place of the IPC line.
    fn healthy(ipc: &str) -> String {
        format!(
            "{KERNEL}[heartbeat] tick=0\n[bench] === Gate 1 Benchmark ===\n{ipc}\
             [bench] Gate 1: IPC < 10 us:           PASS\n[bench] === Gate 1 Complete ===\n\
             [heartbeat] tick=1000\n{}",
            footer("text", 74)
        )
    }

    #[test]
    fn a_full_ipc_count_is_clean_and_its_figures_are_read() {
        let c = classify(healthy(&ipc_line(10_000)).as_bytes(), None);
        assert_eq!(c.class, Class::Clean);
        assert_eq!(
            c.ipc,
            Ipc {
                avg_us: Some(6),
                iters: Some(10_000)
            }
        );
        assert_eq!(c.detail, b"-");
    }

    #[test]
    fn a_short_or_unreadable_ipc_count_is_degraded_with_the_script_s_detail() {
        let cases = [
            // Step 1b's Gate 1 FAIL: no round trip completed.
            (ipc_line(0), Some(0), Some(0)),
            (ipc_line(9_999), Some(6), Some(9_999)),
            (String::new(), None, None),
            // Cut before `(N iters)`, by the end of the log or other output.
            (
                "[bench] IPC round-trip (same core): avg=6 us, p99=8 us, min=49[heartbeat] tick=500\n"
                    .to_string(),
                Some(6),
                None,
            ),
            (
                "[bench] IPC round-trip (same core): avg=6 us, p99=8 us, min=4992 ns, max=7 ns (99999999999999999999 iters)\n"
                    .to_string(),
                Some(6),
                None,
            ),
        ];
        for (ipc, avg_us, iters) in cases {
            let log = healthy(&ipc);
            let c = classify(log.as_bytes(), None);
            assert_eq!(c.class, Class::Degraded, "{log}");
            assert_eq!(c.ipc, Ipc { avg_us, iters }, "{log}");
            // The reason lives in `ipc` alone: detail and lb are CLEAN's.
            assert_eq!((c.detail.as_slice(), c.lb), (&b"-"[..], "-"), "{log}");
            assert!(String::from_utf8(c.base_line())
                .expect("ASCII")
                .starts_with("CLEAN\t"));
        }
    }

    #[test]
    fn the_first_ipc_line_decides_and_no_ipc_line_never_masks_a_failure() {
        // A guest that booted twice: the first boot's line is the one read.
        let log = healthy(&format!("{}{}", ipc_line(0), ipc_line(10_000)));
        let c = classify(log.as_bytes(), None);
        assert_eq!((c.class, c.ipc.iters), (Class::Degraded, Some(0)));
        // A fatal report wins over an IPC count of 0 (pr209-fix-198's shape).
        let log = format!(
            "{KERNEL}[heartbeat] tick=0\n{}PANIC: panicked at kernel/src/mm/frame.rs:51:9:\nout of frames\n",
            ipc_line(0)
        );
        let c = classify(log.as_bytes(), None);
        assert_eq!((c.class, c.ipc.iters), (Class::Panic, Some(0)));
        // So does a wedge: the bench never completed.
        let log = format!(
            "{KERNEL}[heartbeat] tick=0\n=== Gate 1 Benchmark ===\n{}[heartbeat] tick=1000\n",
            ipc_line(0)
        );
        assert_eq!(classify(log.as_bytes(), None).class, Class::WedgeAlive);
    }

    #[test]
    fn info_lines_freeze_at_the_first_fatal_report() {
        let log = "AIOS UEFI stub v0.1.0\nAIOS kernel booting\n[heartbeat] tick=0\n\
                   [   1.000000] [0] INFO  Mm    one\n[   2.000000] [0] INFO  Mm    two\n[   3.000000] [0] INFO  Mm    three\n\
                   [   4.000000] [0] INFO  Sched Load balance: migrated tid=4 from CPU 0 to CPU 2\n\
                   PANIC: panicked at kernel/src/sched/mod.rs:1:1:\nboom\n[   5.000000] [0] INFO  Mm    after the panic\n";
        assert_eq!(
            line(log, None),
            "PANIC\t0\t1\t-\t-\tyes\tafter heartbeat tick 0\tPANIC: panicked at kernel/src/sched/mod.rs:1:1: / boom\t\
             [   2.000000] [0] INFO  Mm    two\t[   3.000000] [0] INFO  Mm    three\t\
             [   4.000000] [0] INFO  Sched Load balance: migrated tid=4 from CPU 0 to CPU 2"
        );
    }

    #[test]
    fn preprocess_drops_nul_cr_and_ansi_sequences() {
        assert_eq!(
            preprocess(b"a\0b\r\n\x1b[1;32mc\x1b[0m\x1b[K\n"),
            b"ab\nc\n"
        );
        // An escape that is not a CSI sequence stays.
        assert_eq!(preprocess(b"\x1b]0;t\x07x"), b"\x1b]0;t\x07x");
    }

    #[test]
    fn the_first_fatal_line_is_clipped_at_200_bytes_even_inside_a_character() {
        let mut log = b"AIOS UEFI stub v0.1.0\nAIOS kernel booting\nPANIC: ".to_vec();
        log.extend(vec![b'a'; 192]);
        log.extend("\u{e9} tail".as_bytes());
        log.push(b'\n');
        let c = classify(&log, None);
        assert_eq!(c.first.len(), 203);
        assert_eq!(&c.first[198..], b"a\xc3...");
    }

    #[test]
    fn an_empty_log_is_inconclusive() {
        assert_eq!(
            line("", None),
            "INCONCLUSIVE\t-\t0\t-\t-\t-\tUEFI stub never ran, not a boot result\t-\t-\t-\t-"
        );
    }
}
