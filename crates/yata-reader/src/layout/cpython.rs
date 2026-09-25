//! The object layout of the game's embedded runtime, CPython before 3.12, on 64-bit Windows, read
//! from outside.
//!
//! Every offset here is CPython's public object layout for those versions, not something learnt
//! from the game. What is learnt from the game is only which layout it uses, and that is checked
//! at attach, not assumed: the `type` object must point at itself, each builtin type must be found
//! exactly once in loaded images with the instance sizes this layout gives it, and the dict key
//! table's header must be one of the two known shapes. A game build that fails any check is a
//! layout mismatch, never a guess.
//!
//! Every read is bounded by the limits in [`super::limits`], and every value that fails a check
//! is reported as unread with the reason, not dropped and not repaired.

use std::cell::RefCell;
use std::collections::HashMap;
use std::convert::Infallible;

use yata_protocol::probe::{
    RawEntry, RawMapping, RawNull, RawSequence, RawUnread, RawValue, SequenceKind, UnreadReason,
    raw_value::Kind,
};

use super::limits::{
    MAX_DEPTH, MAX_DICT_ENTRIES, MAX_ITEMS, MAX_REFCOUNT, MAX_TEXT_CHARS, MAX_TYPE_NAME,
    MIN_POINTER, POINTER_END, SCAN_CHUNK,
};
use super::memory::{Memory, RegionKind};

// The object header, and the fields of a type object this reader uses.
const OB_REFCNT: u64 = 0;
const OB_TYPE: u64 = 8;
const OB_SIZE: u64 = 16;
const TP_NAME: u64 = 24;
const TP_BASICSIZE: u64 = 32;
const TP_ITEMSIZE: u64 = 40;
const TP_DICT: u64 = 264;

// Instances.
const FLOAT_VALUE: u64 = 16;
const INT_DIGITS: u64 = 24;
const INT_DIGIT_BITS: u32 = 30;
const LIST_ITEMS: u64 = 24;
const TUPLE_ITEMS: u64 = 24;
const DICT_USED: u64 = 16;
const DICT_KEYS: u64 = 32;
const DICT_VALUES: u64 = 40;
const STR_LENGTH: u64 = 16;
const STR_STATE: u64 = 32;
const STR_ASCII_DATA: u64 = 48;
const STR_COMPACT_DATA: u64 = 72;
const STR_DATA_POINTER: u64 = 72;

/// The builtin types the value decoder knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Builtin {
    Dict,
    List,
    Tuple,
    Str,
    Int,
    Float,
    Bool,
    NoneType,
}

impl Builtin {
    pub const ALL: [Builtin; 8] = [
        Builtin::Dict,
        Builtin::List,
        Builtin::Tuple,
        Builtin::Str,
        Builtin::Int,
        Builtin::Float,
        Builtin::Bool,
        Builtin::NoneType,
    ];

    /// The type's `tp_name`.
    pub fn name(self) -> &'static str {
        match self {
            Builtin::Dict => "dict",
            Builtin::List => "list",
            Builtin::Tuple => "tuple",
            Builtin::Str => "str",
            Builtin::Int => "int",
            Builtin::Float => "float",
            Builtin::Bool => "bool",
            Builtin::NoneType => "NoneType",
        }
    }

    /// The instance size (`tp_basicsize`, `tp_itemsize`) this layout gives the type, for the
    /// types whose size the layout check reads. A str of 80 bytes is the representation with a
    /// `wstr` field, which 3.12 removed.
    fn size(self) -> Option<(u64, u64)> {
        match self {
            Builtin::Str => Some((80, 0)),
            Builtin::Dict => Some((48, 0)),
            Builtin::Float => Some((24, 0)),
            Builtin::List => Some((40, 0)),
            Builtin::Tuple => Some((24, 8)),
            Builtin::Int => Some((24, 4)),
            Builtin::Bool | Builtin::NoneType => None,
        }
    }
}

/// Why the target's memory is not a runtime this reader can read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LayoutError {
    /// No object in a loaded image is its own type named `type`, or more than one is.
    TypeObject { found: usize },
    /// A builtin type is not found exactly once in loaded images.
    Builtin { builtin: Builtin, found: usize },
    /// A builtin type's instance size is not this layout's.
    Size {
        builtin: Builtin,
        basic: u64,
        item: u64,
    },
    /// A builtin type's instance size could not be read.
    SizeUnreadable { builtin: Builtin },
    /// The dict key table header matches neither known shape.
    DictKeys,
}

/// A read stopped at the daemon's `Cancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cancelled;

/// One chunk a scan copied, or failed to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    /// The index, in [`Memory::regions`], of the region the chunk is in.
    pub region: usize,
    pub bytes: u64,
    pub readable: bool,
}

/// The two shapes of the dict key table in the versions this layout covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DictKeys {
    /// 3.6 to 3.10: a `Py_ssize_t` size, a lookup function, then the index table.
    Sized,
    /// 3.11: byte-wide log sizes and a kind, then the index table.
    Logged,
}

/// The addresses of the builtin types, and the key table shape: everything needed to decode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Runtime {
    pub type_type: u64,
    pub dict: u64,
    pub list: u64,
    pub tuple: u64,
    pub str_: u64,
    pub int: u64,
    pub float: u64,
    pub bool_: u64,
    pub none_type: u64,
    pub keys: DictKeys,
}

impl Runtime {
    /// The engine name the reader reports for this runtime.
    pub fn engine(&self) -> &'static str {
        match self.keys {
            DictKeys::Sized => "cpython-3.6-3.10",
            DictKeys::Logged => "cpython-3.11",
        }
    }

    /// The address of a builtin type.
    pub fn address(&self, b: Builtin) -> u64 {
        match b {
            Builtin::Dict => self.dict,
            Builtin::List => self.list,
            Builtin::Tuple => self.tuple,
            Builtin::Str => self.str_,
            Builtin::Int => self.int,
            Builtin::Float => self.float,
            Builtin::Bool => self.bool_,
            Builtin::NoneType => self.none_type,
        }
    }

    /// Which builtin type a type address is, if any.
    pub fn builtin(&self, t: u64) -> Option<Builtin> {
        Builtin::ALL.into_iter().find(|&b| self.address(b) == t)
    }
}

/// Whether a value looks like a user-space pointer.
pub fn plausible_pointer(p: u64) -> bool {
    (MIN_POINTER..POINTER_END).contains(&p) && p.is_multiple_of(8)
}

/// Every aligned address in the regions `pick` accepts whose 8 bytes satisfy `hit`, in address
/// order. `checkpoint` runs before each chunk, and its error stops the scan; `on_chunk` hears of
/// each chunk copied or not.
pub fn scan<E>(
    mem: &dyn Memory,
    pick: impl Fn(RegionKind, bool) -> bool,
    mut hit: impl FnMut(u64, u64) -> bool,
    mut checkpoint: impl FnMut() -> Result<(), E>,
    mut on_chunk: impl FnMut(Chunk),
) -> Result<Vec<u64>, E> {
    let mut hits = Vec::new();
    let mut buf = vec![0u8; SCAN_CHUNK as usize];
    for (region, r) in mem.regions().iter().enumerate() {
        if !pick(r.kind, r.writable) {
            continue;
        }
        let mut at = r.base;
        while at < r.end() {
            checkpoint()?;
            let len = SCAN_CHUNK.min(r.end() - at) as usize;
            let chunk = &mut buf[..len];
            let readable = mem.read_into(at, chunk).is_ok();
            if readable {
                for (i, q) in chunk.chunks_exact(8).enumerate() {
                    let address = at + 8 * i as u64;
                    let mut b = [0u8; 8];
                    b.copy_from_slice(q);
                    if hit(address, u64::from_le_bytes(b)) {
                        hits.push(address);
                    }
                }
            }
            on_chunk(Chunk {
                region,
                bytes: len as u64,
                readable,
            });
            at += len as u64;
        }
    }
    Ok(hits)
}

/// A scan that runs to its end.
fn scan_whole(
    mem: &dyn Memory,
    pick: impl Fn(RegionKind, bool) -> bool,
    hit: impl FnMut(u64, u64) -> bool,
) -> Vec<u64> {
    let Ok(hits) = scan(mem, pick, hit, || Ok::<(), Infallible>(()), |_| ());
    hits
}

/// A NUL-terminated byte string of at most `max` bytes, read in small steps so a name near the
/// end of a region is still read.
pub fn c_string(mem: &dyn Memory, p: u64, max: usize) -> Option<String> {
    let mut out = Vec::new();
    let mut at = p;
    while out.len() < max {
        let mut b = [0u8; 16];
        mem.read_into(at, &mut b).ok()?;
        for &c in &b {
            if c == 0 {
                return String::from_utf8(out).ok();
            }
            if !(0x20..0x7f).contains(&c) {
                return None;
            }
            out.push(c);
        }
        at += 16;
    }
    None
}

/// Find the runtime's types in the target's loaded images.
pub fn discover(mem: &dyn Memory) -> Result<Runtime, LayoutError> {
    let image = |kind: RegionKind, _writable: bool| kind == RegionKind::Image;
    let name_of = |object: u64| {
        mem.read_u64(object + TP_NAME)
            .ok()
            .filter(|p| (MIN_POINTER..POINTER_END).contains(p))
            .and_then(|p| c_string(mem, p, MAX_TYPE_NAME))
    };
    // `type` is the one object whose type is itself: its ob_type field holds its own address.
    let own_type = scan_whole(mem, image, |a, v| a >= OB_TYPE && v == a - OB_TYPE);
    let type_type: Vec<u64> = own_type
        .into_iter()
        .map(|a| a - OB_TYPE)
        .filter(|&o| name_of(o).as_deref() == Some("type"))
        .collect();
    let [type_type] = type_type.as_slice() else {
        return Err(LayoutError::TypeObject {
            found: type_type.len(),
        });
    };
    let type_type = *type_type;
    // Every builtin type is an instance of `type` in a loaded image.
    let instances = scan_whole(mem, image, |_, v| v == type_type);
    let mut by_builtin: HashMap<Builtin, Vec<u64>> = HashMap::new();
    for a in instances {
        let object = a - OB_TYPE;
        if let Some(n) = name_of(object)
            && let Some(known) = Builtin::ALL.into_iter().find(|b| b.name() == n)
        {
            by_builtin.entry(known).or_default().push(object);
        }
    }
    let one = |builtin: Builtin| match by_builtin.get(&builtin).map(Vec::as_slice) {
        Some([t]) => Ok(*t),
        other => Err(LayoutError::Builtin {
            builtin,
            found: other.map_or(0, <[u64]>::len),
        }),
    };
    for builtin in Builtin::ALL {
        let t = one(builtin)?;
        let Some((basic, item)) = builtin.size() else {
            continue;
        };
        let (Ok(b), Ok(i)) = (
            mem.read_u64(t + TP_BASICSIZE),
            mem.read_u64(t + TP_ITEMSIZE),
        ) else {
            return Err(LayoutError::SizeUnreadable { builtin });
        };
        if (b, i) != (basic, item) {
            return Err(LayoutError::Size {
                builtin,
                basic: b,
                item: i,
            });
        }
    }
    let dict = one(Builtin::Dict)?;
    Ok(Runtime {
        type_type,
        dict,
        list: one(Builtin::List)?,
        tuple: one(Builtin::Tuple)?,
        str_: one(Builtin::Str)?,
        int: one(Builtin::Int)?,
        float: one(Builtin::Float)?,
        bool_: one(Builtin::Bool)?,
        none_type: one(Builtin::NoneType)?,
        keys: key_shape(mem, dict).ok_or(LayoutError::DictKeys)?,
    })
}

/// The key table shape, read from the attribute dict of the `dict` type itself.
fn key_shape(mem: &dyn Memory, dict: u64) -> Option<DictKeys> {
    let d = mem.read_u64(dict + TP_DICT).ok()?;
    if mem.read_u64(d + OB_TYPE).ok()? != dict {
        return None;
    }
    let keys = mem.read_u64(d + DICT_KEYS).ok()?;
    let header = mem.read_vec(keys + 8, 8).ok()?;
    let sized = u64::from_le_bytes(header.as_slice().try_into().ok()?);
    if sized.is_power_of_two() && (8..=1 << 30).contains(&sized) {
        return Some(DictKeys::Sized);
    }
    let (log2_size, log2_index, kind) = (header[0], header[1], header[2]);
    ((3..=30).contains(&log2_size) && log2_index == index_log2(log2_size) && kind <= 2)
        .then_some(DictKeys::Logged)
}

/// 3.11's `dk_log2_index_bytes` for a table of `2^log2_size` slots.
fn index_log2(log2_size: u8) -> u8 {
    match log2_size {
        0..8 => log2_size,
        8..16 => log2_size + 1,
        16..32 => log2_size + 2,
        _ => log2_size + 3,
    }
}

/// A value's decoding failed a check.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bad {
    Unreadable,
    Malformed,
    OutOfRange,
}

fn read<T>(r: Result<T, super::memory::Unreadable>) -> Result<T, Bad> {
    r.map_err(|_| Bad::Unreadable)
}

/// Decodes objects of one runtime. Type names are cached by type address, a name that could not
/// be read as such.
pub struct Decoder<'m> {
    pub mem: &'m dyn Memory,
    pub rt: &'m Runtime,
    names: RefCell<HashMap<u64, Option<String>>>,
}

/// The two sequence types, whose items are found in different places.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Sequence {
    List,
    Tuple,
}

impl<'m> Decoder<'m> {
    pub fn new(mem: &'m dyn Memory, rt: &'m Runtime) -> Decoder<'m> {
        Decoder {
            mem,
            rt,
            names: RefCell::new(HashMap::new()),
        }
    }

    /// An object's type, if its header is plausible.
    pub fn type_of(&self, object: u64) -> Result<u64, Bad> {
        if !plausible_pointer(object) {
            return Err(Bad::Malformed);
        }
        let refcnt = read(self.mem.read_i64(object + OB_REFCNT))?;
        if !(1..=MAX_REFCOUNT).contains(&refcnt) {
            return Err(Bad::Malformed);
        }
        let t = read(self.mem.read_u64(object + OB_TYPE))?;
        if plausible_pointer(t) {
            Ok(t)
        } else {
            Err(Bad::Malformed)
        }
    }

    /// A type's name, or `None` if it cannot be read.
    pub fn type_name(&self, t: u64) -> Option<String> {
        if let Some(n) = self.names.borrow().get(&t) {
            return n.clone();
        }
        let n = self
            .mem
            .read_u64(t + TP_NAME)
            .ok()
            .and_then(|p| c_string(self.mem, p, MAX_TYPE_NAME));
        self.names.borrow_mut().insert(t, n.clone());
        n
    }

    /// A str object's text.
    pub fn text(&self, object: u64) -> Result<String, Bad> {
        if self.type_of(object)? != self.rt.str_ {
            return Err(Bad::Malformed);
        }
        let length = read(self.mem.read_i64(object + STR_LENGTH))?;
        let length = usize::try_from(length).map_err(|_| Bad::Malformed)?;
        if length > MAX_TEXT_CHARS {
            return Err(Bad::OutOfRange);
        }
        let state = read(self.mem.read_u32(object + STR_STATE))?;
        let (kind, compact, ascii, ready) = (
            (state >> 2) & 7,
            state >> 5 & 1 == 1,
            state >> 6 & 1 == 1,
            state >> 7 & 1 == 1,
        );
        if !ready || !matches!(kind, 1 | 2 | 4) || (ascii && kind != 1) {
            return Err(Bad::Malformed);
        }
        let data = match (compact, ascii) {
            (true, true) => object + STR_ASCII_DATA,
            (true, false) => object + STR_COMPACT_DATA,
            (false, _) => read(self.mem.read_u64(object + STR_DATA_POINTER))?,
        };
        let bytes = read(self.mem.read_vec(data, length * kind as usize))?;
        let units: Vec<u32> = match kind {
            1 => bytes.iter().map(|&b| u32::from(b)).collect(),
            2 => bytes
                .chunks_exact(2)
                .map(|c| u32::from(u16::from_le_bytes([c[0], c[1]])))
                .collect(),
            _ => bytes
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect(),
        };
        if ascii && units.iter().any(|&u| u > 0x7f) {
            return Err(Bad::Malformed);
        }
        units
            .into_iter()
            .map(|u| char::from_u32(u).ok_or(Bad::Malformed))
            .collect()
    }

    /// An int object's value, if it fits 64 bits.
    pub fn integer(&self, object: u64) -> Result<i64, Bad> {
        let size = read(self.mem.read_i64(object + OB_SIZE))?;
        let digits = size.unsigned_abs();
        if digits > 3 {
            return Err(Bad::OutOfRange);
        }
        let raw = read(self.mem.read_vec(object + INT_DIGITS, 4 * digits as usize))?;
        let mut magnitude: u128 = 0;
        for (i, c) in raw.chunks_exact(4).enumerate() {
            let d = u32::from_le_bytes([c[0], c[1], c[2], c[3]]);
            if d >> INT_DIGIT_BITS != 0 {
                return Err(Bad::Malformed);
            }
            magnitude |= u128::from(d) << (INT_DIGIT_BITS as usize * i);
        }
        let value = if size < 0 {
            0i128 - magnitude as i128
        } else {
            magnitude as i128
        };
        i64::try_from(value).map_err(|_| Bad::OutOfRange)
    }

    /// The live `(key, value)` pairs of a dict, in insertion order, and its `ma_used`.
    pub fn dict_items(&self, object: u64) -> Result<(Vec<(u64, u64)>, u64), Bad> {
        let used = read(self.mem.read_i64(object + DICT_USED))?;
        let used = u64::try_from(used).map_err(|_| Bad::Malformed)?;
        let keys = read(self.mem.read_u64(object + DICT_KEYS))?;
        let values = read(self.mem.read_u64(object + DICT_VALUES))?;
        if !plausible_pointer(keys) || (values != 0 && !plausible_pointer(values)) {
            return Err(Bad::Malformed);
        }
        let (size, nentries, first, stride) = match self.rt.keys {
            DictKeys::Sized => {
                let size = read(self.mem.read_u64(keys + 8))?;
                let nentries = read(self.mem.read_i64(keys + 32))?;
                if !size.is_power_of_two() || !(8..=1 << 30).contains(&size) {
                    return Err(Bad::Malformed);
                }
                let width = match size {
                    0..=0xff => 1,
                    0x100..=0xffff => 2,
                    0x1_0000..=0xffff_ffff => 4,
                    _ => 8,
                };
                (size, nentries, keys + 40 + size * width, 24u64)
            }
            DictKeys::Logged => {
                let h = read(self.mem.read_vec(keys + 8, 3))?;
                let (log2_size, log2_index, kind) = (h[0], h[1], h[2]);
                if !(3..=30).contains(&log2_size) || log2_index != index_log2(log2_size) || kind > 2
                {
                    return Err(Bad::Malformed);
                }
                let nentries = read(self.mem.read_i64(keys + 24))?;
                let stride = if kind == 0 { 24 } else { 16 };
                (
                    1u64 << log2_size,
                    nentries,
                    keys + 32 + (1u64 << log2_index),
                    stride,
                )
            }
        };
        let nentries = u64::try_from(nentries).map_err(|_| Bad::Malformed)?;
        if nentries > size || used > nentries || nentries > MAX_DICT_ENTRIES {
            return Err(Bad::Malformed);
        }
        let table = read(self.mem.read_vec(first, (nentries * stride) as usize))?;
        let split = if values != 0 {
            Some(read(self.mem.read_vec(values, 8 * nentries as usize))?)
        } else {
            None
        };
        let word = |b: &[u8], at: usize| {
            let mut w = [0u8; 8];
            w.copy_from_slice(&b[at..at + 8]);
            u64::from_le_bytes(w)
        };
        let mut items = Vec::with_capacity(used as usize);
        for i in 0..nentries as usize {
            let e = i * stride as usize;
            // A general entry is (hash, key, value); a unicode or split one is (key, value).
            let key_at = if stride == 24 { e + 8 } else { e };
            let key = word(&table, key_at);
            let value = match &split {
                Some(v) => word(v, 8 * i),
                None => word(&table, key_at + 8),
            };
            if key != 0 && value != 0 {
                items.push((key, value));
            }
        }
        if items.len() as u64 != used {
            return Err(Bad::Malformed);
        }
        Ok((items, used))
    }

    fn sequence(&self, object: u64, kind: Sequence) -> Result<(Vec<u64>, u64), Bad> {
        let n = read(self.mem.read_i64(object + OB_SIZE))?;
        let n = u64::try_from(n).map_err(|_| Bad::Malformed)?;
        let shown = n.min(MAX_ITEMS as u64);
        let items = match kind {
            Sequence::Tuple => object + TUPLE_ITEMS,
            Sequence::List => read(self.mem.read_u64(object + LIST_ITEMS))?,
        };
        if shown > 0 && !plausible_pointer(items) {
            return Err(Bad::Malformed);
        }
        let raw = read(self.mem.read_vec(items, 8 * shown as usize))?;
        let pointers = raw
            .chunks_exact(8)
            .map(|c| {
                let mut w = [0u8; 8];
                w.copy_from_slice(c);
                u64::from_le_bytes(w)
            })
            .collect();
        Ok((pointers, n))
    }

    /// Any object as a raw value, `depth` levels of containers deep.
    pub fn value(&self, object: u64, depth: usize) -> RawValue {
        let t = match self.type_of(object) {
            Ok(t) => t,
            Err(b) => return unread(None, b),
        };
        let Some(builtin) = self.rt.builtin(t) else {
            return unread_reason(self.type_name(t), UnreadReason::UnknownKind);
        };
        // The depth left for a container's items; a scalar has none to pass on.
        let inner = match builtin {
            Builtin::Dict | Builtin::List | Builtin::Tuple => match depth.checked_sub(1) {
                Some(inner) => inner,
                None => {
                    return unread_reason(
                        Some(builtin.name().to_owned()),
                        UnreadReason::DepthLimit,
                    );
                }
            },
            _ => 0,
        };
        let sequence = |kind: Sequence| {
            self.sequence(object, kind).map(|(items, length)| {
                Kind::Sequence(RawSequence {
                    kind: match kind {
                        Sequence::List => SequenceKind::List,
                        Sequence::Tuple => SequenceKind::Tuple,
                    }
                    .into(),
                    full_length: ((items.len() as u64) < length).then_some(length),
                    items: items.iter().map(|&i| self.value(i, inner)).collect(),
                })
            })
        };
        let kind = match builtin {
            Builtin::NoneType => Ok(Kind::Null(RawNull {})),
            Builtin::Bool => self.integer(object).map(|n| Kind::Boolean(n != 0)),
            Builtin::Int => self.integer(object).map(Kind::Integer),
            Builtin::Float => read(self.mem.read_f64(object + FLOAT_VALUE)).map(Kind::Float),
            Builtin::Str => self.text(object).map(Kind::Text),
            Builtin::List => sequence(Sequence::List),
            Builtin::Tuple => sequence(Sequence::Tuple),
            Builtin::Dict => self.dict_items(object).map(|(items, used)| {
                let shown: Vec<RawEntry> = items
                    .iter()
                    .take(MAX_ITEMS)
                    .map(|&(k, v)| RawEntry {
                        key: Some(self.value(k, inner)),
                        value: Some(self.value(v, inner)),
                    })
                    .collect();
                Kind::Mapping(RawMapping {
                    full_length: ((shown.len() as u64) < used).then_some(used),
                    entries: shown,
                })
            }),
        };
        match kind {
            Ok(k) => RawValue { kind: Some(k) },
            Err(b) => unread(Some(builtin.name().to_owned()), b),
        }
    }

    /// A value `MAX_DEPTH` deep: what a record's entries are read as.
    pub fn entry_value(&self, object: u64) -> RawValue {
        self.value(object, MAX_DEPTH)
    }
}

fn unread(type_name: Option<String>, bad: Bad) -> RawValue {
    unread_reason(
        type_name,
        match bad {
            Bad::Unreadable => UnreadReason::Unreadable,
            Bad::Malformed => UnreadReason::Malformed,
            Bad::OutOfRange => UnreadReason::OutOfRange,
        },
    )
}

fn unread_reason(type_name: Option<String>, reason: UnreadReason) -> RawValue {
    RawValue {
        kind: Some(Kind::Unread(RawUnread {
            type_name,
            reason: reason.into(),
        })),
    }
}

#[cfg(test)]
#[path = "cpython_tests.rs"]
mod tests;
