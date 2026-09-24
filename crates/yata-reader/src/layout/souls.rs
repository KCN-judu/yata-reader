//! The souls scope: every object the recognition rule accepts, reported as it is.
//!
//! **What is inherited and what is not.** The recognition rule — a dict holding at least two of
//! five marker keys, whose values have the kinds the rule gives them — is the prior tool's, and
//! this reader states it as inherited evidence on every result. The value kinds matter: the
//! runtime's own table of interned names holds every marker key too, with texts as values. No typed field of `SoulRecord` is mapped yet: no mapping from an entry to a soul
//! id, suit code, star, slot, level, attribute, or flag has been re-established by this project's
//! own recordings, so every one of them stays unset and each record carries its entries verbatim
//! in `observed`. The container key is reported as observed, not as a soul id.
//!
//! **What is read.** Every dict in the target's private writable memory is found by its type
//! pointer, every dict the rule accepts is a soul, and the dict holding the most souls
//! as values is their container, whose key for each soul is reported. Souls in the container come
//! first, in its order; the rest follow in address order. Nothing is judged: a stale copy of a
//! soul is reported like a live one, and the container key tells them apart for whoever reads the
//! result.

use std::collections::{HashMap, HashSet};

use yata_protocol::probe::{
    Coverage, Evidence, FieldEvidence, ObservedRecord, RawEntry, ReadResult, ReadStats, Scope,
    SoulRecord, SoulRecords, read_result::Records,
};

use super::cpython::{Decoder, Runtime, scan};
use super::limits::MAX_SOULS;
use super::memory::{Memory, RegionKind};

/// The prior tool's recognition rule: a dict holding at least [`MIN_MARKERS`] of these keys,
/// each of the [`Checked`] keys it holds having the value kind the rule gives it.
pub const MARKER_KEYS: [&str; 5] = ["base_rindex", "rattr", "single_attr", "others", "sattr"];
pub const MIN_MARKERS: usize = 2;

/// The `FieldEvidence` a result carries for the recognition rule.
pub const RECOGNITION_FIELD: &str = "SoulRecord";
pub const RECOGNITION_BASIS: &str = "prior tool: a dict holding at least two of base_rindex, \
     rattr, single_attr, others, sattr, where base_rindex and others are ints, rattr is a list, \
     single_attr is an int or None, sattr is a str, and base_r is a number";

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
        let Ok(t) = d.type_of(value) else {
            return false;
        };
        let rt = d.rt;
        match self {
            Checked::BaseRindex | Checked::Others => t == rt.int,
            Checked::Rattr => t == rt.list,
            Checked::SingleAttr => t == rt.int || t == rt.none_type,
            Checked::Sattr => t == rt.str_,
            Checked::BaseR => t == rt.int || t == rt.float,
        }
    }
}

/// Progress is reported on this scale: scanning, then recognising, then finding the container.
pub const PROGRESS_TOTAL: u64 = 1000;
const SCANNED: u64 = 400;
const RECOGNISED: u64 = 700;

/// The read stopped at the daemon's `Cancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// Read the souls. `progress` gets `(done, total)`; `cancelled` is checked at every checkpoint.
pub fn read_souls(
    mem: &dyn Memory,
    rt: &Runtime,
    progress: &mut dyn FnMut(u64, u64),
    cancelled: &dyn Fn() -> bool,
) -> Result<ReadResult, Cancelled> {
    let d = Decoder::new(mem, rt);
    let heap = |kind: RegionKind, writable: bool| kind == RegionKind::Private && writable;
    let total_bytes: u64 = mem
        .regions()
        .iter()
        .filter(|r| heap(r.kind, r.writable))
        .map(|r| r.size)
        .sum();
    let mut stats = ReadStats::default();
    let mut unreadable_chunks = 0u64;
    let mut last = u64::MAX;
    let mut report = |done: u64, progress: &mut dyn FnMut(u64, u64)| {
        if done != last {
            last = done;
            progress(done, PROGRESS_TOTAL);
        }
    };

    // Pass 1: every object whose type is dict.
    let mut scanned = 0u64;
    let dicts = scan(
        mem,
        heap,
        |a, v| a >= 8 && v == rt.dict,
        cancelled,
        |n, ok| {
            scanned += n;
            if !ok {
                unreadable_chunks += 1;
            }
            report(scale(scanned, total_bytes, 0, SCANNED), progress);
        },
    )
    .map_err(|_| Cancelled)?
    .into_iter()
    .map(|a| a - 8)
    .collect::<Vec<u64>>();
    stats.regions_scanned = mem
        .regions()
        .iter()
        .filter(|r| heap(r.kind, r.writable))
        .count() as u64;
    stats.regions_unreadable = unreadable_chunks;
    stats.bytes_scanned = scanned;

    // Pass 2: which dicts are souls.
    let mut key_of_name: HashMap<u64, Option<Checked>> = HashMap::new();
    let mut checked = |key: u64| {
        *key_of_name
            .entry(key)
            .or_insert_with(|| d.text(key).ok().and_then(|t| Checked::of(&t)))
    };
    let mut readable: Vec<u64> = Vec::new();
    let mut souls: Vec<u64> = Vec::new();
    let mut rejected = 0u64;
    for (i, &dict) in dicts.iter().enumerate() {
        if i % 1024 == 0 {
            if cancelled() {
                return Err(Cancelled);
            }
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
            souls.push(dict);
        } else {
            rejected += 1;
        }
    }
    stats.candidates_rejected = rejected;
    if souls.len() > MAX_SOULS {
        stats.candidates_rejected += (souls.len() - MAX_SOULS) as u64;
        souls.truncate(MAX_SOULS);
    }

    // Pass 3: the dict holding the most souls as values is their container.
    let soul_set: HashSet<u64> = souls.iter().copied().collect();
    let mut best: Option<(usize, u64)> = None;
    for (i, &dict) in readable.iter().enumerate() {
        if i % 1024 == 0 {
            if cancelled() {
                return Err(Cancelled);
            }
            report(
                scale(i as u64, readable.len() as u64, RECOGNISED, PROGRESS_TOTAL),
                progress,
            );
        }
        if soul_set.contains(&dict) {
            continue;
        }
        let Ok((items, _)) = d.dict_items(dict) else {
            continue;
        };
        let held = items.iter().filter(|(_, v)| soul_set.contains(v)).count();
        if held > 0 && best.is_none_or(|(n, _)| held > n) {
            best = Some((held, dict));
        }
    }
    let mut key_of: HashMap<u64, u64> = HashMap::new();
    let mut order: Vec<u64> = Vec::new();
    if let Some((_, container)) = best
        && let Ok((items, _)) = d.dict_items(container)
    {
        for (k, v) in items {
            if soul_set.contains(&v) && !key_of.contains_key(&v) {
                key_of.insert(v, k);
                order.push(v);
            }
        }
    }
    let mut rest: Vec<u64> = souls
        .iter()
        .copied()
        .filter(|s| !key_of.contains_key(s))
        .collect();
    rest.sort_unstable();
    order.extend(rest);

    let records = order
        .iter()
        .map(|&soul| {
            let entries = d
                .dict_items(soul)
                .map(|(items, _)| {
                    items
                        .iter()
                        .map(|&(k, v)| RawEntry {
                            key: Some(d.value(k, 1)),
                            value: Some(d.entry_value(v)),
                        })
                        .collect()
                })
                .unwrap_or_default();
            SoulRecord {
                observed: Some(ObservedRecord {
                    type_name: d.type_name(rt.dict),
                    container_key: key_of.get(&soul).map(|&k| d.value(k, 1)),
                    entries,
                }),
                ..SoulRecord::default()
            }
        })
        .collect();
    report(PROGRESS_TOTAL, progress);
    Ok(ReadResult {
        request_id: 0,
        scope: Scope::Souls.into(),
        // The reader cannot tell whether the game holds the whole inventory in memory.
        coverage: Coverage::Partial.into(),
        observed_account_id: String::new(),
        records: Some(Records::Souls(SoulRecords { souls: records })),
        field_evidence: vec![FieldEvidence {
            field: RECOGNITION_FIELD.to_owned(),
            evidence: Evidence::Inherited.into(),
            basis: RECOGNITION_BASIS.to_owned(),
        }],
        stats: Some(stats),
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
