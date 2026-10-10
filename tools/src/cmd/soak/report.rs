//! The soak report text: the console line per boot (`format_result`), the
//! `--classify` block, `summary.tsv` rows and `summary.md`, ported from the
//! former `scripts/soak-qemu.sh` (blob at `212df62`: L420-452, L454-471,
//! L656, L737-749, L761-793). Everything here is pure; `runner` does the I/O.
//!
//! awk's `printf "%.0f"` and `"%.2f"` round a tie to even on the exact binary
//! value, as Rust's `{:.0}` and `{:.2}` do (pinned by tests). Accepted
//! divergence: a load average that is not a number takes part in awk's
//! `$1 > m` as a string, and here counts as 0 (loads come from /proc/loadavg or
//! `sysctl`, which print numbers).
//!
//! Crash-fix step 1a: where the script printed a CLEAN boot's detail, a
//! DEGRADED boot shows why it is not CLEAN, from its [`Ipc`] figures, ahead of
//! that detail. `summary.tsv` gains [`STEP_1A_COLUMNS`], then one `tw_<key>`
//! column per tripwire key in [`Key::ALL`] order, then [`LINE_COLUMNS`], after
//! the script's 22 ([`SCRIPT_COLUMNS`]), so positional readers of the first 22
//! keep working. `summary.md` gains a Gate 1 IPC line, the "Tripwire counters
//! by class" table and a `Tripwire` column in the per-boot table. A soak and
//! `--classify --out` both write through [`tsv_row`], [`summary_head`] and
//! [`summary_tail`], fed by a [`Tally`].

use shared::lock::LockClass;
use shared::sched::SchedulerClass;
use shared::tripwire::{BadchanSite, Key, N2Kind, WakeSource, Width, CLASS_COUNT};

use super::awk::to_num;
use super::classify::{Base, Class, Classification, Ipc, Reentry};
use super::stats::wilson;
use super::tripwire::{Line, V1};

/// The script's `summary.tsv` columns.
pub const SCRIPT_COLUMNS: [&str; 22] = [
    "run",
    "mode",
    "class",
    "last_tick",
    "hb_count",
    "stall_s",
    "elapsed_s",
    "qemu_rc",
    "load1",
    "kernel_s",
    "hb_first_s",
    "bench_s",
    "g1done_s",
    "hb_max_gap_s",
    "markers",
    "lb_last",
    "detail",
    "first_fatal",
    "last_info_1",
    "last_info_2",
    "last_info_3",
    "log",
];

/// Crash-fix step 1a's columns ahead of the per-key ones: the Gate 1 IPC
/// figures, a PANIC-LOCK's `lock re-entry:` fields, the `[tripwire-ev]`
/// counts, and the last complete tripwire line's version and prefix.
pub const STEP_1A_COLUMNS: [&str; 14] = [
    "ipc_avg_us",
    "ipc_iters",
    "reentry_lock",
    "reentry_ctx",
    "reentry_holder_irqs",
    "ev_ph",
    "ev_self",
    "ev_self_irq",
    "ev_stuck",
    "tw_v",
    "tw_src",
    "tw_cpu",
    "tw_t",
    "tw_ncpu",
];

/// The columns after the per-key ones: the last complete `src=g1` line's
/// `elrmm`, then the whole lines, last because they are long.
pub const LINE_COLUMNS: [&str; 3] = ["g1_elrmm", "g1_line", "tw_line"];

/// The number of `summary.tsv` columns.
pub const TSV_COLUMNS: usize =
    SCRIPT_COLUMNS.len() + STEP_1A_COLUMNS.len() + Key::COUNT + LINE_COLUMNS.len();

/// `summary.tsv`'s header line, with its newline.
pub fn tsv_header() -> Vec<u8> {
    let mut names: Vec<String> = SCRIPT_COLUMNS
        .iter()
        .chain(&STEP_1A_COLUMNS)
        .map(|s| s.to_string())
        .collect();
    names.extend(Key::ALL.iter().map(|k| format!("tw_{}", k.name())));
    names.extend(LINE_COLUMNS.iter().map(|s| s.to_string()));
    let mut header = names.join("\t").into_bytes();
    header.push(b'\n');
    header
}

/// `s` padded with spaces to `width` bytes (`printf %-Ns` under `LC_ALL=C`).
fn pad(s: &[u8], width: usize) -> Vec<u8> {
    let mut out = s.to_vec();
    out.resize(width.max(s.len()), b' ');
    out
}

/// `$C_STALL` with an `s` suffix, or `-`.
fn stall_text(c: &Classification) -> Vec<u8> {
    if c.stall == b"-" {
        b"-".to_vec()
    } else {
        [&c.stall[..], b"s"].concat()
    }
}

/// Why a DEGRADED boot is not CLEAN: its IPC iteration count, or that the
/// count could not be read.
pub fn ipc_text(ipc: &Ipc) -> String {
    match ipc.iters {
        Some(n) => format!("IPC {n} iters"),
        None => "IPC iteration count unreadable".to_string(),
    }
}

/// The text a boot of base class CLEAN shows where the script showed its
/// detail: the detail itself for CLEAN; for DEGRADED, [`ipc_text`], then the
/// detail after `; ` unless it is `-`.
fn clean_text(c: &Classification) -> Vec<u8> {
    if c.class != Class::Degraded {
        return c.detail.clone();
    }
    let mut text = ipc_text(&c.ipc).into_bytes();
    if c.detail != b"-" {
        text.extend_from_slice(b"; ");
        text.extend_from_slice(&c.detail);
    }
    text
}

/// A number cell of `summary.tsv`, or `-` when unreadable.
fn opt_cell(v: Option<u64>) -> Vec<u8> {
    v.map_or_else(|| b"-".to_vec(), |n| n.to_string().into_bytes())
}

/// `format_result LABEL`: the one-line summary of a classification, with its newline.
pub fn format_result(label: &[u8], c: &Classification) -> Vec<u8> {
    let text: Vec<u8> = match c.class.base() {
        Base::Clean => clean_text(c),
        Base::Inconclusive => c.detail.clone(),
        Base::Wedge => [b"lb_last=", c.lb.as_bytes(), b"  ", &c.detail[..]].concat(),
        Base::PcZero | Base::Panic | Base::Exception => {
            [b"lb_last=", c.lb.as_bytes(), b"  ", &c.first[..]].concat()
        }
    };
    let text = if text.is_empty() || text == b"-" {
        Vec::new()
    } else {
        [&b"  "[..], &text[..]].concat()
    };
    [
        label,
        b"  ",
        &pad(c.class.name().as_bytes(), 12),
        b" tick=",
        &pad(&c.tick, 6),
        b" stall=",
        &pad(&stall_text(c), 5),
        b" [",
        &c.markers,
        b"]",
        &text,
        b"\n",
    ]
    .concat()
}

/// The four lines `--classify` prints under a boot that is not CLEAN.
pub fn classify_details(c: &Classification) -> Vec<u8> {
    [
        b"    first fatal: ",
        &c.first[..],
        b"\n    last INFO:   ",
        &c.info[0],
        b"\n                 ",
        &c.info[1],
        b"\n                 ",
        &c.info[2],
        b"\n",
    ]
    .concat()
}

/// `md_cell`: escape `|` for a Markdown table cell.
pub fn md_cell(s: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for &b in s {
        if b == b'|' {
            out.push(b'\\');
        }
        out.push(b);
    }
    out
}

/// A class's share of all boots: awk `printf "%.0f%%", 100 * a / b`.
pub fn share(count: u64, runs: u64) -> String {
    format!("{:.0}%", 100.0 * count as f64 / runs as f64)
}

/// The per-boot 1-minute load summary: awk over `summary.tsv`'s load1 column,
/// `mean %.2f, max %.2f`, or nothing without rows.
pub fn load_summary(load1: &[&[u8]]) -> String {
    if load1.is_empty() {
        return String::new();
    }
    let mut sum = 0.0;
    let mut max = 0.0;
    for v in load1 {
        let x = to_num(v);
        sum += x;
        if x > max {
            max = x;
        }
    }
    format!("mean {:.2}, max {:.2}", sum / load1.len() as f64, max)
}

/// Per-boot numbers that come from the harness, not the classifier: a soak
/// knows them all; `--classify --out` reads them from a log's `[soak] meta`
/// footer, and a value the footer lacks is `None` (`-`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BootTiming {
    pub elapsed: Option<i64>,
    pub rc: Option<i32>,
    pub load1: Option<Vec<u8>>,
    pub kstart: Option<i64>,
    pub hb_first: Option<i64>,
    pub bench_start: Option<i64>,
    pub g1done: Option<i64>,
    pub hb_max_gap: Option<i64>,
}

/// A text cell of `summary.tsv`, or `-` when absent.
fn text_cell(v: Option<&[u8]>) -> Vec<u8> {
    v.map_or_else(|| b"-".to_vec(), <[u8]>::to_vec)
}

/// A whole tripwire line as a `summary.tsv` cell: a tab between its tokens
/// (the parser splits at blanks and tabs alike) becomes a space, so the row
/// keeps its columns.
fn line_cell(line: Option<&Line>) -> Vec<u8> {
    line.map_or_else(
        || b"-".to_vec(),
        |l| {
            l.text
                .iter()
                .map(|&b| if b == b'\t' { b' ' } else { b })
                .collect()
        },
    )
}

/// The `tw_v` to `tw_ncpu` and `tw_<key>` cells of the last complete
/// tripwire line: decoded for `v=1`; for another schema version, its `v=`
/// and `-` for the rest; `-` throughout without a line.
fn tripwire_cells(line: Option<&Line>) -> Vec<Vec<u8>> {
    let width = 5 + Key::COUNT;
    let Some(line) = line else {
        return vec![b"-".to_vec(); width];
    };
    let mut cells = vec![text_cell(line.version.as_deref())];
    match &line.v1 {
        Some(v1) => {
            cells.push(text_cell(line.src.as_deref()));
            cells.push(text_cell(v1.cpu.as_deref()));
            cells.push(text_cell(v1.t.as_deref()));
            cells.push(text_cell(v1.ncpu.as_deref()));
            cells.extend(Key::ALL.iter().map(|&k| v1.value(k).to_vec()));
        }
        None => cells.resize(width, b"-".to_vec()),
    }
    cells
}

/// One `summary.tsv` row, with its newline: [`TSV_COLUMNS`] cells in
/// [`tsv_header`] order.
pub fn tsv_row(
    idx: &str,
    mode: &str,
    c: &Classification,
    t: &BootTiming,
    log_name: &str,
) -> Vec<u8> {
    let num = |v: Option<i64>| v.map_or_else(|| b"-".to_vec(), |n| n.to_string().into_bytes());
    let count = |n: u64| n.to_string().into_bytes();
    let reentry = |f: fn(&Reentry) -> Option<&[u8]>| text_cell(c.reentry.as_ref().and_then(f));
    let mut cells: Vec<Vec<u8>> = vec![
        idx.as_bytes().to_vec(),
        mode.as_bytes().to_vec(),
        c.class.name().as_bytes().to_vec(),
        c.tick.clone(),
        c.hb.clone(),
        c.stall.clone(),
        num(t.elapsed),
        num(t.rc.map(i64::from)),
        text_cell(t.load1.as_deref()),
        num(t.kstart),
        num(t.hb_first),
        num(t.bench_start),
        num(t.g1done),
        num(t.hb_max_gap),
        c.markers.clone(),
        c.lb.as_bytes().to_vec(),
        c.detail.clone(),
        c.first.clone(),
        c.info[0].clone(),
        c.info[1].clone(),
        c.info[2].clone(),
        log_name.as_bytes().to_vec(),
        opt_cell(c.ipc.avg_us),
        opt_cell(c.ipc.iters),
        reentry(|r| r.lock.as_deref()),
        reentry(|r| r.ctx.as_deref()),
        reentry(|r| r.holder_irqs.as_deref()),
        count(c.events.ph),
        count(c.events.self_held),
        count(c.events.self_irq),
        count(c.events.stuck),
    ];
    cells.extend(tripwire_cells(c.tripwire.last.as_ref()));
    cells.push(text_cell(
        c.tripwire
            .g1
            .as_ref()
            .and_then(|l| l.v1.as_ref())
            .map(|v1| v1.value(Key::Elrmm)),
    ));
    cells.push(line_cell(c.tripwire.g1.as_ref()));
    cells.push(line_cell(c.tripwire.last.as_ref()));
    let mut row = cells.join(&b'\t');
    row.push(b'\n');
    row
}

/// The per-boot table's `Tripwire` cell: the last complete line's `src`
/// (`-` when it has none), `v=<version>` for a line of another schema
/// version, or `-` without a line.
fn tripwire_src(line: Option<&Line>) -> Vec<u8> {
    match line {
        None => b"-".to_vec(),
        Some(l) if l.v1.is_some() => text_cell(l.src.as_deref()),
        Some(l) => [&b"v="[..], &text_cell(l.version.as_deref())].concat(),
    }
}

/// One row of `summary.md`'s per-boot table, with its newline.
pub fn md_row(idx: &str, c: &Classification) -> Vec<u8> {
    let tail = match c.class.base() {
        Base::Clean => clean_text(c),
        Base::Wedge | Base::Inconclusive => c.detail.clone(),
        Base::PcZero | Base::Panic | Base::Exception => c.first.clone(),
    };
    [
        b"| ",
        idx.as_bytes(),
        b" | ",
        c.class.name().as_bytes(),
        b" | ",
        &c.tick,
        b" | ",
        &stall_text(c),
        b" | ",
        &c.markers,
        b" | ",
        c.lb.as_bytes(),
        b" | ",
        &md_cell(&tripwire_src(c.tripwire.last.as_ref())),
        b" | ",
        &md_cell(&tail),
        b" |\n",
    ]
    .concat()
}

/// What `summary.md` needs from the boots, gathered one boot at a time.
#[derive(Debug, Default)]
pub struct Tally {
    /// Boots per class, in [`Class::ALL`] order.
    counts: [u64; Class::COUNT],
    /// The per-boot 1-minute loads that are known.
    loads: Vec<Vec<u8>>,
    /// Each CLEAN boot's `ipc_avg_us`.
    clean_ipc: Vec<Option<u64>>,
    /// Each boot's class and its last complete line, if that line is `v=1`.
    tripwire: Vec<(Class, Option<V1>)>,
    /// The per-boot table's rows.
    rows: Vec<u8>,
}

impl Tally {
    /// Count boot `idx`.
    pub fn add(&mut self, idx: &str, c: &Classification, t: &BootTiming) {
        self.counts[c.class.index()] += 1;
        if let Some(load1) = &t.load1 {
            self.loads.push(load1.clone());
        }
        if c.class == Class::Clean {
            self.clean_ipc.push(c.ipc.avg_us);
        }
        let v1 = c.tripwire.last.as_ref().and_then(|l| l.v1.clone());
        self.tripwire.push((c.class, v1));
        self.rows.extend(md_row(idx, c));
    }

    /// The boots counted.
    pub fn boots(&self) -> u64 {
        self.counts.iter().sum()
    }

    /// The boots of `class`.
    pub fn count(&self, class: Class) -> u64 {
        self.counts[class.index()]
    }
}

/// The settings `summary.md` records about a soak. `--classify --out` gives
/// `-` for what a set of logs cannot tell.
pub struct SummaryInfo<'a> {
    pub mode: &'a str,
    pub runs: &'a str,
    pub secs: &'a str,
    pub stall_secs: &'a str,
    /// `None`: not known (`--classify --out`).
    pub fresh_data: Option<bool>,
    pub git_rev: &'a str,
    pub kernel_sha: &'a str,
    pub qemu_version: &'a [u8],
    pub firmware: &'a [u8],
    pub host: &'a [u8],
    pub load_start: &'a str,
    pub load_end: &'a str,
    pub out: &'a [u8],
}

/// The Gate 1 IPC line: the mean of `ipc_avg_us` over the CLEAN boots.
pub fn gate1_ipc(clean_ipc: &[Option<u64>]) -> String {
    let known: Vec<u64> = clean_ipc.iter().flatten().copied().collect();
    if known.is_empty() {
        let why = if clean_ipc.is_empty() {
            "no CLEAN boot"
        } else {
            "no CLEAN boot with a readable avg="
        };
        return format!("Gate 1 IPC round trip, mean over CLEAN boots: n/a ({why})\n");
    }
    let mean = known.iter().map(|&v| v as f64).sum::<f64>() / known.len() as f64;
    let left_out = clean_ipc.len() - known.len();
    let note = if left_out == 0 {
        String::new()
    } else {
        format!(", {left_out} without a readable avg= left out")
    };
    format!(
        "Gate 1 IPC round trip, mean over CLEAN boots: {mean:.2} us (n={}{note}; each boot's avg= is whole us, truncated)\n",
        known.len()
    )
}

/// `summary.md` up to the Gate 1 IPC line (the part also printed to
/// stdout), with its final newline.
pub fn summary_head(info: &SummaryInfo, tally: &Tally) -> Vec<u8> {
    let runs_n = tally.boots();
    let inconclusive = tally.count(Class::Inconclusive);
    let clean = tally.count(Class::Clean);
    let conclusive = runs_n - inconclusive;
    let rate_note = if inconclusive == 0 {
        String::new()
    } else {
        format!(" over {conclusive} conclusive boots ({inconclusive} INCONCLUSIVE left out)")
    };
    let data = match info.fresh_data {
        Some(true) => "fresh per boot",
        Some(false) => "reused",
        None => "-",
    };
    let load_refs: Vec<&[u8]> = tally.loads.iter().map(Vec::as_slice).collect();
    let mut md = Vec::new();
    md.extend(format!("## AIOS QEMU boot soak ({} mode)\n\n", info.mode).into_bytes());
    md.extend_from_slice(b"| Setting | Value |\n|---|---|\n");
    md.extend(format!("| Commit | `{}` ({}) |\n", info.git_rev, info.kernel_sha).into_bytes());
    md.extend(
        format!(
            "| Boots | {} x {}s, stall limit {}s, data disk {data} |\n",
            info.runs, info.secs, info.stall_secs
        )
        .into_bytes(),
    );
    md.extend([&b"| QEMU | "[..], info.qemu_version, b" |\n"].concat());
    md.extend([&b"| Firmware | `"[..], info.firmware, b"` |\n"].concat());
    md.extend([&b"| Host | "[..], info.host, b" |\n"].concat());
    md.extend(
        format!(
            "| Load average | start {}; end {}; per-boot 1-min {} |\n",
            info.load_start,
            info.load_end,
            load_summary(&load_refs)
        )
        .into_bytes(),
    );
    md.extend([&b"| Logs | `"[..], info.out, b"` |\n\n"].concat());
    md.extend_from_slice(b"| Class | Count | Share |\n|---|---:|---:|\n");
    for (class, count) in Class::ALL.iter().zip(&tally.counts) {
        md.extend(
            format!(
                "| {} | {count} | {} |\n",
                class.name(),
                share(*count, runs_n)
            )
            .into_bytes(),
        );
    }
    md.extend(format!("| **Total** | {} | |\n\n", info.runs).into_bytes());
    md.extend(format!("CLEAN rate: {}{rate_note}\n", wilson(clean, conclusive)).into_bytes());
    md.extend(gate1_ipc(&tally.clean_ipc).into_bytes());
    md
}

/// The scheduler classes in `starved`'s index order (`SchedulerClass as
/// usize`), named as `shared` declares them.
const SCHEDULER_CLASSES: [SchedulerClass; CLASS_COUNT] = [
    SchedulerClass::Idle,
    SchedulerClass::Normal,
    SchedulerClass::Interactive,
    SchedulerClass::RealTime,
];

/// The name of index `i` of a key of `width`: `CPU i`, or the index set's
/// name from `shared`; the bare number past the end of a set.
fn index_name(width: Width, i: usize) -> String {
    let name = match width {
        Width::Cpu => return format!("CPU {i}"),
        Width::One => None,
        Width::Source => WakeSource::ALL.get(i).map(|s| s.name().to_string()),
        Width::Lock => LockClass::ALL.get(i).map(|l| l.name().to_string()),
        Width::Class => SCHEDULER_CLASSES.get(i).map(|c| format!("{c:?}")),
        Width::N2 => N2Kind::ALL.get(i).map(|k| k.name().to_string()),
        Width::Badchan => BadchanSite::ALL.get(i).map(|b| b.name().to_string()),
    };
    name.unwrap_or_else(|| i.to_string())
}

/// A comma list as numbers; a value that is not a decimal `u64` counts as 0.
fn values(list: &[u8]) -> Vec<u128> {
    list.split(|&b| b == b',')
        .map(|v| {
            std::str::from_utf8(v)
                .ok()
                .and_then(|s| s.parse::<u64>().ok())
                .map_or(0, u128::from)
        })
        .collect()
}

/// One row of the tripwire table: its label, and each boot's value (`None`
/// for a boot without a `v=1` line).
struct CounterRow {
    label: String,
    gauge: bool,
    per_boot: Vec<Option<u128>>,
}

/// The rows of the tripwire table for every key, zero rows included.
fn counter_rows(boots: &[(Class, Option<V1>)]) -> Vec<CounterRow> {
    let mut rows = Vec::new();
    for &key in &Key::ALL {
        let gauge = key.is_gauge();
        let lists: Vec<Option<Vec<u128>>> = boots
            .iter()
            .map(|(_, v1)| v1.as_ref().map(|v| values(v.value(key))))
            .collect();
        let combine = |vs: &[u128]| -> u128 {
            if gauge {
                vs.iter().copied().max().unwrap_or(0)
            } else {
                vs.iter().sum()
            }
        };
        let width = key.width();
        if width == Width::Cpu {
            rows.push(CounterRow {
                label: key.name().to_string(),
                gauge,
                per_boot: lists.iter().map(|l| l.as_deref().map(combine)).collect(),
            });
        }
        let n = lists.iter().flatten().map(Vec::len).max().unwrap_or(1);
        if width == Width::One && n == 1 {
            rows.push(CounterRow {
                label: key.name().to_string(),
                gauge,
                per_boot: lists.iter().map(|l| l.as_ref().map(|v| v[0])).collect(),
            });
            continue;
        }
        for i in 0..n {
            rows.push(CounterRow {
                label: format!("{}[{}]", key.name(), index_name(width, i)),
                gauge,
                per_boot: lists
                    .iter()
                    .map(|l| l.as_ref().map(|v| v.get(i).copied().unwrap_or(0)))
                    .collect(),
            });
        }
    }
    rows
}

/// The "Tripwire counters by class" section of `summary.md`, with a leading
/// blank line: one column per class that has boots, one row per counter
/// value that is non-zero in some boot. A cell is "boots with a non-zero
/// value / sum over the class's boots", or "/ max" for a gauge.
pub fn tripwire_table(tally: &Tally) -> Vec<u8> {
    let mut md = b"\n### Tripwire counters by class\n\n".to_vec();
    let boots = &tally.tripwire;
    if boots.iter().all(|(_, v1)| v1.is_none()) {
        md.extend_from_slice(b"No boot has a complete `v=1` tripwire line.\n");
        return md;
    }
    md.extend_from_slice(
        b"From each boot's last complete `[tripwire]` line (the `tw_*` columns of `summary.tsv`). \
A cell is \"boots with a non-zero value / sum over those boots\", or \"/ max\" for a gauge \
(marked max). A per-CPU key's row without an index sums its CPUs. Values that are 0 in every \
boot are left out.\n\n",
    );
    let classes: Vec<Class> = Class::ALL
        .into_iter()
        .filter(|&c| tally.count(c) > 0)
        .collect();
    let line = |label: &str, cells: Vec<String>| format!("| {label} | {} |\n", cells.join(" | "));
    md.extend(
        line(
            "Counter",
            classes.iter().map(|c| c.name().to_string()).collect(),
        )
        .into_bytes(),
    );
    md.extend(format!("|---|{}\n", "---:|".repeat(classes.len())).into_bytes());
    md.extend(
        line(
            "Boots",
            classes
                .iter()
                .map(|&c| tally.count(c).to_string())
                .collect(),
        )
        .into_bytes(),
    );
    let with_line = |c: Class| {
        boots
            .iter()
            .filter(|(k, v1)| *k == c && v1.is_some())
            .count()
    };
    md.extend(
        line(
            "Boots with a `v=1` line",
            classes.iter().map(|&c| with_line(c).to_string()).collect(),
        )
        .into_bytes(),
    );
    for row in counter_rows(boots) {
        if !row.per_boot.iter().any(|v| v.is_some_and(|v| v > 0)) {
            continue;
        }
        let cells = classes
            .iter()
            .map(|&class| {
                let vs: Vec<u128> = boots
                    .iter()
                    .zip(&row.per_boot)
                    .filter(|((c, _), _)| *c == class)
                    .filter_map(|(_, v)| *v)
                    .collect();
                let non_zero = vs.iter().filter(|&&v| v > 0).count();
                let agg = if row.gauge {
                    vs.iter().copied().max().unwrap_or(0)
                } else {
                    vs.iter().sum()
                };
                format!("{non_zero}/{agg}")
            })
            .collect();
        let label = if row.gauge {
            format!("`{}` (max)", row.label)
        } else {
            format!("`{}`", row.label)
        };
        md.extend(line(&label, cells).into_bytes());
    }
    md
}

/// `summary.md` after its head: the tripwire table, then the per-boot table.
pub fn summary_tail(tally: &Tally) -> Vec<u8> {
    [tripwire_table(tally), summary_table(&tally.rows)].concat()
}

/// The per-boot table.
fn summary_table(rows: &[u8]) -> Vec<u8> {
    [
        &b"\n| Run | Class | Last tick | Stall | Markers | LB last | Tripwire | First fatal line / detail |\n|---:|---|---:|---:|---|---|---|---|\n"[..],
        rows,
    ]
    .concat()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A classification built from the awk program's eleven fields.
    fn c(fields: [&str; 11]) -> Classification {
        let class = Class::from_name(fields[0]).expect("a class");
        let lb = ["yes", "no", "-"]
            .into_iter()
            .find(|v| *v == fields[5])
            .expect("an lb value");
        let b = |s: &str| s.as_bytes().to_vec();
        Classification {
            class,
            tick: b(fields[1]),
            hb: b(fields[2]),
            stall: b(fields[3]),
            markers: b(fields[4]),
            lb,
            detail: b(fields[6]),
            first: b(fields[7]),
            info: [b(fields[8]), b(fields[9]), b(fields[10])],
            reentry: None,
            ipc: Ipc {
                avg_us: Some(6),
                iters: Some(10_000),
            },
            tripwire: Default::default(),
            events: Default::default(),
        }
    }

    /// A DEGRADED boot with `detail` and the IPC iteration count `iters`.
    fn degraded(detail: &str, iters: Option<u64>) -> Classification {
        let mut d = c([
            "DEGRADED",
            "2000",
            "3",
            "1",
            "EL1,BOOT,G1DONE",
            "-",
            detail,
            "-",
            "-",
            "-",
            "-",
        ]);
        d.ipc = Ipc {
            avg_us: iters.map(|_| 0),
            iters,
        };
        d
    }

    fn text(v: Vec<u8>) -> String {
        String::from_utf8(v).expect("ASCII")
    }

    const WEDGE: [&str; 11] = [
        "WEDGE-STUCK",
        "0",
        "1",
        "69",
        "EL1,BOOT",
        "no",
        "heartbeat stuck at tick 0 after the Gate 1 bench started",
        "-",
        "-",
        "[   0.200000] [0] INFO  Boot  Boot sequence complete",
        "[   6.613544] [0] INFO  Ipc   Bench main: server ready, starting IPC benchmark",
    ];
    const PANIC: [&str; 11] = [
        "PANIC", "1000", "2", "-", "-", "no", "after heartbeat tick 1000",
        "PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready",
        "-", "-", "[   2.000000] [0] INFO  Mm    frame allocator ready",
    ];

    // Expected lines below were printed by the script's format_result and md_cell.
    #[test]
    fn format_result_pads_and_picks_the_trailing_text_by_class() {
        let clean = c([
            "CLEAN",
            "2000",
            "3",
            "1",
            "EL1,BOOT,G1PASS,G1DONE",
            "-",
            "-",
            "-",
            "-",
            "-",
            "-",
        ]);
        assert_eq!(
            text(format_result(b"clean-text", &clean)),
            "clean-text  CLEAN        tick=2000   stall=1s    [EL1,BOOT,G1PASS,G1DONE]\n"
        );
        assert_eq!(
            text(format_result(b"tick0-after-bench", &c(WEDGE))),
            "tick0-after-bench  WEDGE-STUCK  tick=0      stall=69s   [EL1,BOOT]  lb_last=no  heartbeat stuck at tick 0 after the Gate 1 bench started\n"
        );
        assert_eq!(
            text(format_result(b"panic-with-message", &c(PANIC))),
            "panic-with-message  PANIC        tick=1000   stall=-     [-]  lb_last=no  PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready\n"
        );
    }

    #[test]
    fn a_degraded_boot_shows_its_ipc_count_ahead_of_its_detail() {
        assert_eq!(
            text(format_result(b"r", &degraded("-", Some(0)))),
            "r  DEGRADED     tick=2000   stall=1s    [EL1,BOOT,G1DONE]  IPC 0 iters\n"
        );
        assert_eq!(
            text(format_result(
                b"r",
                &degraded("qemu exited before the time limit (rc=0)", None)
            )),
            "r  DEGRADED     tick=2000   stall=1s    [EL1,BOOT,G1DONE]  IPC iteration count unreadable; qemu exited before the time limit (rc=0)\n"
        );
        assert_eq!(
            text(md_row("03", &degraded("-", Some(9_999)))),
            "| 03 | DEGRADED | 2000 | 1s | EL1,BOOT,G1DONE | - | - | IPC 9999 iters |\n"
        );
    }

    #[test]
    fn format_result_never_truncates_a_long_field() {
        let long = c([
            "CLEAN", "1234567", "1", "123456", "-", "-", "-", "-", "-", "-", "-",
        ]);
        assert_eq!(
            text(format_result(b"x", &long)),
            "x  CLEAN        tick=1234567 stall=123456s [-]\n"
        );
    }

    #[test]
    fn classify_details_lists_the_first_fatal_and_last_info_lines() {
        assert_eq!(
            text(classify_details(&c(PANIC))),
            "    first fatal: PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready\n\
             \x20   last INFO:   -\n\
             \x20                -\n\
             \x20                [   2.000000] [0] INFO  Mm    frame allocator ready\n"
        );
    }

    #[test]
    fn md_row_escapes_pipes_and_picks_detail_or_first_line() {
        assert_eq!(
            text(md_row("07", &c(WEDGE))),
            "| 07 | WEDGE-STUCK | 0 | 69s | EL1,BOOT | no | - | heartbeat stuck at tick 0 after the Gate 1 bench started |\n"
        );
        assert_eq!(
            text(md_row("07", &c(PANIC))),
            "| 07 | PANIC | 1000 | - | - | no | - | PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready |\n"
        );
        assert_eq!(md_cell(b"a|b||c"), b"a\\|b\\|\\|c");
    }

    // Values printed by the script's awk share one-liner (`wilson` is in
    // `stats`).
    #[test]
    fn share_matches_awk() {
        // %.0f rounds an exact tie to even, as C does: 12.5 -> 12, 37.5 -> 38.
        assert_eq!(share(1, 8), "12%");
        assert_eq!(share(3, 8), "38%");
        assert_eq!(share(0, 3), "0%");
    }

    #[test]
    fn load_summary_matches_the_awk_over_the_load1_column() {
        assert_eq!(load_summary(&[b"1.50", b"2.25"]), "mean 1.88, max 2.25");
        assert_eq!(load_summary(&[b""]), "mean 0.00, max 0.00");
        assert_eq!(load_summary(&[b"0.125", b"0.375"]), "mean 0.25, max 0.38");
        assert_eq!(
            load_summary(&[b"3.00", b"", b"1.005"]),
            "mean 1.33, max 3.00"
        );
        assert_eq!(load_summary(&[]), "");
    }

    /// The cell of column `name` in a `summary.tsv` row.
    fn cell<'a>(row: &'a str, name: &str) -> &'a str {
        let header = String::from_utf8(tsv_header()).expect("ASCII");
        let i = header
            .trim_end()
            .split('\t')
            .position(|h| h == name)
            .unwrap_or_else(|| panic!("no column {name}"));
        row.trim_end_matches('\n')
            .split('\t')
            .nth(i)
            .expect("a cell")
    }

    /// A complete `v=1` tripwire line with the prefix and the `key=value`
    /// tokens `keys`.
    fn tw(src: &str, keys: &str) -> String {
        let n = 5 + keys.split_whitespace().count();
        format!("[tripwire] v=1 src={src} cpu=0 t=5000 ncpu=4 {keys} n={n}")
    }

    /// `base` with the log lines `lines` observed, as the classifier's scan does.
    fn with_lines(mut base: Classification, lines: &[String]) -> Classification {
        for l in lines {
            base.tripwire.observe(l.as_bytes());
            base.events.observe(l.as_bytes());
        }
        base
    }

    fn timing() -> BootTiming {
        BootTiming {
            elapsed: Some(76),
            rc: Some(124),
            load1: Some(b"1.50".to_vec()),
            kstart: Some(1),
            hb_first: Some(2),
            bench_start: Some(7),
            g1done: Some(8),
            hb_max_gap: Some(3),
        }
    }

    #[test]
    fn tsv_row_has_every_column_in_header_order() {
        // 22 script columns, 14 step-1a ones, one per key and 3 for the lines.
        assert_eq!(TSV_COLUMNS, 22 + 14 + Key::COUNT + 3);
        let header = text(tsv_header());
        assert_eq!(header.split('\t').count(), TSV_COLUMNS);
        assert!(header.starts_with(
            "run\tmode\tclass\tlast_tick\thb_count\tstall_s\telapsed_s\tqemu_rc\tload1\tkernel_s\t\
             hb_first_s\tbench_s\tg1done_s\thb_max_gap_s\tmarkers\tlb_last\tdetail\tfirst_fatal\t\
             last_info_1\tlast_info_2\tlast_info_3\tlog\tipc_avg_us\tipc_iters\treentry_lock\t\
             reentry_ctx\treentry_holder_irqs\tev_ph\tev_self\tev_self_irq\tev_stuck\ttw_v\ttw_src\t\
             tw_cpu\ttw_t\ttw_ncpu\ttw_tick\ttw_irqsw\t"
        ));
        assert!(header.ends_with("\ttw_twmax\tg1_elrmm\tg1_line\ttw_line\n"));

        let row = text(tsv_row("07", "text", &c(WEDGE), &timing(), "run-07.log"));
        let without_tripwire = "-\t".repeat(5 + Key::COUNT + 3);
        assert_eq!(
            row,
            format!(
                "07\ttext\tWEDGE-STUCK\t0\t1\t69\t76\t124\t1.50\t1\t2\t7\t8\t3\tEL1,BOOT\tno\t\
                 heartbeat stuck at tick 0 after the Gate 1 bench started\t-\t-\t\
                 [   0.200000] [0] INFO  Boot  Boot sequence complete\t\
                 [   6.613544] [0] INFO  Ipc   Bench main: server ready, starting IPC benchmark\t\
                 run-07.log\t6\t10000\t-\t-\t-\t0\t0\t0\t0\t{}\n",
                without_tripwire.trim_end_matches('\t')
            )
        );
        assert_eq!(row.split('\t').count(), TSV_COLUMNS);
        // An unreadable figure, and a value no footer gave, is `-`.
        let row = text(tsv_row(
            "01",
            "-",
            &degraded("-", None),
            &BootTiming::default(),
            "a.log",
        ));
        for name in [
            "elapsed_s",
            "qemu_rc",
            "load1",
            "kernel_s",
            "g1done_s",
            "hb_max_gap_s",
            "ipc_avg_us",
            "ipc_iters",
        ] {
            assert_eq!(cell(&row, name), "-", "{name}");
        }
        assert_eq!(row.split('\t').count(), TSV_COLUMNS);
    }

    #[test]
    fn tsv_row_carries_the_tripwire_lines_events_and_lock_re_entry() {
        let mut p = c(PANIC);
        p.class = Class::PanicLock;
        p.reentry = Some(Reentry {
            lock: Some(b"CURRENT_THREAD[0]".to_vec()),
            ctx: Some(b"irq-exit".to_vec()),
            holder_irqs: None,
        });
        let hb = tw("hb", "elrmm=1,0,0,0 twc=9 twn=1 twmax=94000");
        let g1 = tw(
            "g1",
            "elrmm=0,2,0,0 lkph=1,0,0,0,0,0,0,0,0 twc=9 twn=1 twmax=94000",
        );
        let panic = tw(
            "panic",
            "irqsw=6,0,0,0 badchan=4,0,6 twc=9 twn=2 twmax=94000",
        );
        let p = with_lines(
            p,
            &[
                hb,
                g1.clone(),
                "[tripwire-ev] kind=self cpu=0 lock=THREAD_TABLE idx=- ctx=irq-exit".to_string(),
                "[tripwire-ev] kind=stuck cpu=3 lock=THREAD_TABLE idx=- ctx=thread-off".to_string(),
                format!("[heartbeat] tick=6000{panic}"),
            ],
        );
        let row = text(tsv_row("05", "text", &p, &timing(), "run-05.log"));
        assert_eq!(row.split('\t').count(), TSV_COLUMNS);
        for (name, want) in [
            ("class", "PANIC-LOCK"),
            ("reentry_lock", "CURRENT_THREAD[0]"),
            ("reentry_ctx", "irq-exit"),
            ("reentry_holder_irqs", "-"),
            ("ev_ph", "0"),
            ("ev_self", "1"),
            ("ev_self_irq", "1"),
            ("ev_stuck", "1"),
            ("tw_v", "1"),
            ("tw_src", "panic"),
            ("tw_cpu", "0"),
            ("tw_t", "5000"),
            ("tw_ncpu", "4"),
            ("tw_irqsw", "6,0,0,0"),
            ("tw_badchan", "4,0,6"),
            // A key the last line leaves out is 0, whatever earlier lines said.
            ("tw_elrmm", "0"),
            ("tw_lkph", "0"),
            ("tw_twn", "2"),
            ("g1_elrmm", "0,2,0,0"),
            ("g1_line", &g1),
            ("tw_line", &panic),
        ] {
            assert_eq!(cell(&row, name), want, "{name}");
        }
        assert_eq!(text(md_row("05", &p)).split(" | ").nth(6), Some("panic"));
    }

    #[test]
    fn another_schema_version_fills_tw_v_and_tw_line_only() {
        let v2 = "[tripwire] v=2 src=hb\tcpu=0 t=5000 ncpu=4 newkey=7 n=6";
        let w = with_lines(c(WEDGE), &[tw("g1", "twc=1 twn=1 twmax=1"), v2.to_string()]);
        let row = text(tsv_row("07", "text", &w, &timing(), "run-07.log"));
        assert_eq!(
            row.split('\t').count(),
            TSV_COLUMNS,
            "a tab in a line stays inside its cell"
        );
        assert_eq!(cell(&row, "tw_v"), "2");
        for &key in &Key::ALL {
            assert_eq!(cell(&row, &format!("tw_{}", key.name())), "-");
        }
        for name in ["tw_src", "tw_cpu", "tw_t", "tw_ncpu"] {
            assert_eq!(cell(&row, name), "-", "{name}");
        }
        assert_eq!(cell(&row, "tw_line"), v2.replace('\t', " "));
        // The g1 line is v=1, with no `elrmm`: 0.
        assert_eq!(cell(&row, "g1_elrmm"), "0");
        assert_eq!(text(md_row("07", &w)).split(" | ").nth(6), Some("v=2"));
        assert_eq!(text(md_row("07", &c(WEDGE))).split(" | ").nth(6), Some("-"));
    }

    #[test]
    fn index_names_come_from_shared() {
        for (i, class) in SCHEDULER_CLASSES.iter().enumerate() {
            assert_eq!(*class as usize, i, "{class:?}");
        }
        assert_eq!(index_name(Width::Cpu, 3), "CPU 3");
        assert_eq!(index_name(Width::Source, 1), "reply");
        assert_eq!(index_name(Width::Source, 14), "cancel");
        assert_eq!(index_name(Width::Lock, 0), "THREAD_TABLE");
        assert_eq!(index_name(Width::Lock, 3), "WAKEUP_ERRORS");
        assert_eq!(index_name(Width::Class, 1), "Normal");
        assert_eq!(index_name(Width::N2, 1), "rblk");
        assert_eq!(index_name(Width::Badchan, 2), "slot");
        // Past the end of a set (a later kernel), the bare number.
        assert_eq!(index_name(Width::Badchan, 3), "3");
    }

    #[test]
    fn gate1_ipc_is_the_mean_over_clean_boots() {
        assert_eq!(
            gate1_ipc(&[]),
            "Gate 1 IPC round trip, mean over CLEAN boots: n/a (no CLEAN boot)\n"
        );
        assert_eq!(
            gate1_ipc(&[None]),
            "Gate 1 IPC round trip, mean over CLEAN boots: n/a (no CLEAN boot with a readable avg=)\n"
        );
        assert_eq!(
            gate1_ipc(&[Some(6), Some(7), None]),
            "Gate 1 IPC round trip, mean over CLEAN boots: 6.50 us (n=2, 1 without a readable avg= left out; each boot's avg= is whole us, truncated)\n"
        );
    }

    /// A tally of `boots`, numbered from 1.
    fn tally(boots: &[Classification]) -> Tally {
        let mut t = Tally::default();
        for (n, b) in boots.iter().enumerate() {
            t.add(&format!("{:02}", n + 1), b, &timing());
        }
        t
    }

    fn of_class(class: &str) -> Classification {
        let mut b = c(WEDGE);
        b.class = Class::from_name(class).expect("a class");
        b
    }

    #[test]
    fn the_tripwire_table_counts_boots_and_sums_by_class() {
        let boots = [
            with_lines(
                of_class("CLEAN"),
                &[tw(
                    "hb",
                    "irqsw=3000,2,0,0 elrmm=1,0,0,0 twc=9 twn=1 twmax=100",
                )],
            ),
            with_lines(
                of_class("CLEAN"),
                &[tw(
                    "hb",
                    "elrmm=1,0,0,0 starved=0,9,0,0 twc=9 twn=1 twmax=300",
                )],
            ),
            with_lines(
                of_class("WEDGE-ALIVE"),
                &[tw(
                    "hb",
                    "n2=0,1,0,0 ubrbl=0,1,0,0,0,0,0,0,0,0,0,0,0,0,0 nowaker=1 twc=9 twn=1 twmax=50",
                )],
            ),
            of_class("INCONCLUSIVE"),
            with_lines(
                of_class("PANIC-LOCK"),
                &["[tripwire] v=2 src=panic elrmm=7 n=3".to_string()],
            ),
        ];
        assert_eq!(
            text(tripwire_table(&tally(&boots))),
            "\n### Tripwire counters by class\n\n\
             From each boot's last complete `[tripwire]` line (the `tw_*` columns of `summary.tsv`). \
             A cell is \"boots with a non-zero value / sum over those boots\", or \"/ max\" for a gauge \
             (marked max). A per-CPU key's row without an index sums its CPUs. Values that are 0 in every \
             boot are left out.\n\n\
             | Counter | PANIC-LOCK | WEDGE-ALIVE | INCONCLUSIVE | CLEAN |\n\
             |---|---:|---:|---:|---:|\n\
             | Boots | 1 | 1 | 1 | 2 |\n\
             | Boots with a `v=1` line | 0 | 1 | 0 | 2 |\n\
             | `irqsw` | 0/0 | 0/0 | 0/0 | 1/3002 |\n\
             | `irqsw[CPU 0]` | 0/0 | 0/0 | 0/0 | 1/3000 |\n\
             | `irqsw[CPU 1]` | 0/0 | 0/0 | 0/0 | 1/2 |\n\
             | `elrmm` | 0/0 | 0/0 | 0/0 | 2/2 |\n\
             | `elrmm[CPU 0]` | 0/0 | 0/0 | 0/0 | 2/2 |\n\
             | `n2[rblk]` | 0/0 | 1/1 | 0/0 | 0/0 |\n\
             | `ubrbl[reply]` | 0/0 | 1/1 | 0/0 | 0/0 |\n\
             | `nowaker` | 0/0 | 1/1 | 0/0 | 0/0 |\n\
             | `starved[Normal]` | 0/0 | 0/0 | 0/0 | 1/9 |\n\
             | `twc` | 0/0 | 1/9 | 0/0 | 2/18 |\n\
             | `twn` | 0/0 | 1/1 | 0/0 | 2/2 |\n\
             | `twmax` (max) | 0/0 | 1/50 | 0/0 | 2/300 |\n"
        );
        assert_eq!(
            text(tripwire_table(&tally(&[of_class("CLEAN")]))),
            "\n### Tripwire counters by class\n\nNo boot has a complete `v=1` tripwire line.\n"
        );
    }

    #[test]
    fn summary_head_and_tail() {
        let info = SummaryInfo {
            mode: "text",
            runs: "3",
            secs: "75",
            stall_secs: "15",
            fresh_data: Some(true),
            git_rev: "abc1234-dirty",
            kernel_sha: "kernel ELF sha256 `0123456789abcdef`",
            qemu_version: b"QEMU emulator version 10.1.0",
            firmware: b"/fw.fd",
            host: b"Darwin 27.2.0 arm64, 10 CPUs",
            load_start: "1.00 2.00 3.00",
            load_end: "4.00 5.00 6.00",
            out: b"/out",
        };
        let mut t = Tally::default();
        for (idx, class, load) in [
            ("1", "PANIC", "1.00"),
            ("2", "INCONCLUSIVE", "2.00"),
            ("3", "CLEAN", "4.00"),
        ] {
            let timing = BootTiming {
                load1: Some(load.as_bytes().to_vec()),
                ..BootTiming::default()
            };
            t.add(idx, &of_class(class), &timing);
        }
        let head = text(summary_head(&info, &t));
        assert_eq!(
            head,
            "## AIOS QEMU boot soak (text mode)\n\n\
             | Setting | Value |\n|---|---|\n\
             | Commit | `abc1234-dirty` (kernel ELF sha256 `0123456789abcdef`) |\n\
             | Boots | 3 x 75s, stall limit 15s, data disk fresh per boot |\n\
             | QEMU | QEMU emulator version 10.1.0 |\n\
             | Firmware | `/fw.fd` |\n\
             | Host | Darwin 27.2.0 arm64, 10 CPUs |\n\
             | Load average | start 1.00 2.00 3.00; end 4.00 5.00 6.00; per-boot 1-min mean 2.33, max 4.00 |\n\
             | Logs | `/out` |\n\n\
             | Class | Count | Share |\n|---|---:|---:|\n\
             | PCZERO | 0 | 0% |\n| PANIC-LOCK | 0 | 0% |\n| PANIC | 1 | 33% |\n\
             | EXCEPTION | 0 | 0% |\n| WEDGE-STUCK | 0 | 0% |\n| WEDGE-ALIVE | 0 | 0% |\n\
             | INCONCLUSIVE | 1 | 33% |\n| DEGRADED | 0 | 0% |\n| CLEAN | 1 | 33% |\n\
             | **Total** | 3 | |\n\n\
             CLEAN rate: 50% (95% Wilson interval 9%-91%) over 2 conclusive boots (1 INCONCLUSIVE left out)\n\
             Gate 1 IPC round trip, mean over CLEAN boots: 6.00 us (n=1; each boot's avg= is whole us, truncated)\n"
        );
        let tail = text(summary_tail(&t));
        assert!(tail.starts_with(&text(tripwire_table(&t))), "{tail}");
        assert!(
            tail.ends_with(
                "\n| Run | Class | Last tick | Stall | Markers | LB last | Tripwire | First fatal line / detail |\n\
                 |---:|---|---:|---:|---|---|---|---|\n\
                 | 1 | PANIC | 0 | 69s | EL1,BOOT | no | - | - |\n\
                 | 2 | INCONCLUSIVE | 0 | 69s | EL1,BOOT | no | - | heartbeat stuck at tick 0 after the Gate 1 bench started |\n\
                 | 3 | CLEAN | 0 | 69s | EL1,BOOT | no | - | heartbeat stuck at tick 0 after the Gate 1 bench started |\n"
            ),
            "{tail}"
        );
        // Without footers, the data disk and the per-boot load are unknown.
        let info = SummaryInfo {
            fresh_data: None,
            ..info
        };
        let head = text(summary_head(&info, &tally(&[])));
        assert!(head.contains("data disk - |"), "{head}");
        assert!(head.contains("per-boot 1-min  |"), "{head}");
    }
}
