//! The soak statistics (crash-fix step 1a): Fisher's exact test on a 2×2
//! table, one- and two-sided, and the Wilson score interval of `summary.md`'s
//! CLEAN rate.
//!
//! A table is `[[a, b], [c, d]]`: one row per arm, the first column the boots
//! in a class (or a group of classes), the second the conclusive boots not in
//! it. With the margins fixed, the top-left cell follows the hypergeometric
//! distribution, computed through log-factorials, so no new crate is needed:
//!
//! - [`fisher_low`]: P(X ≤ a), the one-sided p toward row 1 having the lower
//!   rate. The crash-fix regression guard puts the new arm in row 1 and the
//!   CLEAN counts in the first column; "class removed" does the same with the
//!   class's counts.
//! - [`fisher_high`]: P(X ≥ a), toward row 1 having the higher rate.
//! - [`fisher_two`]: the total probability of the tables no more likely than
//!   this one, with R's relative tolerance of 1e−7 (`fisher.test`).
//!
//! Each p is clamped to 1, since rounding can push a sum of probabilities a
//! hair above it. The log-factorial sums stay accurate far beyond soak sizes;
//! the tests compare every table up to 30 + 30 boots with exact binomials.

/// `[[a, b], [c, d]]` with its margins and the log-factorials up to `n`.
struct Hyper {
    /// `lf[k]` = ln k!.
    lf: Vec<f64>,
    /// Row 1's total, a + b.
    r1: u64,
    /// Row 2's total, c + d.
    r2: u64,
    /// Column 1's total, a + c.
    c1: u64,
    /// The top-left cell.
    a: u64,
}

impl Hyper {
    fn new(a: u64, b: u64, c: u64, d: u64) -> Self {
        let n = a + b + c + d;
        let mut lf = Vec::with_capacity(n as usize + 1);
        let mut sum = 0.0;
        lf.push(sum);
        for k in 1..=n {
            sum += (k as f64).ln();
            lf.push(sum);
        }
        Self {
            lf,
            r1: a + b,
            r2: c + d,
            c1: a + c,
            a,
        }
    }

    /// ln C(n, k), for k ≤ n.
    fn ln_choose(&self, n: u64, k: u64) -> f64 {
        self.lf[n as usize] - self.lf[k as usize] - self.lf[(n - k) as usize]
    }

    /// The probability of `x` in the top-left cell given the margins:
    /// C(r1, x) · C(r2, c1 − x) / C(n, c1), for x in [`Self::lo`, `Self::hi`].
    fn p(&self, x: u64) -> f64 {
        let ln = self.ln_choose(self.r1, x) + self.ln_choose(self.r2, self.c1 - x)
            - self.ln_choose(self.r1 + self.r2, self.c1);
        ln.exp()
    }

    /// The smallest possible top-left cell: c1 − r2, or 0.
    fn lo(&self) -> u64 {
        self.c1.saturating_sub(self.r2)
    }

    /// The largest possible top-left cell: min(r1, c1).
    fn hi(&self) -> u64 {
        self.r1.min(self.c1)
    }
}

/// One-sided Fisher exact test on `[[a, b], [c, d]]` toward a low top-left
/// cell: the probability, given the margins, that row 1 has `a` or fewer in
/// column 1 (the alternative being that row 1's rate is the lower one).
pub fn fisher_low(a: u64, b: u64, c: u64, d: u64) -> f64 {
    let h = Hyper::new(a, b, c, d);
    let s: f64 = (h.lo()..=h.a).map(|x| h.p(x)).sum();
    s.min(1.0)
}

/// One-sided Fisher exact test on `[[a, b], [c, d]]` toward a high top-left
/// cell: the probability, given the margins, that row 1 has `a` or more in
/// column 1 (the alternative being that row 1's rate is the higher one).
pub fn fisher_high(a: u64, b: u64, c: u64, d: u64) -> f64 {
    let h = Hyper::new(a, b, c, d);
    let s: f64 = (h.a..=h.hi()).map(|x| h.p(x)).sum();
    s.min(1.0)
}

/// Two-sided Fisher exact test on `[[a, b], [c, d]]`: the total probability
/// of the tables with the same margins that are no more likely than this one,
/// a table counting as no more likely within a relative 1e−7 (R's
/// `fisher.test` convention).
pub fn fisher_two(a: u64, b: u64, c: u64, d: u64) -> f64 {
    let h = Hyper::new(a, b, c, d);
    let limit = h.p(h.a) * (1.0 + 1e-7);
    let s: f64 = (h.lo()..=h.hi())
        .map(|x| h.p(x))
        .filter(|&p| p <= limit)
        .sum();
    s.min(1.0)
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The largest row total the exact oracle covers.
    const MAX_ROW: u64 = 30;

    /// Exact binomials C(n, k) for n ≤ 2 · [`MAX_ROW`], by Pascal's triangle.
    /// C(60, 30) ≈ 1.2e17, and a product of two row binomials stays below
    /// C(30, 15)² ≈ 2.4e16, so u128 never overflows.
    fn binomials() -> Vec<Vec<u128>> {
        let top = 2 * MAX_ROW as usize;
        let mut t: Vec<Vec<u128>> = Vec::with_capacity(top + 1);
        for n in 0..=top {
            let mut row = vec![1u128; n + 1];
            for k in 1..n {
                row[k] = t[n - 1][k - 1] + t[n - 1][k];
            }
            t.push(row);
        }
        t
    }

    /// The three exact p values of `[[a, b], [c, d]]`, as (low, high, two):
    /// each a ratio of exact integer sums, converted to f64 once.
    fn exact(bin: &[Vec<u128>], a: u64, b: u64, c: u64, d: u64) -> (f64, f64, f64) {
        let (r1, r2, c1) = ((a + b) as usize, (c + d) as usize, (a + c) as usize);
        let lo = c1.saturating_sub(r2);
        let hi = r1.min(c1);
        let w = |x: usize| bin[r1][x] * bin[r2][c1 - x];
        let den = bin[r1 + r2][c1];
        let wa = w(a as usize);
        let (mut low, mut high, mut two) = (0u128, 0u128, 0u128);
        for x in lo..=hi {
            let wx = w(x);
            if x <= a as usize {
                low += wx;
            }
            if x >= a as usize {
                high += wx;
            }
            if wx <= wa {
                two += wx;
            }
        }
        let r = |num: u128| num as f64 / den as f64;
        (r(low), r(high), r(two))
    }

    fn assert_close(got: f64, want: f64, what: &str) {
        let rel = if want == 0.0 {
            got.abs()
        } else {
            ((got - want) / want).abs()
        };
        assert!(rel < 1e-9, "{what}: got {got:e}, exact {want:e}");
    }

    // Every table with row totals up to 30 + 30, against exact binomials.
    #[test]
    fn fisher_matches_exact_binomials_up_to_30_plus_30() {
        let bin = binomials();
        let mut tables = 0u64;
        for r1 in 0..=MAX_ROW {
            for r2 in 0..=MAX_ROW {
                for a in 0..=r1 {
                    for c in 0..=r2 {
                        let (b, d) = (r1 - a, r2 - c);
                        let (low, high, two) = exact(&bin, a, b, c, d);
                        let t = format!("[[{a}, {b}], [{c}, {d}]]");
                        assert_close(fisher_low(a, b, c, d), low, &format!("low {t}"));
                        assert_close(fisher_high(a, b, c, d), high, &format!("high {t}"));
                        assert_close(fisher_two(a, b, c, d), two, &format!("two {t}"));
                        tables += 1;
                    }
                }
            }
        }
        // (1 + 2 + ... + 31)² tables.
        assert_eq!(tables, 496 * 496);
    }

    // The ADR's "class removed" figures at 30 boots per arm, the new arm in
    // row 1: 4 → 0 is not removed, 5 → 0 and 9 → 0 are.
    #[test]
    fn class_removed_figures_from_the_adr() {
        let removed = |prev: u64| fisher_low(0, 30, prev, 30 - prev);
        assert_eq!(format!("{:.4}", removed(4)), "0.0562");
        assert_eq!(format!("{:.4}", removed(5)), "0.0261");
        assert_eq!(format!("{:.4}", removed(9)), "0.0010");
        assert!(removed(4) >= 0.05);
        assert!(removed(5) < 0.05);
        // The mirror image: the class appears in the new arm.
        assert_eq!(fisher_high(5, 25, 0, 30), removed(5));
    }

    // Developer guide §5.6: 6/20 vs 12/20 is not significant, 6/20 vs 18/20
    // is (two-sided).
    #[test]
    fn two_sided_figures_from_the_developer_guide() {
        assert_eq!(format!("{:.3}", fisher_two(6, 14, 12, 8)), "0.111");
        assert_eq!(format!("{:.3}", fisher_two(12, 8, 6, 14)), "0.111");
        assert_eq!(format!("{:.6}", fisher_two(6, 14, 18, 2)), "0.000244");
    }

    // Degenerate margins: an empty table, an empty row, a full column.
    #[test]
    fn fisher_on_degenerate_tables_is_one() {
        for (a, b, c, d) in [(0, 0, 0, 0), (0, 0, 3, 4), (5, 0, 7, 0), (0, 5, 0, 7)] {
            assert_eq!(fisher_low(a, b, c, d), 1.0);
            assert_eq!(fisher_high(a, b, c, d), 1.0);
            assert_eq!(fisher_two(a, b, c, d), 1.0);
        }
    }

    // Values printed by the former script's wilson.
    #[test]
    fn wilson_matches_awk() {
        assert_eq!(wilson(0, 0), "n/a");
        assert_eq!(wilson(0, 5), "0% (95% Wilson interval 0%-43%)");
        assert_eq!(wilson(1, 8), "12% (95% Wilson interval 2%-47%)");
        assert_eq!(wilson(3, 8), "38% (95% Wilson interval 14%-69%)");
        assert_eq!(wilson(5, 5), "100% (95% Wilson interval 57%-100%)");
        assert_eq!(wilson(6, 20), "30% (95% Wilson interval 15%-52%)");
        assert_eq!(wilson(12, 20), "60% (95% Wilson interval 39%-78%)");
    }
}
