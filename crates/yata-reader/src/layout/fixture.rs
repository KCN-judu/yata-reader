//! The synthetic inventory the public fixtures and tests read: made-up values, in the shape the
//! recognition rule accepts, in a synthetic runtime. Nothing here is a claim about what the game
//! stores; it exercises every path the reader has.

use super::cpython::DictKeys;
use super::memory::{Image, Overlap};
use super::synthetic::Builder;

/// Container keys of the three souls the inventory holds, in its order.
pub const IDS: [&str; 3] = [
    "000000000000000000000001",
    "000000000000000000000002",
    "000000000000000000000003",
];

/// Three souls in a container, a fourth outside any container, a second container holding one
/// of the three, a dict with only one marker key, which is not a soul, and a dict with two
/// marker keys whose values are texts, as in the runtime's table of interned names, which the
/// rule rejects.
pub fn inventory(keys: DictKeys) -> Result<Image, Overlap> {
    let mut b = Builder::new(keys);
    let k = |b: &mut Builder, s: &str| b.str(s);
    let (base_rindex, rattr, single_attr, others, sattr, base_r, note) = (
        k(&mut b, "base_rindex"),
        k(&mut b, "rattr"),
        k(&mut b, "single_attr"),
        k(&mut b, "others"),
        k(&mut b, "sattr"),
        k(&mut b, "base_r"),
        k(&mut b, "synthetic_note"),
    );
    let soul = |b: &mut Builder, n: i64, innate: Option<i64>| {
        let pairs: Vec<u64> = (0..n)
            .map(|i| {
                let (code, value) = (b.int(i + 1), b.float(0.5 * i as f64));
                b.tuple(&[code, value])
            })
            .collect();
        let values = [
            b.int(n),
            b.list(&pairs),
            match innate {
                Some(v) => b.int(v),
                None => b.none(),
            },
            b.int(1 << 40 | n),
            b.str(&format!("synthetic-{n}")),
            b.float(10.0 * n as f64),
        ];
        b.dict(&[
            (base_rindex, values[0]),
            (rattr, values[1]),
            (single_attr, values[2]),
            (others, values[3]),
            (sattr, values[4]),
            (base_r, values[5]),
        ])
    };
    let s1 = soul(&mut b, 1, None);
    let s2 = soul(&mut b, 2, Some(7));
    let s3 = soul(&mut b, 3, None);
    let stray = soul(&mut b, 4, Some(9));
    let ids: Vec<u64> = IDS.iter().map(|id| b.str(id)).collect();
    b.dict(&[(ids[0], s1), (ids[1], s2), (ids[2], s3)]);
    let slot = b.int(2);
    b.dict(&[(slot, s2)]);
    let one = b.int(1);
    let text = b.str("not a soul");
    b.dict(&[(base_rindex, one), (note, text)]);
    b.dict(&[(base_rindex, base_rindex), (rattr, rattr)]);
    let _ = stray;
    b.finish()
}
