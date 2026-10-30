//! The tripwire catalogue a line is decoded with, by schema version.
//!
//! A `v=1` line is decoded with the frozen catalogue of [`super::schema_v1`], a
//! `v=2` line with `shared`'s current one ([`Key::ALL`] and the index sets
//! behind it). Capability-lifetime step 11 shrank the wake sources from 16 to
//! 12 and the lock classes from 9 to 8, so the same index names a different
//! source or lock in the two versions: each line is read with its own
//! catalogue, and anything that compares lines of both joins them by key name
//! and index name, never by position.
//!
//! The key list (and so the order of `summary.tsv`'s `tw_<key>` columns) is
//! the same in both versions today. [`union_keys`] is the join: `v=1`'s keys
//! in order, then those only `v=2` has.

use std::collections::HashMap;
use std::sync::LazyLock;

use shared::lock::LockClass;
use shared::sched::SchedulerClass;
use shared::tripwire::{BadchanSite, Key, N2Kind, WakeSource, Width, SCHEMA_VERSION};

use super::schema_v1;

// `Schema::V2` is `shared`'s catalogue as of version 2. A bump of
// `SCHEMA_VERSION` makes that catalogue version 3: the version-2 one must be
// frozen next to `schema_v1` first, and this assertion stops the build until
// it is.
const _: () = assert!(SCHEMA_VERSION == 2);

/// A key of a catalogue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyDef {
    /// The printed name.
    pub name: &'static str,
    /// How many values the key has.
    pub width: Width,
    /// Whether the key is a gauge (combined by maximum, not by sum).
    pub gauge: bool,
}

/// A tripwire schema version this parser decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Schema {
    /// `v=1`: the frozen catalogue of [`super::schema_v1`] (16 wake sources,
    /// 9 lock classes).
    V1,
    /// `v=2`: `shared`'s current catalogue (12 wake sources, 8 lock classes).
    V2,
}

/// The scheduler classes in `starved`'s index order (`SchedulerClass as
/// usize`), named as `shared` declares them.
const V2_SCHEDULER_CLASSES: [SchedulerClass; shared::tripwire::CLASS_COUNT] = [
    SchedulerClass::Idle,
    SchedulerClass::Normal,
    SchedulerClass::Interactive,
    SchedulerClass::RealTime,
];

static V2_KEYS: LazyLock<Vec<KeyDef>> = LazyLock::new(|| {
    Key::ALL
        .iter()
        .map(|k| KeyDef {
            name: k.name(),
            width: k.width(),
            gauge: k.is_gauge(),
        })
        .collect()
});

/// Each schema's key positions by printed name, in [`Schema::ALL`] order.
static POSITIONS: LazyLock<[HashMap<&'static str, usize>; 2]> = LazyLock::new(|| {
    Schema::ALL.map(|s| {
        s.keys()
            .iter()
            .enumerate()
            .map(|(i, k)| (k.name, i))
            .collect()
    })
});

static UNION: LazyLock<Vec<KeyDef>> = LazyLock::new(|| {
    let mut keys: Vec<KeyDef> = Vec::new();
    for schema in Schema::ALL {
        for key in schema.keys() {
            if !keys.iter().any(|k| k.name == key.name) {
                keys.push(*key);
            }
        }
    }
    keys
});

impl Schema {
    /// Every schema, oldest first.
    pub const ALL: [Schema; 2] = [Schema::V1, Schema::V2];

    /// The schema a `v=` value names, `None` for any other version.
    pub fn of_version(v: &[u8]) -> Option<Schema> {
        Schema::ALL
            .into_iter()
            .find(|s| s.version().to_string().as_bytes() == v)
    }

    /// The `v=` value of the schema.
    pub const fn version(self) -> u64 {
        match self {
            Schema::V1 => 1,
            Schema::V2 => SCHEMA_VERSION,
        }
    }

    /// The schema's keys in print order.
    pub fn keys(self) -> &'static [KeyDef] {
        match self {
            Schema::V1 => &schema_v1::KEYS,
            Schema::V2 => &V2_KEYS,
        }
    }

    /// The position of key `name` in [`Schema::keys`], `None` for a key the
    /// schema does not have.
    pub fn position(self, name: &[u8]) -> Option<usize> {
        let name = std::str::from_utf8(name).ok()?;
        POSITIONS[self.slot()].get(name).copied()
    }

    /// The name of index `i` of a key of `width`: `CPU i`, or the index set's
    /// name in this schema; the bare number past the end of a set (a
    /// one-value key has no index names).
    pub fn index_name(self, width: Width, i: usize) -> String {
        let name = match (self, width) {
            (_, Width::Cpu) => return format!("CPU {i}"),
            (_, Width::One) => None,
            (Schema::V1, Width::Source) => schema_v1::WAKE_SOURCES.get(i).map(|s| s.to_string()),
            (Schema::V1, Width::Lock) => schema_v1::LOCK_CLASSES.get(i).map(|s| s.to_string()),
            (Schema::V1, Width::Class) => {
                schema_v1::SCHEDULER_CLASSES.get(i).map(|s| s.to_string())
            }
            (Schema::V1, Width::N2) => schema_v1::N2_KINDS.get(i).map(|s| s.to_string()),
            (Schema::V1, Width::Badchan) => schema_v1::BADCHAN_SITES.get(i).map(|s| s.to_string()),
            (Schema::V2, Width::Source) => WakeSource::ALL.get(i).map(|s| s.name().to_string()),
            (Schema::V2, Width::Lock) => LockClass::ALL.get(i).map(|l| l.name().to_string()),
            (Schema::V2, Width::Class) => V2_SCHEDULER_CLASSES.get(i).map(|c| format!("{c:?}")),
            (Schema::V2, Width::N2) => N2Kind::ALL.get(i).map(|k| k.name().to_string()),
            (Schema::V2, Width::Badchan) => BadchanSite::ALL.get(i).map(|b| b.name().to_string()),
        };
        name.unwrap_or_else(|| i.to_string())
    }

    /// The schema's position in [`Schema::ALL`].
    const fn slot(self) -> usize {
        match self {
            Schema::V1 => 0,
            Schema::V2 => 1,
        }
    }
}

/// The keys of every schema joined by name: `v=1`'s in order, then those
/// only a later schema has. The `tw_<key>` columns of `summary.tsv` and the
/// rows of the tripwire tables follow this order.
pub fn union_keys() -> &'static [KeyDef] {
    &UNION
}

/// Whether `name` is a lock static of some schema: a `lock re-entry:` panic
/// from a kernel of either version names one.
pub fn is_lock_class(name: &[u8]) -> bool {
    schema_v1::LOCK_CLASSES.iter().any(|l| l.as_bytes() == name)
        || LockClass::ALL.iter().any(|l| l.name().as_bytes() == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn versions_name_their_schema_and_any_other_is_unknown() {
        assert_eq!(Schema::of_version(b"1"), Some(Schema::V1));
        assert_eq!(Schema::of_version(b"2"), Some(Schema::V2));
        for other in [&b"0"[..], b"3", b"99", b"", b"01", b"1 ", b"x"] {
            assert_eq!(Schema::of_version(other), None, "{other:?}");
        }
        assert_eq!(Schema::V2.version(), SCHEMA_VERSION);
    }

    #[test]
    fn the_frozen_v1_catalogue_has_the_widths_of_step_10() {
        assert_eq!(schema_v1::KEYS.len(), 64);
        assert_eq!(schema_v1::WAKE_SOURCES.len(), 16);
        assert_eq!(schema_v1::LOCK_CLASSES.len(), 9);
        let widths = |w: Width| -> Vec<&str> {
            schema_v1::KEYS
                .iter()
                .filter(|k| k.width == w)
                .map(|k| k.name)
                .collect()
        };
        assert_eq!(
            widths(Width::Cpu),
            [
                "tick", "irqsw", "irqsw0", "nest", "elrmm", "spsrmm", "n4", "insched", "rsthold",
                "tpidrbad"
            ]
        );
        assert_eq!(
            widths(Width::Source),
            ["ubrun", "ubrbl", "ubdead", "ubnone", "ubmove", "ubtok", "ubpend"]
        );
        assert_eq!(
            widths(Width::Lock),
            ["lkph", "lkpho", "lkphrun", "lkoxp", "lkself", "lktry", "lktph", "lkstk"]
        );
        assert_eq!(widths(Width::Class), ["starved"]);
        assert_eq!(widths(Width::N2), ["n2"]);
        assert_eq!(widths(Width::Badchan), ["badchan"]);
        assert_eq!(widths(Width::One).len(), 64 - 10 - 7 - 8 - 3);
        let gauges: Vec<&str> = schema_v1::KEYS
            .iter()
            .filter(|k| k.gauge)
            .map(|k| k.name)
            .collect();
        assert_eq!(
            gauges,
            [
                "orphan_now",
                "nowaker_now",
                "wakefl_now",
                "scanhold1",
                "scanhold2",
                "twmax"
            ]
        );
    }

    #[test]
    fn the_key_lists_of_the_two_versions_are_the_same_today() {
        // Step 11 shifted the index sets, not the keys. The one place a
        // difference would matter, `report::TSV_COLUMNS`, counts the union as
        // `Key::COUNT`; a version that adds or drops a key changes it there.
        let v1: Vec<&str> = Schema::V1.keys().iter().map(|k| k.name).collect();
        let v2: Vec<&str> = Schema::V2.keys().iter().map(|k| k.name).collect();
        assert_eq!(v1, v2);
        assert_eq!(union_keys().len(), Key::COUNT);
        let widths_agree = Schema::V1
            .keys()
            .iter()
            .zip(Schema::V2.keys())
            .all(|(a, b)| a == b);
        assert!(widths_agree, "a key's width or gauge flag differs");
    }

    #[test]
    fn positions_come_from_the_schemas_own_list() {
        for schema in Schema::ALL {
            for (i, key) in schema.keys().iter().enumerate() {
                assert_eq!(schema.position(key.name.as_bytes()), Some(i));
            }
            assert_eq!(schema.position(b"zzz"), None);
            assert_eq!(schema.position(&[0xff]), None);
        }
    }

    #[test]
    fn index_names_come_from_the_schemas_own_sets() {
        let (v1, v2) = (Schema::V1, Schema::V2);
        for (i, class) in V2_SCHEDULER_CLASSES.iter().enumerate() {
            assert_eq!(*class as usize, i, "{class:?}");
            assert_eq!(format!("{class:?}"), schema_v1::SCHEDULER_CLASSES[i]);
        }
        let n2: Vec<&str> = N2Kind::ALL.iter().map(|k| k.name()).collect();
        assert_eq!(n2, schema_v1::N2_KINDS);
        let sites: Vec<&str> = BadchanSite::ALL.iter().map(|b| b.name()).collect();
        assert_eq!(sites, schema_v1::BADCHAN_SITES);
        assert_eq!(v1.index_name(Width::Cpu, 3), "CPU 3");
        assert_eq!(v2.index_name(Width::Cpu, 3), "CPU 3");
        assert_eq!(v1.index_name(Width::One, 0), "0");
        // The wake sources: v=1 index 14 is `cancel`, v=2 index 10.
        assert_eq!(v1.index_name(Width::Source, 14), "cancel");
        assert_eq!(v1.index_name(Width::Source, 15), "selftest");
        assert_eq!(v1.index_name(Width::Source, 16), "16");
        assert_eq!(v2.index_name(Width::Source, 10), "cancel");
        assert_eq!(v2.index_name(Width::Source, 11), "selftest");
        assert_eq!(v2.index_name(Width::Source, 12), "12");
        assert_eq!(v1.index_name(Width::Source, 3), "selcall");
        assert_eq!(v2.index_name(Width::Source, 3), "sig");
        // The lock classes: BOOT_LOG is v=1 index 8, v=2 index 7.
        assert_eq!(v1.index_name(Width::Lock, 7), "SELECT_WAITERS");
        assert_eq!(v1.index_name(Width::Lock, 8), "BOOT_LOG");
        assert_eq!(v2.index_name(Width::Lock, 7), "BOOT_LOG");
        assert_eq!(v2.index_name(Width::Lock, 8), "8");
        // The sets step 11 left alone.
        for schema in Schema::ALL {
            assert_eq!(schema.index_name(Width::Lock, 3), "WAKEUP_ERRORS");
            assert_eq!(schema.index_name(Width::Class, 1), "Normal");
            assert_eq!(schema.index_name(Width::Class, 3), "RealTime");
            assert_eq!(schema.index_name(Width::N2, 1), "rblk");
            assert_eq!(schema.index_name(Width::Badchan, 2), "slot");
            assert_eq!(schema.index_name(Width::Badchan, 3), "3");
        }
    }

    #[test]
    fn the_v2_sets_are_the_v1_sets_less_what_step_11_deleted() {
        let v2_sources: Vec<&str> = WakeSource::ALL.iter().map(|s| s.name()).collect();
        let kept: Vec<&str> = schema_v1::WAKE_SOURCES
            .iter()
            .copied()
            .filter(|s| !["selcall", "selsend", "selsig", "nsto"].contains(s))
            .collect();
        assert_eq!(v2_sources, kept);
        let v2_locks: Vec<&str> = LockClass::ALL.iter().map(|l| l.name()).collect();
        let kept: Vec<&str> = schema_v1::LOCK_CLASSES
            .iter()
            .copied()
            .filter(|l| *l != "SELECT_WAITERS")
            .collect();
        assert_eq!(v2_locks, kept);
    }

    #[test]
    fn a_lock_re_entry_can_name_a_lock_of_either_version() {
        assert!(is_lock_class(b"SELECT_WAITERS"));
        assert!(is_lock_class(b"BOOT_LOG"));
        assert!(!is_lock_class(b"PROCESS_TABLE"));
    }
}
