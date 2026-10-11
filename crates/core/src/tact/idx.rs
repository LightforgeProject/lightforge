//! CASC bucket index (`.idx`) reader and writer.
//!
//! One `.idx` holds every entry whose truncated encoding key falls in one of
//! 16 buckets, mapping that key to a location inside a `data.NNN` archive.
//! Without a correct index the archives are unreadable, the bytes are there,
//! but nothing can find them.
//!
//! Lightforge writes these itself because `casc-storage` cannot: its
//! `IndexFile` has `load` and no `save`, `rebuild_indices` is a stub, and
//! nothing in the crate ever creates an index file. Its write path appends to
//! an archive and updates an in-memory map that is discarded on drop.
//!
//! The format below was verified against a real 12.1.0 retail install: all 16
//! of its index files round-trip through this module byte for byte, hashes
//! included. That is the only standard worth holding a writer to here, Agent
//! treats an index it doesn't like as a damaged install and starts the repair
//! scan this whole project exists to avoid.
//!
//! ```text
//! 0x00  u32 LE  data_size            bytes of header body that follow the hash
//! 0x04  u32 LE  header_hash          hashlittle(body, 0)
//! 0x08  u16 LE  version              7
//! 0x0a  u8      bucket               0x00..=0x0f
//! 0x0b  u8      unused
//! 0x0c  u8      length_field_size    4
//! 0x0d  u8      location_field_size  5
//! 0x0e  u8      key_field_size       9
//! 0x0f  u8      segment_bits         30
//! 0x10  block table, (data_size - 8) / 8 pairs of u32 BE
//!       padding to the next 16-byte boundary
//! 0x20  u32 LE  data_section_size
//! 0x24  u32 LE  data_section_hash    chained hashlittle2 over each entry
//! 0x28  entries, then zero padding to the file's fixed size
//! ```

use std::path::{Path, PathBuf};

use tact_parser::jenkins3::{hashlittle, hashlittle2};

/// Number of buckets a CASC store spreads its keys across.
pub const BUCKETS: u8 = 16;

/// The only index version Blizzard currently ships for WoW.
pub const VERSION: u16 = 7;

/// Field widths of a V7 entry. Every real WoW index uses exactly these.
pub const KEY_SIZE: usize = 9;
const LOCATION_SIZE: usize = 5;
const LENGTH_SIZE: usize = 4;
const ENTRY_SIZE: usize = KEY_SIZE + LOCATION_SIZE + LENGTH_SIZE;

/// Bits of an entry's packed location word given over to the offset. The two
/// bits above them carry the low end of the archive id.
const SEGMENT_BITS: u8 = 30;
const OFFSET_MASK: u32 = (1 << SEGMENT_BITS) - 1;

/// Offset of the data section, fixed by the 16-byte alignment rule for every
/// header Blizzard ships (`data_size` 16 → body ends at 0x18 → pad to 0x20).
const DATA_SECTION_ALIGN: usize = 16;

/// Byte size a fresh index file is padded out to, 28 × 64 KiB, the shape a
/// real install ships.
///
/// It is a *floor*, not a cap. Sizes in the wild vary with the store: retail's
/// sixteen files are 2,031,616 bytes each, while the WoW client itself rewrites
/// a small store's indices at 196,608 or 262,144. What every one of them has in
/// common is being a whole number of 64 KiB units big enough to hold the
/// bucket, so that is the rule [`Index::to_bytes`] follows.
pub const DEFAULT_FILE_SIZE: usize = 1_835_008;

/// Index files are allocated in whole 64 KiB units. Every real file observed,
/// Agent's and the client's, is a multiple of this.
pub const SIZE_GRANULARITY: usize = 65_536;

/// Free space left after the entries, for the client's own use.
///
/// **This is not padding.** The running client appends incremental index
/// updates into the tail of the file, its "KMT update section", and it must
/// fit. Size a file to just past its entries and the client reports
///
/// ```text
/// Truncated KMT update section detected.
/// Failed to bind mapping table `0`
/// failed to bind Container index at '…/Data/data': 9 (NeedsRepair)
/// ```
///
/// and refuses to open the store at all. That happened on a real 123 GiB
/// retail install: our sixteen files left 7-13 KiB free where Agent's own
/// leave 61-91 KiB. One 64 KiB unit of reserve reproduces Agent's sizing for
/// 17 of its 18 files and is more generous for the eighteenth.
pub const KMT_RESERVE: usize = 65_536;

/// A sanity bound, far above anything observed (retail's buckets are ~2 MB).
/// Exceeding it means something is wrong with the caller, not with the format.
pub const MAX_FILE_SIZE: usize = 64 * 1024 * 1024;

/// Bytes of CASC entry header in front of every archived file's BLTE payload.
pub const CASC_ENTRY_HEADER: usize = 30;

/// Bytes reserved at the head of every `data.NNN` for the per-bucket markers.
pub const ARCHIVE_RESERVED: usize = BUCKETS as usize * CASC_ENTRY_HEADER;

/// Fixed tail shared by every archive-marker key. Constant across all 123
/// archives and all 16 buckets of a real install.
const ARCHIVE_MARKER_SUFFIX: [u8; 7] = [0x00, 0x7e, 0xc0, 0x16, 0xa8, 0x90, 0x21];

/// One key-to-location mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Entry {
    /// Truncated encoding key. Full EKeys are 16 bytes; the index stores the
    /// first 9, which is what makes a bucket lookup cheap.
    pub key: [u8; KEY_SIZE],
    /// Which `data.NNN` holds the bytes.
    pub archive_id: u16,
    /// Byte offset of the CASC entry header within that archive.
    pub offset: u32,
    /// Bytes occupied, counting the 30-byte CASC entry header.
    pub size: u32,
}

impl Entry {
    /// Which bucket this key belongs to: xor every key byte together, then
    /// fold the result down to one nibble.
    ///
    /// Verified against a real install: correct for all 1,564,901 file
    /// entries. The only exceptions are [`Entry::is_archive_marker`] entries,
    /// which are bookkeeping and sit in every bucket regardless of their key.
    pub fn bucket(&self) -> u8 {
        let x = self.key.iter().fold(0u8, |a, b| a ^ b);
        (x & 0x0f) ^ (x >> 4)
    }

    /// Whether this is an archive marker rather than a file.
    ///
    /// Every `data.NNN` reserves `BUCKETS * CASC_ENTRY_HEADER` = 480 bytes at
    /// its head: sixteen bare 30-byte entry headers, one per bucket, with no
    /// BLTE payload behind any of them. Each bucket's index records its own
    /// slot, so the marker for bucket `b` sits at `offset = b * 30`. A real
    /// install therefore holds exactly `archives * 16` of these, 123 * 16 =
    /// 1968 here, 123 in every bucket.
    ///
    /// They matter twice over: they are why a naive "decode every index entry"
    /// pass reports failures on a perfectly healthy install, and an index we
    /// write has to reproduce them rather than treat them as files.
    pub fn is_archive_marker(&self) -> bool {
        let header = CASC_ENTRY_HEADER as u32;
        self.key[2..] == ARCHIVE_MARKER_SUFFIX
            && self.size == header
            && self.offset.is_multiple_of(header)
            && self.offset / header < u32::from(BUCKETS)
    }

    /// The marker `archive_id` gets in `bucket`'s index.
    pub fn archive_marker(archive_id: u16, bucket: u8) -> Self {
        let mut key = [0u8; KEY_SIZE];
        key[1] = archive_id as u8;
        key[2..].copy_from_slice(&ARCHIVE_MARKER_SUFFIX);
        Self {
            key,
            archive_id,
            offset: u32::from(bucket) * CASC_ENTRY_HEADER as u32,
            size: CASC_ENTRY_HEADER as u32,
        }
    }

    fn read(bytes: &[u8]) -> Self {
        let mut key = [0u8; KEY_SIZE];
        key.copy_from_slice(&bytes[..KEY_SIZE]);

        let high = bytes[KEY_SIZE] as u32;
        let low = u32::from_be_bytes(bytes[KEY_SIZE + 1..KEY_SIZE + 5].try_into().expect("5-byte location"));
        let size = u32::from_le_bytes(bytes[KEY_SIZE + 5..ENTRY_SIZE].try_into().expect("4-byte size"));

        Self {
            key,
            // The archive id straddles the byte boundary: 8 high bits in their
            // own byte, the low 2 above the offset in the packed word.
            archive_id: ((high << 2) | (low >> SEGMENT_BITS)) as u16,
            offset: low & OFFSET_MASK,
            size,
        }
    }

    fn write(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(&self.key);
        out.push((self.archive_id >> 2) as u8);
        let low = ((u32::from(self.archive_id) & 0b11) << SEGMENT_BITS) | (self.offset & OFFSET_MASK);
        out.extend_from_slice(&low.to_be_bytes());
        out.extend_from_slice(&self.size.to_le_bytes());
    }
}

/// A parsed `.idx`, faithful enough to write back unchanged.
///
/// Fields Lightforge never interprets, the block table, the unused header
/// byte, the file's padded size, are preserved rather than regenerated,
/// because an index that differs from Blizzard's in any byte is an index we
/// cannot prove Agent accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Index {
    version: u16,
    bucket: u8,
    unused: u8,
    /// Verbatim `(start, end)` pairs. Purpose undocumented; preserved as-is.
    blocks: Vec<(u32, u32)>,
    /// In file order, which is key-ascending, verified across a real
    /// install's 1.5 M entries, where nothing is out of order. Duplicate keys
    /// do occur there (57 of them), so this is a `Vec` in file order rather
    /// than a map: collapsing them would change bytes Blizzard wrote.
    entries: Vec<Entry>,
    file_size: usize,
}

impl Index {
    /// An empty index for `bucket`, shaped like the ones Blizzard ships.
    pub fn new(bucket: u8) -> Self {
        Self {
            version: VERSION,
            bucket,
            unused: 0,
            blocks: vec![(0x0000_00c0, 0xff00_0000)],
            entries: Vec::new(),
            file_size: DEFAULT_FILE_SIZE,
        }
    }

    pub fn bucket(&self) -> u8 {
        self.bucket
    }

    pub fn version(&self) -> u16 {
        self.version
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    /// How many more entries fit before the file's fixed size is exceeded.
    pub fn remaining_capacity(&self) -> usize {
        let used = data_offset(self.blocks.len()) + 8 + self.entries.len() * ENTRY_SIZE;
        self.file_size.saturating_sub(used) / ENTRY_SIZE
    }

    /// Entries that name real files, skipping the archive markers.
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.entries.iter().filter(|e| !e.is_archive_marker())
    }

    /// Find an entry by truncated key.
    pub fn lookup(&self, key: &[u8; KEY_SIZE]) -> Option<&Entry> {
        let i = self.entries.binary_search_by(|e| e.key.cmp(key)).ok()?;
        Some(&self.entries[i])
    }

    /// Insert or replace an entry, keeping the key ordering the format needs.
    ///
    /// Returns the entry it displaced. Replacement is the normal case during a
    /// patch: the same file moves to a new archive location.
    ///
    /// Note this *collapses* a duplicate key rather than appending a second
    /// copy. Real indices do contain a few duplicates, and [`Index::parse`]
    /// preserves them, but there is no reason for a writer to create one.
    pub fn insert(&mut self, entry: Entry) -> Option<Entry> {
        match self.entries.binary_search_by(|e| e.key.cmp(&entry.key)) {
            Ok(i) => Some(std::mem::replace(&mut self.entries[i], entry)),
            Err(i) => {
                self.entries.insert(i, entry);
                None
            }
        }
    }

    pub fn remove(&mut self, key: &[u8; KEY_SIZE]) -> Option<Entry> {
        let i = self.entries.binary_search_by(|e| e.key.cmp(key)).ok()?;
        Some(self.entries.remove(i))
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, IdxError> {
        if bytes.len() < 16 {
            return Err(IdxError::Truncated { need: 16, got: bytes.len() });
        }

        let data_size = le_u32(bytes, 0) as usize;
        let stored_hash = le_u32(bytes, 4);
        let body_end = 8 + data_size;
        if data_size < 8 || bytes.len() < body_end {
            return Err(IdxError::Truncated { need: body_end, got: bytes.len() });
        }

        let body = &bytes[8..body_end];
        let hash = hashlittle(body, 0);
        if hash != stored_hash {
            return Err(IdxError::HeaderHash { expected: stored_hash, found: hash });
        }

        let version = u16::from_le_bytes([body[0], body[1]]);
        if version != VERSION {
            return Err(IdxError::UnsupportedVersion(version));
        }
        let bucket = body[2];
        let unused = body[3];
        let (length_field, location_field, key_field, segment_bits) =
            (body[4], body[5], body[6], body[7]);

        // Only the shape Blizzard actually ships is supported. Guessing at a
        // layout we have never seen would produce an index we cannot verify.
        if (key_field as usize, location_field as usize, length_field as usize)
            != (KEY_SIZE, LOCATION_SIZE, LENGTH_SIZE)
            || segment_bits != SEGMENT_BITS
        {
            return Err(IdxError::UnsupportedLayout {
                key: key_field,
                location: location_field,
                length: length_field,
                segment_bits,
            });
        }

        let blocks = body[8..]
            .as_chunks::<8>()
            .0
            .iter()
            .map(|c| (u32::from_be_bytes(c[..4].try_into().unwrap()), u32::from_be_bytes(c[4..].try_into().unwrap())))
            .collect::<Vec<_>>();

        let ds = data_offset(blocks.len());
        if bytes.len() < ds + 8 {
            return Err(IdxError::Truncated { need: ds + 8, got: bytes.len() });
        }
        let section_size = le_u32(bytes, ds) as usize;
        let stored_data_hash = le_u32(bytes, ds + 4);
        let end = ds + 8 + section_size;
        if bytes.len() < end {
            return Err(IdxError::Truncated { need: end, got: bytes.len() });
        }
        if !section_size.is_multiple_of(ENTRY_SIZE) {
            return Err(IdxError::RaggedSection { size: section_size, entry_size: ENTRY_SIZE });
        }

        let raw = &bytes[ds + 8..end];
        let hash = entries_hash(raw);
        if hash != stored_data_hash {
            return Err(IdxError::DataHash { expected: stored_data_hash, found: hash });
        }

        let entries: Vec<Entry> = raw.as_chunks::<ENTRY_SIZE>().0.iter().map(|c| Entry::read(c)).collect();

        Ok(Self {
            version,
            bucket,
            unused,
            blocks,
            entries,
            file_size: bytes.len(),
        })
    }

    /// Serialize to the on-disk form, padded to the file's fixed size.
    pub fn to_bytes(&self) -> Result<Vec<u8>, IdxError> {
        let mut body = Vec::with_capacity(8 + self.blocks.len() * 8);
        body.extend_from_slice(&self.version.to_le_bytes());
        body.push(self.bucket);
        body.push(self.unused);
        body.push(LENGTH_SIZE as u8);
        body.push(LOCATION_SIZE as u8);
        body.push(KEY_SIZE as u8);
        body.push(SEGMENT_BITS);
        for (start, end) in &self.blocks {
            body.extend_from_slice(&start.to_be_bytes());
            body.extend_from_slice(&end.to_be_bytes());
        }

        let mut entries = Vec::with_capacity(self.entries.len() * ENTRY_SIZE);
        for entry in &self.entries {
            entry.write(&mut entries);
        }

        // Grow to fit rather than refusing. A store this writer resumes was
        // very likely last written by someone else, the client rewrites
        // indices at its own size, so the size a file happened to have when
        // it was loaded is not a budget the next generation has to live within.
        let total = data_offset(self.blocks.len()) + 8 + entries.len();
        // Reserve before rounding: the client needs the tail free, so a file
        // that ends exactly on a 64 KiB boundary still has to grow.
        let file_size = self
            .file_size
            .max((total + KMT_RESERVE).div_ceil(SIZE_GRANULARITY) * SIZE_GRANULARITY);
        if file_size > MAX_FILE_SIZE {
            return Err(IdxError::BucketFull {
                bucket: self.bucket,
                entries: self.entries.len(),
                capacity: (MAX_FILE_SIZE - data_offset(self.blocks.len()) - 8) / ENTRY_SIZE,
            });
        }

        let mut out = Vec::with_capacity(file_size);
        out.extend_from_slice(&(body.len() as u32).to_le_bytes());
        out.extend_from_slice(&hashlittle(&body, 0).to_le_bytes());
        out.extend_from_slice(&body);
        out.resize(data_offset(self.blocks.len()), 0);

        out.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        out.extend_from_slice(&entries_hash(&entries).to_le_bytes());
        out.extend_from_slice(&entries);
        out.resize(file_size, 0);
        Ok(out)
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, IdxError> {
        let path = path.as_ref();
        let bytes = std::fs::read(path).map_err(|source| IdxError::Io { path: path.to_path_buf(), source })?;
        Self::parse(&bytes)
    }

    /// Write this index into `dir` as a new generation.
    ///
    /// The sequence number in the file name is how CASC updates an index
    /// atomically: a fresh file appears and the previous generation is left in
    /// place until it is pruned, so a reader never sees a half-written index.
    /// Rewriting the existing file instead would put a torn index in front of
    /// Agent, which is the failure this module exists to prevent.
    ///
    /// Returns the path written.
    pub fn save_new_generation(&self, dir: impl AsRef<Path>) -> Result<PathBuf, IdxError> {
        let dir = dir.as_ref();
        let sequence = latest_sequence(dir, self.bucket)?.map_or(0, |s| s + 1);
        self.save_generation(dir, sequence)
    }

    /// Write this index as a specific generation.
    ///
    /// Callers rebuilding an existing store should carry the source's
    /// generation numbers over rather than restarting from zero. A real
    /// install's are in the 0x32-0x38 range and there is exactly one per
    /// bucket; an index written at generation 0 is not a shape the client ever
    /// produces, and it is not safe to assume that goes unnoticed.
    pub fn save_generation(&self, dir: impl AsRef<Path>, sequence: u32) -> Result<PathBuf, IdxError> {
        let dir = dir.as_ref();
        let bytes = self.to_bytes()?;
        let path = dir.join(file_name(self.bucket, sequence));

        // A new generation is a new file, so a plain create is already atomic
        // with respect to readers of the previous one; the temp-and-rename is
        // here so a crash mid-write cannot leave a short file wearing a valid
        // generation's name.
        write_atomic(&path, &bytes)?;
        Ok(path)
    }
}

/// `<bucket:02x><sequence:08x>.idx`
pub fn file_name(bucket: u8, sequence: u32) -> String {
    format!("{bucket:02x}{sequence:08x}.idx")
}

/// Parse `<bucket><sequence>.idx` back into its parts.
pub fn parse_file_name(name: &str) -> Option<(u8, u32)> {
    let stem = name.strip_suffix(".idx")?;
    if stem.len() != 10 {
        return None;
    }
    let bucket = u8::from_str_radix(&stem[..2], 16).ok()?;
    let sequence = u32::from_str_radix(&stem[2..], 16).ok()?;
    Some((bucket, sequence))
}

/// Highest generation present for `bucket`, if the bucket has any index yet.
pub fn latest_sequence(dir: &Path, bucket: u8) -> Result<Option<u32>, IdxError> {
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(IdxError::Io { path: dir.to_path_buf(), source }),
    };
    Ok(entries
        .flatten()
        .filter_map(|e| parse_file_name(&e.file_name().to_string_lossy()))
        .filter(|(b, _)| *b == bucket)
        .map(|(_, seq)| seq)
        .max())
}

/// The newest generation of every bucket present in `dir`.
pub fn discover(dir: &Path) -> Result<Vec<PathBuf>, IdxError> {
    let mut newest: std::collections::HashMap<u8, (u32, PathBuf)> = std::collections::HashMap::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(source) => return Err(IdxError::Io { path: dir.to_path_buf(), source }),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Some((bucket, seq)) = parse_file_name(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        match newest.get(&bucket) {
            Some((best, _)) if *best >= seq => {}
            _ => {
                newest.insert(bucket, (seq, path));
            }
        }
    }
    let mut out: Vec<_> = newest.into_iter().collect();
    out.sort_by_key(|(bucket, _)| *bucket);
    Ok(out.into_iter().map(|(_, (_, path))| path).collect())
}

/// Where the data section starts: the header body, padded to 16 bytes.
fn data_offset(block_count: usize) -> usize {
    let body_end = 8 + 8 + block_count * 8;
    body_end + ((DATA_SECTION_ALIGN - (body_end % DATA_SECTION_ALIGN)) % DATA_SECTION_ALIGN)
}

/// The data section's checksum: `hashlittle2` applied to each entry in turn
/// with the state carried across, taking the final `pc`.
///
/// Deliberately not a single hash over the whole region, that produces a
/// different value, and it is the mistake that would make every index we write
/// look corrupt to Agent while passing any test we wrote against ourselves.
fn entries_hash(entries: &[u8]) -> u32 {
    let (mut pc, mut pb) = (0u32, 0u32);
    for entry in entries.as_chunks::<ENTRY_SIZE>().0 {
        hashlittle2(entry, &mut pc, &mut pb);
    }
    pc
}

fn le_u32(bytes: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(bytes[at..at + 4].try_into().expect("4 bytes in range"))
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), IdxError> {
    use std::io::Write;

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|source| IdxError::Io { path: dir.to_path_buf(), source })?;

    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |source| IdxError::Io { path: p.clone(), source }
    };
    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io(dir))?;
    tmp.write_all(bytes).map_err(io(tmp.path()))?;
    tmp.as_file().sync_all().map_err(io(tmp.path()))?;
    tmp.persist(path).map_err(|e| IdxError::Io { path: path.to_path_buf(), source: e.error })?;

    // A real install's indices are 0644. tempfile creates at 0600, which would
    // hand Agent and the client an index only this user can read, the same
    // silent tightening `buildinfo::save` guards against.
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
        .map_err(io(path))?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum IdxError {
    #[error(".idx is truncated: need at least {need} bytes, got {got}")]
    Truncated { need: usize, got: usize },

    #[error(".idx header hash is {found:08x}, expected {expected:08x} — the file is corrupt")]
    HeaderHash { expected: u32, found: u32 },

    #[error(".idx data hash is {found:08x}, expected {expected:08x} — the file is corrupt")]
    DataHash { expected: u32, found: u32 },

    #[error(".idx version {0} is not supported (only {VERSION} is)")]
    UnsupportedVersion(u16),

    #[error(
        ".idx uses an unsupported field layout (key {key}, location {location}, length {length}, \
         segment bits {segment_bits}); only {KEY_SIZE}/{LOCATION_SIZE}/{LENGTH_SIZE}/{SEGMENT_BITS} is known"
    )]
    UnsupportedLayout { key: u8, location: u8, length: u8, segment_bits: u8 },

    #[error(".idx data section is {size} bytes, not a whole number of {entry_size}-byte entries")]
    RaggedSection { size: usize, entry_size: usize },

    #[error("bucket {bucket:02x} holds {entries} entries but only {capacity} fit in an index file")]
    BucketFull { bucket: u8, entries: usize, capacity: usize },

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: [u8; 9], archive_id: u16, offset: u32, size: u32) -> Entry {
        Entry { key, archive_id, offset, size }
    }

    #[test]
    fn an_entry_round_trips_through_its_packed_form() {
        // The real first entry of bucket 00 in a 12.1.0 install: archive 27 at
        // offset 1049269890, which is past the 2-bit boundary and so exercises
        // the split archive id rather than a value that happens to fit.
        let e = entry([0x00, 0x00, 0x52, 0xe5, 0x14, 0xf8, 0x9d, 0x30, 0x2b], 27, 1_049_269_890, 18_355);
        let mut buf = Vec::new();
        e.write(&mut buf);
        assert_eq!(buf.len(), ENTRY_SIZE);
        assert_eq!(Entry::read(&buf), e);

        // Byte-level shape, so a refactor can't quietly change the packing.
        assert_eq!(buf[9], 0x06, "archive id high byte");
        assert_eq!(&buf[10..14], &0xfe8a_9682u32.to_be_bytes(), "packed id + offset");
        assert_eq!(&buf[14..18], &18_355u32.to_le_bytes(), "size is little-endian");
    }

    #[test]
    fn the_largest_representable_location_survives() {
        let e = entry([0xff; 9], 0x3ff, OFFSET_MASK, u32::MAX);
        let mut buf = Vec::new();
        e.write(&mut buf);
        assert_eq!(Entry::read(&buf), e);
    }

    #[test]
    fn the_data_hash_is_chained_per_entry_not_one_pass() {
        // Pinning the distinction that cost the most to find: a single hash
        // over the whole region is a different value, and using it would make
        // every index we write look corrupt to Agent.
        let mut raw = Vec::new();
        for i in 0..4u8 {
            entry([i; 9], i as u16, i as u32 * 1000, 100).write(&mut raw);
        }
        assert_ne!(entries_hash(&raw), hashlittle(&raw, 0));

        let (mut pc, mut pb) = (0u32, 0u32);
        for e in raw.as_chunks::<ENTRY_SIZE>().0 {
            hashlittle2(e, &mut pc, &mut pb);
        }
        assert_eq!(entries_hash(&raw), pc);
    }

    #[test]
    fn a_written_index_parses_back_identically() {
        let mut idx = Index::new(0x03);
        for i in 0..500u32 {
            let mut key = [0u8; 9];
            key[..4].copy_from_slice(&i.to_be_bytes());
            idx.insert(entry(key, (i % 128) as u16, i * 4096, 1024 + i));
        }

        let bytes = idx.to_bytes().unwrap();
        assert_eq!(bytes.len(), DEFAULT_FILE_SIZE);
        assert_eq!(Index::parse(&bytes).unwrap(), idx);
    }

    #[test]
    fn the_data_section_lands_where_blizzard_puts_it() {
        // One block table entry -> body ends at 0x18 -> padded to 0x20.
        assert_eq!(data_offset(1), 0x20);
    }

    #[test]
    fn entries_stay_sorted_and_insert_replaces_in_place() {
        let mut idx = Index::new(0);
        idx.insert(entry([9; 9], 1, 0, 10));
        idx.insert(entry([1; 9], 2, 0, 20));
        idx.insert(entry([5; 9], 3, 0, 30));
        assert_eq!(idx.entries().iter().map(|e| e.key[0]).collect::<Vec<_>>(), [1, 5, 9]);

        // A patch moving a file to a new archive must replace, not duplicate.
        let old = idx.insert(entry([5; 9], 99, 4096, 40));
        assert_eq!(old.unwrap().archive_id, 3);
        assert_eq!(idx.entries().len(), 3);
        assert_eq!(idx.lookup(&[5; 9]).unwrap().archive_id, 99);

        assert_eq!(idx.remove(&[5; 9]).unwrap().archive_id, 99);
        assert!(idx.lookup(&[5; 9]).is_none());
    }

    #[test]
    fn corruption_is_reported_rather_than_parsed() {
        let mut bytes = Index::new(0).to_bytes().unwrap();
        bytes[9] ^= 0xff; // flip a bit inside the hashed header body
        assert!(matches!(Index::parse(&bytes), Err(IdxError::HeaderHash { .. })));

        let mut idx = Index::new(0);
        idx.insert(entry([7; 9], 1, 0, 10));
        let mut bytes = idx.to_bytes().unwrap();
        bytes[0x28] ^= 0xff; // flip a bit inside an entry
        assert!(matches!(Index::parse(&bytes), Err(IdxError::DataHash { .. })));
    }

    #[test]
    fn every_index_keeps_the_clients_update_section_free() {
        // The failure this prevents: with entries packed to the end of the
        // file the client reports a truncated KMT update section and declares
        // the whole container NeedsRepair.
        for count in [1usize, 1000, 50_000] {
            let mut idx = Index::new(0);
            for i in 0..count {
                let mut key = [0u8; 9];
                key[..8].copy_from_slice(&(i as u64).to_be_bytes());
                idx.insert(entry(key, 0, 0, 1));
            }
            let bytes = idx.to_bytes().unwrap();
            let used = data_offset(1) + 8 + count * ENTRY_SIZE;

            assert!(
                bytes.len() - used >= KMT_RESERVE,
                "{count} entries left only {} bytes free",
                bytes.len() - used
            );
            assert_eq!(bytes.len() % SIZE_GRANULARITY, 0);
        }
    }

    #[test]
    fn entries_ending_on_a_block_boundary_still_grow() {
        // Rounding alone would leave zero free space here.
        let per_block = SIZE_GRANULARITY / ENTRY_SIZE;
        let mut idx = Index::new(0);
        for i in 0..per_block * 4 {
            let mut key = [0u8; 9];
            key[..8].copy_from_slice(&(i as u64).to_be_bytes());
            idx.insert(entry(key, 0, 0, 1));
        }
        let bytes = idx.to_bytes().unwrap();
        let used = data_offset(1) + 8 + per_block * 4 * ENTRY_SIZE;
        assert!(bytes.len() - used >= KMT_RESERVE);
    }

    #[test]
    fn a_bucket_that_outgrows_its_file_is_grown_in_64_kib_units() {
        // The failure this guards against is concrete: a store the WoW client
        // last wrote has small indices (196,608 bytes on a real one), and a
        // resumed download that adds entries must grow the file rather than
        // refuse to checkpoint, refusing loses everything the run fetched.
        let mut idx = Index::new(0);
        let capacity = idx.remaining_capacity();
        for i in 0..=capacity {
            let mut key = [0u8; 9];
            key[..8].copy_from_slice(&(i as u64).to_be_bytes());
            idx.insert(entry(key, 0, 0, 1));
        }

        let bytes = idx.to_bytes().expect("grown, not refused");
        assert!(bytes.len() > DEFAULT_FILE_SIZE);
        assert_eq!(bytes.len() % SIZE_GRANULARITY, 0);
        // And it still reads back as itself.
        assert_eq!(Index::parse(&bytes).unwrap().entries().len(), capacity + 1);
    }

    #[test]
    fn a_small_file_from_the_client_is_grown_rather_than_treated_as_a_budget() {
        // Round-trip a deliberately small index, then add enough to overflow it.
        let mut idx = Index::new(3);
        idx.insert(entry([1; 9], 0, 0, 10));
        let small = idx.to_bytes().unwrap();
        let mut reloaded = Index::parse(&small).unwrap();

        let capacity = reloaded.remaining_capacity();
        for i in 0..=capacity {
            let mut key = [0u8; 9];
            key[..8].copy_from_slice(&(i as u64).to_be_bytes());
            reloaded.insert(entry(key, 0, 0, 1));
        }
        let grown = reloaded.to_bytes().expect("grown, not refused");
        assert!(grown.len() > small.len());
        assert_eq!(grown.len() % SIZE_GRANULARITY, 0);
    }

    #[test]
    fn parsing_preserves_duplicate_keys_even_though_insert_collapses_them() {
        // Real indices carry a handful of duplicate keys. Round-tripping one
        // must not quietly drop the second copy, or the file we write back
        // stops matching the file we read.
        let mut raw = Vec::new();
        for (key, archive) in [([1u8; 9], 1u16), ([5; 9], 2), ([5; 9], 3), ([9; 9], 4)] {
            entry(key, archive, 0, 100).write(&mut raw);
        }
        let mut idx = Index::new(0);
        idx.entries = raw.as_chunks::<ENTRY_SIZE>().0.iter().map(|c| Entry::read(c)).collect();

        let bytes = idx.to_bytes().unwrap();
        let parsed = Index::parse(&bytes).unwrap();
        assert_eq!(parsed.entries().len(), 4);
        assert_eq!(parsed, idx);

        // An explicit insert of that key is a replacement, not a third copy.
        let mut writer = Index::new(0);
        writer.insert(entry([5; 9], 2, 0, 100));
        writer.insert(entry([5; 9], 3, 0, 100));
        assert_eq!(writer.entries().len(), 1);
    }

    #[test]
    fn archive_markers_are_recognised_and_excluded_from_files() {
        // Shapes taken verbatim from a real install. The marker's offset is
        // its bucket's slot in the archive's reserved head, so the same
        // archive appears at a different offset in each of the 16 indices,
        // checking only bucket 00's offset of 0 misses 15/16 of them.
        let real = entry([0x00, 0x05, 0x00, 0x7e, 0xc0, 0x16, 0xa8, 0x90, 0x21], 5, 0, 30);
        assert!(real.is_archive_marker());
        assert_eq!(Entry::archive_marker(5, 0), real);

        let in_bucket_3 = entry([0x00, 0x08, 0x00, 0x7e, 0xc0, 0x16, 0xa8, 0x90, 0x21], 8, 90, 30);
        assert!(in_bucket_3.is_archive_marker());
        assert_eq!(Entry::archive_marker(8, 3), in_bucket_3);

        // One slot past the last bucket is a file, not a marker.
        let past_end = entry([0x00, 0x08, 0x00, 0x7e, 0xc0, 0x16, 0xa8, 0x90, 0x21], 8, 480, 30);
        assert!(!past_end.is_archive_marker());
        assert_eq!(ARCHIVE_RESERVED, 480);

        // A file entry that happens to start with the same two bytes is not a
        // marker, the suffix, offset and size all have to agree.
        let file = entry([0x00, 0x05, 0x52, 0xe5, 0x14, 0xf8, 0x9d, 0x30, 0x2b], 27, 4096, 18_355);
        assert!(!file.is_archive_marker());

        let mut idx = Index::new(0);
        idx.insert(real);
        idx.insert(file);
        assert_eq!(idx.entries().len(), 2);
        assert_eq!(idx.files().count(), 1);
    }

    #[test]
    fn the_bucket_rule_folds_every_key_byte() {
        // Real key from bucket 00 of a 12.1.0 install.
        let e = entry([0x00, 0x00, 0x52, 0xe5, 0x14, 0xf8, 0x9d, 0x30, 0x2b], 27, 0, 1);
        assert_eq!(e.bucket(), 0x00);
        assert!(e.bucket() < BUCKETS);
    }

    #[test]
    fn file_names_round_trip() {
        assert_eq!(file_name(0x00, 0x35), "0000000035.idx");
        assert_eq!(file_name(0x0f, 0x35), "0f00000035.idx");
        assert_eq!(parse_file_name("0000000035.idx"), Some((0x00, 0x35)));
        assert_eq!(parse_file_name("0a00000032.idx"), Some((0x0a, 0x32)));
        assert_eq!(parse_file_name("data.027"), None);
        assert_eq!(parse_file_name("short.idx"), None);
    }

    #[test]
    fn saving_writes_a_new_generation_and_leaves_the_old_one_alone() {
        let dir = std::env::temp_dir().join("lightforge-idx-generations");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let mut idx = Index::new(0x02);
        idx.insert(entry([1; 9], 0, 0, 100));

        let first = idx.save_new_generation(&dir).unwrap();
        assert_eq!(first.file_name().unwrap(), "0200000000.idx");

        idx.insert(entry([2; 9], 0, 4096, 200));
        let second = idx.save_new_generation(&dir).unwrap();
        assert_eq!(second.file_name().unwrap(), "0200000001.idx");

        // The previous generation must survive: a reader mid-parse depends on it.
        assert!(first.exists());
        assert_eq!(Index::load(&first).unwrap().entries().len(), 1);
        assert_eq!(Index::load(&second).unwrap().entries().len(), 2);

        // Discovery hands back only the newest generation per bucket.
        assert_eq!(discover(&dir).unwrap(), vec![second]);
    }
}
