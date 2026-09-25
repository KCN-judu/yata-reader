//! A synthetic runtime: objects laid out in an [`Image`] exactly as [`super::cpython`] reads them,
//! built here from values, with no game involved. The tests and the public fixtures read these,
//! so every path of the parsers runs in CI, and a fixture carries no one's account.
//!
//! The builder writes only what the reader reads. The index table of a dict is left zeroed,
//! because the reader walks entries in insertion order and never looks a key up.

use super::cpython::{Builtin, DictKeys};
use super::memory::{Image, Overlap, RegionKind};

/// Where the synthetic image and heap start: ordinary 64-bit user-space addresses.
pub const IMAGE_BASE: u64 = 0x1_4000_0000;
pub const HEAP_BASE: u64 = 0x2_0000_0000;

/// Bytes reserved for each type object: past `tp_dict`, the last field the reader reads.
const TYPE_OBJECT: usize = 400;

/// The instance size of `type` itself.
const TYPE_SIZE: (u64, u64) = (880, 40);

/// The instance size of each builtin type; the image holds them in [`Builtin::ALL`]'s order,
/// after `type`.
fn size(b: Builtin) -> (u64, u64) {
    match b {
        Builtin::Dict => (48, 0),
        Builtin::List => (40, 0),
        Builtin::Tuple => (24, 8),
        Builtin::Str => (80, 0),
        Builtin::Int => (24, 4),
        Builtin::Float => (24, 0),
        Builtin::Bool => (32, 4),
        Builtin::NoneType => (16, 0),
    }
}

pub struct Builder {
    keys: DictKeys,
    image: Vec<u8>,
    heap: Vec<u8>,
    type_type: u64,
    /// The builtin types' addresses, by `Builtin as usize`.
    types: [u64; 8],
    none: u64,
    heap_types: Vec<(String, u64)>,
}

impl Builder {
    /// A runtime with the builtin types, `None`, and the `dict` type's attribute dict, whose
    /// key table tells the reader which shape `keys` is.
    pub fn new(keys: DictKeys) -> Builder {
        let mut b = Builder {
            keys,
            image: vec![0; 4096],
            heap: vec![0; 64],
            type_type: 0,
            types: [0; 8],
            none: 0,
            heap_types: Vec::new(),
        };
        let type_type = b.type_object("type", TYPE_SIZE);
        b.type_type = type_type;
        for builtin in Builtin::ALL {
            b.types[builtin as usize] = b.type_object(builtin.name(), size(builtin));
        }
        for t in std::iter::once(type_type).chain(b.types) {
            b.put_u64(t + 8, type_type);
        }
        let none = b.alloc_image(16);
        b.put_u64(none, 5_000);
        b.put_u64(none + 8, b.ty(Builtin::NoneType));
        b.none = none;
        let doc = b.str("__doc__");
        let attrs = b.dict(&[(doc, none)]);
        b.put_u64(b.ty(Builtin::Dict) + 264, attrs);
        b
    }

    /// A type object in the image; its type is written once `type` exists.
    fn type_object(&mut self, name: &str, (basic, item): (u64, u64)) -> u64 {
        let at = self.alloc_image(TYPE_OBJECT);
        let name_at = self.alloc_image(name.len() + 1);
        self.put_image(name_at, name.as_bytes());
        self.put_u64(at, 1_000);
        self.put_u64(at + 24, name_at);
        self.put_u64(at + 32, basic);
        self.put_u64(at + 40, item);
        at
    }

    pub fn type_type(&self) -> u64 {
        self.type_type
    }

    pub fn ty(&self, b: Builtin) -> u64 {
        self.types[b as usize]
    }

    pub fn none(&self) -> u64 {
        self.none
    }

    fn alloc_image(&mut self, n: usize) -> u64 {
        let at = IMAGE_BASE + self.image.len() as u64;
        self.image
            .resize(self.image.len() + n.next_multiple_of(16), 0);
        at
    }

    fn alloc(&mut self, n: usize) -> u64 {
        let at = HEAP_BASE + self.heap.len() as u64;
        self.heap
            .resize(self.heap.len() + n.next_multiple_of(16), 0);
        at
    }

    fn put_image(&mut self, at: u64, bytes: &[u8]) {
        let i = (at - IMAGE_BASE) as usize;
        self.image[i..i + bytes.len()].copy_from_slice(bytes);
    }

    /// Write bytes at an address in either region.
    pub fn put(&mut self, at: u64, bytes: &[u8]) {
        if at >= HEAP_BASE {
            let i = (at - HEAP_BASE) as usize;
            self.heap[i..i + bytes.len()].copy_from_slice(bytes);
        } else {
            self.put_image(at, bytes);
        }
    }

    pub fn put_u64(&mut self, at: u64, v: u64) {
        self.put(at, &v.to_le_bytes());
    }

    /// A heap object's header: reference count 1 and its type.
    fn object(&mut self, size: usize, ty: u64) -> u64 {
        let at = self.alloc(size);
        self.put_u64(at, 1);
        self.put_u64(at + 8, ty);
        at
    }

    /// A str, in the most compact representation its characters allow.
    pub fn str(&mut self, s: &str) -> u64 {
        let chars: Vec<u32> = s.chars().map(u32::from).collect();
        let max = chars.iter().copied().max().unwrap_or(0);
        let (kind, ascii) = match max {
            0..=0x7f => (1u32, true),
            0x80..=0xff => (1, false),
            0x100..=0xffff => (2, false),
            _ => (4, false),
        };
        let data = if ascii { 48 } else { 72 };
        let at = self.object(
            data + chars.len() * kind as usize + kind as usize,
            self.ty(Builtin::Str),
        );
        self.put_u64(at + 16, chars.len() as u64);
        let state = (kind << 2) | (1 << 5) | (u32::from(ascii) << 6) | (1 << 7);
        self.put(at + 32, &state.to_le_bytes());
        let bytes: Vec<u8> = chars
            .iter()
            .flat_map(|&c| c.to_le_bytes()[..kind as usize].to_vec())
            .collect();
        self.put(at + data as u64, &bytes);
        at
    }

    pub fn int(&mut self, n: i64) -> u64 {
        self.long(n, Builtin::Int)
    }

    pub fn boolean(&mut self, b: bool) -> u64 {
        self.long(i64::from(b), Builtin::Bool)
    }

    fn long(&mut self, n: i64, ty: Builtin) -> u64 {
        let mut magnitude = n.unsigned_abs();
        let mut digits = Vec::new();
        while magnitude > 0 {
            digits.push((magnitude & ((1 << 30) - 1)) as u32);
            magnitude >>= 30;
        }
        let at = self.object(24 + 4 * digits.len().max(1), self.ty(ty));
        let size = digits.len() as i64 * n.signum();
        self.put(at + 16, &size.to_le_bytes());
        for (i, d) in digits.iter().enumerate() {
            self.put(at + 24 + 4 * i as u64, &d.to_le_bytes());
        }
        at
    }

    /// An int with these raw 30-bit digits and sign, for values beyond 64 bits.
    pub fn int_digits(&mut self, digits: &[u32], negative: bool) -> u64 {
        let at = self.object(24 + 4 * digits.len(), self.ty(Builtin::Int));
        let size = digits.len() as i64 * if negative { -1 } else { 1 };
        self.put(at + 16, &size.to_le_bytes());
        for (i, d) in digits.iter().enumerate() {
            self.put(at + 24 + 4 * i as u64, &d.to_le_bytes());
        }
        at
    }

    pub fn float(&mut self, x: f64) -> u64 {
        let at = self.object(24, self.ty(Builtin::Float));
        self.put(at + 16, &x.to_le_bytes());
        at
    }

    pub fn list(&mut self, items: &[u64]) -> u64 {
        let at = self.object(40, self.ty(Builtin::List));
        let array = self.alloc(8 * items.len().max(1));
        for (i, &p) in items.iter().enumerate() {
            self.put_u64(array + 8 * i as u64, p);
        }
        self.put(at + 16, &(items.len() as i64).to_le_bytes());
        self.put_u64(at + 24, array);
        self.put(at + 32, &(items.len() as i64).to_le_bytes());
        at
    }

    pub fn tuple(&mut self, items: &[u64]) -> u64 {
        let at = self.object(24 + 8 * items.len(), self.ty(Builtin::Tuple));
        self.put(at + 16, &(items.len() as i64).to_le_bytes());
        for (i, &p) in items.iter().enumerate() {
            self.put_u64(at + 24 + 8 * i as u64, p);
        }
        at
    }

    /// A dict of these entries, in this order. An entry with a zero key is a deleted slot.
    pub fn dict(&mut self, entries: &[(u64, u64)]) -> u64 {
        let live = entries.iter().filter(|e| e.0 != 0).count();
        let mut size = 8u64;
        while size * 2 / 3 < entries.len() as u64 {
            size *= 2;
        }
        let keys = match self.keys {
            DictKeys::Sized => {
                let first = 40 + size as usize;
                let at = self.alloc(first + 24 * entries.len());
                self.put_u64(at, 1);
                self.put_u64(at + 8, size);
                self.put_u64(at + 24, size * 2 / 3 - entries.len() as u64);
                self.put_u64(at + 32, entries.len() as u64);
                for (i, &(k, v)) in entries.iter().enumerate() {
                    let e = at + first as u64 + 24 * i as u64;
                    self.put_u64(e + 8, k);
                    self.put_u64(e + 16, if k == 0 { 0 } else { v });
                }
                at
            }
            DictKeys::Logged => {
                let log2 = size.trailing_zeros() as u8;
                let first = 32 + size as usize;
                let at = self.alloc(first + 24 * entries.len());
                self.put_u64(at, 1);
                // A general table: (hash, key, value) entries.
                self.put(at + 8, &[log2, log2, 0]);
                self.put_u64(at + 16, size * 2 / 3 - entries.len() as u64);
                self.put_u64(at + 24, entries.len() as u64);
                for (i, &(k, v)) in entries.iter().enumerate() {
                    let e = at + first as u64 + 24 * i as u64;
                    self.put_u64(e + 8, k);
                    self.put_u64(e + 16, if k == 0 { 0 } else { v });
                }
                at
            }
        };
        let at = self.object(48, self.ty(Builtin::Dict));
        self.put(at + 16, &(live as i64).to_le_bytes());
        self.put_u64(at + 32, keys);
        at
    }

    /// An instance of a type the decoder does not know, named `name`.
    pub fn foreign(&mut self, name: &str) -> u64 {
        let ty = match self.heap_types.iter().find(|t| t.0 == name) {
            Some(t) => t.1,
            None => {
                let t = self.alloc(TYPE_OBJECT);
                let n = self.alloc(name.len() + 1);
                self.put(n, name.as_bytes());
                self.put_u64(t, 3);
                self.put_u64(t + 8, self.type_type);
                self.put_u64(t + 24, n);
                self.heap_types.push((name.to_owned(), t));
                t
            }
        };
        self.object(16, ty)
    }

    /// The image: the type objects' region and the heap, unless the image grew into the heap.
    pub fn finish(self) -> Result<Image, Overlap> {
        let mut m = Image::new();
        m.add(IMAGE_BASE, RegionKind::Image, true, self.image)?;
        m.add(HEAP_BASE, RegionKind::Private, true, self.heap)?;
        Ok(m)
    }
}
