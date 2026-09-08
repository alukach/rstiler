//! Reading only the IFD tags a request actually needs.
//!
//! `async-tiff`'s `read_all_ifds` materialises every tag of every level, and
//! `TileOffsets`/`TileByteCounts` are one entry per tile. On a large COG that
//! dominates everything else: a 463832 × 102252 raster carries 181,200 tile
//! offsets at full resolution alone, ~3.7 MiB of arrays across the chain — and
//! `/cog/info` needs none of them, while a tile needs one level's worth.
//!
//! So walk the chain reading only the small tags, and pull the per-tile arrays
//! for the single level we end up reading pixels from.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::Mutex;

use async_trait::async_trait;
use bytes::Bytes;

use async_tiff::error::AsyncTiffResult;
use async_tiff::metadata::{ImageFileDirectoryReader, MetadataFetch, TiffMetadataReader};
use async_tiff::reader::Endianness;
use async_tiff::tags::Tag;
use async_tiff::{ImageFileDirectory, TIFF};

/// Tags whose values are one entry per tile or strip.
const HEAVY: [u16; 4] = [
    273, // StripOffsets
    279, // StripByteCounts
    324, // TileOffsets
    325, // TileByteCounts
];

/// Reads in aligned blocks and remembers them.
///
/// A tag parse makes many small reads, so they want coalescing — but
/// `ReadaheadMetadataCache` coalesces by reading sequentially *from the start
/// of the file*, which is right for walking the IFD chain at the front and
/// badly wrong for a per-tile array megabytes in. This fetches only around
/// what it was asked for.
#[derive(Debug)]
pub(crate) struct BlockFetch<F: MetadataFetch> {
    inner: F,
    block: u64,
    blocks: Mutex<Vec<(Range<u64>, Bytes)>>,
}

/// A floor, not a cap: a read larger than this is fetched whole. Big enough
/// that the many small reads of one tag parse land in a couple of requests,
/// small enough not to drag in an array nobody asked for. Measured on a COG
/// with 3.7 MiB of tile offsets, 512 KiB beat both 64 KiB (too many requests
/// for a full-resolution array) and 256 KiB.
const BLOCK: u64 = 512 * 1024;

impl<F: MetadataFetch> BlockFetch<F> {
    /// Sized for reading a level's per-tile arrays.
    pub(crate) fn new(inner: F) -> Self {
        Self::with_block(inner, BLOCK)
    }

    /// Walking the IFD chain touches entry tables and small tag values, so a
    /// smaller block wastes less on a COG whose whole chain is a few KiB.
    pub(crate) fn with_block(inner: F, block: u64) -> Self {
        Self {
            inner,
            block,
            blocks: Mutex::new(Vec::new()),
        }
    }
}

#[cfg_attr(target_arch = "wasm32", async_trait(?Send))]
#[cfg_attr(not(target_arch = "wasm32"), async_trait)]
impl<F: MetadataFetch> MetadataFetch for BlockFetch<F> {
    async fn fetch(&self, range: Range<u64>) -> AsyncTiffResult<Bytes> {
        if let Ok(blocks) = self.blocks.lock() {
            for (have, bytes) in blocks.iter() {
                if have.start <= range.start && range.end <= have.end {
                    let from = (range.start - have.start) as usize;
                    let to = (range.end - have.start) as usize;
                    return Ok(bytes.slice(from..to));
                }
            }
        }

        let start = range.start / self.block * self.block;
        let end = range.end.max(start + self.block).div_ceil(self.block) * self.block;
        let bytes = self.inner.fetch(start..end).await?;

        let from = (range.start - start) as usize;
        let to = ((range.end - start) as usize).min(bytes.len());
        let slice = bytes.slice(from..to);
        if let Ok(mut blocks) = self.blocks.lock() {
            blocks.push((start..start + bytes.len() as u64, bytes));
        }
        Ok(slice)
    }
}

/// Block size for walking the IFD chain. Small, because the chain is entry
/// tables and short tag values — but it must still be a *block* read rather
/// than a sequential one: overview IFDs are not always at the front of the
/// file. NLCD's CONUS land cover puts them 962 MB in.
pub(crate) const CHAIN_BLOCK: u64 = 128 * 1024;

/// The IFD chain, parsed without its per-tile arrays.
pub(crate) struct Levels {
    endianness: Endianness,
    bigtiff: bool,
    /// Byte offset of each level's IFD, so a level can be re-read in full.
    starts: Vec<u64>,
    light: Vec<ImageFileDirectory>,
}

impl Levels {
    /// Walk the chain, reading every tag except the per-tile arrays.
    pub(crate) async fn open<F: MetadataFetch>(fetch: &F) -> AsyncTiffResult<Self> {
        let meta = TiffMetadataReader::try_open(fetch).await?;
        let (endianness, bigtiff) = (meta.endianness(), meta.bigtiff());

        let mut next = meta.next_ifd_offset();
        let (mut starts, mut light) = (Vec::new(), Vec::new());
        while let Some(start) = next {
            let reader = ImageFileDirectoryReader::open(fetch, start, bigtiff, endianness).await?;
            let tags = read_tags(fetch, &reader, start, bigtiff, endianness, false).await?;
            let ifd = ImageFileDirectory::from_tags(tags, endianness)?;
            // COGs also carry mask IFDs (NewSubfileType bit 2). Drop them here
            // so a level index means the same thing everywhere.
            if ifd.new_subfile_type().unwrap_or(0) & 4 == 0 {
                starts.push(start);
                light.push(ifd);
            }
            next = reader.finish(fetch).await?;
        }
        Ok(Self {
            endianness,
            bigtiff,
            starts,
            light,
        })
    }

    /// Every image level's small metadata: dimensions, CRS, compression,
    /// nodata. Enough to answer `/cog/info` and to choose an overview.
    pub(crate) fn light(&self) -> &[ImageFileDirectory] {
        &self.light
    }

    /// Re-read one level with its per-tile arrays, so pixels can be fetched.
    pub(crate) async fn full<F: MetadataFetch + Clone>(
        &self,
        fetch: &F,
        level: usize,
    ) -> AsyncTiffResult<TIFF> {
        // Parsing an IFD makes many small reads. Coalesce them into blocks
        // around the tags themselves — a sequential readahead would instead
        // pull everything from the start of the file up to a deep array.
        let fetch = BlockFetch::new(fetch.clone());
        let start = self.starts[level];
        let reader =
            ImageFileDirectoryReader::open(&fetch, start, self.bigtiff, self.endianness).await?;
        let tags = read_tags(&fetch, &reader, start, self.bigtiff, self.endianness, true).await?;
        let ifd = ImageFileDirectory::from_tags(tags, self.endianness)?;
        Ok(TIFF::new(vec![ifd], self.endianness))
    }
}

/// Read an IFD's tags, optionally including the per-tile arrays.
///
/// `ImageFileDirectoryReader` can read a tag by index but does not expose how
/// many there are, so the entry table is read directly to recover the count and
/// each entry's tag number. Values are still parsed by `read_tag`.
async fn read_tags<F: MetadataFetch>(
    fetch: &F,
    reader: &ImageFileDirectoryReader,
    start: u64,
    bigtiff: bool,
    endianness: Endianness,
    heavy: bool,
) -> AsyncTiffResult<HashMap<Tag, async_tiff::TagValue>> {
    let (count_size, entry_size) = if bigtiff { (8u64, 20u64) } else { (2, 12) };

    let raw = fetch.fetch(start..start + count_size).await?;
    let count = match (bigtiff, endianness) {
        (true, Endianness::LittleEndian) => u64::from_le_bytes(raw[..8].try_into().unwrap()),
        (true, Endianness::BigEndian) => u64::from_be_bytes(raw[..8].try_into().unwrap()),
        (false, Endianness::LittleEndian) => {
            u16::from_le_bytes(raw[..2].try_into().unwrap()) as u64
        }
        (false, Endianness::BigEndian) => u16::from_be_bytes(raw[..2].try_into().unwrap()) as u64,
    };

    // Each entry begins with its tag number; the rest is parsed by read_tag.
    let table_start = start + count_size;
    let table = fetch
        .fetch(table_start..table_start + count * entry_size)
        .await?;

    let mut tags = HashMap::with_capacity(count as usize);
    for idx in 0..count {
        let at = (idx * entry_size) as usize;
        let id = match endianness {
            Endianness::LittleEndian => u16::from_le_bytes(table[at..at + 2].try_into().unwrap()),
            Endianness::BigEndian => u16::from_be_bytes(table[at..at + 2].try_into().unwrap()),
        };
        if !heavy && HEAVY.contains(&id) {
            continue;
        }
        let (tag, value) = reader.read_tag(fetch, idx).await?;
        tags.insert(tag, value);
    }
    Ok(tags)
}
