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
//! that detail; and `summary.tsv` gains the `ipc_avg_us` and `ipc_iters`
//! columns after the script's 22.

use super::awk::to_num;
use super::classify::{Base, Class, Classification, Ipc};

/// `summary.tsv`'s header line.
pub const TSV_HEADER: &[u8] = b"run\tmode\tclass\tlast_tick\thb_count\tstall_s\telapsed_s\tqemu_rc\tload1\tkernel_s\thb_first_s\tbench_s\tg1done_s\thb_max_gap_s\tmarkers\tlb_last\tdetail\tfirst_fatal\tlast_info_1\tlast_info_2\tlast_info_3\tlog\tipc_avg_us\tipc_iters\n";

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

/// `wilson K N`: the CLEAN rate with its 95% Wilson score interval, or `n/a`.
pub fn wilson(k: u64, n: u64) -> String {
    if n == 0 {
        return "n/a".to_string();
    }
    let (k, n) = (k as f64, n as f64);
    let z = 1.96;
    let p = k / n;
    let d = 1.0 + z * z / n;
    let c = (p + z * z / (2.0 * n)) / d;
    let h = z * (p * (1.0 - p) / n + z * z / (4.0 * n * n)).sqrt() / d;
    let lo = (c - h).max(0.0);
    let hi = (c + h).min(1.0);
    format!(
        "{:.0}% (95% Wilson interval {:.0}%-{:.0}%)",
        100.0 * p,
        100.0 * lo,
        100.0 * hi
    )
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

/// Per-boot numbers that come from the harness, not the classifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BootTiming {
    pub elapsed: i64,
    pub rc: i32,
    pub load1: Vec<u8>,
    pub kstart: i64,
    pub hb_first: i64,
    pub bench_start: i64,
    pub g1done: i64,
    pub hb_max_gap: i64,
}

/// One `summary.tsv` row, with its newline.
pub fn tsv_row(
    idx: &str,
    mode: &str,
    c: &Classification,
    t: &BootTiming,
    log_name: &str,
) -> Vec<u8> {
    let num = |v: i64| v.to_string().into_bytes();
    let cells: Vec<Vec<u8>> = vec![
        idx.as_bytes().to_vec(),
        mode.as_bytes().to_vec(),
        c.class.name().as_bytes().to_vec(),
        c.tick.clone(),
        c.hb.clone(),
        c.stall.clone(),
        num(t.elapsed),
        t.rc.to_string().into_bytes(),
        t.load1.clone(),
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
    ];
    let mut row = cells.join(&b'\t');
    row.push(b'\n');
    row
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
        &md_cell(&tail),
        b" |\n",
    ]
    .concat()
}

/// The settings `summary.md` records about a soak.
pub struct SummaryInfo<'a> {
    pub mode: &'a str,
    pub runs: &'a str,
    pub secs: &'a str,
    pub stall_secs: &'a str,
    pub fresh_data: bool,
    pub git_rev: &'a str,
    pub kernel_sha: &'a str,
    pub qemu_version: &'a [u8],
    pub firmware: &'a [u8],
    pub host: &'a [u8],
    pub load_start: &'a str,
    pub load_end: &'a str,
    pub out: &'a [u8],
}

/// `summary.md` up to the CLEAN rate (the part also printed to stdout), with
/// its final newline. `counts` follows [`Class::ALL`]; `runs_n` is the boot count.
pub fn summary_head(
    info: &SummaryInfo,
    counts: &[u64; Class::COUNT],
    runs_n: u64,
    load1: &[&[u8]],
) -> Vec<u8> {
    let inconclusive = counts[Class::Inconclusive.index()];
    let clean = counts[Class::Clean.index()];
    let conclusive = runs_n - inconclusive;
    let rate_note = if inconclusive == 0 {
        String::new()
    } else {
        format!(" over {conclusive} conclusive boots ({inconclusive} INCONCLUSIVE left out)")
    };
    let data = if info.fresh_data {
        "fresh per boot"
    } else {
        "reused"
    };
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
            load_summary(load1)
        )
        .into_bytes(),
    );
    md.extend([&b"| Logs | `"[..], info.out, b"` |\n\n"].concat());
    md.extend_from_slice(b"| Class | Count | Share |\n|---|---:|---:|\n");
    for (class, count) in Class::ALL.iter().zip(counts) {
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
    md
}

/// The per-boot table appended to `summary.md` after the head was printed.
pub fn summary_table(rows: &[u8]) -> Vec<u8> {
    [
        &b"\n| Run | Class | Last tick | Stall | Markers | LB last | First fatal line / detail |\n|---:|---|---:|---:|---|---|---|\n"[..],
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
            "| 03 | DEGRADED | 2000 | 1s | EL1,BOOT,G1DONE | - | IPC 9999 iters |\n"
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
            "| 07 | WEDGE-STUCK | 0 | 69s | EL1,BOOT | no | heartbeat stuck at tick 0 after the Gate 1 bench started |\n"
        );
        assert_eq!(
            text(md_row("07", &c(PANIC))),
            "| 07 | PANIC | 1000 | - | - | no | PANIC: panicked at kernel/src/sched/mod.rs:120:9: / assertion failed: thread.state == Ready |\n"
        );
        assert_eq!(md_cell(b"a|b||c"), b"a\\|b\\|\\|c");
    }

    // Values printed by the script's wilson and the awk share one-liner.
    #[test]
    fn wilson_and_share_match_awk() {
        assert_eq!(wilson(0, 0), "n/a");
        assert_eq!(wilson(0, 5), "0% (95% Wilson interval 0%-43%)");
        assert_eq!(wilson(1, 8), "12% (95% Wilson interval 2%-47%)");
        assert_eq!(wilson(3, 8), "38% (95% Wilson interval 14%-69%)");
        assert_eq!(wilson(5, 5), "100% (95% Wilson interval 57%-100%)");
        assert_eq!(wilson(6, 20), "30% (95% Wilson interval 15%-52%)");
        assert_eq!(wilson(12, 20), "60% (95% Wilson interval 39%-78%)");
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

    #[test]
    fn tsv_row_has_24_fields_in_header_order() {
        let t = BootTiming {
            elapsed: 76,
            rc: 124,
            load1: b"1.50".to_vec(),
            kstart: 1,
            hb_first: 2,
            bench_start: 7,
            g1done: 8,
            hb_max_gap: 3,
        };
        let row = text(tsv_row("07", "text", &c(WEDGE), &t, "run-07.log"));
        assert_eq!(
            row,
            "07\ttext\tWEDGE-STUCK\t0\t1\t69\t76\t124\t1.50\t1\t2\t7\t8\t3\tEL1,BOOT\tno\t\
             heartbeat stuck at tick 0 after the Gate 1 bench started\t-\t-\t\
             [   0.200000] [0] INFO  Boot  Boot sequence complete\t\
             [   6.613544] [0] INFO  Ipc   Bench main: server ready, starting IPC benchmark\trun-07.log\t6\t10000\n"
        );
        assert_eq!(row.split('\t').count(), 24);
        assert_eq!(TSV_HEADER.split(|&b| b == b'\t').count(), 24);
        // An unreadable figure is `-`.
        let row = text(tsv_row(
            "01",
            "text",
            &degraded("-", None),
            &t,
            "run-01.log",
        ));
        assert!(row.ends_with("\trun-01.log\t-\t-\n"), "{row}");
    }

    #[test]
    fn summary_head_and_table() {
        let info = SummaryInfo {
            mode: "text",
            runs: "3",
            secs: "75",
            stall_secs: "15",
            fresh_data: true,
            git_rev: "abc1234-dirty",
            kernel_sha: "kernel ELF sha256 `0123456789abcdef`",
            qemu_version: b"QEMU emulator version 10.1.0",
            firmware: b"/fw.fd",
            host: b"Darwin 27.2.0 arm64, 10 CPUs",
            load_start: "1.00 2.00 3.00",
            load_end: "4.00 5.00 6.00",
            out: b"/out",
        };
        let head = text(summary_head(
            &info,
            &[0, 0, 1, 0, 0, 0, 1, 0, 1],
            3,
            &[b"1.00", b"2.00", b"4.00"],
        ));
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
             CLEAN rate: 50% (95% Wilson interval 9%-91%) over 2 conclusive boots (1 INCONCLUSIVE left out)\n"
        );
        assert_eq!(
            text(summary_table(b"| 1 | x |\n")),
            "\n| Run | Class | Last tick | Stall | Markers | LB last | First fatal line / detail |\n\
             |---:|---|---:|---:|---|---|---|\n| 1 | x |\n"
        );
    }
}
