//! awk value semantics that the soak classifier depends on, as the former
//! `scripts/soak-qemu.sh` ran them: one-true-awk (macOS `awk version 20200816`)
//! under `LC_ALL=C`. mawk, gawk and original-awk print the same classifier line
//! for every corpus case (checked on Ubuntu 26.04, 2026-09-29).
//!
//! - String to number (`s + 0`): C `atof` on the longest numeric prefix: leading
//!   white space, a sign, then `inf`/`infinity`/`nan`, a hexadecimal number, or a
//!   decimal one; 0 when there is no prefix. The result is never `-0`, because
//!   awk's `+ 0` turns `-0` into `0`.
//! - Number to string: an integral value prints with `%.30g`, any other with
//!   `%.6g` (the classifier never changes CONVFMT or OFMT).
//! - `length`, `substr`, `index` and the regexes count bytes.
//!
//! Accepted divergences: a hexadecimal mantissa longer than 13 digits can round
//! differently (it is accumulated in an f64; strtod rounds once), and a NaN
//! always prints as `nan` (glibc's printf prints `-nan` for a negative one).
//! Neither reaches the classifier from a harness-written footer.

/// The value of the awk expression `s + 0`.
pub fn to_num(s: &[u8]) -> f64 {
    let mut i = 0;
    while i < s.len() && matches!(s[i], b' ' | b'\t' | b'\n' | 0x0b | 0x0c | b'\r') {
        i += 1;
    }
    let mut negative = false;
    if i < s.len() && (s[i] == b'+' || s[i] == b'-') {
        negative = s[i] == b'-';
        i += 1;
    }
    let rest = &s[i..];
    let magnitude = special(rest)
        .or_else(|| hexadecimal(rest))
        .unwrap_or_else(|| decimal(rest));
    let value = if negative { -magnitude } else { magnitude };
    value + 0.0
}

/// `inf`, `infinity` and `nan` (with an optional `(...)`), case-insensitive.
fn special(s: &[u8]) -> Option<f64> {
    let lower: Vec<u8> = s.iter().take(3).map(u8::to_ascii_lowercase).collect();
    match lower.as_slice() {
        b"inf" => Some(f64::INFINITY),
        b"nan" => Some(f64::NAN),
        _ => None,
    }
}

/// A C99 hexadecimal floating constant: `0x`, hex digits with an optional point,
/// then an optional binary exponent `p[+-]digits`. `None` when `0x` is not
/// followed by a hex digit (strtod then reads just the `0`).
fn hexadecimal(s: &[u8]) -> Option<f64> {
    if s.len() < 2 || s[0] != b'0' || (s[1] != b'x' && s[1] != b'X') {
        return None;
    }
    let mut i = 2;
    let mut mantissa = 0.0f64;
    let mut digits = 0usize;
    let mut fraction_digits = 0i64;
    let mut seen_point = false;
    while i < s.len() {
        let c = s[i];
        if c == b'.' && !seen_point {
            seen_point = true;
        } else if let Some(d) = (c as char).to_digit(16) {
            mantissa = mantissa * 16.0 + f64::from(d);
            digits += 1;
            if seen_point {
                fraction_digits += 1;
            }
        } else {
            break;
        }
        i += 1;
    }
    if digits == 0 {
        return None;
    }
    let mut exponent = -4 * fraction_digits;
    if i < s.len() && (s[i] == b'p' || s[i] == b'P') {
        let mut j = i + 1;
        let mut exp_negative = false;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            exp_negative = s[j] == b'-';
            j += 1;
        }
        let exp_start = j;
        let mut value: i64 = 0;
        while j < s.len() && s[j].is_ascii_digit() {
            value = value
                .saturating_mul(10)
                .saturating_add(i64::from(s[j] - b'0'));
            j += 1;
        }
        if j > exp_start {
            exponent += if exp_negative { -value } else { value };
        }
    }
    let exponent = i32::try_from(exponent.clamp(-100_000, 100_000)).expect("clamped exponent");
    Some(mantissa * 2f64.powi(exponent))
}

/// The longest prefix of the form `digits [. digits] [e [+-] digits]` with at
/// least one digit in the mantissa, parsed with correct rounding; 0 when none.
fn decimal(s: &[u8]) -> f64 {
    let mut i = 0;
    let mut mantissa_digits = 0;
    while i < s.len() && s[i].is_ascii_digit() {
        i += 1;
        mantissa_digits += 1;
    }
    if i < s.len() && s[i] == b'.' {
        let mut j = i + 1;
        let mut fraction = 0;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
            fraction += 1;
        }
        if mantissa_digits + fraction > 0 {
            i = j;
            mantissa_digits += fraction;
        }
    }
    if mantissa_digits == 0 {
        return 0.0;
    }
    if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
        let mut j = i + 1;
        if j < s.len() && (s[j] == b'+' || s[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < s.len() && s[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    let text = std::str::from_utf8(&s[..i]).expect("ASCII digits");
    text.parse::<f64>().expect("a valid decimal prefix")
}

/// awk's number-to-string conversion (CONVFMT, and OFMT in `print`).
pub fn num_str(v: f64) -> String {
    if v.is_nan() {
        "nan".to_string()
    } else if v.is_infinite() {
        if v > 0.0 { "inf" } else { "-inf" }.to_string()
    } else if v.fract() == 0.0 {
        fmt_g(v, 30)
    } else {
        fmt_g(v, 6)
    }
}

/// C `printf("%.<precision>g", v)` for a finite `v`.
pub fn fmt_g(v: f64, precision: usize) -> String {
    let p = precision.max(1);
    let scientific = format!("{:.*e}", p - 1, v);
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("Rust's {:e} always has an exponent");
    let x: i64 = exponent.parse().expect("an integer exponent");
    if x < -4 || x >= p as i64 {
        let sign = if x < 0 { '-' } else { '+' };
        format!("{}e{sign}{:02}", strip_zeros(mantissa), x.abs())
    } else {
        let decimals = usize::try_from(p as i64 - 1 - x).expect("non-negative in this branch");
        strip_zeros(&format!("{v:.decimals$}")).to_string()
    }
}

/// Drop the trailing zeros of a fraction, and the point if nothing is left after it.
fn strip_zeros(s: &str) -> &str {
    if s.contains('.') {
        s.trim_end_matches('0').trim_end_matches('.')
    } else {
        s
    }
}

/// `clip(s, n)`: the first `n` bytes plus `...` when `s` is longer than `n` bytes.
pub fn clip(s: &[u8], n: usize) -> Vec<u8> {
    if s.len() > n {
        let mut out = s[..n].to_vec();
        out.extend_from_slice(b"...");
        out
    } else {
        s.to_vec()
    }
}

/// `trim(s)`: tabs become spaces, then leading and trailing spaces go.
pub fn trim(s: &[u8]) -> Vec<u8> {
    let spaced: Vec<u8> = s
        .iter()
        .map(|&b| if b == b'\t' { b' ' } else { b })
        .collect();
    let start = spaced
        .iter()
        .position(|&b| b != b' ')
        .unwrap_or(spaced.len());
    let end = spaced
        .iter()
        .rposition(|&b| b != b' ')
        .map_or(start, |p| p + 1);
    spaced[start..end].to_vec()
}

/// The fields of `line` under awk's default field splitting: runs of blanks
/// (space, tab) separate fields, and leading and trailing blanks are ignored.
pub fn fields(line: &[u8]) -> impl Iterator<Item = &[u8]> {
    line.split(|&b| b == b' ' || b == b'\t')
        .filter(|f| !f.is_empty())
}

/// Whether `haystack` contains `needle` (awk `index(...) > 0`, or a literal regex).
pub fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

/// The byte offset of the first `needle` in `haystack`.
pub fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `printf '%.6g'` and `printf '%.30g'` (C) for each value.
    const FMT_G: &[(f64, &str, &str)] = &[
        (0.0, "0", "0"),
        (0.5, "0.5", "0.5"),
        (1.5, "1.5", "1.5"),
        (2.5, "2.5", "2.5"),
        (0.1, "0.1", "0.100000000000000005551115123126"),
        (0.3, "0.3", "0.299999999999999988897769753748"),
        (
            0.30000000000000004,
            "0.3",
            "0.300000000000000044408920985006",
        ),
        (
            0.3333333333333333,
            "0.333333",
            "0.333333333333333314829616256247",
        ),
        (
            33.333333333333336,
            "33.3333",
            "33.3333333333333357018091192003",
        ),
        (
            1.2345e-05,
            "1.2345e-05",
            "1.2344999999999999541950602977e-05",
        ),
        (1e-05, "1e-05", "1.00000000000000008180305391403e-05"),
        (0.0001, "0.0001", "0.000100000000000000004792173602386"),
        (
            0.00012345678,
            "0.000123457",
            "0.000123456780000000005542704073491",
        ),
        (123456.5, "123456", "123456.5"),
        (123457.5, "123458", "123457.5"),
        (999999.5, "1e+06", "999999.5"),
        (999999.4, "999999", "999999.400000000023283064365387"),
        (1234567.0, "1.23457e+06", "1234567"),
        (1e+20, "1e+20", "100000000000000000000"),
        (1e+30, "1e+30", "1.00000000000000001988462483866e+30"),
        (1e+31, "1e+31", "9.99999999999999963589629496525e+30"),
        (2.5e-300, "2.5e-300", "2.49999999999999997975726900344e-300"),
        (-0.5, "-0.5", "-0.5"),
        (-123.456, "-123.456", "-123.456000000000003069544618484"),
        (
            5e-324,
            "4.94066e-324",
            "4.94065645841246544176568792868e-324",
        ),
        (
            1.7976931348623157e+308,
            "1.79769e+308",
            "1.79769313486231570814527423732e+308",
        ),
        (62.5, "62.5", "62.5"),
        (1234.5678, "1234.57", "1234.56780000000003383320290595"),
        (100.0, "100", "100"),
        (99.99995, "99.9999", "99.9999499999999983401721692644"),
    ];

    /// `awk -v s=S 'BEGIN { x = s + 0; print x "" }'` under `LC_ALL=C` (macOS awk 20200816).
    const TO_NUM: &[(&[u8], &str)] = &[
        (b"0x10", "16"),
        (b"inf", "inf"),
        (b"+inf", "inf"),
        (b"-inf", "-inf"),
        (b"nan", "nan"),
        (b" 12abc", "12"),
        (b".5e1x", "5"),
        (b"1e", "1"),
        (b"1e+", "1"),
        (b"-", "0"),
        (b"", "0"),
        (b"  -3.5", "-3.5"),
        (b"1.", "1"),
        (b".e5", "0"),
        (b"12 34", "12"),
        (b"1e400", "inf"),
        (b"-0", "0"),
        (b"00012", "12"),
        (b"+.5", "0.5"),
        (b"0x", "0"),
        (b"0x.8", "0.5"),
        (b"0x1p4", "16"),
        (b"0X1.8P1", "3"),
        (b"0xg", "0"),
        (b"INFINITY", "inf"),
        (b"info", "inf"),
        (b"1.5e3", "1500"),
        (b"  +7e-2", "0.07"),
        (b"99999999999999999999", "100000000000000000000"),
        (b"3.0", "3"),
        (b"12.50", "12.5"),
        (b"0.1e1", "1"),
        (b"e5", "0"),
        (b"+-1", "0"),
        (b"1e-400", "0"),
        (b"\x099", "9"),
        (b"\x0b8", "8"),
    ];

    #[test]
    fn fmt_g_matches_c_printf() {
        for &(v, g6, g30) in FMT_G {
            assert_eq!(fmt_g(v, 6), g6, "%.6g of {v:e}");
            assert_eq!(fmt_g(v, 30), g30, "%.30g of {v:e}");
        }
    }

    #[test]
    fn to_num_then_num_str_matches_awk() {
        for &(s, want) in TO_NUM {
            assert_eq!(
                num_str(to_num(s)),
                want,
                "{:?} + 0",
                String::from_utf8_lossy(s)
            );
        }
    }

    #[test]
    fn num_str_prints_integral_values_exactly_and_others_with_six_digits() {
        assert_eq!(num_str(3.0), "3");
        assert_eq!(num_str(-1.0), "-1");
        assert_eq!(num_str(1e20), "100000000000000000000");
        assert_eq!(num_str(0.1 + 0.2), "0.3");
        assert_eq!(num_str(100.0 / 3.0), "33.3333");
        assert_eq!(num_str(f64::INFINITY), "inf");
        assert_eq!(num_str(f64::NEG_INFINITY), "-inf");
        assert_eq!(num_str(f64::NAN), "nan");
    }

    #[test]
    fn to_num_never_returns_negative_zero() {
        assert!(to_num(b"-0").is_sign_positive());
        assert!(to_num(b"-").is_sign_positive());
        assert!(to_num(b"-0x0").is_sign_positive());
    }

    #[test]
    fn clip_counts_bytes_and_can_split_a_character() {
        assert_eq!(clip(b"abc", 3), b"abc");
        assert_eq!(clip(b"abcd", 3), b"abc...");
        assert_eq!(clip("a\u{e9}".as_bytes(), 2), b"a\xc3...");
        assert_eq!(clip(b"", 0), b"");
    }

    #[test]
    fn trim_turns_tabs_into_spaces_and_strips_only_spaces() {
        assert_eq!(trim(b"  a\tb  "), b"a b");
        assert_eq!(trim(b"\ta\t"), b"a");
        assert_eq!(trim(b"\x0ba "), b"\x0ba");
        assert_eq!(trim(b"   "), b"");
    }

    #[test]
    fn fields_split_on_runs_of_blanks() {
        let f: Vec<&[u8]> = fields(b"  [soak] meta\t a=1  b=  ").collect();
        assert_eq!(f, vec![&b"[soak]"[..], b"meta", b"a=1", b"b="]);
        assert_eq!(fields(b"").count(), 0);
    }

    #[test]
    fn find_and_contains_work_on_bytes() {
        assert_eq!(find(b"xxPANIC: y", b"PANIC: "), Some(2));
        assert_eq!(find(b"abc", b"abcd"), None);
        assert!(contains(b"a\xffb", b"\xffb"));
    }
}
