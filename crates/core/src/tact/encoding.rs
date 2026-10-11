//! The encoding manifest: content key → encoding key.
//!
//! This is the hop that turns "the file the game refers to" into "the file
//! local storage is addressed by". Everything except the encoding manifest
//! itself has to pass through it.
//!
//! Written here rather than taken from `tact-parser` because that crate reads
//! the two page tables back to back and only then the pages. The real layout
//! interleaves them, each table is immediately followed by *its own* pages:
//!
//! ```text
//! header               22 bytes
//! espec block          espec_block_size bytes
//! ckey page table      ckey_page_count x 32   (first key + md5 per page)
//! ckey pages           ckey_page_count x page_size
//! ekey page table      ekey_page_count x 32
//! ekey pages           ekey_page_count x page_size
//! ```
//!
//! Reading the tables consecutively lands the page cursor 564 KB early, mid
//! entry. The parse does not fail, it walks garbage, finds a zero byte a few
//! entries into each page, and moves on, so the result is a manifest that
//! looks plausible and is missing 95% of its contents. On a real 12.1.0
//! install it yields 138,096 entries where there are 2,874,085, and the root
//! manifest's own key is not among them.
//!
//! Entry layout inside a ckey page, repeating until a zero `key_count`:
//!
//! ```text
//! u8       key_count
//! u40 BE   decoded file size
//! [u8; 16] content key
//! [u8; 16] x key_count   encoding keys
//! ```

use std::path::Path;

use super::config::CKey;
use super::storage::EKey;

/// Bytes of fixed header before the espec block.
const HEADER_SIZE: usize = 22;
/// A page table entry: the page's first key, then its MD5.
const PAGE_TABLE_ENTRY: usize = 32;

/// What the manifest knows about one content key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Encoded {
    pub ckey: CKey,
    /// The first encoding key listed. A content key occasionally has more than
    /// one, the same bytes encoded differently, and any of them decodes to
    /// the same file, so the first is taken.
    pub ekey: EKey,
    /// Decoded size in bytes. The index only knows the compressed size, so
    /// this is the number to show a user or check an extraction against.
    pub size: u64,
}

/// A parsed encoding manifest.
///
/// Held as one sorted `Vec` rather than a hash map: entries arrive in content
/// key order already, lookups are binary searches, and on a current retail
/// build this is 2.9 M entries, where a `HashMap`'s per-entry overhead is
/// tens of megabytes of pure waste.
#[derive(Debug, Clone, Default)]
pub struct Encoding {
    entries: Vec<Encoded>,
}

impl Encoding {
    pub fn parse(bytes: &[u8]) -> Result<Self, EncodingError> {
        let header = Header::parse(bytes)?;

        let table = HEADER_SIZE + header.espec_block_size;
        // The interleaving this module exists to get right.
        let pages_start = table + header.ckey_page_count * PAGE_TABLE_ENTRY;
        let pages_end = pages_start + header.ckey_page_count * header.ckey_page_size;
        if pages_end > bytes.len() {
            return Err(EncodingError::Truncated { need: pages_end, got: bytes.len() });
        }

        // Every page is full of entries, so this is a close upper bound and
        // saves repeatedly reallocating a multi-hundred-megabyte vector.
        let approx = header.ckey_page_count * header.ckey_page_size / 38;
        let mut entries = Vec::with_capacity(approx);

        for page in 0..header.ckey_page_count {
            let start = pages_start + page * header.ckey_page_size;
            parse_page(&bytes[start..start + header.ckey_page_size], &mut entries)?;
        }

        if entries.is_empty() {
            return Err(EncodingError::Empty);
        }

        // Pages are ordered by first key and entries within a page are sorted,
        // so the concatenation should already be sorted. Verify rather than
        // assume: an unsorted vector would make every binary search silently
        // wrong instead of loudly broken.
        if !entries.windows(2).all(|w| w[0].ckey <= w[1].ckey) {
            entries.sort_unstable_by_key(|e| e.ckey);
        }

        Ok(Self { entries })
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, EncodingError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path)
            .map_err(|source| EncodingError::Io { path: path.to_path_buf(), source })?;
        Self::parse(&bytes)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn entries(&self) -> &[Encoded] {
        &self.entries
    }

    pub fn get(&self, ckey: &CKey) -> Option<&Encoded> {
        let i = self.entries.binary_search_by(|e| e.ckey.cmp(ckey)).ok()?;
        Some(&self.entries[i])
    }

    /// Content key → encoding key.
    pub fn ekey_for(&self, ckey: &CKey) -> Option<EKey> {
        self.get(ckey).map(|e| e.ekey)
    }

    /// Decoded size of the file a content key names.
    pub fn size_of(&self, ckey: &CKey) -> Option<u64> {
        self.get(ckey).map(|e| e.size)
    }

    /// Encoding key -> content key.
    ///
    /// A linear scan: the manifest is sorted by content key, so the reverse
    /// direction has no index. Callers that need many of these should build
    /// their own map once rather than calling this in a loop over millions of
    /// entries.
    pub fn ckey_for_ekey(&self, ekey: &EKey) -> Option<CKey> {
        self.entries.iter().find(|e| &e.ekey == ekey).map(|e| e.ckey)
    }
}

/// Walk one ckey page, appending its entries.
fn parse_page(page: &[u8], out: &mut Vec<Encoded>) -> Result<(), EncodingError> {
    let mut at = 0usize;
    // A page ends at its first zero `key_count`; the remainder is padding.
    while at + 6 + 16 <= page.len() {
        let key_count = page[at] as usize;
        if key_count == 0 {
            break;
        }

        let size = u40_be(&page[at + 1..at + 6]);
        let entry_len = 6 + 16 + key_count * 16;
        if at + entry_len > page.len() {
            // A page whose last entry overruns it is corrupt, not merely full:
            // the writer would have started a new page instead.
            return Err(EncodingError::EntryOverrunsPage { at, key_count });
        }

        let ckey: CKey = page[at + 6..at + 22].try_into().expect("16 bytes");
        let ekey: EKey = page[at + 22..at + 38].try_into().expect("16 bytes");
        out.push(Encoded { ckey, ekey, size });

        at += entry_len;
    }
    Ok(())
}

/// The fixed header, big-endian throughout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Header {
    version: u8,
    ckey_page_size: usize,
    ckey_page_count: usize,
    ekey_page_count: usize,
    espec_block_size: usize,
}

impl Header {
    fn parse(d: &[u8]) -> Result<Self, EncodingError> {
        if d.len() < HEADER_SIZE {
            return Err(EncodingError::Truncated { need: HEADER_SIZE, got: d.len() });
        }
        if &d[..2] != b"EN" {
            return Err(EncodingError::BadMagic([d[0], d[1]]));
        }

        let (ckey_hash, ekey_hash) = (d[3], d[4]);
        if ckey_hash != 16 || ekey_hash != 16 {
            return Err(EncodingError::UnsupportedKeySize { ckey: ckey_hash, ekey: ekey_hash });
        }

        Ok(Self {
            version: d[2],
            ckey_page_size: usize::from(be16(d, 5)) * 1024,
            ckey_page_count: be32(d, 9) as usize,
            ekey_page_count: be32(d, 13) as usize,
            // One unknown byte sits at 17, between the page counts and this.
            espec_block_size: be32(d, 18) as usize,
        })
    }
}

fn be16(d: &[u8], at: usize) -> u16 {
    u16::from_be_bytes(d[at..at + 2].try_into().expect("2 bytes"))
}

fn be32(d: &[u8], at: usize) -> u32 {
    u32::from_be_bytes(d[at..at + 4].try_into().expect("4 bytes"))
}

/// 40-bit big-endian size field.
fn u40_be(d: &[u8]) -> u64 {
    d.iter().fold(0u64, |acc, b| (acc << 8) | u64::from(*b))
}

#[derive(Debug, thiserror::Error)]
pub enum EncodingError {
    #[error("not an encoding manifest: magic is {:02x?}, expected \"EN\"", .0)]
    BadMagic([u8; 2]),

    #[error("encoding manifest uses {ckey}/{ekey}-byte keys; only 16/16 is supported")]
    UnsupportedKeySize { ckey: u8, ekey: u8 },

    #[error("encoding manifest is truncated: need {need} bytes, got {got}")]
    Truncated { need: usize, got: usize },

    #[error("an entry at page offset {at} claims {key_count} keys and overruns its page")]
    EntryOverrunsPage { at: usize, key_count: usize },

    #[error("encoding manifest parsed to zero entries")]
    Empty,

    #[error("i/o error at {path}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact::config::parse_hash;

    /// Build a manifest with the real layout, so the test fails if the page
    /// interleaving is ever "simplified" back to the broken form.
    fn manifest(entries: &[(CKey, EKey, u64)], page_size: usize, espec: &[u8]) -> Vec<u8> {
        let mut page = Vec::new();
        for (ckey, ekey, size) in entries {
            page.push(1u8);
            page.extend_from_slice(&size.to_be_bytes()[3..]);
            page.extend_from_slice(ckey);
            page.extend_from_slice(ekey);
        }
        page.resize(page_size, 0);

        let mut out = Vec::new();
        out.extend_from_slice(b"EN");
        out.push(1);
        out.push(16);
        out.push(16);
        out.extend_from_slice(&((page_size / 1024) as u16).to_be_bytes());
        out.extend_from_slice(&((page_size / 1024) as u16).to_be_bytes());
        out.extend_from_slice(&1u32.to_be_bytes()); // one ckey page
        out.extend_from_slice(&1u32.to_be_bytes()); // one ekey page
        out.push(0);
        out.extend_from_slice(&(espec.len() as u32).to_be_bytes());
        assert_eq!(out.len(), HEADER_SIZE);

        out.extend_from_slice(espec);
        out.extend_from_slice(&[0u8; PAGE_TABLE_ENTRY]); // ckey page table
        out.extend_from_slice(&page); // ckey pages follow their own table
        out.extend_from_slice(&[0u8; PAGE_TABLE_ENTRY]); // ekey page table
        out.resize(out.len() + page_size, 0); // ekey pages
        out
    }

    fn key(n: u8) -> [u8; 16] {
        let mut k = [0u8; 16];
        k[0] = n;
        k
    }

    #[test]
    fn a_manifest_round_trips_its_entries() {
        let entries = [
            (key(1), key(0x81), 1024u64),
            (key(2), key(0x82), 2048),
            (key(3), key(0x83), 0xff_ffff_ffffu64),
        ];
        // A non-empty espec block, because getting its size wrong is exactly
        // how the page offset drifts.
        let enc = Encoding::parse(&manifest(&entries, 4096, b"b:{164=z}\0z\0")).unwrap();

        assert_eq!(enc.len(), 3);
        assert_eq!(enc.ekey_for(&key(2)), Some(key(0x82)));
        assert_eq!(enc.size_of(&key(1)), Some(1024));
        // The 40-bit size field must not be truncated to 32 bits.
        assert_eq!(enc.size_of(&key(3)), Some(0xff_ffff_ffff));
        assert_eq!(enc.ekey_for(&key(9)), None);
    }

    #[test]
    fn the_ckey_pages_follow_their_own_table_not_both_tables() {
        // The regression this module exists for. With the broken layout the
        // page cursor lands inside the ekey page table, and the parse quietly
        // yields far too few entries rather than failing.
        let entries: Vec<_> = (1..=40u8).map(|n| (key(n), key(n ^ 0x80), n as u64 * 16)).collect();
        let enc = Encoding::parse(&manifest(&entries, 4096, b"z\0")).unwrap();

        assert_eq!(enc.len(), 40, "every entry must be found, not a prefix of them");
        for (ckey, ekey, size) in &entries {
            assert_eq!(enc.ekey_for(ckey), Some(*ekey));
            assert_eq!(enc.size_of(ckey), Some(*size));
        }
    }

    #[test]
    fn entries_end_up_sorted_so_binary_search_is_valid() {
        let entries: Vec<_> = (1..=20u8).rev().map(|n| (key(n), key(n), n as u64)).collect();
        let enc = Encoding::parse(&manifest(&entries, 4096, b"z\0")).unwrap();
        assert!(enc.entries().windows(2).all(|w| w[0].ckey < w[1].ckey));
        // Every key still resolves after the reordering.
        for (ckey, ekey, _) in &entries {
            assert_eq!(enc.ekey_for(ckey), Some(*ekey));
        }
    }

    #[test]
    fn a_page_ends_at_its_first_zero_key_count() {
        let mut bytes = manifest(&[(key(1), key(0x81), 10)], 4096, b"z\0");
        // Padding after the single entry must not be read as entries.
        assert_eq!(Encoding::parse(&bytes).unwrap().len(), 1);

        // Nor should trailing garbage in the ekey region leak in.
        let len = bytes.len();
        bytes[len - 16..].fill(0xcd);
        assert_eq!(Encoding::parse(&bytes).unwrap().len(), 1);
    }

    #[test]
    fn junk_is_rejected_rather_than_parsed_into_nonsense() {
        assert!(matches!(Encoding::parse(b"XX"), Err(EncodingError::Truncated { .. })));
        let mut bytes = manifest(&[(key(1), key(2), 4)], 4096, b"z\0");
        bytes[0] = b'Z';
        assert!(matches!(Encoding::parse(&bytes), Err(EncodingError::BadMagic(_))));

        bytes[0] = b'E';
        bytes[3] = 9; // an unsupported ckey hash size
        assert!(matches!(
            Encoding::parse(&bytes),
            Err(EncodingError::UnsupportedKeySize { ckey: 9, .. })
        ));
    }

    #[test]
    fn the_real_root_key_shape_survives_a_lookup() {
        // Values lifted from a real 12.1.0 install, so the test carries the
        // actual bytes rather than a synthetic pattern.
        let ckey = parse_hash("669be7c403ea35a20f73ea7effcaa544").unwrap();
        let ekey = parse_hash("d175f4afe795101d0cf3660228b3283b").unwrap();
        let enc = Encoding::parse(&manifest(&[(ckey, ekey, 44_876)], 4096, b"z\0")).unwrap();
        assert_eq!(enc.ekey_for(&ckey), Some(ekey));
        assert_eq!(enc.size_of(&ckey), Some(44_876));
    }
}
