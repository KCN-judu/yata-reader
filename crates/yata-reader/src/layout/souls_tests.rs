use yata_protocol::probe::{
    Coverage, RawValue, SoulMappings, mapping, raw_value::Kind, reading::Records,
};

use super::*;
use crate::layout::cpython::{DictKeys, discover};
use crate::layout::fixture::{IDS, inventory};

fn never() -> bool {
    false
}

fn read(keys: DictKeys) -> (Reading, Vec<(u64, u64)>) {
    let image = inventory(keys).expect("disjoint");
    let rt = discover(&image).expect("discovered");
    let mut seen = Vec::new();
    let result = read_souls(&image, &rt, &mut |d, t| seen.push((d, t)), &never).expect("read");
    (result, seen)
}

fn text(v: &Option<RawValue>) -> Option<&str> {
    match v.as_ref()?.kind.as_ref()? {
        Kind::Text(t) => Some(t),
        _ => None,
    }
}

#[test]
fn the_souls_are_found_container_first_with_their_keys() {
    for keys in [DictKeys::Sized, DictKeys::Logged] {
        let (result, _) = read(keys);
        let Some(Records::Souls(s)) = result.records else {
            panic!("souls")
        };
        assert_eq!(s.souls.len(), 4, "three in the container, one outside");
        let keys: Vec<Option<&str>> = s
            .souls
            .iter()
            .map(|r| text(&r.observed.as_ref().expect("observed").container_key))
            .collect();
        assert_eq!(keys, vec![Some(IDS[0]), Some(IDS[1]), Some(IDS[2]), None]);
    }
}

#[test]
fn every_entry_is_kept_and_no_typed_field_is_filled() {
    let (result, _) = read(DictKeys::Logged);
    let Some(Records::Souls(s)) = result.records else {
        panic!("souls")
    };
    let first = &s.souls[0];
    assert_eq!(first.soul_id, None);
    assert_eq!(first.suit_code, None);
    assert_eq!(first.star, None);
    assert_eq!(first.main, None);
    assert_eq!(first.subs, None);
    assert_eq!(first.innate, None);
    let observed = first.observed.as_ref().expect("observed");
    assert_eq!(observed.type_name, "dict");
    let names: Vec<&str> = observed
        .entries
        .iter()
        .filter_map(|e| text(&e.key))
        .collect();
    assert_eq!(
        names,
        vec![
            "base_rindex",
            "rattr",
            "single_attr",
            "others",
            "sattr",
            "base_r"
        ]
    );
    // The first soul's third entry holds None, the second soul's an integer.
    assert!(matches!(
        observed.entries[2]
            .value
            .as_ref()
            .and_then(|v| v.kind.as_ref()),
        Some(Kind::Null(_))
    ));
    let second = s.souls[1].observed.as_ref().expect("observed");
    assert_eq!(
        second.entries[2]
            .value
            .as_ref()
            .and_then(|v| v.kind.clone()),
        Some(Kind::Integer(7))
    );
}

#[test]
fn the_reading_states_the_recognition_rule_as_inherited_and_maps_no_field() {
    let (result, _) = read(DictKeys::Logged);
    let Some(Records::Souls(s)) = &result.records else {
        panic!("souls")
    };
    assert!(matches!(
        s.recognition.as_ref().and_then(|m| m.evidence.as_ref()),
        Some(mapping::Evidence::Inherited(_))
    ));
    assert_eq!(s.mappings, Some(SoulMappings::default()));
    assert_eq!(result.coverage(), Coverage::Partial);
    assert_eq!(result.observed_account_id, None);
    let stats = result.stats.expect("stats");
    assert_eq!(stats.regions_scanned, 1);
    assert_eq!(stats.regions_unreadable, 0);
    // The dict of texts under marker keys.
    assert_eq!(stats.candidates_rejected, 1);
    assert!(stats.bytes_scanned > 0);
}

#[test]
fn progress_only_grows_and_ends_at_the_total() {
    let (_, seen) = read(DictKeys::Logged);
    assert!(seen.windows(2).all(|w| w[0].0 < w[1].0));
    assert_eq!(seen.last(), Some(&(PROGRESS_TOTAL, PROGRESS_TOTAL)));
    assert!(seen.iter().all(|&(_, t)| t == PROGRESS_TOTAL));
}

#[test]
fn a_cancelled_read_stops_at_a_checkpoint() {
    let image = inventory(DictKeys::Logged).expect("disjoint");
    let rt = discover(&image).expect("discovered");
    assert_eq!(
        read_souls(&image, &rt, &mut |_, _| (), &|| true),
        Err(Cancelled)
    );
}
