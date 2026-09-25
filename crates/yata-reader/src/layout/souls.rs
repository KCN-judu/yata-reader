//! The souls scope: every object the recognition rule accepts, reported as it is.
//!
//! **What is inherited and what is not.** The recognition rule — a dict holding at least two of
//! five marker keys, whose values have the kinds the rule gives them — is the prior tool's, and
//! this reader states it as inherited on every reading. The value kinds matter: the runtime's own
//! table of interned names holds every marker key too, with texts as values. No typed field of
//! `SoulRecord` is mapped yet: no mapping from an entry to a soul id, suit code, star, slot,
//! level, attribute, or flag has been re-established by this project's own recordings, so every
//! field's mapping is absent, every typed field stays unset, and each record carries its entries
//! verbatim in `observed`. The container key is reported as observed, not as a soul id.
//!
//! **What is read.** Every dict in the target's private writable memory is found by its type
//! pointer, every dict the rule accepts is a soul, and the dict holding the most souls
//! as values is their container, whose key for each soul is reported. Souls in the container come
//! first, in its order; the rest follow in address order. Nothing is judged: a stale copy of a
//! soul is reported like a live one, and the container key tells them apart for whoever reads the
//! result. Each soul's entries are the ones read when it was recognised, so the entries a record
//! carries are the ones the rule accepted.

use std::collections::{BTreeSet, HashMap};

use yata_protocol::probe::{
    Coverage, Inherited, Mapping, ObservedRecord, RawEntry, ReadStats, Reading, SoulMappings,
    SoulRecord, SoulRecords, mapping, reading::Records,
};

use super::cpython::{Builtin, Cancelled, Chunk, Decoder, Runtime, scan};
use super::limits::MAX_SOULS;
use super::memory::{Memory, RegionKind};

/// The prior tool's recognition rule: a dict holding at least [`MIN_MARKERS`] of these keys,
/// each of the [`Checked`] keys it holds having the value kind the rule gives it: `base_rindex`
/// and `others` are ints, `rattr` is a list, `single_attr` is an int or None, `sattr` is a str,
/// and `base_r` is a number.
pub const MARKER_KEYS: [&str; 5] = ["base_rindex", "rattr", "single_attr", "others", "sattr"];
pub const MIN_MARKERS: usize = 2;

/// The keys whose value kinds the rule checks: the five markers, and `base_r`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Checked {
    BaseRindex,
    Rattr,
    SingleAttr,
    Others,
    Sattr,
    BaseR,
}

impl Checked {
    fn of(name: &str) -> Option<Checked> {
        Some(match name {
            "base_rindex" => Checked::BaseRindex,
            "rattr" => Checked::Rattr,
            "single_attr" => Checked::SingleAttr,
            "others" => Checked::Others,
            "sattr" => Checked::Sattr,
            "base_r" => Checked::BaseR,
            _ => return None,
        })
    }

    fn is_marker(self) -> bool {
        self != Checked::BaseR
    }

    /// Whether a value has the kind the rule gives this key. A bool is not an int here.
    fn fits(self, d: &Decoder<'_>, value: u64) -> bool {
        let Some(b) = d.type_of(value).ok().and_then(|t| d.rt.builtin(t)) else {
            return false;
        };
        match self {
            Checked::BaseRindex | Checked::Others => b == Builtin::Int,
            Checked::Rattr => b == Builtin::List,
            Checked::SingleAttr => matches!(b, Builtin::Int | Builtin::NoneType),
            Checked::Sattr => b == Builtin::Str,
            Checked::BaseR => matches!(b, Builtin::Int | Builtin::Float),
        }
    }
}

/// Progress is reported on this scale: scanning, then recognising, then finding the container.
pub const PROGRESS_TOTAL: u64 = 1000;
const SCANNED: u64 = 400;
const RECOGNISED: u64 = 700;

/// A dict's address and its live `(key, value)` pairs.
type Dict = (u64, Vec<(u64, u64)>);

/// Read the souls. `progress` gets `(done, total)`; `cancelled` is checked at every checkpoint.
pub fn read_souls(
    mem: &dyn Memory,
    rt: &Runtime,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<Reading, Cancelled> {
    let d = Decoder::new(mem, rt);
    let heap = |kind: RegionKind, writable: bool| kind == RegionKind::Private && writable;
    let total_bytes: u64 = mem
        .regions()
        .iter()
        .filter(|r| heap(r.kind, r.writable))
        .map(|r| r.size)
        .sum();
    let checkpoint = || if cancelled() { Err(Cancelled) } else { Ok(()) };
    let mut last: Option<u64> = None;
    let mut report = |done: u64, progress: &mut dyn FnMut(u64, u64)| {
        if last != Some(done) {
            last = Some(done);
            progress(done, PROGRESS_TOTAL);
        }
    };

    // Pass 1: every object whose type is dict.
    let mut scanned = 0u64;
    let mut unreadable_regions: BTreeSet<usize> = BTreeSet::new();
    let dicts = scan(
        mem,
        heap,
        |a, v| a >= 8 && v == rt.dict,
        checkpoint,
        |c: Chunk| {
            scanned += c.bytes;
            if !c.readable {
                unreadable_regions.insert(c.region);
            }
            report(scale(scanned, total_bytes, 0, SCANNED), progress);
        },
    )?
    .into_iter()
    .map(|a| a - 8)
    .collect::<Vec<u64>>();

    // Pass 2: which dicts are souls. A soul keeps the items it was recognised by.
    let mut key_of_name: HashMap<u64, Option<Checked>> = HashMap::new();
    let mut checked = |key: u64| {
        *key_of_name
            .entry(key)
            .or_insert_with(|| d.text(key).ok().and_then(|t| Checked::of(&t)))
    };
    let mut readable: Vec<u64> = Vec::new();
    let mut souls: Vec<Dict> = Vec::new();
    let mut rejected = 0u64;
    for (i, &dict) in dicts.iter().enumerate() {
        if i % 1024 == 0 {
            checkpoint()?;
            report(
                scale(i as u64, dicts.len() as u64, SCANNED, RECOGNISED),
                progress,
            );
        }
        if d.type_of(dict).is_err() {
            continue;
        }
        let Ok((items, _)) = d.dict_items(dict) else {
            continue;
        };
        readable.push(dict);
        let named: Vec<(Checked, u64)> = items
            .iter()
            .filter_map(|&(k, v)| checked(k).map(|c| (c, v)))
            .collect();
        if named.iter().filter(|(c, _)| c.is_marker()).count() < MIN_MARKERS {
            continue;
        }
        if named.iter().all(|&(c, v)| c.fits(&d, v)) {
            souls.push((dict, items));
        } else {
            rejected += 1;
        }
    }
    let over = souls.len().saturating_sub(MAX_SOULS);
    souls.truncate(MAX_SOULS);

    // Pass 3: the dict holding the most souls as values is their container. Its items are kept
    // as read here, so its keys are the ones it was chosen by.
    let soul_at: HashMap<u64, usize> = souls
        .iter()
        .enumerate()
        .map(|(i, (dict, _))| (*dict, i))
        .collect();
    let mut best: Option<(usize, Vec<(u64, u64)>)> = None;
    for (i, &dict) in readable.iter().enumerate() {
        if i % 1024 == 0 {
            checkpoint()?;
            report(
                scale(i as u64, readable.len() as u64, RECOGNISED, PROGRESS_TOTAL),
                progress,
            );
        }
        if soul_at.contains_key(&dict) {
            continue;
        }
        let Ok((items, _)) = d.dict_items(dict) else {
            continue;
        };
        let held = items
            .iter()
            .filter(|(_, v)| soul_at.contains_key(v))
            .count();
        if held > 0 && best.as_ref().is_none_or(|(n, _)| held > *n) {
            best = Some((held, items));
        }
    }
    let mut key_of: HashMap<usize, u64> = HashMap::new();
    let mut order: Vec<usize> = Vec::new();
    for (k, v) in best.map(|(_, items)| items).unwrap_or_default() {
        if let Some(&soul) = soul_at.get(&v)
            && !key_of.contains_key(&soul)
        {
            key_of.insert(soul, k);
            order.push(soul);
        }
    }
    // `souls` is in address order already: pass 1 scans in address order.
    order.extend((0..souls.len()).filter(|s| !key_of.contains_key(s)));

    let records = order
        .iter()
        .map(|soul| SoulRecord {
            observed: Some(ObservedRecord {
                type_name: Builtin::Dict.name().to_owned(),
                container_key: key_of.get(soul).map(|&k| d.value(k, 1)),
                entries: souls[*soul]
                    .1
                    .iter()
                    .map(|&(k, v)| RawEntry {
                        key: Some(d.value(k, 1)),
                        value: Some(d.entry_value(v)),
                    })
                    .collect(),
            }),
            ..SoulRecord::default()
        })
        .collect();
    report(PROGRESS_TOTAL, progress);
    let stats = ReadStats {
        regions_scanned: mem
            .regions()
            .iter()
            .filter(|r| heap(r.kind, r.writable))
            .count() as u64,
        regions_unreadable: unreadable_regions.len() as u64,
        bytes_scanned: scanned,
        candidates_rejected: rejected + over as u64,
    };
    Ok(Reading {
        // The reader cannot tell whether the game holds the whole inventory in memory.
        coverage: Coverage::Partial.into(),
        observed_account_id: None,
        stats: Some(stats),
        records: Some(Records::Souls(SoulRecords {
            souls: records,
            recognition: Some(Mapping {
                evidence: Some(mapping::Evidence::Inherited(Inherited {})),
            }),
            // No typed field is mapped.
            mappings: Some(SoulMappings::default()),
        })),
    })
}

/// `done / total` of the way from `from` to `to`.
fn scale(done: u64, total: u64, from: u64, to: u64) -> u64 {
    if total == 0 {
        return to;
    }
    from + (to - from) * done.min(total) / total
}

#[cfg(test)]
#[path = "souls_tests.rs"]
mod tests;
