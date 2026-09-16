//! Building a local CASC store: archives plus the indices that find them.
//!
//! The inverse of [`super::storage`]. A downloader hands BLTE payloads in,
//! this lays them out the way Agent would, and the result has to be
//! indistinguishable from an install Blizzard produced — anything else and
//! Agent decides the install is damaged and starts the repair scan the whole
//! project exists to avoid.
//!
//! Three details are load-bearing and none are guessable:
//!
//! 1. Every archive reserves [`idx::ARCHIVE_RESERVED`] bytes at its head, and
//!    each bucket's index records a marker pointing at its own slot there.
//! 2. Each file gets a 30-byte [`entry::Header`] whose second checksum depends
//!    on the archive number and offset it is written at, so payload and
//!    position have to be decided together.
//! 3. Indices are written as a **new generation** rather than in place, which
//!    is how CASC updates an index without a reader ever seeing a half-written
//!    one.
//!
//! Offsets are 30 bits in an index entry, so an archive cannot exceed
//! [`MAX_ARCHIVE_SIZE`]; the writer rolls to a new one before that happens.

use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use super::idx::{self, Entry, Index, IdxError, BUCKETS, CASC_ENTRY_HEADER};
use super::entry::Header;
use super::storage::{archive_name, EKey};

/// Largest an archive may get: index entries carry a 30-bit offset.
pub const MAX_ARCHIVE_SIZE: u64 = 1 << 30;

/// Writes files into a CASC store.
///
/// Nothing is durable until [`StoreWriter::finish`] runs — it is what writes
/// the indices, and an archive nothing indexes is unreachable bytes.
#[derive(Debug)]
pub struct StoreWriter {
    data_dir: PathBuf,
    indices: HashMap<u8, Index>,
    archive_id: u16,
    archive: Option<BufWriter<File>>,
    /// Write position within the current archive.
    offset: u64,
    max_archive: u64,
    files: usize,
    bytes: u64,
}

impl StoreWriter {
    /// Start writing into `data_dir`, which is created if absent.
    ///
    /// Existing archives are not appended to. A fresh set is started after the
    /// highest `data.NNN` already present, so an interrupted run leaves the
    /// previous archives untouched rather than half-rewritten.
    pub fn create(data_dir: impl Into<PathBuf>) -> Result<Self, WriteError> {
        let data_dir = data_dir.into();
        std::fs::create_dir_all(&data_dir)
            .map_err(|source| WriteError::Io { path: data_dir.clone(), source })?;

        let archive_id = next_archive_id(&data_dir)?;
        let indices = (0..BUCKETS).map(|b| (b, Index::new(b))).collect();

        Ok(Self {
            data_dir,
            indices,
            archive_id,
            archive: None,
            offset: 0,
            max_archive: MAX_ARCHIVE_SIZE,
            files: 0,
            bytes: 0,
        })
    }

    /// Cap archives lower than the format allows. Only useful for tests, which
    /// would otherwise have to write a gigabyte to see a roll-over.
    pub fn with_max_archive_size(mut self, bytes: u64) -> Self {
        self.max_archive = bytes.min(MAX_ARCHIVE_SIZE);
        self
    }

    pub fn files_written(&self) -> usize {
        self.files
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes
    }

    /// Whether this key has already been written.
    pub fn contains(&self, ekey: &EKey) -> bool {
        let Ok(key) = <[u8; 9]>::try_from(&ekey[..9]) else {
            return false;
        };
        let probe = Entry { key, archive_id: 0, offset: 0, size: 0 };
        self.indices.get(&probe.bucket()).is_some_and(|i| i.lookup(&key).is_some())
    }

    /// Write one BLTE payload under its encoding key.
    ///
    /// `blte` is the encoded file exactly as the CDN serves it — no CASC
    /// header, which this adds. Returns where it landed, or where it already
    /// was: a key written twice is stored once.
    ///
    /// Deduplication is not an optimisation here, it is required. WoW shares
    /// content aggressively — many file ids resolve to one encoding key — so a
    /// caller walking file ids hands the same payload over repeatedly. Writing
    /// each copy would append bytes that the index, which holds one entry per
    /// key, then has no way to reach: dead weight in the archive forever.
    pub fn write(&mut self, ekey: &EKey, blte: &[u8]) -> Result<Entry, WriteError> {
        if let Ok(key) = <[u8; 9]>::try_from(&ekey[..9]) {
            let probe = Entry { key, archive_id: 0, offset: 0, size: 0 };
            if let Some(existing) = self.indices.get(&probe.bucket()).and_then(|i| i.lookup(&key)) {
                return Ok(*existing);
            }
        }

        let total = CASC_ENTRY_HEADER as u64 + blte.len() as u64;
        if total > self.max_archive {
            return Err(WriteError::FileTooLarge { size: total, max: self.max_archive });
        }

        // Roll to a new archive rather than let an offset exceed 30 bits.
        if self.archive.is_none() || self.offset + total > self.max_archive {
            self.roll_archive()?;
        }

        let offset = self.offset as u32;
        let key: [u8; 9] = ekey[..9].try_into().expect("9 of 16 bytes");
        let header = Header::build(&key, total as u32, self.archive_id, offset);

        let archive = self.archive.as_mut().expect("an archive is open");
        let path = || self.data_dir.join(archive_name(self.archive_id));
        archive
            .write_all(&header)
            .and_then(|()| archive.write_all(blte))
            .map_err(|source| WriteError::Io { path: path(), source })?;

        self.offset += total;
        self.files += 1;
        self.bytes += total;

        let entry = Entry { key, archive_id: self.archive_id, offset, size: total as u32 };
        self.index_for(&entry).insert(entry);
        Ok(entry)
    }

    /// Finish the store: flush the open archive and write every index.
    pub fn finish(mut self) -> Result<Summary, WriteError> {
        if let Some(mut archive) = self.archive.take() {
            let path = self.data_dir.join(archive_name(self.archive_id));
            archive
                .flush()
                .and_then(|()| archive.into_inner().map_err(std::io::Error::from)?.sync_all())
                .map_err(|source| WriteError::Io { path, source })?;
        }

        // Buckets that received nothing still get an index: a store missing
        // one of its sixteen is not a shape Blizzard ever produces.
        let mut written = Vec::new();
        let mut indices: Vec<_> = self.indices.into_values().collect();
        indices.sort_by_key(Index::bucket);
        for index in &indices {
            written.push(index.save_new_generation(&self.data_dir)?);
        }

        Ok(Summary {
            data_dir: self.data_dir,
            archives: (self.files > 0).then_some(self.archive_id).map_or(0, |id| usize::from(id) + 1),
            files: self.files,
            bytes: self.bytes,
            indices: written,
        })
    }

    /// Open the next archive and seed its reserved head.
    fn roll_archive(&mut self) -> Result<(), WriteError> {
        if let Some(mut open) = self.archive.take() {
            let path = self.data_dir.join(archive_name(self.archive_id));
            open.flush()
                .and_then(|()| open.into_inner().map_err(std::io::Error::from)?.sync_all())
                .map_err(|source| WriteError::Io { path, source })?;
            self.archive_id += 1;
        }

        let path = self.data_dir.join(archive_name(self.archive_id));
        let file = File::create(&path).map_err(|source| WriteError::Io { path: path.clone(), source })?;
        let mut archive = BufWriter::new(file);

        // The reserved head: sixteen 30-byte slots, one per bucket, which each
        // bucket's index points a marker entry at. Real archives have them and
        // an archive without them indexes its first real file at offset 0,
        // where Agent expects a marker.
        archive
            .write_all(&vec![0u8; idx::ARCHIVE_RESERVED])
            .map_err(|source| WriteError::Io { path, source })?;

        for bucket in 0..BUCKETS {
            let marker = Entry::archive_marker(self.archive_id, bucket);
            self.indices
                .entry(bucket)
                .or_insert_with(|| Index::new(bucket))
                .insert(marker);
        }

        self.archive = Some(archive);
        self.offset = idx::ARCHIVE_RESERVED as u64;
        Ok(())
    }

    fn index_for(&mut self, entry: &Entry) -> &mut Index {
        let bucket = entry.bucket();
        self.indices.entry(bucket).or_insert_with(|| Index::new(bucket))
    }
}

/// What a completed store contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub data_dir: PathBuf,
    pub archives: usize,
    pub files: usize,
    /// Archived bytes, entry headers included.
    pub bytes: u64,
    /// Index files written, one per bucket.
    pub indices: Vec<PathBuf>,
}

/// The archive number a fresh writer should start at.
fn next_archive_id(data_dir: &Path) -> Result<u16, WriteError> {
    let entries = match std::fs::read_dir(data_dir) {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(source) => return Err(WriteError::Io { path: data_dir.to_path_buf(), source }),
    };

    let highest = entries
        .flatten()
        .filter_map(|e| {
            e.file_name()
                .to_string_lossy()
                .strip_prefix("data.")
                .and_then(|n| n.parse::<u16>().ok())
        })
        .max();
    Ok(highest.map_or(0, |id| id + 1))
}

#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("a {size}-byte entry cannot fit in an archive capped at {max} bytes")]
    FileTooLarge { size: u64, max: u64 },

    #[error(transparent)]
    Idx(#[from] IdxError),

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
    use crate::tact::storage::Storage;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-writer-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn blte(payload: &[u8]) -> Vec<u8> {
        blte::compress_data_single(payload.to_vec(), blte::CompressionMode::ZLib, None).unwrap()
    }

    fn ekey(n: u8) -> EKey {
        let mut k = [0u8; 16];
        k[0] = n;
        k[8] = n.wrapping_mul(7).wrapping_add(1);
        k
    }

    #[test]
    fn a_written_store_reads_back_through_the_normal_reader() {
        // The loop that matters: write with this module, read with the module
        // that was verified against a real install.
        let dir = scratch("roundtrip");
        let data = dir.join("Data").join("data");

        let payloads: Vec<Vec<u8>> = (0..25u8)
            .map(|n| format!("lightforge payload {n}").repeat(20).into_bytes())
            .collect();

        let mut writer = StoreWriter::create(&data).unwrap();
        for (n, payload) in payloads.iter().enumerate() {
            writer.write(&ekey(n as u8), &blte(payload)).unwrap();
        }
        let summary = writer.finish().unwrap();

        assert_eq!(summary.files, 25);
        assert_eq!(summary.indices.len(), usize::from(BUCKETS));

        let mut storage = Storage::open(&dir).unwrap();
        for (n, payload) in payloads.iter().enumerate() {
            let got = storage.read(&ekey(n as u8)).unwrap();
            assert_eq!(&got, payload, "payload {n} did not survive the round trip");
        }
        assert_eq!(storage.stats().files, 25);
    }

    #[test]
    fn every_archive_gets_its_reserved_head_and_markers() {
        let dir = scratch("markers");
        let data = dir.join("Data").join("data");

        let mut writer = StoreWriter::create(&data).unwrap();
        writer.write(&ekey(1), &blte(b"one")).unwrap();
        writer.finish().unwrap();

        // The first real entry must start after the reserved head, never at 0.
        let storage = Storage::open(&dir).unwrap();
        let file = storage.files().next().unwrap();
        assert!(file.offset >= idx::ARCHIVE_RESERVED as u32);

        // One marker per bucket for the one archive written.
        let markers: usize = storage
            .indices()
            .iter()
            .map(|i| i.entries().len() - i.files().count())
            .sum();
        assert_eq!(markers, usize::from(BUCKETS));
    }

    #[test]
    fn entry_headers_verify_where_they_were_written() {
        let dir = scratch("headers");
        let data = dir.join("Data").join("data");

        let mut writer = StoreWriter::create(&data).unwrap();
        for n in 0..8u8 {
            writer.write(&ekey(n), &blte(b"payload")).unwrap();
        }
        writer.finish().unwrap();

        let mut storage = Storage::open(&dir).unwrap();
        let files: Vec<_> = storage.files().copied().collect();
        assert_eq!(files.len(), 8);
        for entry in &files {
            let raw = storage.read_raw(entry).unwrap();
            assert!(
                Header::verify(&raw, entry.archive_id, entry.offset),
                "header at archive {} offset {} does not verify",
                entry.archive_id,
                entry.offset
            );
        }
    }

    #[test]
    fn archives_roll_over_rather_than_overflow_an_offset() {
        let dir = scratch("rollover");
        let data = dir.join("Data").join("data");

        // The cap has to be tight in terms of *stored* bytes: a payload of one
        // repeated byte compresses to almost nothing, so sizing this against
        // the uncompressed length would never trigger a roll at all.
        let mut writer = StoreWriter::create(&data).unwrap().with_max_archive_size(700);
        for n in 0..12u8 {
            writer.write(&ekey(n), &blte(&[n; 400])).unwrap();
        }
        let summary = writer.finish().unwrap();
        assert!(summary.archives > 1, "expected several archives, got {}", summary.archives);

        // Every file still resolves, and every archive still got its markers.
        let mut storage = Storage::open(&dir).unwrap();
        for n in 0..12u8 {
            assert!(storage.read(&ekey(n)).is_ok(), "file {n} lost across the roll");
        }
        let markers: usize = storage
            .indices()
            .iter()
            .map(|i| i.entries().len() - i.files().count())
            .sum();
        assert_eq!(markers, summary.archives * usize::from(BUCKETS));
    }

    #[test]
    fn writing_the_same_key_twice_stores_it_once() {
        // Not hypothetical: copying 244 real files out of a live install hands
        // 14 repeats over, because different file ids share content.
        let dir = scratch("dedupe");
        let data = dir.join("Data").join("data");

        let payload = blte(b"shared content");
        let mut writer = StoreWriter::create(&data).unwrap();
        let first = writer.write(&ekey(1), &payload).unwrap();
        assert!(writer.contains(&ekey(1)));
        let again = writer.write(&ekey(1), &payload).unwrap();
        assert_eq!(first, again, "the second write must report the first location");
        assert_eq!(writer.files_written(), 1);

        let summary = writer.finish().unwrap();
        assert_eq!(summary.files, 1);

        let mut storage = Storage::open(&dir).unwrap();
        assert_eq!(storage.stats().files, 1);
        assert_eq!(storage.read(&ekey(1)).unwrap(), b"shared content");
    }

    #[test]
    fn a_file_too_large_for_an_archive_is_refused() {
        let dir = scratch("toobig");
        let mut writer = StoreWriter::create(dir.join("data")).unwrap().with_max_archive_size(1024);
        let err = writer.write(&ekey(1), &vec![0u8; 4096]).unwrap_err();
        assert!(matches!(err, WriteError::FileTooLarge { .. }));
    }

    #[test]
    fn a_fresh_writer_does_not_reuse_an_existing_archive_number() {
        let dir = scratch("resume");
        let data = dir.join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join("data.000"), b"existing").unwrap();
        std::fs::write(data.join("data.007"), b"existing").unwrap();

        assert_eq!(next_archive_id(&data).unwrap(), 8);

        // And the previous archives are left exactly as they were.
        let mut writer = StoreWriter::create(&data).unwrap();
        writer.write(&ekey(3), &blte(b"new")).unwrap();
        writer.finish().unwrap();
        assert_eq!(std::fs::read(data.join("data.000")).unwrap(), b"existing");
    }
}
