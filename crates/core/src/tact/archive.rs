//! Archive indices: which archive holds a file, and where inside it.
//!
//! Ordinary content is not fetchable by its own key, the CDN answers 403 for
//! a file that lives in an archive. Instead each of a build's archives (1362
//! of them for 12.1.0) publishes a `<hash>.index` next to itself, and the
//! archive answers HTTP range requests. Downloading one file therefore means:
//! find its archive, read the offset and size out of that archive's index,
//! and ask for exactly those bytes.
//!
//! ```text
//! blocks   ceil(count / per_block) x block_size, entries then zero padding
//! toc      one last-key per block, then one checksum per block
//! footer   28 bytes, at the very end of the file
//! ```
//!
//! The footer is read first because it carries the field widths everything
//! else depends on. Sizes are validated against the file length: blocks, TOC
//! and footer must account for every byte, which is a strong check that the
//! widths were read correctly rather than merely plausible.
//!
//! Entry layout, `size` before `offset`: the order matters and is easy to
//! get backwards, since both are 4-byte big-endian:
//!
//! ```text
//! [u8; 16]  encoding key
//! u32 BE    size of the file's bytes within the archive
//! u32 BE    offset of those bytes from the start of the archive
//! ```

use super::config::to_hex;
use super::storage::EKey;

/// Bytes of footer at the end of every archive index.
const FOOTER_SIZE: usize = 28;

/// One file's location inside one archive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Located {
    pub ekey: EKey,
    /// Bytes to request. Covers the CASC entry header and the BLTE payload.
    pub size: u32,
    /// Offset from the start of the archive.
    pub offset: u32,
}

impl Located {
    /// The value of an HTTP `Range` header that fetches exactly this file.
    ///
    /// `None` for a zero-size entry, which no range can express.
    pub fn range(&self) -> Option<String> {
        // Inclusive on both ends, which is what RFC 9110 specifies and what
        // an off-by-one here would silently violate, a short read only shows
        // up later as a BLTE decode failure.
        let last = u64::from(self.offset).checked_add(u64::from(self.size))?.checked_sub(1)?;
        Some(format!("bytes={}-{last}", self.offset))
    }
}

/// A parsed archive index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ArchiveIndex {
    /// Hash of the archive this indexes, as it appears in the CDN config.
    archive: String,
    /// Sorted by encoding key, as the format stores them.
    entries: Vec<Located>,
}

impl ArchiveIndex {
    pub fn parse(bytes: &[u8], archive: impl Into<String>) -> Result<Self, ArchiveError> {
        let archive = archive.into();
        let (footer, rows) = walk(bytes)?;
        if footer.offset_bytes == 0 {
            // The file index shares this container but has no offset column,
            // because its files are not inside an archive at all.
            return Err(ArchiveError::NotAnArchiveIndex);
        }

        let mut entries = rows
            .into_iter()
            // A zero-size row cannot be range-requested. Dropping it costs one
            // file, refusing the index would cost every file in the archive.
            .filter(|&(ekey, size, _)| {
                if size == 0 {
                    tracing::warn!("archive {archive} lists {} with size 0, skipping it", to_hex(&ekey));
                }
                size != 0
            })
            .map(|(ekey, size, offset)| {
                Ok(Located { ekey, size: narrow(size)?, offset: narrow(offset)? })
            })
            .collect::<Result<Vec<_>, ArchiveError>>()?;

        // Lookups binary search, so ordering is a correctness requirement
        // rather than an optimisation.
        if !entries.windows(2).all(|w| w[0].ekey <= w[1].ekey) {
            entries.sort_unstable_by_key(|e| e.ekey);
        }

        Ok(Self { archive, entries })
    }

    /// Hash of the archive this indexes.
    pub fn archive(&self) -> &str {
        &self.archive
    }

    pub fn entries(&self) -> &[Located] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn lookup(&self, ekey: &EKey) -> Option<&Located> {
        let i = self.entries.binary_search_by(|e| e.ekey.cmp(ekey)).ok()?;
        Some(&self.entries[i])
    }

    /// Total archived bytes this index accounts for.
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| u64::from(e.size)).sum()
    }
}

/// One loose file: served whole, at its own URL, with no archive around it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Loose {
    pub ekey: EKey,
    /// Bytes the CDN serves for this key. There is no CASC entry header on a
    /// loose file, that header is a property of being inside an archive.
    pub size: u32,
}

/// The build's file index: the keys that are *not* in any archive.
///
/// Named by `file-index` in the CDN config and fetched like any other index,
/// but its footer declares `offset_bytes = 0`: there is no offset column,
/// because there is nothing to be offset into. These ~25k keys are the only
/// content fetchable by a plain GET; everything else answers 403 loose and
/// must be range-requested out of its archive.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileIndex {
    entries: Vec<Loose>,
}

impl FileIndex {
    pub fn parse(bytes: &[u8]) -> Result<Self, ArchiveError> {
        let (footer, rows) = walk(bytes)?;
        if footer.offset_bytes != 0 {
            return Err(ArchiveError::NotAFileIndex { offset_bytes: footer.offset_bytes });
        }

        let mut entries = rows
            .into_iter()
            .map(|(ekey, size, _)| Ok(Loose { ekey, size: narrow(size)? }))
            .collect::<Result<Vec<_>, ArchiveError>>()?;
        if !entries.windows(2).all(|w| w[0].ekey <= w[1].ekey) {
            entries.sort_unstable_by_key(|e| e.ekey);
        }

        Ok(Self { entries })
    }

    pub fn entries(&self) -> &[Loose] {
        &self.entries
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn contains(&self, ekey: &EKey) -> bool {
        self.lookup(ekey).is_some()
    }

    pub fn lookup(&self, ekey: &EKey) -> Option<&Loose> {
        let i = self.entries.binary_search_by(|e| e.ekey.cmp(ekey)).ok()?;
        Some(&self.entries[i])
    }

    /// Total loose bytes this index accounts for.
    pub fn bytes(&self) -> u64 {
        self.entries.iter().map(|e| u64::from(e.size)).sum()
    }
}

/// Read the block/TOC/footer container both index shapes share.
///
/// Returns the footer and every non-padding row as `(key, size, offset)`,
/// where `offset` is zero for a container that has no offset column.
#[allow(clippy::type_complexity)]
fn walk(bytes: &[u8]) -> Result<(Footer, Vec<(EKey, u64, u64)>), ArchiveError> {
    let footer = Footer::parse(bytes)?;

    let entry_size = footer.key_size + footer.size_bytes + footer.offset_bytes;
    let per_block = footer.block_size / entry_size;
    if per_block == 0 {
        return Err(ArchiveError::UnusableGeometry { block_size: footer.block_size, entry_size });
    }
    let blocks = footer.count.div_ceil(per_block);

    // Every byte must be accounted for. If the field widths were misread,
    // this is where it shows up, rather than as garbage entries.
    let expected =
        blocks * footer.block_size + blocks * (footer.key_size + footer.checksum_size) + FOOTER_SIZE;
    if expected != bytes.len() {
        return Err(ArchiveError::SizeMismatch { expected, actual: bytes.len() });
    }

    let mut rows = Vec::with_capacity(footer.count);
    for block in 0..blocks {
        let start = block * footer.block_size;
        let page = &bytes[start..start + footer.block_size];

        for slot in 0..per_block {
            if rows.len() == footer.count {
                break;
            }
            let at = slot * entry_size;
            let ekey: EKey = page[at..at + footer.key_size]
                .try_into()
                .map_err(|_| ArchiveError::UnsupportedKeySize(footer.key_size))?;
            // An all-zero key is padding inside a partially filled block.
            if ekey == [0u8; 16] {
                continue;
            }
            let size = be_uint(&page[at + footer.key_size..at + footer.key_size + footer.size_bytes]);
            let offset = be_uint(&page[at + footer.key_size + footer.size_bytes..at + entry_size]);
            rows.push((ekey, size, offset));
        }
    }

    Ok((footer, rows))
}

/// The trailing 28 bytes, which describe every field width in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Footer {
    version: u8,
    block_size: usize,
    offset_bytes: usize,
    size_bytes: usize,
    key_size: usize,
    checksum_size: usize,
    count: usize,
}

impl Footer {
    fn parse(bytes: &[u8]) -> Result<Self, ArchiveError> {
        if bytes.len() < FOOTER_SIZE {
            return Err(ArchiveError::Truncated { need: FOOTER_SIZE, got: bytes.len() });
        }
        let f = &bytes[bytes.len() - FOOTER_SIZE..];

        // 0..8 is the toc hash, 20..28 the footer's own checksum; neither is
        // verified here, the content itself is verified by its encoding key
        // after the range request, which is the check that actually matters.
        let version = f[8];
        if version != 1 {
            return Err(ArchiveError::UnsupportedVersion(version));
        }

        let footer = Self {
            version,
            block_size: usize::from(f[11]) * 1024,
            offset_bytes: usize::from(f[12]),
            size_bytes: usize::from(f[13]),
            key_size: usize::from(f[14]),
            checksum_size: usize::from(f[15]),
            count: u32::from_le_bytes(f[16..20].try_into().expect("4 bytes")) as usize,
        };

        if footer.key_size != 16 {
            return Err(ArchiveError::UnsupportedKeySize(footer.key_size));
        }
        // `offset_bytes == 0` is legal and means the file index, which has no
        // offset column; the two index shapes are told apart by their callers.
        if footer.offset_bytes > 8 || footer.size_bytes == 0 || footer.size_bytes > 8 {
            return Err(ArchiveError::UnusableGeometry {
                block_size: footer.block_size,
                entry_size: footer.key_size + footer.size_bytes + footer.offset_bytes,
            });
        }
        Ok(footer)
    }
}

/// The footer allows columns up to 8 bytes wide, entries are stored as `u32`.
fn narrow(value: u64) -> Result<u32, ArchiveError> {
    u32::try_from(value).map_err(|_| ArchiveError::Overflow(value))
}

/// Big-endian integer of 1..=8 bytes.
fn be_uint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
}

#[derive(Debug, thiserror::Error)]
pub enum ArchiveError {
    #[error("archive index is truncated: need {need} bytes, got {got}")]
    Truncated { need: usize, got: usize },

    #[error("archive index version {0} is not supported (only 1 is)")]
    UnsupportedVersion(u8),

    #[error("archive index uses {0}-byte keys; only 16 is supported")]
    UnsupportedKeySize(usize),

    #[error("archive index geometry is unusable: {entry_size}-byte entries in {block_size}-byte blocks")]
    UnusableGeometry { block_size: usize, entry_size: usize },

    #[error("archive index should be {expected} bytes by its own footer but is {actual}")]
    SizeMismatch { expected: usize, actual: usize },

    #[error("archive index entry value {0} does not fit in 32 bits")]
    Overflow(u64),

    #[error("this is the file index, not an archive index: it has no offset column")]
    NotAnArchiveIndex,

    #[error("this is an archive index, not the file index: it has a {offset_bytes}-byte offset column")]
    NotAFileIndex { offset_bytes: usize },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(n: u8) -> EKey {
        let mut k = [0u8; 16];
        // Never all-zero: that is the padding marker.
        k[0] = n.max(1);
        k[15] = n;
        k
    }

    /// Build an index in the real layout, so a test fails if the geometry is
    /// ever "simplified" into something the CDN does not actually serve.
    fn index(entries: &[(EKey, u32, u32)], block_size: usize) -> Vec<u8> {
        let entry_size = 24;
        let per_block = block_size / entry_size;
        let blocks = entries.len().div_ceil(per_block).max(1);

        let mut out = vec![0u8; blocks * block_size];
        for (i, (ekey, size, offset)) in entries.iter().enumerate() {
            let at = (i / per_block) * block_size + (i % per_block) * entry_size;
            out[at..at + 16].copy_from_slice(ekey);
            out[at + 16..at + 20].copy_from_slice(&size.to_be_bytes());
            out[at + 20..at + 24].copy_from_slice(&offset.to_be_bytes());
        }

        // TOC: last key per block, then a checksum per block.
        for b in 0..blocks {
            let last = ((b + 1) * per_block - 1).min(entries.len().saturating_sub(1));
            out.extend_from_slice(&entries.get(last).map_or([0u8; 16], |e| e.0));
        }
        out.extend(std::iter::repeat_n(0u8, blocks * 8));

        let mut footer = vec![0u8; 8]; // toc hash
        footer.push(1); // version
        footer.push(0);
        footer.push(0);
        footer.push((block_size / 1024) as u8);
        footer.push(4); // offset bytes
        footer.push(4); // size bytes
        footer.push(16); // key size
        footer.push(8); // checksum size
        footer.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]); // footer checksum
        assert_eq!(footer.len(), FOOTER_SIZE);

        out.extend_from_slice(&footer);
        out
    }

    /// The same container with the offset column absent, as `file-index` has it.
    fn file_index(entries: &[(EKey, u32)], block_size: usize) -> Vec<u8> {
        let entry_size = 20;
        let per_block = block_size / entry_size;
        let blocks = entries.len().div_ceil(per_block).max(1);

        let mut out = vec![0u8; blocks * block_size];
        for (i, (ekey, size)) in entries.iter().enumerate() {
            let at = (i / per_block) * block_size + (i % per_block) * entry_size;
            out[at..at + 16].copy_from_slice(ekey);
            out[at + 16..at + 20].copy_from_slice(&size.to_be_bytes());
        }

        for b in 0..blocks {
            let last = ((b + 1) * per_block - 1).min(entries.len().saturating_sub(1));
            out.extend_from_slice(&entries.get(last).map_or([0u8; 16], |e| e.0));
        }
        out.extend(std::iter::repeat_n(0u8, blocks * 8));

        let mut footer = vec![0u8; 8];
        footer.push(1);
        footer.push(0);
        footer.push(0);
        footer.push((block_size / 1024) as u8);
        footer.push(0); // offset bytes: none
        footer.push(4); // size bytes
        footer.push(16); // key size
        footer.push(8); // checksum size
        footer.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]);
        assert_eq!(footer.len(), FOOTER_SIZE);

        out.extend_from_slice(&footer);
        out
    }

    #[test]
    fn the_file_index_geometry_of_the_real_build_is_reproduced() {
        // 12.1.0's file-index: 25,693 entries, 4 KiB blocks, 519,148 bytes.
        // 20-byte entries give 204 per block and 126 blocks.
        let entries: Vec<(EKey, u32)> = (0..25_693u32)
            .map(|n| {
                let mut k = [0u8; 16];
                k[..4].copy_from_slice(&n.to_be_bytes());
                k[15] = 1;
                (k, n)
            })
            .collect();
        let bytes = file_index(&entries, 4096);
        assert_eq!(bytes.len(), 519_148);

        let index = FileIndex::parse(&bytes).unwrap();
        assert_eq!(index.len(), 25_693);
    }

    #[test]
    fn a_loose_entry_carries_a_size_and_no_offset() {
        let bytes = file_index(&[(key(1), 1234), (key(2), 99)], 4096);
        let index = FileIndex::parse(&bytes).unwrap();

        assert_eq!(index.lookup(&key(1)).unwrap().size, 1234);
        assert!(index.contains(&key(2)));
        assert!(!index.contains(&key(3)));
        assert_eq!(index.bytes(), 1333);
    }

    #[test]
    fn the_two_index_shapes_refuse_to_be_read_as_each_other() {
        let loose = file_index(&[(key(1), 10)], 4096);
        let archived = index(&[(key(1), 10, 30)], 4096);

        assert!(matches!(
            ArchiveIndex::parse(&loose, "x"),
            Err(ArchiveError::NotAnArchiveIndex)
        ));
        assert!(matches!(
            FileIndex::parse(&archived),
            Err(ArchiveError::NotAFileIndex { offset_bytes: 4 })
        ));
    }

    #[test]
    fn a_real_footer_geometry_accounts_for_every_byte() {
        // The numbers from a live index of archive 0017a402…: 7060 entries,
        // 4 KB blocks, 24-byte entries. 42 blocks x 4096 + 42 x 24 + 28 is
        // exactly the 173,068 bytes the CDN served.
        let per_block = 4096 / 24;
        let blocks = 7060usize.div_ceil(per_block);
        assert_eq!(per_block, 170);
        assert_eq!(blocks, 42);
        assert_eq!(blocks * 4096 + blocks * (16 + 8) + FOOTER_SIZE, 173_068);
    }

    #[test]
    fn entries_round_trip_with_size_before_offset() {
        // Both fields are 4-byte big-endian, so swapping them parses cleanly
        // and yields nonsense, a 786-byte file at offset 186 MB rather than
        // the other way round.
        let entries = [(key(1), 786u32, 186_722_006u32), (key(2), 12_345, 2_803_136)];
        let parsed = ArchiveIndex::parse(&index(&entries, 4096), "0017a402").unwrap();

        assert_eq!(parsed.len(), 2);
        let first = parsed.lookup(&key(1)).unwrap();
        assert_eq!(first.size, 786);
        assert_eq!(first.offset, 186_722_006);
        assert_eq!(parsed.archive(), "0017a402");
    }

    #[test]
    fn a_range_header_is_inclusive_on_both_ends() {
        let located = Located { ekey: key(1), size: 100, offset: 1000 };
        // 1000..=1099 is 100 bytes. `bytes=1000-1100` would fetch 101 and
        // corrupt the following file's first byte into this one's payload.
        assert_eq!(located.range().unwrap(), "bytes=1000-1099");

        let single = Located { ekey: key(2), size: 1, offset: 0 };
        assert_eq!(single.range().unwrap(), "bytes=0-0");
    }

    #[test]
    fn a_zero_size_entry_has_no_range() {
        assert_eq!(Located { ekey: key(1), size: 0, offset: 0 }.range(), None);
    }

    #[test]
    fn a_zero_size_row_is_skipped_not_fatal() {
        let parsed = ArchiveIndex::parse(&index(&[(key(1), 0, 0), (key(2), 5, 0)], 4096), "x").unwrap();
        assert_eq!(parsed.len(), 1);
        assert!(parsed.lookup(&key(1)).is_none());
    }

    #[test]
    fn entries_spanning_several_blocks_are_all_found() {
        // 400 entries at 170 per block is three blocks, the last partly empty.
        let entries: Vec<_> = (1..=200u8)
            .flat_map(|n| {
                let mut a = key(n);
                a[1] = 1;
                let mut b = key(n);
                b[1] = 2;
                [(a, u32::from(n) * 10, u32::from(n) * 1000), (b, 5, 7)]
            })
            .collect();
        let parsed = ArchiveIndex::parse(&index(&entries, 4096), "x").unwrap();

        assert_eq!(parsed.len(), entries.len());
        for (ekey, size, offset) in &entries {
            let got = parsed.lookup(ekey).expect("every entry must be found");
            assert_eq!((got.size, got.offset), (*size, *offset));
        }
    }

    #[test]
    fn padding_inside_a_partly_filled_block_is_not_an_entry() {
        // One entry in a 170-slot block leaves 169 zeroed slots behind it.
        let parsed = ArchiveIndex::parse(&index(&[(key(7), 42, 84)], 4096), "x").unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed.bytes(), 42);
        assert!(parsed.lookup(&[0u8; 16]).is_none());
    }

    #[test]
    fn a_footer_that_disagrees_with_the_file_length_is_refused() {
        // The check that catches misread field widths: if the geometry were
        // wrong, entries would still "parse", just into garbage.
        let mut bytes = index(&[(key(1), 1, 2)], 4096);
        let len = bytes.len();
        bytes[len - 12..len - 8].copy_from_slice(&9_999u32.to_le_bytes());
        assert!(matches!(
            ArchiveIndex::parse(&bytes, "x"),
            Err(ArchiveError::SizeMismatch { .. })
        ));
    }

    #[test]
    fn an_offset_past_four_gib_is_refused_not_truncated() {
        // A 5-byte offset column: one 25-byte entry at offset 2^32 + 7.
        let mut bytes = vec![0u8; 4096];
        bytes[..16].copy_from_slice(&key(1));
        bytes[16..20].copy_from_slice(&10u32.to_be_bytes());
        bytes[20..25].copy_from_slice(&(u64::from(u32::MAX) + 8).to_be_bytes()[3..]);
        bytes.extend_from_slice(&key(1));
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&[0u8; 8]);
        bytes.extend_from_slice(&[1, 0, 0, 4, 5, 4, 16, 8]);
        bytes.extend_from_slice(&1u32.to_le_bytes());
        bytes.extend_from_slice(&[0u8; 8]);

        assert!(matches!(
            ArchiveIndex::parse(&bytes, "x"),
            Err(ArchiveError::Overflow(v)) if v == u64::from(u32::MAX) + 8
        ));
    }

    #[test]
    fn junk_and_unsupported_shapes_are_reported() {
        assert!(matches!(
            ArchiveIndex::parse(b"short", "x"),
            Err(ArchiveError::Truncated { .. })
        ));

        let mut bytes = index(&[(key(1), 1, 2)], 4096);
        let len = bytes.len();
        bytes[len - 20] = 2; // version
        assert!(matches!(
            ArchiveIndex::parse(&bytes, "x"),
            Err(ArchiveError::UnsupportedVersion(2))
        ));

        let mut bytes = index(&[(key(1), 1, 2)], 4096);
        bytes[len - 14] = 9; // key size
        assert!(matches!(
            ArchiveIndex::parse(&bytes, "x"),
            Err(ArchiveError::UnsupportedKeySize(9))
        ));
    }
}
