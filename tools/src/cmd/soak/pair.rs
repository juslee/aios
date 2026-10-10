//! The interleaved soak's pair report (crash-fix step 1a, D4 sections 4 to 8):
//! the per-arm load and its 25% rule, the pair tests, the Gate 1 IPC means,
//! the tripwire counters per arm with the "non-CLEAN boot without a tripwire
//! line or fatal report" count, and the non-CLEAN boots. Everything here is
//! pure; [`super::interleave`] gathers one [`Boot`] per boot and writes the
//! text into the top-level `summary.md`.
//!
//! Pairs are every two arms, the earlier one (in `--arm` order) the previous
//! arm and the later one the new arm. Each test runs on conclusive boots
//! only (INCONCLUSIVE is left out of every count and denominator), with the
//! new arm in row 1 of the table ([`super::stats`]):
//!
//! - the regression guard: CLEAN counts, one-sided toward fewer CLEAN in the
//!   new arm; it fails when p < [`ALPHA`];
//! - one row per class (INCONCLUSIVE aside) and per `--combine` group: both
//!   one-sided p values, the two-sided p, the rate difference (recorded, never
//!   gated on), and the markers `removed` (none in the new arm, one-sided p
//!   toward fewer < [`ALPHA`]) and `new` (none in the previous arm, one-sided
//!   p toward more < [`ALPHA`]).
//!
//! The load rule: a pair whose arms' mean per-boot load1 differ by more than
//! [`LOAD_LIMIT`] of the lower mean is redone (owner's Q6: |a − b| / min(a, b),
//! the stricter reading).

use anyhow::{bail, Result};

use super::classify::{Base, Class, Classification};
use super::report::{self, BootTiming};
use super::stats::{fisher_high, fisher_low, fisher_two};
use super::tripwire::V1;

/// The significance level of the regression guard and the markers.
pub const ALPHA: f64 = 0.05;

/// The largest difference between two arms' mean loads, as a share of the
/// lower mean, that keeps a pair comparable.
pub const LOAD_LIMIT: f64 = 0.25;

/// Whether `p` is below [`ALPHA`]. A p equal to 0.05 up to floating-point
/// rounding is not: tables such as 3/3 against 0/3 give exactly 1/20, which
/// the log-factorial sum may put a hair below it.
pub fn significant(p: f64) -> bool {
    p < ALPHA * (1.0 - 1e-9)
}

/// One boot, as the pair report needs it.
#[derive(Debug, Clone)]
pub struct Boot {
    /// The round, from 1.
    pub round: u64,
    /// The arm, an index into the labels.
    pub arm: usize,
    pub class: Class,
    /// The last heartbeat tick, or `-`.
    pub tick: Vec<u8>,
    /// The host's 1-minute load before the boot, when it reads as a number.
    pub load1: Option<f64>,
    /// The Gate 1 IPC average in whole µs, when readable.
    pub ipc_avg_us: Option<u64>,
    /// Whether the boot's markers include G1PASS.
    pub g1pass: bool,
    /// Whether the boot printed a complete tripwire line, of any version.
    pub has_line: bool,
    /// The last complete tripwire line's fields, when that line is `v=1`.
    pub v1: Option<V1>,
    /// The first fatal line or the detail ([`report::boot_text`]).
    pub text: Vec<u8>,
    /// The last complete line's `src` ([`report::tripwire_src`]).
    pub tw_src: Vec<u8>,
    /// The log, relative to the soak's top-level directory.
    pub log: String,
}

impl Boot {
    /// Boot `round` of arm `arm`, classified as `c`, with log `log`.
    pub fn new(round: u64, arm: usize, c: &Classification, t: &BootTiming, log: String) -> Boot {
        Boot {
            round,
            arm,
            class: c.class,
            tick: c.tick.clone(),
            load1: t.load1.as_deref().and_then(number),
            ipc_avg_us: c.ipc.avg_us,
            g1pass: c.markers.split(|&b| b == b',').any(|m| m == b"G1PASS"),
            has_line: c.tripwire.last.is_some(),
            v1: c.tripwire.last.as_ref().and_then(|l| l.v1.clone()),
            text: report::boot_text(c),
            tw_src: report::tripwire_src(c.tripwire.last.as_ref()),
            log,
        }
    }

    /// Whether the boot's class is a fatal report (PCZERO, PANIC-LOCK,
    /// PANIC or EXCEPTION).
    fn fatal(&self) -> bool {
        matches!(
            self.class.base(),
            Base::PcZero | Base::Panic | Base::Exception
        )
    }
}

/// A load average as a number, when it reads as a finite one.
fn number(v: &[u8]) -> Option<f64> {
    std::str::from_utf8(v)
        .ok()?
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|x| x.is_finite())
}

/// The classes of a `--combine` value, `CLASS+CLASS[+...]`: two or more
/// different class names, INCONCLUSIVE excluded (the tests leave it out).
pub fn parse_combine(value: &str) -> Result<Vec<Class>> {
    let mut classes = Vec::new();
    for name in value.split('+') {
        let Some(class) = Class::from_name(name) else {
            let names: Vec<&str> = Class::ALL.iter().map(|c| c.name()).collect();
            bail!(
                "--combine: unknown class '{name}' in '{value}' (classes: {})",
                names.join(", ")
            );
        };
        if class == Class::Inconclusive {
            bail!("--combine: INCONCLUSIVE boots are left out of the pair tests, so INCONCLUSIVE cannot be combined");
        }
        if classes.contains(&class) {
            bail!("--combine: {name} is named twice in '{value}'");
        }
        classes.push(class);
    }
    if classes.len() < 2 {
        bail!("--combine needs two or more classes joined by +, got '{value}'");
    }
    Ok(classes)
}

/// The name of a group of classes: their names joined by `+`.
fn group_name(classes: &[Class]) -> String {
    classes
        .iter()
        .map(|c| c.name())
        .collect::<Vec<_>>()
        .join("+")
}

/// `p` as C's `printf "%.3g"` prints it: three significant digits, trailing
/// zeros dropped, an exponent below 1e-4.
pub fn p_text(p: f64) -> String {
    if p == 0.0 {
        return "0".to_string();
    }
    let sci = format!("{p:.2e}");
    let (mantissa, exp) = sci.split_once('e').expect("an exponent");
    let exp: i32 = exp.parse().expect("an integer exponent");
    let trim = |s: &str| -> String {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s.to_string()
        }
    };
    if !(-4..3).contains(&exp) {
        let sign = if exp < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", trim(mantissa), exp.abs())
    } else {
        let decimals = usize::try_from(2 - exp).expect("exp below 3");
        trim(&format!("{p:.decimals$}"))
    }
}

/// Every pair of arms, earlier first: (previous, new).
pub fn pairs(arms: usize) -> Vec<(usize, usize)> {
    (0..arms)
        .flat_map(|a| (a + 1..arms).map(move |b| (a, b)))
        .collect()
}

/// An arm's conclusive boots, by class.
struct Counts {
    conclusive: u64,
    by_class: [u64; Class::COUNT],
}

impl Counts {
    fn of(boots: &[Boot], arm: usize) -> Counts {
        let mut by_class = [0; Class::COUNT];
        for b in boots.iter().filter(|b| b.arm == arm) {
            by_class[b.class.index()] += 1;
        }
        let conclusive = by_class.iter().sum::<u64>() - by_class[Class::Inconclusive.index()];
        Counts {
            conclusive,
            by_class,
        }
    }

    /// The conclusive boots in any of `classes`.
    fn in_classes(&self, classes: &[Class]) -> u64 {
        classes.iter().map(|c| self.by_class[c.index()]).sum()
    }
}

/// A class's (or group's) test in one pair.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Test {
    /// The previous arm's boots in the class, and its conclusive boots.
    pub prev: (u64, u64),
    /// The new arm's.
    pub new: (u64, u64),
    /// One-sided p toward fewer in the new arm.
    pub fewer: f64,
    /// One-sided p toward more in the new arm.
    pub more: f64,
    /// Two-sided p.
    pub two: f64,
}

impl Test {
    /// The test of `prev` = (k, n) against `new` = (k, n); `None` when an arm
    /// has no conclusive boot.
    pub fn of(prev: (u64, u64), new: (u64, u64)) -> Option<Test> {
        if prev.1 == 0 || new.1 == 0 {
            return None;
        }
        let t = (new.0, new.1 - new.0, prev.0, prev.1 - prev.0);
        Some(Test {
            prev,
            new,
            fewer: fisher_low(t.0, t.1, t.2, t.3),
            more: fisher_high(t.0, t.1, t.2, t.3),
            two: fisher_two(t.0, t.1, t.2, t.3),
        })
    }

    /// None in the new arm, and significantly fewer than in the previous one.
    pub fn removed(&self) -> bool {
        self.new.0 == 0 && significant(self.fewer)
    }

    /// None in the previous arm, and significantly more in the new one.
    pub fn appeared(&self) -> bool {
        self.prev.0 == 0 && significant(self.more)
    }

    /// The new arm's rate minus the previous arm's, in percentage points.
    pub fn difference_pp(&self) -> f64 {
        100.0 * (self.new.0 as f64 / self.new.1 as f64 - self.prev.0 as f64 / self.prev.1 as f64)
    }
}

/// The test of `classes` in the pair (`prev`, `new`).
fn test(boots: &[Boot], prev: usize, new: usize, classes: &[Class]) -> Option<Test> {
    let (p, n) = (Counts::of(boots, prev), Counts::of(boots, new));
    Test::of(
        (p.in_classes(classes), p.conclusive),
        (n.in_classes(classes), n.conclusive),
    )
}

/// The regression guard of the pair (`prev`, `new`): the CLEAN test.
pub fn guard(boots: &[Boot], prev: usize, new: usize) -> Option<Test> {
    test(boots, prev, new, &[Class::Clean])
}

/// Whether some pair's regression guard fails.
pub fn regression(arms: usize, boots: &[Boot]) -> bool {
    pairs(arms)
        .into_iter()
        .filter_map(|(p, n)| guard(boots, p, n))
        .any(|t| significant(t.fewer))
}

/// An arm's per-boot load1 mean and max, over the boots with a load; `None`
/// without one.
fn load(boots: &[Boot], arm: usize) -> Option<(f64, f64, usize)> {
    let loads: Vec<f64> = boots
        .iter()
        .filter(|b| b.arm == arm)
        .filter_map(|b| b.load1)
        .collect();
    if loads.is_empty() {
        return None;
    }
    let mean = loads.iter().sum::<f64>() / loads.len() as f64;
    let max = loads.iter().copied().fold(f64::MIN, f64::max);
    Some((mean, max, loads.len()))
}

/// How far apart two mean loads are, as a share of the lower: 0 when both
/// are 0, infinite when only the lower is.
pub fn load_apart(a: f64, b: f64) -> f64 {
    let (lo, hi) = (a.min(b), a.max(b));
    if hi == lo {
        0.0
    } else if lo <= 0.0 {
        f64::INFINITY
    } else {
        (hi - lo) / lo
    }
}

/// The load verdict of a pair whose arms' means are `a` and `b`.
fn load_verdict(a: f64, b: f64) -> String {
    let apart = load_apart(a, b);
    let how = if apart.is_finite() {
        format!("{:.0}% apart", 100.0 * apart)
    } else {
        "apart, the lower mean 0".to_string()
    };
    if apart > LOAD_LIMIT {
        format!("{a:.2} vs {b:.2}, {how}: **redo the pair**")
    } else {
        format!("{a:.2} vs {b:.2}, {how}: comparable")
    }
}

/// A Markdown table row.
fn row(cells: &[String]) -> String {
    format!("| {} |\n", cells.join(" | "))
}

/// Section 4: per-arm load, and each pair's verdict.
fn load_section(labels: &[&str], boots: &[Boot]) -> String {
    let mut md = String::from(
        "\n### Load\n\nThe host's 1-minute load average before each boot.\n\n| Arm | Boots with a load | load1 mean | load1 max |\n|---|---:|---:|---:|\n",
    );
    let loads: Vec<Option<(f64, f64, usize)>> = (0..labels.len()).map(|a| load(boots, a)).collect();
    for (label, l) in labels.iter().zip(&loads) {
        md.push_str(&match l {
            Some((mean, max, n)) => row(&[
                label.to_string(),
                n.to_string(),
                format!("{mean:.2}"),
                format!("{max:.2}"),
            ]),
            None => row(&[label.to_string(), "0".into(), "-".into(), "-".into()]),
        });
    }
    md.push_str(&format!(
        "\nA pair whose arms' mean loads differ by more than {:.0}% of the lower mean is redone: the load, not the change, may explain its differences.\n\n",
        100.0 * LOAD_LIMIT
    ));
    for (p, n) in pairs(labels.len()) {
        let verdict = match (loads[p], loads[n]) {
            (Some((a, _, _)), Some((b, _, _))) => load_verdict(a, b),
            _ => "n/a (no load recorded for an arm)".to_string(),
        };
        md.push_str(&format!("- {} vs {}: {verdict}\n", labels[p], labels[n]));
    }
    md
}

/// A test's cells: counts, difference, the three p values and the marker.
fn test_cells(t: Option<&Test>, counts: ((u64, u64), (u64, u64))) -> Vec<String> {
    let ((kp, np), (kn, nn)) = counts;
    let mut cells = vec![format!("{kp}/{np}"), format!("{kn}/{nn}")];
    match t {
        None => cells.extend(["n/a", "n/a", "n/a", "n/a", "-"].map(String::from)),
        Some(t) => {
            let d = t.difference_pp().round();
            cells.push(if d == 0.0 {
                "0 pp".to_string()
            } else {
                format!("{d:+.0} pp")
            });
            cells.push(p_text(t.fewer));
            cells.push(p_text(t.more));
            cells.push(p_text(t.two));
            cells.push(
                if t.removed() {
                    "removed"
                } else if t.appeared() {
                    "new"
                } else {
                    "-"
                }
                .to_string(),
            );
        }
    }
    cells
}

/// Section 5: one block per pair.
fn pair_section(labels: &[&str], boots: &[Boot], combine: &[Vec<Class>]) -> String {
    let all = pairs(labels.len());
    let mut md = format!(
        "\n### Pair tests\n\nFisher's exact test on conclusive boots only (INCONCLUSIVE is left out of every count and denominator). In each pair the earlier arm is the previous one and the later arm the new one. The regression guard fails when the new arm has fewer CLEAN boots with one-sided p < {ALPHA}. A class is marked `removed` when the new arm has none of it and the one-sided p (fewer in new) < {ALPHA}, and `new` when the previous arm has none of it and the one-sided p (more in new) < {ALPHA}. The rate differences (new - previous, in percentage points) are recorded, never gated on.\n"
    );
    if all.len() > 1 {
        md.push_str(&format!(
            "\n{n} pairs are listed with no correction: read the planned pair at {ALPHA}; a pair picked after seeing the tables needs p < {} (Bonferroni, {ALPHA}/{n}).\n",
            p_text(ALPHA / all.len() as f64),
            n = all.len()
        ));
    }
    for (p, n) in all {
        let (lp, ln) = (labels[p], labels[n]);
        let (cp, cn) = (Counts::of(boots, p), Counts::of(boots, n));
        md.push_str(&format!(
            "\n#### {lp} vs {ln} ({lp} previous, {ln} new)\n\n"
        ));
        let clean = (
            cp.in_classes(&[Class::Clean]),
            cn.in_classes(&[Class::Clean]),
        );
        md.push_str(&match guard(boots, p, n) {
            None => {
                let empty = if cp.conclusive == 0 { lp } else { ln };
                format!("**Regression guard:** n/a (arm {empty} has no conclusive boot)\n\n")
            }
            Some(t) => {
                let verdict = if significant(t.fewer) {
                    format!("**fails** (p < {ALPHA})")
                } else {
                    "passes".to_string()
                };
                format!(
                    "**Regression guard:** CLEAN {}/{} in {lp}, {}/{} in {ln}; one-sided p (fewer CLEAN in {ln}) = {}: {verdict}\n\n",
                    clean.0,
                    cp.conclusive,
                    clean.1,
                    cn.conclusive,
                    p_text(t.fewer)
                )
            }
        });
        md.push_str(&format!(
            "| Class | {lp} | {ln} | {ln} - {lp} | p fewer in {ln} | p more in {ln} | p two-sided | Marker |\n|---|---:|---:|---:|---:|---:|---:|---|\n"
        ));
        let groups: Vec<Vec<Class>> = Class::ALL
            .into_iter()
            .filter(|&c| c != Class::Inconclusive)
            .map(|c| vec![c])
            .chain(combine.iter().cloned())
            .collect();
        for classes in &groups {
            let counts = (
                (cp.in_classes(classes), cp.conclusive),
                (cn.in_classes(classes), cn.conclusive),
            );
            let t = Test::of(counts.0, counts.1);
            let mut cells = vec![group_name(classes)];
            cells.extend(test_cells(t.as_ref(), counts));
            md.push_str(&row(&cells));
        }
    }
    md
}

/// Section 6: the Gate 1 IPC mean per arm, over its CLEAN boots.
fn ipc_section(labels: &[&str], boots: &[Boot]) -> String {
    let mut md = String::from(
        "\n### Gate 1 IPC\n\n| Arm | IPC round trip, mean over CLEAN boots | n | CLEAN boots without avg= | G1PASS boots |\n|---|---:|---:|---:|---:|\n",
    );
    for (arm, label) in labels.iter().enumerate() {
        let mine: Vec<&Boot> = boots.iter().filter(|b| b.arm == arm).collect();
        let clean: Vec<Option<u64>> = mine
            .iter()
            .filter(|b| b.class == Class::Clean)
            .map(|b| b.ipc_avg_us)
            .collect();
        let known: Vec<u64> = clean.iter().flatten().copied().collect();
        let mean = if known.is_empty() {
            "n/a".to_string()
        } else {
            format!(
                "{:.2} us",
                known.iter().map(|&v| v as f64).sum::<f64>() / known.len() as f64
            )
        };
        let g1pass = mine.iter().filter(|b| b.g1pass).count();
        md.push_str(&row(&[
            label.to_string(),
            mean,
            known.len().to_string(),
            (clean.len() - known.len()).to_string(),
            format!("{g1pass} of {}", mine.len()),
        ]));
    }
    md.push_str("\nEach boot's avg= is whole us, as the kernel truncates it.\n");
    md
}

/// R16's count for one arm: its conclusive non-CLEAN boots with neither a
/// complete tripwire line nor a fatal report, or `n/a` when no boot of the arm
/// has a complete tripwire line. INCONCLUSIVE boots are left out, as in every
/// other test of the report: they carry no kernel result (the stub never ran,
/// QEMU was killed by a signal, or the boot was cut short), so counting them
/// would charge a harness error to the arm's kernel.
pub fn unexplained(boots: &[Boot], arm: usize) -> String {
    let mine: Vec<&Boot> = boots.iter().filter(|b| b.arm == arm).collect();
    if !mine.iter().any(|b| b.has_line) {
        return "n/a (no tripwire line in any boot)".to_string();
    }
    mine.iter()
        .filter(|b| {
            !matches!(b.class, Class::Clean | Class::Inconclusive) && !b.has_line && !b.fatal()
        })
        .count()
        .to_string()
}

/// The note under R16's count.
pub const UNEXPLAINED_NOTE: &str = "That count is meaningful only for an arm whose kernel prints tripwire lines (step 1b, e211d6d, or later); for an older arm every conclusive non-CLEAN boot without a fatal report counts. INCONCLUSIVE boots (no kernel result) are never counted.";

/// Section 7: R16's count, then the tripwire counters with arms as columns.
fn tripwire_section(labels: &[&str], boots: &[Boot]) -> Vec<u8> {
    let mut md = b"\n### Tripwire per arm\n\n| Arm | Conclusive non-CLEAN boots with neither a complete tripwire line nor a fatal report |\n|---|---:|\n".to_vec();
    for (arm, label) in labels.iter().enumerate() {
        md.extend(row(&[label.to_string(), unexplained(boots, arm)]).into_bytes());
    }
    md.extend(format!("\n{UNEXPLAINED_NOTE}\n\n").into_bytes());
    let columns: Vec<String> = labels.iter().map(|l| l.to_string()).collect();
    let lines: Vec<(usize, Option<&V1>)> = boots.iter().map(|b| (b.arm, b.v1.as_ref())).collect();
    match report::counter_table(&columns, &lines) {
        None => md.extend_from_slice(b"No boot has a complete `v=1` tripwire line.\n"),
        Some(table) => {
            md.extend(
                format!(
                    "From each boot's last complete `[tripwire]` line (the `tw_*` columns of `boots.tsv`), by arm. {}\n\n",
                    report::COUNTER_TABLE_NOTE
                )
                .into_bytes(),
            );
            md.extend(table);
        }
    }
    md
}

/// Section 8: every non-CLEAN boot, in boot order.
fn non_clean_section(labels: &[&str], boots: &[Boot]) -> Vec<u8> {
    let mut md = b"\n### Non-CLEAN boots\n\n".to_vec();
    let non_clean: Vec<&Boot> = boots.iter().filter(|b| b.class != Class::Clean).collect();
    if non_clean.is_empty() {
        md.extend_from_slice(b"None.\n");
        return md;
    }
    md.extend_from_slice(
        b"| Round | Arm | Class | Last tick | First fatal line / detail | Tripwire | Log |\n|---:|---|---|---:|---|---|---|\n",
    );
    for b in non_clean {
        md.extend(
            [
                format!("| {} | {} | {} | ", b.round, labels[b.arm], b.class.name()).as_bytes(),
                &report::md_cell(&b.tick),
                b" | ",
                &report::md_cell(&b.text),
                b" | ",
                &report::md_cell(&b.tw_src),
                b" | `",
                &report::md_cell(b.log.as_bytes()),
                b"` |\n",
            ]
            .concat(),
        );
    }
    md
}

/// Sections 4 to 8 of the top-level `summary.md`, for arms `labels` and the
/// boots so far, with the `--combine` groups as extra rows of every pair.
pub fn sections(labels: &[&str], boots: &[Boot], combine: &[Vec<Class>]) -> Vec<u8> {
    [
        load_section(labels, boots).into_bytes(),
        pair_section(labels, boots, combine).into_bytes(),
        ipc_section(labels, boots).into_bytes(),
        tripwire_section(labels, boots),
        non_clean_section(labels, boots),
    ]
    .concat()
}

/// One console line per pair: the regression guard and the load verdict.
pub fn console(labels: &[&str], boots: &[Boot]) -> Vec<u8> {
    let mut out = String::new();
    for (p, n) in pairs(labels.len()) {
        let guard_text = match guard(boots, p, n) {
            None => "regression guard n/a (an arm has no conclusive boot)".to_string(),
            Some(t) => format!(
                "regression guard {} (CLEAN {}/{} vs {}/{}, one-sided p {})",
                if significant(t.fewer) {
                    "FAILS"
                } else {
                    "passes"
                },
                t.prev.0,
                t.prev.1,
                t.new.0,
                t.new.1,
                p_text(t.fewer)
            ),
        };
        let load_text = match (load(boots, p), load(boots, n)) {
            (Some((a, _, _)), Some((b, _, _))) => {
                format!("mean load {}", load_verdict(a, b).replace("**", ""))
            }
            _ => "mean load n/a".to_string(),
        };
        out.push_str(&format!(
            "soak: {} vs {}: {guard_text}; {load_text}\n",
            labels[p], labels[n]
        ));
    }
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    const LABELS: [&str; 3] = ["A", "B", "C"];

    /// A boot of `arm` in `class`, with load `load1`.
    fn boot(arm: usize, class: Class, load1: Option<f64>) -> Boot {
        let fatal = matches!(class.base(), Base::PcZero | Base::Panic | Base::Exception);
        Boot {
            round: 1,
            arm,
            class,
            tick: b"1000".to_vec(),
            load1,
            ipc_avg_us: (class == Class::Clean).then_some(6),
            g1pass: class == Class::Clean,
            has_line: false,
            v1: None,
            text: if fatal {
                b"PANIC: x".to_vec()
            } else {
                b"-".to_vec()
            },
            tw_src: b"-".to_vec(),
            log: format!("arm-{}/run-01.log", LABELS[arm]),
        }
    }

    /// `n` boots of `arm` in `class`.
    fn many(arm: usize, class: Class, n: usize) -> Vec<Boot> {
        (0..n).map(|_| boot(arm, class, Some(1.0))).collect()
    }

    /// Arm A with `a` boots of `class` and arm B with `b`, each arm 30
    /// conclusive boots (the rest CLEAN, or PANIC when `class` is CLEAN).
    fn thirty(class: Class, a: usize, b: usize) -> Vec<Boot> {
        let other = if class == Class::Clean {
            Class::Panic
        } else {
            Class::Clean
        };
        [
            many(0, class, a),
            many(0, other, 30 - a),
            many(1, class, b),
            many(1, other, 30 - b),
        ]
        .concat()
    }

    fn text(v: Vec<u8>) -> String {
        String::from_utf8(v).expect("UTF-8")
    }

    /// The pair table row of `name` in `md`, as cells.
    fn table_row<'a>(md: &'a str, name: &str) -> Vec<&'a str> {
        md.lines()
            .find(|l| l.starts_with(&format!("| {name} |")))
            .unwrap_or_else(|| panic!("no row {name} in\n{md}"))
            .trim_matches('|')
            .split('|')
            .map(str::trim)
            .collect()
    }

    #[test]
    fn p_text_matches_printf_3g() {
        for (p, want) in [
            (0.0, "0"),
            (1.0, "1"),
            (0.5, "0.5"),
            (0.05, "0.05"),
            (0.0562, "0.0562"),
            (0.056_17, "0.0562"),
            (0.111_11, "0.111"),
            (0.000_244_4, "0.000244"),
            (0.000_099_99, "0.0001"),
            (0.000_012_34, "1.23e-05"),
            (2.5e-12, "2.5e-12"),
            (0.999_9, "1"),
            (0.016_666, "0.0167"),
        ] {
            assert_eq!(p_text(p), want, "{p}");
        }
    }

    #[test]
    fn pairs_are_every_two_arms_earlier_first() {
        assert_eq!(pairs(2), [(0, 1)]);
        assert_eq!(pairs(3), [(0, 1), (0, 2), (1, 2)]);
        assert_eq!(pairs(4).len(), 6);
    }

    #[test]
    fn the_regression_guard_at_the_p_005_boundary() {
        // The ADR's figures at 30 boots per arm: 4 fewer CLEAN p = 0.0562
        // (passes), 5 fewer p = 0.0261 (fails).
        let passes = thirty(Class::Clean, 30, 26);
        let t = guard(&passes, 0, 1).expect("a test");
        assert_eq!(p_text(t.fewer), "0.0562");
        assert!(!regression(2, &passes));
        let fails = thirty(Class::Clean, 30, 25);
        let t = guard(&fails, 0, 1).expect("a test");
        assert_eq!(p_text(t.fewer), "0.0261");
        assert!(regression(2, &fails));
        // Exactly 0.05 (3/3 against 0/3 is 1/20) is not below it.
        let exact = [many(0, Class::Clean, 3), many(1, Class::Panic, 3)].concat();
        let t = guard(&exact, 0, 1).expect("a test");
        assert!((t.fewer - 0.05).abs() < 1e-12, "{}", t.fewer);
        assert!(!regression(2, &exact));
        // More CLEAN in the new arm never fails.
        assert!(!regression(2, &thirty(Class::Clean, 0, 30)));

        let md = text(sections(&LABELS[..2], &fails, &[]));
        assert!(md.contains("**Regression guard:** CLEAN 30/30 in A, 25/30 in B; one-sided p (fewer CLEAN in B) = 0.0261: **fails** (p < 0.05)\n"), "{md}");
        let md = text(sections(&LABELS[..2], &passes, &[]));
        assert!(md.contains("**Regression guard:** CLEAN 30/30 in A, 26/30 in B; one-sided p (fewer CLEAN in B) = 0.0562: passes\n"), "{md}");
        assert_eq!(
            text(console(&LABELS[..2], &fails)),
            "soak: A vs B: regression guard FAILS (CLEAN 30/30 vs 25/30, one-sided p 0.0261); mean load 1.00 vs 1.00, 0% apart: comparable\n"
        );
    }

    #[test]
    fn removed_and_new_markers() {
        // 5 -> 0 of 30: removed (p = 0.0261); 4 -> 0: not (p = 0.0562).
        let md = text(sections(&LABELS[..2], &thirty(Class::PanicLock, 5, 0), &[]));
        assert_eq!(
            table_row(&md, "PANIC-LOCK"),
            [
                "PANIC-LOCK",
                "5/30",
                "0/30",
                "-17 pp",
                "0.0261",
                "1",
                "0.0522",
                "removed"
            ]
        );
        let md = text(sections(&LABELS[..2], &thirty(Class::PanicLock, 4, 0), &[]));
        assert_eq!(table_row(&md, "PANIC-LOCK")[7], "-", "{md}");
        assert_eq!(table_row(&md, "PANIC-LOCK")[4], "0.0562");
        // 0 -> 5: new; 0 -> 4: not.
        let md = text(sections(
            &LABELS[..2],
            &thirty(Class::WedgeStuck, 0, 5),
            &[],
        ));
        assert_eq!(
            table_row(&md, "WEDGE-STUCK"),
            [
                "WEDGE-STUCK",
                "0/30",
                "5/30",
                "+17 pp",
                "1",
                "0.0261",
                "0.0522",
                "new"
            ]
        );
        let md = text(sections(
            &LABELS[..2],
            &thirty(Class::WedgeStuck, 0, 4),
            &[],
        ));
        assert_eq!(table_row(&md, "WEDGE-STUCK")[7], "-");
        // A class absent from both arms: no difference, no marker.
        assert_eq!(
            table_row(&md, "PCZERO"),
            ["PCZERO", "0/30", "0/30", "0 pp", "1", "1", "1", "-"]
        );
        // INCONCLUSIVE has no row.
        assert!(!md.contains("| INCONCLUSIVE |"), "{md}");
    }

    #[test]
    fn a_combined_row_sums_its_classes() {
        // Step 1b's test: WEDGE-STUCK + PANIC-LOCK, two-sided.
        let boots = [
            many(0, Class::WedgeStuck, 4),
            many(0, Class::Clean, 26),
            many(1, Class::PanicLock, 1),
            many(1, Class::Clean, 29),
        ]
        .concat();
        let combine = vec![parse_combine("WEDGE-STUCK+PANIC-LOCK").expect("a group")];
        let md = text(sections(&LABELS[..2], &boots, &combine));
        let cells = table_row(&md, "WEDGE-STUCK+PANIC-LOCK");
        assert_eq!(
            &cells[..4],
            ["WEDGE-STUCK+PANIC-LOCK", "4/30", "1/30", "-10 pp"]
        );
        let t = Test::of((4, 30), (1, 30)).expect("a test");
        assert_eq!(cells[6], p_text(t.two));
        assert_eq!(cells[6], "0.353");
        // The group comes after the classes.
        let classes = md.find("| CLEAN |").expect("CLEAN row");
        assert!(md.find("| WEDGE-STUCK+PANIC-LOCK |").expect("group") > classes);
    }

    #[test]
    fn parse_combine_names_classes() {
        assert_eq!(
            parse_combine("WEDGE-STUCK+PANIC-LOCK").expect("ok"),
            [Class::WedgeStuck, Class::PanicLock]
        );
        let err = |v: &str| format!("{:#}", parse_combine(v).expect_err(v));
        assert!(err("WEDGE+PANIC")
            .starts_with("--combine: unknown class 'WEDGE' in 'WEDGE+PANIC' (classes: PCZERO, "));
        assert_eq!(
            err("PANIC"),
            "--combine needs two or more classes joined by +, got 'PANIC'"
        );
        assert_eq!(
            err("PANIC+PANIC"),
            "--combine: PANIC is named twice in 'PANIC+PANIC'"
        );
        assert!(err("PANIC+INCONCLUSIVE").starts_with("--combine: INCONCLUSIVE boots are left out"));
        assert!(err("").starts_with("--combine: unknown class ''"));
    }

    #[test]
    fn inconclusive_boots_are_left_out_of_the_denominators() {
        let boots = [
            many(0, Class::Clean, 3),
            many(0, Class::Inconclusive, 7),
            many(1, Class::Clean, 1),
            many(1, Class::Panic, 1),
            many(1, Class::Inconclusive, 2),
        ]
        .concat();
        let t = guard(&boots, 0, 1).expect("a test");
        assert_eq!((t.prev, t.new), ((3, 3), (1, 2)));
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert_eq!(&table_row(&md, "CLEAN")[..3], ["CLEAN", "3/3", "1/2"]);
        // An arm with only INCONCLUSIVE boots: n/a.
        let none = [many(0, Class::Clean, 2), many(1, Class::Inconclusive, 2)].concat();
        assert_eq!(guard(&none, 0, 1), None);
        assert!(!regression(2, &none));
        let md = text(sections(&LABELS[..2], &none, &[]));
        assert!(
            md.contains("**Regression guard:** n/a (arm B has no conclusive boot)\n"),
            "{md}"
        );
        assert_eq!(
            table_row(&md, "CLEAN"),
            ["CLEAN", "2/2", "0/0", "n/a", "n/a", "n/a", "n/a", "-"]
        );
    }

    #[test]
    fn the_load_rule_redoes_a_pair_above_25_percent_of_the_lower_mean() {
        assert!((load_apart(1.0, 1.24) - 0.24).abs() < 1e-9);
        assert_eq!(load_apart(0.0, 0.0), 0.0);
        assert!(load_apart(0.0, 0.1).is_infinite());
        let at = |b: f64| {
            let boots = vec![
                boot(0, Class::Clean, Some(1.0)),
                boot(0, Class::Clean, Some(1.0)),
                boot(1, Class::Clean, Some(b)),
                boot(1, Class::Clean, None),
            ];
            text(sections(&LABELS[..2], &boots, &[]))
        };
        let md = at(1.24);
        assert!(
            md.contains("- A vs B: 1.00 vs 1.24, 24% apart: comparable\n"),
            "{md}"
        );
        assert!(md.contains("| B | 1 | 1.24 | 1.24 |\n"), "{md}");
        let md = at(1.26);
        assert!(
            md.contains("- A vs B: 1.00 vs 1.26, 26% apart: **redo the pair**\n"),
            "{md}"
        );
        // Relative to the lower mean, whichever arm has it.
        let boots = vec![
            boot(0, Class::Clean, Some(1.26)),
            boot(1, Class::Clean, Some(1.0)),
        ];
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert!(
            md.contains("- A vs B: 1.26 vs 1.00, 26% apart: **redo the pair**\n"),
            "{md}"
        );
        let boots = vec![
            boot(0, Class::Clean, None),
            boot(1, Class::Clean, Some(1.0)),
        ];
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert!(md.contains("| A | 0 | - | - |\n"), "{md}");
        assert!(
            md.contains("- A vs B: n/a (no load recorded for an arm)\n"),
            "{md}"
        );
    }

    #[test]
    fn the_gate1_mean_is_over_clean_boots_only() {
        let mut boots = vec![
            boot(0, Class::Clean, None),
            boot(0, Class::Clean, None),
            boot(0, Class::Degraded, None),
            boot(0, Class::Clean, None),
        ];
        boots[1].ipc_avg_us = Some(9);
        boots[2].ipc_avg_us = Some(100);
        boots[3].ipc_avg_us = None;
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert!(md.contains("| A | 7.50 us | 2 | 1 | 3 of 4 |\n"), "{md}");
        assert!(md.contains("| B | n/a | 0 | 0 | 0 of 0 |\n"), "{md}");
    }

    #[test]
    fn the_r16_count_and_its_label() {
        let mut lined = boot(0, Class::Clean, None);
        lined.has_line = true;
        let boots = vec![
            lined,
            // Counted: not CLEAN, no line, no fatal report.
            boot(0, Class::WedgeStuck, None),
            boot(0, Class::Degraded, None),
            // Not counted: a fatal report.
            boot(0, Class::Panic, None),
            // Not counted: no kernel result, so not the kernel's to explain.
            boot(0, Class::Inconclusive, None),
            // Arm B has no tripwire line in any boot.
            boot(1, Class::WedgeAlive, None),
        ];
        assert_eq!(unexplained(&boots, 0), "2");
        assert_eq!(unexplained(&boots, 1), "n/a (no tripwire line in any boot)");
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert!(md.contains("| Arm | Conclusive non-CLEAN boots with neither a complete tripwire line nor a fatal report |\n|---|---:|\n| A | 2 |\n| B | n/a (no tripwire line in any boot) |\n"), "{md}");
        assert!(md.contains(UNEXPLAINED_NOTE), "{md}");
        assert!(
            md.contains("No boot has a complete `v=1` tripwire line.\n"),
            "{md}"
        );
    }

    #[test]
    fn three_arms_get_a_bonferroni_note_and_three_blocks() {
        let boots = [
            many(0, Class::Clean, 2),
            many(1, Class::Clean, 2),
            many(2, Class::Panic, 2),
        ]
        .concat();
        let md = text(sections(&LABELS, &boots, &[]));
        assert!(md.contains("\n3 pairs are listed with no correction: read the planned pair at 0.05; a pair picked after seeing the tables needs p < 0.0167 (Bonferroni, 0.05/3).\n"), "{md}");
        for block in [
            "#### A vs B (A previous, B new)",
            "#### A vs C (A previous, C new)",
            "#### B vs C (B previous, C new)",
        ] {
            assert!(md.contains(block), "{md}");
        }
        let two = text(sections(&LABELS[..2], &boots[..4], &[]));
        assert!(!two.contains("Bonferroni"), "{two}");
    }

    #[test]
    fn non_clean_boots_are_listed_in_boot_order() {
        let mut p = boot(1, Class::Panic, None);
        p.round = 2;
        p.text = b"PANIC: a | b".to_vec();
        let boots = vec![
            boot(0, Class::Clean, None),
            p,
            boot(0, Class::WedgeStuck, None),
        ];
        let md = text(sections(&LABELS[..2], &boots, &[]));
        assert!(md.ends_with(
            "\n### Non-CLEAN boots\n\n| Round | Arm | Class | Last tick | First fatal line / detail | Tripwire | Log |\n|---:|---|---|---:|---|---|---|\n\
             | 2 | B | PANIC | 1000 | PANIC: a \\| b | - | `arm-B/run-01.log` |\n\
             | 1 | A | WEDGE-STUCK | 1000 | - | - | `arm-A/run-01.log` |\n"
        ), "{md}");
        let md = text(sections(&LABELS[..2], &many(0, Class::Clean, 1), &[]));
        assert!(md.ends_with("\n### Non-CLEAN boots\n\nNone.\n"), "{md}");
    }
}
