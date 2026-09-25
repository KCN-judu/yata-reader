use yata_protocol::probe::{RawValue, SequenceKind, UnreadReason, raw_value::Kind};

use super::*;
use crate::layout::memory::{Image, RegionKind};
use crate::layout::synthetic::{Builder, HEAP_BASE};

fn runtime(b: Builder) -> (Image, Runtime) {
    let image = b.finish().expect("disjoint");
    let rt = discover(&image).expect("a synthetic runtime is discovered");
    (image, rt)
}

fn kind(v: &RawValue) -> &Kind {
    v.kind.as_ref().expect("a kind")
}

fn reason(v: &RawValue) -> UnreadReason {
    match kind(v) {
        Kind::Unread(u) => u.reason(),
        other => panic!("expected unread, got {other:?}"),
    }
}

#[test]
fn discovery_finds_every_builtin_and_the_key_shape() {
    for keys in [DictKeys::Sized, DictKeys::Logged] {
        let b = Builder::new(keys);
        let expected = (
            b.type_type(),
            b.ty(Builtin::Dict),
            b.ty(Builtin::Str),
            b.ty(Builtin::NoneType),
        );
        let (_, rt) = runtime(b);
        assert_eq!((rt.type_type, rt.dict, rt.str_, rt.none_type), expected);
        assert_eq!(rt.keys, keys);
    }
}

#[test]
fn the_engine_names_the_key_shape() {
    let (_, rt) = runtime(Builder::new(DictKeys::Logged));
    assert_eq!(rt.engine(), "cpython-3.11");
    let (_, rt) = runtime(Builder::new(DictKeys::Sized));
    assert_eq!(rt.engine(), "cpython-3.6-3.10");
}

#[test]
fn memory_with_no_runtime_is_a_layout_mismatch() {
    let mut m = Image::new();
    m.add(0x1_4000_0000, RegionKind::Image, false, vec![0x5a; 4096])
        .expect("disjoint");
    assert_eq!(discover(&m), Err(LayoutError::TypeObject { found: 0 }));
}

#[test]
fn a_str_of_another_size_is_a_layout_mismatch() {
    let mut b = Builder::new(DictKeys::Logged);
    // 3.12 removed the wide-character field and shrank str to 64 bytes.
    let str_type = b.ty(Builtin::Str);
    b.put_u64(str_type + 32, 64);
    let image = b.finish().expect("disjoint");
    assert_eq!(
        discover(&image),
        Err(LayoutError::Size {
            builtin: Builtin::Str,
            basic: 64,
            item: 0
        })
    );
}

#[test]
fn a_scan_stops_at_the_checkpoints_error_and_counts_regions() {
    let image = Builder::new(DictKeys::Logged).finish().expect("disjoint");
    let any = |_: RegionKind, _: bool| true;
    assert_eq!(
        scan(&image, any, |_, _| true, || Err(Cancelled), |_| ()),
        Err(Cancelled)
    );
    let mut regions = Vec::new();
    let hits = scan(
        &image,
        any,
        |_, _| false,
        || Ok::<(), Cancelled>(()),
        |c| regions.push((c.region, c.readable)),
    );
    assert_eq!(hits, Ok(vec![]));
    assert_eq!(regions, vec![(0, true), (1, true)]);
}

#[test]
fn scalars_decode_to_their_kinds() {
    let mut b = Builder::new(DictKeys::Logged);
    let values = [
        b.int(0),
        b.int(-7),
        b.int(i64::MAX),
        b.int(i64::MIN + 1),
        b.float(2.5),
        b.boolean(true),
        b.boolean(false),
        b.none(),
    ];
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    let got: Vec<Kind> = values
        .iter()
        .map(|&v| kind(&d.value(v, 1)).clone())
        .collect();
    assert_eq!(
        got,
        vec![
            Kind::Integer(0),
            Kind::Integer(-7),
            Kind::Integer(i64::MAX),
            Kind::Integer(i64::MIN + 1),
            Kind::Float(2.5),
            Kind::Boolean(true),
            Kind::Boolean(false),
            Kind::Null(yata_protocol::probe::RawNull {}),
        ]
    );
}

#[test]
fn text_decodes_in_each_width() {
    let mut b = Builder::new(DictKeys::Sized);
    let samples = ["base", "café", "御魂", "𝄞 clef", ""];
    let at: Vec<u64> = samples.iter().map(|s| b.str(s)).collect();
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    for (s, a) in samples.iter().zip(at) {
        assert_eq!(d.text(a).as_deref(), Ok(*s));
    }
}

#[test]
fn an_integer_beyond_64_bits_is_out_of_range() {
    let mut b = Builder::new(DictKeys::Logged);
    let big = b.int_digits(&[0, 0, 1 << 20], false);
    let four = b.int_digits(&[1, 1, 1, 1], true);
    let bad_digit = b.int_digits(&[1 << 30], false);
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    assert_eq!(reason(&d.value(big, 1)), UnreadReason::OutOfRange);
    assert_eq!(reason(&d.value(four, 1)), UnreadReason::OutOfRange);
    assert_eq!(reason(&d.value(bad_digit, 1)), UnreadReason::Malformed);
}

#[test]
fn containers_decode_to_the_depth_limit() {
    for keys in [DictKeys::Sized, DictKeys::Logged] {
        let mut b = Builder::new(keys);
        let one = b.int(1);
        let inner = b.tuple(&[one]);
        let list = b.list(&[inner, one]);
        let k = b.str("k");
        let map = b.dict(&[(k, list)]);
        let (m, rt) = runtime(b);
        let d = Decoder::new(&m, &rt);
        let Kind::Mapping(outer) = kind(&d.value(map, 3)).clone() else {
            panic!("mapping")
        };
        assert_eq!(outer.full_length, None);
        let entry = &outer.entries[0];
        assert_eq!(entry.key.as_ref().map(kind), Some(&Kind::Text("k".into())));
        let Some(Kind::Sequence(seq)) = entry.value.as_ref().map(kind).cloned() else {
            panic!("sequence")
        };
        assert_eq!(seq.kind(), SequenceKind::List);
        assert_eq!(seq.items.len(), 2);
        assert_eq!(seq.full_length, None);
        let Kind::Sequence(t) = kind(&seq.items[0]).clone() else {
            panic!("tuple")
        };
        assert_eq!(t.kind(), SequenceKind::Tuple);
        assert_eq!(t.items.len(), 1);
        // One level less and the tuple is past the limit.
        let Kind::Mapping(shallow) = kind(&d.value(map, 2)).clone() else {
            panic!("mapping")
        };
        let Some(Kind::Sequence(seq)) = shallow.entries[0].value.as_ref().map(kind).cloned() else {
            panic!("sequence")
        };
        assert_eq!(reason(&seq.items[0]), UnreadReason::DepthLimit);
    }
}

#[test]
fn a_long_sequence_is_cut_and_keeps_its_length() {
    let mut b = Builder::new(DictKeys::Logged);
    let items: Vec<u64> = (0..100).map(|i| b.int(i)).collect();
    let list = b.list(&items);
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    let Kind::Sequence(s) = kind(&d.value(list, 1)).clone() else {
        panic!("sequence")
    };
    assert_eq!(s.items.len(), crate::layout::limits::MAX_ITEMS);
    assert_eq!(s.full_length, Some(100));
}

#[test]
fn deleted_dict_entries_are_skipped() {
    for keys in [DictKeys::Sized, DictKeys::Logged] {
        let mut b = Builder::new(keys);
        let (a, c, one, two) = (b.str("a"), b.str("c"), b.int(1), b.int(2));
        let map = b.dict(&[(a, one), (0, 0), (c, two)]);
        let (m, rt) = runtime(b);
        let d = Decoder::new(&m, &rt);
        let (items, used) = d.dict_items(map).expect("a dict");
        assert_eq!(items, vec![(a, one), (c, two)]);
        assert_eq!(used, 2);
    }
}

#[test]
fn an_unknown_kind_is_unread_with_its_type_name() {
    let mut b = Builder::new(DictKeys::Logged);
    let obj = b.foreign("SoulView");
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    let v = d.value(obj, 1);
    let Kind::Unread(u) = kind(&v) else {
        panic!("unread")
    };
    assert_eq!(u.type_name.as_deref(), Some("SoulView"));
    assert_eq!(u.reason(), UnreadReason::UnknownKind);
}

#[test]
fn garbage_is_malformed_and_missing_memory_unreadable() {
    let mut b = Builder::new(DictKeys::Logged);
    let bad = b.int(5);
    // A reference count of zero: a freed object.
    b.put_u64(bad, 0);
    let (m, rt) = runtime(b);
    let d = Decoder::new(&m, &rt);
    assert_eq!(reason(&d.value(bad, 1)), UnreadReason::Malformed);
    let Kind::Unread(u) = kind(&d.value(3, 1)).clone() else {
        panic!("unread")
    };
    // Nothing is known of the type of an object whose header is not one.
    assert_eq!((u.reason(), u.type_name), (UnreadReason::Malformed, None));
    assert_eq!(
        reason(&d.value(HEAP_BASE + (1 << 30), 1)),
        UnreadReason::Unreadable
    );
}
