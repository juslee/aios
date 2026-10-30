//! The kernel's tripwire output in a serial log (crash-fix step 1a): the
//! `[tripwire]` lines and the `[tripwire-ev]` lock event lines of step 1b.
//!
//! The format and the parser contract are `docs/kernel/observability.md`
//! §6.5 and the module docs of `shared::tripwire`. A `v=2` line is decoded
//! with `shared`'s key catalogue ([`Key::ALL`]), so a key the kernel adds
//! needs no edit here; a `v=1` line, with the frozen catalogue of
//! [`super::schema_v1`] (see [`super::catalogue`]).
//!
//! - A line contributes from its first `[tripwire] `, since a line printed
//!   after another CPU's partial output starts mid-line. `[tripwire-ev]` never
//!   matches.
//! - It is complete when every token after `[tripwire]` is `key=value` (both
//!   non-empty), the last is `n=N`, and N is the number of `key=value` tokens
//!   before `n=`: the prefix (`v`, `src`, `cpu`, `t`, `ncpu`) included,
//!   `[tripwire]` itself and `n=` excluded.
//! - The last complete line of a log wins, whatever its `src` or schema
//!   version ([`LastLines::last`]), and so does the last complete `src=g1`
//!   line ([`LastLines::g1`]).
//! - A `v=1` or `v=2` line is decoded ([`Decoded`]), each with its own
//!   catalogue; a key missing from it is 0. A line of any other schema version
//!   is kept whole, undecoded. Value counts are not checked against key
//!   widths: the contract does not ask for it, and a check could only turn
//!   data into nothing.
//!
//! Nothing here guesses: a torn line is incomplete, and the line before it
//! stands.

use shared::tripwire::{Ctx, Key, LineSrc, LINE_PREFIX};

use super::awk::{fields, find};
use super::catalogue::Schema;

/// The number of prefix tokens of a `v=1` or `v=2` line: `v`, `src`, `cpu`, `t` and `ncpu`.
pub const PREFIX_TOKENS: usize = 5;

/// The marker of a `[tripwire-ev]` lock event line.
const EVENT_PREFIX: &[u8] = b"[tripwire-ev] ";

/// A complete `[tripwire]` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    /// The whole line from `[tripwire]` on, trailing blanks dropped.
    pub text: Vec<u8>,
    /// The `v=` value, `None` when the line has no `v=` token.
    pub version: Option<Vec<u8>>,
    /// The `src=` value, `None` when the line has no `src=` token.
    pub src: Option<Vec<u8>>,
    /// The decoded fields of a `v=1` or `v=2` line; `None` for any other
    /// version.
    pub decoded: Option<Decoded>,
}

/// A decoded `v=1` or `v=2` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded {
    /// The schema the line was decoded with.
    pub schema: Schema,
    /// `cpu=`: the CPU that printed the line.
    pub cpu: Option<Vec<u8>>,
    /// `t=`: CPU 0's tick when the line was printed.
    pub t: Option<Vec<u8>>,
    /// `ncpu=`: the online CPUs the per-CPU keys cover.
    pub ncpu: Option<Vec<u8>>,
    /// One value per key of the schema, in its order ([`Schema::keys`]): the
    /// comma list as printed, or `0` for a key the line leaves out.
    values: Vec<Vec<u8>>,
}

impl Decoded {
    /// The value of the key at position `i` of [`Schema::keys`]: the comma
    /// list as printed, or `0` when the line left it out.
    pub fn value_at(&self, i: usize) -> &[u8] {
        self.values.get(i).map_or(&b"0"[..], Vec::as_slice)
    }

    /// The value of key `name`, `None` when the line's schema has no such key.
    pub fn get(&self, name: &str) -> Option<&[u8]> {
        self.schema
            .position(name.as_bytes())
            .map(|i| self.value_at(i))
    }

    /// `key`'s value: the comma list as printed, or `0` when the line left it
    /// out (or its schema has no such key).
    pub fn value(&self, key: Key) -> &[u8] {
        self.get(key.name()).unwrap_or(b"0")
    }
}

/// The `key=value` split of a token, `None` unless both sides are non-empty.
fn key_value(token: &[u8]) -> Option<(&[u8], &[u8])> {
    let eq = token.iter().position(|&b| b == b'=')?;
    let (key, value) = (&token[..eq], &token[eq + 1..]);
    (!key.is_empty() && !value.is_empty()).then_some((key, value))
}

/// The complete `[tripwire]` line in `line`, from its first `[tripwire] `, or
/// `None` when it has none or the line is incomplete.
pub fn parse_line(line: &[u8]) -> Option<Line> {
    let marker = [LINE_PREFIX.as_bytes(), b" "].concat();
    let start = find(line, &marker)?;
    let rest = &line[start + marker.len()..];
    let mut pairs: Vec<(&[u8], &[u8])> = Vec::new();
    for token in fields(rest) {
        pairs.push(key_value(token)?);
    }
    let (last, before) = pairs.split_last()?;
    let n: usize = match *last {
        (b"n", v) if v.iter().all(u8::is_ascii_digit) => {
            std::str::from_utf8(v).ok()?.parse().ok()?
        }
        _ => return None,
    };
    if n != before.len() {
        return None;
    }
    let get = |name: &str| {
        before
            .iter()
            .find(|(k, _)| *k == name.as_bytes())
            .map(|(_, v)| v.to_vec())
    };
    let version = get("v");
    let decoded = version
        .as_deref()
        .and_then(Schema::of_version)
        .map(|schema| {
            let mut values = vec![b"0".to_vec(); schema.keys().len()];
            for (k, v) in before {
                if let Some(slot) = schema.position(k).and_then(|i| values.get_mut(i)) {
                    *slot = v.to_vec();
                }
            }
            Decoded {
                schema,
                cpu: get("cpu"),
                t: get("t"),
                ncpu: get("ncpu"),
                values,
            }
        });
    let end = line
        .iter()
        .rposition(|&b| b != b' ' && b != b'\t')
        .map_or(start, |p| p + 1);
    Some(Line {
        text: line[start..end].to_vec(),
        version,
        src: get("src"),
        decoded,
    })
}

/// The last complete `[tripwire]` lines of a log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LastLines {
    /// The last complete line, whatever its `src` and version.
    pub last: Option<Line>,
    /// The last complete `src=g1` line (the end of the Gate 1 bench).
    pub g1: Option<Line>,
}

impl LastLines {
    /// Take one log line into account.
    pub fn observe(&mut self, line: &[u8]) {
        if let Some(l) = parse_line(line) {
            if l.src.as_deref() == Some(LineSrc::G1.name().as_bytes()) {
                self.g1 = Some(l.clone());
            }
            self.last = Some(l);
        }
    }
}

/// The `[tripwire-ev]` lock event lines of a log, counted by `kind=`.
///
/// The kinds are those of `EventKind` in `kernel/src/sync/irq_spin_lock.rs`:
/// `ph` (a preempted holder met with IRQs masked), `self` (the holder is the
/// waiter's own thread) and `stuck` (a wait of more than 2 s).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EventCounts {
    /// `kind=ph`.
    pub ph: u64,
    /// `kind=self`, every context.
    pub self_held: u64,
    /// `kind=self` with `ctx=irq` or `ctx=irq-exit`: the crash-fix ADR's H3
    /// evidence.
    pub self_irq: u64,
    /// `kind=stuck`.
    pub stuck: u64,
}

impl EventCounts {
    /// Count the events in one log line. Each `[tripwire-ev] ` starts an
    /// event, so two CPUs' events on one line count twice. An event counts
    /// only when the first token after the marker is a known `kind=`, and a
    /// `self` event counts as IRQ context only when its own `ctx=` token reads
    /// so: the fifth token (`kind`, `cpu`, `lock`, `idx`, `ctx`, the order
    /// `irq_spin_lock.rs` prints them in), never a later `ctx=` from another
    /// CPU's line (`[panic] ... ctx=irq-exit`) that landed on the same line.
    pub fn observe(&mut self, line: &[u8]) {
        let mut rest = line;
        while let Some(p) = find(rest, EVENT_PREFIX) {
            rest = &rest[p + EVENT_PREFIX.len()..];
            let segment = find(rest, EVENT_PREFIX).map_or(rest, |q| &rest[..q]);
            let mut tokens = fields(segment);
            match tokens.next() {
                Some(b"kind=ph") => self.ph += 1,
                Some(b"kind=stuck") => self.stuck += 1,
                Some(b"kind=self") => {
                    self.self_held += 1;
                    let irq = tokens.nth(3).is_some_and(|t| {
                        [Ctx::Irq, Ctx::IrqExit]
                            .iter()
                            .any(|c| t == [b"ctx=", c.name().as_bytes()].concat())
                    });
                    if irq {
                        self.self_irq += 1;
                    }
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::tripwire::Width;
    use std::path::Path;

    /// The Rust string literal `const <name>: &str = "...";` in
    /// `shared/src/tripwire.rs`'s tests, decoded: the fixtures stay where the
    /// kernel's own golden tests pin them, with no copy here.
    fn shared_golden(name: &str) -> String {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../shared/src/tripwire.rs");
        let text = std::fs::read_to_string(&path).expect("shared/src/tripwire.rs");
        let decl = format!("const {name}: &str =");
        let start = text.find(&decl).expect("the golden's declaration") + decl.len();
        let open = start
            + text[start..]
                .find('"')
                .expect("the literal's opening quote")
            + 1;
        let mut out = String::new();
        let mut chars = text[open..].chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => return out,
                '\\' => match chars.next().expect("an escape") {
                    'n' => out.push('\n'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    // A line continuation: the newline and the next line's
                    // leading whitespace go.
                    '\n' => {
                        while chars.peek().is_some_and(|c| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    other => panic!("an escape this decoder does not know: \\{other}"),
                },
                c => out.push(c),
            }
        }
        panic!("{name}: no closing quote");
    }

    fn parse(s: &str) -> Option<Line> {
        parse_line(s.as_bytes())
    }

    /// A golden as a log record: the classifier splits at `\n`, so no
    /// record holds one.
    fn record(golden: &str) -> &str {
        golden
            .strip_suffix('\n')
            .expect("a golden ends with its newline")
    }

    fn text(v: &[u8]) -> &str {
        std::str::from_utf8(v).expect("ASCII")
    }

    /// The tokens after `[tripwire]`, as `(key, value)`.
    fn pairs(line: &str) -> Vec<(String, String)> {
        line.split_whitespace()
            .skip(1)
            .map(|t| {
                let (k, v) = t.split_once('=').expect("key=value");
                (k.to_string(), v.to_string())
            })
            .collect()
    }

    /// `v=1`'s `GOLDEN_FULL` and `GOLDEN_NONZERO` as of capability-lifetime
    /// step 10 (`9498a9a`, the last commit that printed `v=1`): 16-wide
    /// per-source keys and 9-wide per-lock keys.
    const V1_GOLDEN_FULL: &str = include_str!("../../../tests/fixtures/soak/tripwire-v1-full.txt");
    const V1_GOLDEN_NONZERO: &str =
        include_str!("../../../tests/fixtures/soak/tripwire-v1-nonzero.txt");

    /// The `n=` of a line with the prefix and every key: the 5 prefix tokens
    /// and the 64 keys.
    const FULL_N: usize = 69;

    /// A full golden of `schema`, parsed: its keys are `schema`'s in order,
    /// and each decodes to its printed value.
    fn check_full_golden(golden: &str, schema: Schema, version: &str) {
        let tokens = pairs(golden);
        assert_eq!(tokens.last(), Some(&("n".to_string(), FULL_N.to_string())));
        assert_eq!(PREFIX_TOKENS + schema.keys().len(), FULL_N);
        let names: Vec<&str> = tokens[PREFIX_TOKENS..tokens.len() - 1]
            .iter()
            .map(|(k, _)| k.as_str())
            .collect();
        let catalogue: Vec<&str> = schema.keys().iter().map(|k| k.name).collect();
        assert_eq!(names, catalogue, "the golden's keys against the catalogue");

        let line = parse(record(golden)).expect("the golden is complete");
        assert_eq!(text(&line.text), record(golden));
        assert_eq!(text(line.version.as_deref().expect("v")), version);
        assert_eq!(line.src.as_deref(), Some(&b"g1"[..]));
        let d = line.decoded.expect("the line is decoded");
        assert_eq!(d.schema, schema);
        assert_eq!(d.cpu.as_deref(), Some(&b"0"[..]));
        assert_eq!(d.t.as_deref(), Some(&b"12000"[..]));
        assert_eq!(d.ncpu.as_deref(), Some(&b"4"[..]));
        for (i, (key, (_, value))) in schema
            .keys()
            .iter()
            .zip(&tokens[PREFIX_TOKENS..])
            .enumerate()
        {
            assert_eq!(text(d.value_at(i)), value, "{}", key.name);
            assert_eq!(d.get(key.name).map(text), Some(value.as_str()));
        }
    }

    /// The number of values `name` has on the full golden.
    fn width_of(golden: &str, name: &str) -> usize {
        pairs(golden)
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.split(',').count())
            .expect("the key is on the line")
    }

    #[test]
    fn the_full_v2_golden_parses_complete_with_every_key_in_catalogue_order() {
        let golden = shared_golden("GOLDEN_FULL");
        check_full_golden(&golden, Schema::V2, "2");
        // `shared`'s own catalogue, which a `v=2` line is decoded with.
        let names: Vec<&str> = Key::ALL.iter().map(|k| k.name()).collect();
        let schema: Vec<&str> = Schema::V2.keys().iter().map(|k| k.name).collect();
        assert_eq!(schema, names);
        assert_eq!(PREFIX_TOKENS + Key::COUNT, FULL_N);
        // 12 sources and 8 locks.
        assert_eq!(width_of(&golden, "ubrun"), 12);
        assert_eq!(width_of(&golden, "ubpend"), 12);
        assert_eq!(width_of(&golden, "lkph"), 8);
        let line = parse(record(&golden)).expect("complete");
        let d = line.decoded.expect("decoded");
        assert_eq!(
            text(d.value(Key::Lkstk)),
            "3600,3601,3602,3603,3604,3605,3606,3607"
        );
    }

    #[test]
    fn the_full_v1_golden_parses_complete_with_every_key_in_the_frozen_catalogue_order() {
        check_full_golden(V1_GOLDEN_FULL, Schema::V1, "1");
        // 16 sources and 9 locks.
        assert_eq!(width_of(V1_GOLDEN_FULL, "ubrun"), 16);
        assert_eq!(width_of(V1_GOLDEN_FULL, "ubtok"), 16);
        assert_eq!(width_of(V1_GOLDEN_FULL, "lkph"), 9);
        let line = parse(record(V1_GOLDEN_FULL)).expect("complete");
        let d = line.decoded.expect("decoded");
        assert_eq!(
            text(d.value(Key::Lkstk)),
            "3600,3601,3602,3603,3604,3605,3606,3607,3608"
        );
    }

    /// A nonzero golden of `schema`: nine keys printed, the rest read 0.
    fn check_nonzero_golden(golden: &str, schema: Schema, version: &str, ubrun: &str, lkph: &str) {
        // n=14: the 5 prefix tokens and 9 keys.
        assert_eq!(pairs(golden).len(), PREFIX_TOKENS + 9 + 1);
        assert!(golden.trim_end().ends_with(" n=14"));
        let line = parse(record(golden)).expect("the golden is complete");
        assert_eq!(line.src.as_deref(), Some(&b"hb"[..]));
        assert_eq!(text(line.version.as_deref().expect("v")), version);
        let d = line.decoded.expect("decoded");
        assert_eq!(d.schema, schema);
        assert_eq!(text(d.value(Key::Tick)), "12001,11890,11875,11902");
        assert_eq!(text(d.value(Key::Ubrun)), ubrun);
        assert_eq!(text(d.value(Key::Lkph)), lkph);
        assert_eq!(text(d.value(Key::Badchan)), "4,0,6");
        assert_eq!(text(d.value(Key::Twmax)), "94000");
        assert_eq!(text(d.value(Key::Elrmm)), "0");
        assert_eq!(text(d.value(Key::Lkself)), "0");
        let printed: Vec<String> = pairs(golden).into_iter().map(|(k, _)| k).collect();
        let missing = schema
            .keys()
            .iter()
            .filter(|k| !printed.iter().any(|p| p == k.name))
            .count();
        assert_eq!(missing, schema.keys().len() - 9);
    }

    #[test]
    fn the_nonzero_v2_golden_parses_and_its_missing_keys_read_0() {
        check_nonzero_golden(
            &shared_golden("GOLDEN_NONZERO"),
            Schema::V2,
            "2",
            "0,3,0,0,0,0,0,0,0,0,0,0",
            "2,0,0,0,0,0,0,0",
        );
    }

    #[test]
    fn the_nonzero_v1_golden_parses_and_its_missing_keys_read_0() {
        check_nonzero_golden(
            V1_GOLDEN_NONZERO,
            Schema::V1,
            "1",
            "0,3,0,0,0,0,0,0,0,0,0,0,0,0,0,0",
            "2,0,0,0,0,0,0,0,0",
        );
    }

    /// A `v=1` line from before capability-lifetime steps 8 to 10 (the `main`
    /// arm's): 15-wide sources, no `ubtok`, `ubpend` or `bw`.
    const OLD_V1: &str = "[tripwire] v=1 src=hb cpu=0 t=9000 ncpu=4 \
        ubrun=0,0,0,0,0,0,0,0,0,0,0,0,0,0,7 lkph=0,0,0,0,0,0,0,0,5 twc=3 n=8";

    #[test]
    fn an_older_v1_line_with_shorter_sources_and_without_late_keys_decodes() {
        let d = parse(OLD_V1).expect("complete").decoded.expect("decoded");
        assert_eq!(d.schema, Schema::V1);
        assert_eq!(text(d.value(Key::Ubrun)), "0,0,0,0,0,0,0,0,0,0,0,0,0,0,7");
        // Index 14 is `cancel` in v=1; the missing index 15 (`selftest`) is
        // not on the line and the key `ubtok` is not on it either.
        assert_eq!(Schema::V1.index_name(Width::Source, 14), "cancel");
        assert_eq!(text(d.value(Key::Lkph)), "0,0,0,0,0,0,0,0,5");
        assert_eq!(Schema::V1.index_name(Width::Lock, 8), "BOOT_LOG");
        for late in [Key::Ubtok, Key::Ubpend, Key::Bw, Key::Xdir] {
            assert_eq!(text(d.value(late)), "0", "{}", late.name());
        }
    }

    /// The same boot's counters as the one in [`OLD_V1`], as `v=2` prints
    /// them: `cancel` is index 10, `BOOT_LOG` index 7.
    const NEW_V2: &str = "[tripwire] v=2 src=hb cpu=0 t=9500 ncpu=4 \
        ubrun=0,0,0,0,0,0,0,0,0,0,7,0 lkph=0,0,0,0,0,0,0,5 twc=3 n=8";

    #[test]
    fn a_v2_line_is_decoded_with_the_current_catalogue() {
        let d = parse(NEW_V2).expect("complete").decoded.expect("decoded");
        assert_eq!(d.schema, Schema::V2);
        assert_eq!(text(d.value(Key::Ubrun)), "0,0,0,0,0,0,0,0,0,0,7,0");
        assert_eq!(text(d.value(Key::Lkph)), "0,0,0,0,0,0,0,5");
        assert_eq!(Schema::V2.index_name(Width::Source, 10), "cancel");
        assert_eq!(Schema::V2.index_name(Width::Lock, 7), "BOOT_LOG");
        assert_eq!(text(d.value(Key::Twc)), "3");
        // A key the line leaves out reads 0, a trailing index too.
        assert_eq!(text(d.value(Key::Bw)), "0");
    }

    #[test]
    fn a_v1_and_a_v2_line_in_one_log_both_decode_with_their_own_catalogue() {
        let mut last = LastLines::default();
        let g1 = OLD_V1.replace("src=hb", "src=g1");
        for line in [g1.as_str(), NEW_V2] {
            last.observe(line.as_bytes());
        }
        // The last complete line wins, and the g1 line is the earlier v=1 one.
        let (v2, v1) = (
            last.last.expect("last").decoded.expect("v=2 decoded"),
            last.g1.expect("g1").decoded.expect("v=1 decoded"),
        );
        assert_eq!((v1.schema, v2.schema), (Schema::V1, Schema::V2));
        // Each value is named by its own catalogue: 7 is `cancel` in both.
        let named = |d: &Decoded, key: &str, width: Width| -> Vec<(String, String)> {
            d.get(key)
                .expect("the schema has the key")
                .split(|&b| b == b',')
                .enumerate()
                .map(|(i, v)| (d.schema.index_name(width, i), text(v).to_string()))
                .filter(|(_, v)| v != "0")
                .collect()
        };
        let cancel = vec![("cancel".to_string(), "7".to_string())];
        assert_eq!(named(&v1, "ubrun", Width::Source), cancel);
        assert_eq!(named(&v2, "ubrun", Width::Source), cancel);
        let boot_log = vec![("BOOT_LOG".to_string(), "5".to_string())];
        assert_eq!(named(&v1, "lkph", Width::Lock), boot_log);
        assert_eq!(named(&v2, "lkph", Width::Lock), boot_log);
        // The positions differ, which is the point.
        assert_ne!(v1.get("ubrun"), v2.get("ubrun"));
    }

    const SHORT: &str = "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elrmm=1,0,0,2 twc=9 n=7";

    #[test]
    fn n_counts_the_prefix_but_not_the_marker_or_itself() {
        assert!(parse(SHORT).is_some());
        // Counting `[tripwire]`, or leaving out the 5 prefix tokens, is wrong.
        assert_eq!(parse(&SHORT.replace("n=7", "n=8")), None);
        assert_eq!(parse(&SHORT.replace("n=7", "n=2")), None);
        // So is counting `n=` itself.
        assert_eq!(parse(&SHORT.replace("n=7", "n=6")), None);
    }

    #[test]
    fn a_torn_or_malformed_line_is_incomplete() {
        for torn in [
            // Cut before its `n=`.
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elrmm=1,0,0,2",
            // Cut inside the line, so a later token is the next line's.
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elr[heartbeat] tick=6000 twc=9 n=7",
            // Other output after `n=`.
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elrmm=1,0,0,2 twc=9 n=7[heartbeat]",
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elrmm=1,0,0,2 twc=9 n=7 tick=1",
            // A token that is not key=value.
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 elrmm= twc=9 n=7",
            "[tripwire] v=1 src=hb cpu=0 t=5000 ncpu=4 =1 twc=9 n=7",
            "[tripwire] n=0x0",
            // No tokens at all.
            "[tripwire] ",
        ] {
            assert_eq!(parse(torn), None, "{torn}");
        }
    }

    #[test]
    fn an_incomplete_last_line_falls_back_to_the_previous_complete_one() {
        let mut last = LastLines::default();
        last.observe(SHORT.as_bytes());
        last.observe(b"[tripwire] v=1 src=hb cpu=0 t=6000 ncpu=4 elrmm=1,0");
        let l = last.last.expect("the earlier line stands");
        assert_eq!(
            text(l.decoded.expect("decoded").t.as_deref().expect("t")),
            "5000"
        );
        assert_eq!(last.g1, None);
    }

    #[test]
    fn a_line_that_starts_mid_line_is_accepted_and_kept_from_its_marker() {
        let l = parse(&format!("[heartbeat] tick=50{SHORT}  ")).expect("complete");
        assert_eq!(text(&l.text), SHORT);
    }

    #[test]
    fn an_event_line_is_not_a_tripwire_line() {
        assert_eq!(
            parse("[tripwire-ev] kind=ph cpu=0 lock=THREAD_TABLE idx=- ctx=irq n=0"),
            None
        );
    }

    #[test]
    fn another_schema_version_is_kept_whole_and_not_decoded() {
        // The versions on either side of the two decoded ones, and a far one.
        for version in ["0", "3", "99"] {
            let other = format!("[tripwire] v={version} src=hb cpu=0 t=5000 ncpu=4 newkey=7 n=6");
            let l = parse(&other).expect("complete");
            assert_eq!(text(&l.text), other);
            assert_eq!(text(l.version.as_deref().expect("v")), version);
            assert_eq!(l.src.as_deref(), Some(&b"hb"[..]));
            assert_eq!(l.decoded, None, "v={version}");
        }
        // A complete line without `v=` is not decoded either.
        let l = parse("[tripwire] src=hb n=1").expect("complete");
        assert_eq!((l.version, l.decoded), (None, None));
    }

    #[test]
    fn panic_and_exception_lines_count_and_the_last_g1_line_is_kept() {
        let mut last = LastLines::default();
        let g1 = SHORT.replace("src=hb", "src=g1");
        for line in [
            SHORT.to_string(),
            g1.clone(),
            SHORT.replace("src=hb", "src=exc"),
            SHORT.replace("src=hb", "src=panic"),
        ] {
            last.observe(line.as_bytes());
        }
        assert_eq!(
            last.last.and_then(|l| l.src),
            Some(LineSrc::Panic.name().as_bytes().to_vec())
        );
        assert_eq!(last.g1.map(|l| l.text), Some(g1.into_bytes()));
        let mut last = LastLines::default();
        last.observe(SHORT.replace("src=hb", "src=exc").as_bytes());
        assert_eq!(last.last.and_then(|l| l.src), Some(b"exc".to_vec()));
    }

    #[test]
    fn a_key_the_catalogue_does_not_know_is_left_to_the_whole_line() {
        for version in [1, 2] {
            let l = parse(&format!(
                "[tripwire] v={version} src=hb cpu=0 t=1 ncpu=1 zzz=4 tick=9 n=7"
            ))
            .expect("complete");
            let d = l.decoded.expect("decoded");
            assert_eq!(text(d.value(Key::Tick)), "9");
            assert_eq!(d.get("zzz"), None);
        }
        assert!(Key::ALL.iter().all(|k| k.name() != "zzz"));
    }

    #[test]
    fn a_name_the_schema_does_not_have_reads_none() {
        // The lookup is by name, so a key of another schema only is `None`.
        let d = parse(SHORT).expect("complete").decoded.expect("decoded");
        assert_eq!(d.get("twc").map(text), Some("9"));
        assert_eq!(d.get("selcall"), None);
        assert_eq!(text(d.value_at(usize::MAX)), "0");
    }

    #[test]
    fn a_log_without_a_line_has_none() {
        let mut last = LastLines::default();
        last.observe(b"[heartbeat] tick=1000");
        last.observe(b"[tripwire-ev] kind=stuck cpu=1 lock=THREAD_TABLE");
        assert_eq!(last, LastLines::default());
    }

    #[test]
    fn events_count_by_kind_and_self_by_context() {
        let ev = |kind: &str, ctx: &str| {
            format!(
                "[tripwire-ev] kind={kind} cpu=0 lock=THREAD_TABLE idx=- ctx={ctx} owner_cpu=0 \
                 owner_gen=4711 holder_tid=12 cur_tid=3 holder_running=none holder=kernel/src/cap/mod.rs:39"
            )
        };
        let mut e = EventCounts::default();
        for line in [
            ev("ph", "irq"),
            ev("ph", "thread-off"),
            ev("stuck", "thread"),
            ev("self", "irq"),
            ev("self", "irq-exit"),
            ev("self", "thread"),
            ev("self", "thread-off"),
            // Two CPUs' events on one line, the second starting mid-line.
            format!("{}{}", ev("stuck", "irq"), ev("self", "irq")),
            // Not counted: an unknown kind, a kind broken by other output,
            // and a `[tripwire]` line.
            ev("bogus", "irq"),
            "[tripwire-ev] ki[heartbeat] tick=5000".to_string(),
            SHORT.to_string(),
        ] {
            e.observe(line.as_bytes());
        }
        assert_eq!(
            e,
            EventCounts {
                ph: 2,
                self_held: 5,
                self_irq: 3,
                stuck: 2,
            }
        );
        // `self` with its ctx cut off counts, but not as IRQ context.
        let mut e = EventCounts::default();
        e.observe(b"[tripwire-ev] kind=self cpu=0 lock=THREAD_TABLE idx=- ct");
        assert_eq!((e.self_held, e.self_irq), (1, 0));
        // Another CPU's `ctx=irq-exit` on the same line is not the event's
        // own context, after a whole `ctx=thread` token or a cut-off one.
        for line in [
            format!("{} [panic] cpu=0 tid=3 ctx=irq-exit", ev("self", "thread")),
            "[tripwire-ev] kind=self cpu=1 lock=THREAD_TABLE idx=- ct[panic] cpu=0 tid=3 \
             ctx=irq-exit"
                .to_string(),
        ] {
            let mut e = EventCounts::default();
            e.observe(line.as_bytes());
            assert_eq!((e.self_held, e.self_irq), (1, 0), "{line}");
        }
    }
}
