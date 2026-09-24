//! Every bound the parsers read under. Memory the game controls decides nothing about how much the
//! reader allocates, how deep it recurses, or how long it runs.

/// Container levels decoded below a record's entries; deeper values are unread.
pub const MAX_DEPTH: usize = 4;
/// Items of a sequence, or entries of a mapping, decoded; the true length is still reported.
pub const MAX_ITEMS: usize = 64;
/// Characters of a text decoded; a longer text is out of range.
pub const MAX_TEXT_CHARS: usize = 4096;
/// Bytes of a type name read.
pub const MAX_TYPE_NAME: usize = 64;
/// The largest reference count an object header may claim.
pub const MAX_REFCOUNT: i64 = 1 << 40;
/// Entries of one dict read: more is a malformed dict, not an allocation.
pub const MAX_DICT_ENTRIES: u64 = 1 << 20;
/// The lowest address a pointer may hold, and the end of user space on 64-bit Windows.
pub const MIN_POINTER: u64 = 0x1_0000;
pub const POINTER_END: u64 = 0x8000_0000_0000;
/// Bytes copied from the target at a time while scanning.
pub const SCAN_CHUNK: u64 = 1 << 20;
/// Souls one reading reports; more candidates are left out and counted.
pub const MAX_SOULS: usize = 100_000;
