//! Another process's memory as the parsers see it: a list of readable regions and a way to copy
//! bytes out of them. The desktop channel implements it over the game process; [`Image`]
//! implements it over bytes held here, for tests and fixtures. Parsing never needs to know which.

use std::cell::RefCell;
use std::collections::HashMap;

/// What backs a region, as the operating system reports it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionKind {
    /// Memory the process allocated: its heaps.
    Private,
    /// A loaded executable or library.
    Image,
    /// A mapped file or section.
    Mapped,
}

/// One committed, readable region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub base: u64,
    pub size: u64,
    pub kind: RegionKind,
    pub writable: bool,
}

impl Region {
    pub fn end(&self) -> u64 {
        self.base.saturating_add(self.size)
    }

    pub fn contains(&self, address: u64) -> bool {
        address >= self.base && address < self.end()
    }
}

/// Bytes that could not be copied: not committed, not readable, or gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unreadable {
    pub address: u64,
    pub len: usize,
}

/// A process's memory, read-only.
pub trait Memory {
    /// The readable regions, in address order.
    fn regions(&self) -> &[Region];

    /// Fill `buf` from `address`, all of it or nothing.
    fn read_into(&self, address: u64, buf: &mut [u8]) -> Result<(), Unreadable>;

    fn read_u64(&self, address: u64) -> Result<u64, Unreadable> {
        let mut b = [0u8; 8];
        self.read_into(address, &mut b)?;
        Ok(u64::from_le_bytes(b))
    }

    fn read_i64(&self, address: u64) -> Result<i64, Unreadable> {
        let mut b = [0u8; 8];
        self.read_into(address, &mut b)?;
        Ok(i64::from_le_bytes(b))
    }

    fn read_u32(&self, address: u64) -> Result<u32, Unreadable> {
        let mut b = [0u8; 4];
        self.read_into(address, &mut b)?;
        Ok(u32::from_le_bytes(b))
    }

    fn read_f64(&self, address: u64) -> Result<f64, Unreadable> {
        let mut b = [0u8; 8];
        self.read_into(address, &mut b)?;
        Ok(f64::from_le_bytes(b))
    }

    fn read_vec(&self, address: u64, len: usize) -> Result<Vec<u8>, Unreadable> {
        let mut v = vec![0u8; len];
        self.read_into(address, &mut v)?;
        Ok(v)
    }
}

/// Memory held in this process: regions and their bytes. What the tests and the synthetic
/// fixtures read.
#[derive(Debug, Clone, Default)]
pub struct Image {
    regions: Vec<Region>,
    bytes: Vec<Vec<u8>>,
}

impl Image {
    pub fn new() -> Image {
        Image::default()
    }

    /// Add a region holding `bytes`. Regions must not overlap.
    pub fn add(&mut self, base: u64, kind: RegionKind, writable: bool, bytes: Vec<u8>) {
        let region = Region {
            base,
            size: bytes.len() as u64,
            kind,
            writable,
        };
        let at = self.regions.partition_point(|r| r.base < base);
        self.regions.insert(at, region);
        self.bytes.insert(at, bytes);
    }
}

impl Memory for Image {
    fn regions(&self) -> &[Region] {
        &self.regions
    }

    fn read_into(&self, address: u64, buf: &mut [u8]) -> Result<(), Unreadable> {
        let unreadable = Unreadable {
            address,
            len: buf.len(),
        };
        let i = self
            .regions
            .iter()
            .position(|r| r.contains(address))
            .ok_or(unreadable)?;
        let start = usize::try_from(address - self.regions[i].base).map_err(|_| unreadable)?;
        let end = start.checked_add(buf.len()).ok_or(unreadable)?;
        let src = self.bytes[i].get(start..end).ok_or(unreadable)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// The block a [`Cached`] memory copies at once.
pub const BLOCK: u64 = 64 * 1024;
/// Blocks a [`Cached`] memory keeps before it starts over: 64 MiB.
pub const CACHED_BLOCKS: usize = 1024;

/// A memory whose reads go through a cache of whole blocks, so the many small reads a parser
/// makes near one another cost one copy from the other process.
pub struct Cached<M> {
    inner: M,
    blocks: RefCell<HashMap<u64, Option<Box<[u8]>>>>,
}

impl<M: Memory> Cached<M> {
    pub fn new(inner: M) -> Cached<M> {
        Cached {
            inner,
            blocks: RefCell::new(HashMap::new()),
        }
    }

    pub fn inner(&self) -> &M {
        &self.inner
    }

    /// The start of the block holding `address`, and the block's length. Blocks are counted from
    /// the start of the region that holds the address, never across a region's edge: a region
    /// is only page-aligned, so a block aligned to anything larger could begin in the region
    /// before it.
    fn block_of(&self, address: u64) -> Option<(u64, u64)> {
        let regions = self.inner.regions();
        let i = regions.partition_point(|r| r.end() <= address);
        let r = regions.get(i).filter(|r| r.contains(address))?;
        let start = r.base + (address - r.base) / BLOCK * BLOCK;
        Some((start, BLOCK.min(r.end() - start)))
    }

    /// The block at `start`, or `None` if it cannot be read whole.
    fn block(&self, start: u64, len: u64) -> Option<Box<[u8]>> {
        let mut buf = vec![0u8; usize::try_from(len).ok()?];
        self.inner.read_into(start, &mut buf).ok()?;
        Some(buf.into_boxed_slice())
    }
}

impl<M: Memory> Memory for Cached<M> {
    fn regions(&self) -> &[Region] {
        self.inner.regions()
    }

    fn read_into(&self, address: u64, buf: &mut [u8]) -> Result<(), Unreadable> {
        let unreadable = Unreadable {
            address,
            len: buf.len(),
        };
        let mut done = 0usize;
        while done < buf.len() {
            let at = address.checked_add(done as u64).ok_or(unreadable)?;
            let (start, len) = self.block_of(at).ok_or(unreadable)?;
            let mut blocks = self.blocks.borrow_mut();
            if blocks.len() >= CACHED_BLOCKS && !blocks.contains_key(&start) {
                blocks.clear();
            }
            let block = blocks
                .entry(start)
                .or_insert_with(|| self.block(start, len));
            let block = block.as_ref().ok_or(unreadable)?;
            let offset = usize::try_from(at - start).map_err(|_| unreadable)?;
            let available = block.get(offset..).ok_or(unreadable)?;
            if available.is_empty() {
                return Err(unreadable);
            }
            let n = available.len().min(buf.len() - done);
            buf[done..done + n].copy_from_slice(&available[..n]);
            done += n;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> Image {
        let mut m = Image::new();
        m.add(0x2_0000, RegionKind::Private, true, (0..=255).collect());
        m.add(0x1_0000, RegionKind::Image, false, vec![7; 16]);
        m
    }

    #[test]
    fn regions_are_kept_in_address_order() {
        let m = image();
        let bases: Vec<u64> = m.regions().iter().map(|r| r.base).collect();
        assert_eq!(bases, vec![0x1_0000, 0x2_0000]);
    }

    #[test]
    fn a_read_is_whole_or_unreadable() {
        let m = image();
        assert_eq!(m.read_vec(0x2_0002, 3), Ok(vec![2, 3, 4]));
        assert_eq!(m.read_u32(0x2_0000), Ok(0x0302_0100));
        assert!(m.read_vec(0x2_00fe, 4).is_err());
        assert!(m.read_vec(0x9_0000, 1).is_err());
    }

    #[test]
    fn a_cached_read_matches_the_memory_it_caches() {
        let mut m = Image::new();
        let bytes: Vec<u8> = (0..(BLOCK as usize * 2 + 100)).map(|i| i as u8).collect();
        m.add(0x10_0000, RegionKind::Private, true, bytes.clone());
        let c = Cached::new(m);
        let across = 0x10_0000 + BLOCK - 3;
        assert_eq!(
            c.read_vec(across, 8),
            Ok(bytes[BLOCK as usize - 3..BLOCK as usize + 5].to_vec())
        );
        let tail = 0x10_0000 + 2 * BLOCK + 90;
        assert_eq!(c.read_vec(tail, 10), Ok(bytes[bytes.len() - 10..].to_vec()));
        assert!(c.read_vec(tail, 11).is_err());
    }

    #[test]
    fn a_region_that_is_only_page_aligned_is_cached_from_its_own_start() {
        // A region starting 4 KiB past a block boundary, right after an unreadable gap: a block
        // counted from the boundary would begin in the gap and fail.
        let mut m = Image::new();
        let base = 0x7fff_0000_1000;
        let bytes: Vec<u8> = (0..3 * BLOCK as usize).map(|i| (i / 7) as u8).collect();
        m.add(base, RegionKind::Image, true, bytes.clone());
        let c = Cached::new(m);
        for at in [0, 8, BLOCK - 4, BLOCK, 2 * BLOCK + 100] {
            let i = at as usize;
            assert_eq!(
                c.read_vec(base + at, 16),
                Ok(bytes[i..i + 16].to_vec()),
                "{at}"
            );
        }
        assert!(c.read_vec(base - 8, 8).is_err());
    }
}
