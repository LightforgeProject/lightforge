//! Verifying a local store against itself: is each file where the index says,
//! and is it the file the index claims?
//!
//! The planner already answers "what is missing" by comparing the index
//! against a build's download manifest. It cannot answer "what is *wrong*",
//! because an index entry is believed on sight. This closes that gap without
//! reading the store.
//!
//! **What makes it cheap.** Every file inside a `data.NNN` carries a 30-byte
//! CASC entry header (see [`crate::tact::entry`]), and that header's second
//! checksum mixes in the archive number and the byte offset. So reading 30
//! bytes tells you that the bytes at this offset really are this key, at this
//! size, written to this place. Across a 140 GB store that is about 55 MB of
//! reads rather than 140 GB, and sorting by `(archive, offset)` makes it
//! sequential.
//!
//! **What it cannot tell you.** A payload that rotted while its header stayed
//! intact reads as healthy here. [`deep_scan`] is the answer to that, and it
//! costs a read of the whole store.
//!
//! **What makes the deep scan affordable.** It never decodes anything. A BLTE
//! file carries a chunk table giving an MD5 of every chunk *as stored*:
//! compressed, and for encrypted chunks still encrypted, and the file's
//! encoding key is the MD5 of that table. So hashing the raw bytes verifies the
//! table against the key and every chunk against the table, which covers every
//! stored byte without zlib, without the encoding manifest, and without
//! decryption keys. Measured against a real 12.1.0 store: 3,672 of 3,672
//! sampled keys equal the MD5 of their chunk table, and 5,810 of 5,810 chunks
//! match their checksum, 77 of them encrypted.
//!
//! **Why a deep scan checks the store did not move under it.** It runs while
//! the game is up, because it only reads. Something else may not: an
//! `Agent.exe` update or a transfer writes the same archives, and reading a
//! file mid-write looks exactly like damage. A [`Snapshot`] of every index and
//! archive, taken before and compared after, turns that into
//! [`VerifyError::StoreChanged`] instead of a false report, whoever the
//! writer was.
//!
//! Repair is deliberately not a separate mechanism: a damaged entry is removed
//! from the index, which makes the planner see the file as absent, which makes
//! the ordinary download path fetch it again.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use super::download::Cancel;
use super::entry::Header;
use super::idx::{Entry, Index, CASC_ENTRY_HEADER, KEY_SIZE};
use super::storage::{archive_name, blte_payload, resolve_data_dir, Storage, StorageError};

/// Why one entry failed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// The `data.NNN` the entry points into is not there at all.
    MissingArchive,
    /// The entry runs past the end of its archive. Arithmetic alone catches
    /// this, and it is what a truncated or half-written archive looks like.
    PastEnd,
    /// The archive is long enough, but reading the header failed.
    Unreadable,
    /// The header names a different key than the index does.
    KeyMismatch,
    /// The header's size disagrees with the index's.
    SizeMismatch,
    /// One of the two header checksums did not reproduce. Checksum B covers
    /// the entry's location, so this also catches a file written at the wrong
    /// offset or copied from elsewhere in the store.
    BadChecksum,
    /// The bytes after the header are not a BLTE file at all.
    NotBlte,
    /// The BLTE chunk table cannot be read, or its chunks do not fill the
    /// entry.
    MalformedBlte,
    /// The file does not hash to its encoding key: for a chunked file the
    /// chunk table, for a single-chunk one the whole file.
    WrongPayload,
    /// A chunk's bytes do not hash to the checksum its table gives.
    BadChunk,
}

impl Fault {
    pub fn describe(self) -> &'static str {
        match self {
            Self::MissingArchive => "archive file is missing",
            Self::PastEnd => "entry runs past the end of its archive",
            Self::Unreadable => "header could not be read",
            Self::KeyMismatch => "header names a different key",
            Self::SizeMismatch => "header size disagrees with the index",
            Self::BadChecksum => "header checksum does not reproduce",
            Self::NotBlte => "payload is not a BLTE file",
            Self::MalformedBlte => "chunk table is unreadable",
            Self::WrongPayload => "file does not hash to its key",
            Self::BadChunk => "a chunk does not match its checksum",
        }
    }
}

/// One entry that failed, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Damage {
    pub entry: Entry,
    pub fault: Fault,
}

/// How far along a scan is, for a caller that wants to show it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub checked: u64,
    pub total: u64,
    pub damaged: u64,
    /// Payload bytes hashed so far, and in all. Zero for a header scan, which
    /// reads so little that files are the only honest measure.
    pub bytes: u64,
    pub bytes_total: u64,
}

/// What a scan found.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub checked: u64,
    /// Headers actually read. Lower than `checked` when entries failed the
    /// free arithmetic checks and never needed a read.
    pub headers_read: u64,
    pub bytes_read: u64,
    pub archives: usize,
    /// Archives named by the index that are not on disk.
    pub missing_archives: Vec<u16>,
    pub damaged: Vec<Damage>,
    /// Files a deep scan could read but has no way to check, see
    /// [`Payload::Unverifiable`]. Healthy as far as anyone can tell, and never
    /// repaired.
    pub unverifiable: u64,
    /// The store as it was when the scan began, for [`repair_unchanged`].
    pub snapshot: Snapshot,
}

impl Report {
    pub fn is_healthy(&self) -> bool {
        self.damaged.is_empty()
    }
}

/// Verify every file entry in `store`.
///
/// Read-only. `on_progress` is called periodically rather than per entry,
/// there are over a million of them, and a callback per entry costs more than
/// the check does.
pub fn scan(store: &Storage, mut on_progress: impl FnMut(Progress)) -> Report {
    let data_dir = store.data_dir().to_path_buf();
    let mut report = Report {
        snapshot: Snapshot::take(&data_dir).unwrap_or_default(),
        ..Report::default()
    };

    // Sorted by where the bytes are, not by key: the whole point of reading
    // headers rather than files is that the reads stay near each other, and
    // index order is hash order, which is to say random.
    let mut entries: Vec<Entry> = store.files().copied().collect();
    entries.sort_unstable_by_key(|e| (e.archive_id, e.offset));
    let total = entries.len() as u64;

    let mut lengths: BTreeMap<u16, Option<u64>> = BTreeMap::new();
    let mut open: Option<(u16, File)> = None;
    let mut header = [0u8; CASC_ENTRY_HEADER];

    for entry in entries {
        report.checked += 1;
        if report.checked.is_multiple_of(8192) {
            on_progress(Progress {
                checked: report.checked,
                total,
                damaged: report.damaged.len() as u64,
                ..Progress::default()
            });
        }

        let length = *lengths.entry(entry.archive_id).or_insert_with(|| {
            std::fs::metadata(data_dir.join(archive_name(entry.archive_id)))
                .ok()
                .map(|m| m.len())
        });
        let Some(length) = length else {
            if !report.missing_archives.contains(&entry.archive_id) {
                report.missing_archives.push(entry.archive_id);
            }
            report.damaged.push(Damage { entry, fault: Fault::MissingArchive });
            continue;
        };

        // Free: no read can make an entry fit inside an archive that ends
        // before it does.
        if u64::from(entry.offset) + u64::from(entry.size) > length {
            report.damaged.push(Damage { entry, fault: Fault::PastEnd });
            continue;
        }

        let file = match &mut open {
            Some((id, file)) if *id == entry.archive_id => file,
            _ => {
                let path = data_dir.join(archive_name(entry.archive_id));
                match File::open(&path) {
                    Ok(file) => {
                        report.archives += 1;
                        open = Some((entry.archive_id, file));
                        &mut open.as_mut().expect("just assigned").1
                    }
                    Err(_) => {
                        report.damaged.push(Damage { entry, fault: Fault::MissingArchive });
                        continue;
                    }
                }
            }
        };

        let read = file
            .seek(SeekFrom::Start(u64::from(entry.offset)))
            .and_then(|_| file.read_exact(&mut header));
        if read.is_err() {
            report.damaged.push(Damage { entry, fault: Fault::Unreadable });
            continue;
        }
        report.headers_read += 1;
        report.bytes_read += CASC_ENTRY_HEADER as u64;

        if let Some(fault) = fault_in(&header, &entry) {
            report.damaged.push(Damage { entry, fault });
        }
    }

    on_progress(Progress {
        checked: report.checked,
        total,
        damaged: report.damaged.len() as u64,
        ..Progress::default()
    });
    report
}

/// Check one header against the index entry that pointed at it.
///
/// An entry pointing straight at `BLTE` has no header to check: it is the
/// headerless alias of a file indexed again 30 bytes earlier, see
/// [`super::storage`]. Reading those bytes as a header reported every alias
/// as a key mismatch, and repairing that dropped real entries.
fn fault_in(header: &[u8], entry: &Entry) -> Option<Fault> {
    if header.starts_with(b"BLTE") {
        return None;
    }
    let parsed = Header::parse(header)?;
    if parsed.key != entry.key {
        return Some(Fault::KeyMismatch);
    }
    if parsed.size != entry.size {
        return Some(Fault::SizeMismatch);
    }
    if !Header::verify(header, entry.archive_id, entry.offset) {
        return Some(Fault::BadChecksum);
    }
    None
}

/// Verify every file in the store at `path` down to its last byte.
///
/// [`scan`] plus the payload, chunk table against encoding key and every chunk
/// against the table, with no decoding (see the module docs). Read-only, so
/// safe beside a running client (see [`VerifyError::StoreChanged`]). Archives
/// are spread over threads: hashing caps one thread at about 730 MB/s.
/// `on_progress` runs on the calling thread a few times a second.
pub fn deep_scan(
    path: impl AsRef<Path>,
    cancel: &Cancel,
    mut on_progress: impl FnMut(Progress),
) -> Result<Report, VerifyError> {
    let data_dir = resolve_data_dir(path.as_ref());
    // Before the indices are read, so a write that lands while they load is
    // caught too.
    let before = Snapshot::take(&data_dir)?;
    let store = Storage::open(path.as_ref())?;

    let mut by_archive: BTreeMap<u16, Vec<Entry>> = BTreeMap::new();
    for entry in store.files() {
        by_archive.entry(entry.archive_id).or_default().push(*entry);
    }
    let work: Vec<(u16, Vec<Entry>)> = by_archive
        .into_iter()
        .map(|(id, mut entries)| {
            entries.sort_unstable_by_key(|e| e.offset);
            (id, entries)
        })
        .collect();

    let total = work.iter().map(|(_, e)| e.len() as u64).sum();
    let bytes_total = work.iter().flat_map(|(_, e)| e).map(|e| u64::from(e.size)).sum();
    let shared = Shared::default();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).min(8);

    let parts: Vec<Part> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads)
            .map(|_| scope.spawn(|| deep_worker(&data_dir, &work, &shared, cancel)))
            .collect();

        while !workers.iter().all(|w| w.is_finished()) {
            on_progress(shared.progress(total, bytes_total));
            std::thread::sleep(Duration::from_millis(100));
        }
        workers
            .into_iter()
            .map(|w| w.join().expect("a verify worker panicked"))
            .collect()
    });
    on_progress(shared.progress(total, bytes_total));

    if cancel.is_cancelled() {
        return Err(VerifyError::Cancelled);
    }
    if Snapshot::take(&data_dir)? != before {
        return Err(VerifyError::StoreChanged);
    }

    let mut report = Report { snapshot: before, ..Report::default() };
    for part in parts {
        report.checked += part.checked;
        report.headers_read += part.headers_read;
        report.bytes_read += part.bytes_read;
        report.archives += part.archives;
        report.unverifiable += part.unverifiable;
        report.missing_archives.extend(part.missing_archives);
        report.damaged.extend(part.damaged);
    }
    report.missing_archives.sort_unstable();
    report.damaged.sort_unstable_by_key(|d| (d.entry.archive_id, d.entry.offset));
    Ok(report)
}

/// Counters every worker bumps, for progress.
#[derive(Default)]
struct Shared {
    next: AtomicUsize,
    checked: AtomicU64,
    damaged: AtomicU64,
    bytes: AtomicU64,
}

impl Shared {
    fn progress(&self, total: u64, bytes_total: u64) -> Progress {
        Progress {
            checked: self.checked.load(Ordering::Relaxed),
            total,
            damaged: self.damaged.load(Ordering::Relaxed),
            bytes: self.bytes.load(Ordering::Relaxed),
            bytes_total,
        }
    }
}

/// What one worker found across the archives it took.
#[derive(Default)]
struct Part {
    checked: u64,
    headers_read: u64,
    bytes_read: u64,
    archives: usize,
    unverifiable: u64,
    missing_archives: Vec<u16>,
    damaged: Vec<Damage>,
}

/// Take archives off the shared list until there are none left.
fn deep_worker(
    data_dir: &Path,
    work: &[(u16, Vec<Entry>)],
    shared: &Shared,
    cancel: &Cancel,
) -> Part {
    let mut part = Part::default();
    // Reused across entries: the largest file is a few hundred MB, and
    // allocating per entry would churn that for a million small ones.
    let mut buf = Vec::new();

    while let Some((archive_id, entries)) = work.get(shared.next.fetch_add(1, Ordering::Relaxed)) {
        let damaged = |part: &mut Part, entry: Entry, fault: Fault| {
            part.damaged.push(Damage { entry, fault });
            shared.damaged.fetch_add(1, Ordering::Relaxed);
        };

        let path = data_dir.join(archive_name(*archive_id));
        let opened = File::open(&path).and_then(|f| f.metadata().map(|m| (f, m.len())));
        let Ok((file, length)) = opened else {
            part.missing_archives.push(*archive_id);
            for entry in entries {
                damaged(&mut part, *entry, Fault::MissingArchive);
            }
            part.checked += entries.len() as u64;
            shared.checked.fetch_add(entries.len() as u64, Ordering::Relaxed);
            continue;
        };
        part.archives += 1;

        for entry in entries {
            if cancel.is_cancelled() {
                return part;
            }
            part.checked += 1;
            shared.checked.fetch_add(1, Ordering::Relaxed);

            if u64::from(entry.offset) + u64::from(entry.size) > length {
                damaged(&mut part, *entry, Fault::PastEnd);
                continue;
            }
            buf.resize(entry.size as usize, 0);
            if file.read_exact_at(&mut buf, u64::from(entry.offset)).is_err() {
                damaged(&mut part, *entry, Fault::Unreadable);
                continue;
            }
            part.headers_read += 1;
            part.bytes_read += u64::from(entry.size);
            shared.bytes.fetch_add(u64::from(entry.size), Ordering::Relaxed);

            let header = match buf.get(..CASC_ENTRY_HEADER) {
                Some(header) => fault_in(header, entry),
                None => Some(Fault::Unreadable),
            };
            match header.map_or_else(|| payload(&buf, &entry.key), Payload::Damaged) {
                Payload::Verified => {}
                Payload::Unverifiable => part.unverifiable += 1,
                Payload::Damaged(fault) => damaged(&mut part, *entry, fault),
            }
        }
    }
    part
}

/// What a deep scan could establish about one file's payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Payload {
    Verified,
    /// Stored raw, in a format whose key is not a hash of anything the scan
    /// can see. The only ones in a real store are patch manifests (`PA`), which
    /// Agent keeps for patching old builds; their key is neither the MD5 of
    /// the file nor of any part checked so far. Calling them damaged would be
    /// worse than useless: the downloader never fetches them, so a repair
    /// would delete them outright.
    Unverifiable,
    Damaged(Fault),
}

/// Check a file's stored bytes against its own chunk table and key.
///
/// ```text
/// 0x00  "BLTE"
/// 0x04  u32 BE   header size, 0 for a single chunk with no table
/// 0x08  u8       table format: 0x0F, or 0x10 with a decoded checksum per chunk
/// 0x09  u24 BE   chunk count
/// 0x0c  per chunk: u32 BE stored size, u32 BE decoded size, [u8; 16] MD5 ...
/// ```
///
/// The MD5 in the table is of the chunk as stored, mode byte included, which
/// is what lets this run without decoding.
fn payload(raw: &[u8], key: &[u8; KEY_SIZE]) -> Payload {
    let Some(blte) = blte_payload(raw) else {
        return match raw.get(CASC_ENTRY_HEADER..CASC_ENTRY_HEADER + 2) {
            Some(b"PA") => Payload::Unverifiable,
            _ => Payload::Damaged(Fault::NotBlte),
        };
    };
    match blte_fault(blte, key) {
        Some(fault) => Payload::Damaged(fault),
        None => Payload::Verified,
    }
}

pub(super) fn blte_fault(blte: &[u8], key: &[u8; KEY_SIZE]) -> Option<Fault> {
    let be32 = |at: usize| {
        blte.get(at..at + 4)
            .map(|b| u32::from_be_bytes(b.try_into().expect("four bytes")) as usize)
    };
    let hashes_to_key = |bytes: &[u8]| md5::compute(bytes).0[..KEY_SIZE] == key[..];

    let Some(header_size) = be32(4) else {
        return Some(Fault::MalformedBlte);
    };
    if header_size == 0 {
        return (!hashes_to_key(blte)).then_some(Fault::WrongPayload);
    }
    let Some(table) = blte.get(..header_size) else {
        return Some(Fault::MalformedBlte);
    };
    // First, and before any size in it is believed: a table that hashes to
    // the key is the one Blizzard published, so its sizes can be trusted.
    if !hashes_to_key(table) {
        return Some(Fault::WrongPayload);
    }

    let stride = match table.get(8) {
        Some(0x0f) => 24,
        Some(0x10) => 40,
        _ => return Some(Fault::MalformedBlte),
    };
    let count = table
        .get(9..12)
        .map(|c| (c[0] as usize) << 16 | (c[1] as usize) << 8 | c[2] as usize);
    let Some(count) = count.filter(|&n| 12 + n * stride == header_size) else {
        return Some(Fault::MalformedBlte);
    };

    let mut at = header_size;
    for i in 0..count {
        let row = 12 + i * stride;
        let size = be32(row).expect("inside a table whose length was checked");
        let want = &table[row + 8..row + 24];
        let Some(chunk) = blte.get(at..at + size) else {
            return Some(Fault::MalformedBlte);
        };
        if md5::compute(chunk).0[..] != want[..] {
            return Some(Fault::BadChunk);
        }
        at += size;
    }
    (at != blte.len()).then_some(Fault::MalformedBlte)
}

/// Every index and archive file in a store, by size and modification time.
///
/// Cheap, a couple of hundred `stat`s, and deliberately blunt: it cannot say
/// *who* wrote, only that the bytes a scan read may no longer be the bytes on
/// disk. That is the whole question a scan needs answered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Snapshot(BTreeMap<String, (u64, SystemTime)>);

impl Snapshot {
    pub fn take(data_dir: &Path) -> Result<Self, VerifyError> {
        let io = |source| VerifyError::Io { path: data_dir.to_path_buf(), source };
        let mut files = BTreeMap::new();
        for entry in std::fs::read_dir(data_dir).map_err(io)? {
            let entry = entry.map_err(io)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if !name.starts_with("data.") && super::idx::parse_file_name(&name).is_none() {
                continue;
            }
            let meta = entry.metadata().map_err(io)?;
            files.insert(name, (meta.len(), meta.modified().map_err(io)?));
        }
        Ok(Self(files))
    }
}

/// [`repair`], but only if the store is still the one `report` describes.
///
/// A scan's findings are positions in files. If an index generation or an
/// archive changed since, those positions may name healthy files now, and
/// dropping them would throw away good data.
pub fn repair_unchanged(data_dir: &Path, report: &Report) -> Result<usize, VerifyError> {
    if Snapshot::take(data_dir)? != report.snapshot {
        return Err(VerifyError::StoreChanged);
    }
    repair(data_dir, &report.damaged)
}

/// Remove damaged entries from the store's indices.
///
/// This *is* the repair: the planner asks the index what the store holds, so
/// an entry that is gone becomes a file the next download fetches again. The
/// archives are not touched, the bad bytes stay where they are and are
/// written over or orphaned, which is exactly what the writer already does
/// with superseded content.
///
/// Returns how many entries were removed. Never call it against a live client;
/// it rewrites the files the client memory-maps, which is what pre-flight
/// exists to prevent.
pub fn repair(data_dir: &Path, damaged: &[Damage]) -> Result<usize, VerifyError> {
    remove_entries(data_dir, damaged.iter().map(|d| d.entry))
}

/// Drop `entries` from the latest generation of each bucket's index, writing
/// a new generation per bucket touched. Returns how many were removed.
///
/// The archives are not touched. Same rule as [`repair`]: never against a
/// live client.
pub fn remove_entries(
    data_dir: &Path,
    entries: impl IntoIterator<Item = Entry>,
) -> Result<usize, VerifyError> {
    let mut by_bucket: BTreeMap<u8, Vec<Entry>> = BTreeMap::new();
    for entry in entries {
        by_bucket.entry(entry.bucket()).or_default().push(entry);
    }

    let mut removed = 0;
    for (bucket, entries) in by_bucket {
        let Some(sequence) = super::idx::latest_sequence(data_dir, bucket)? else {
            continue;
        };
        let path = data_dir.join(super::idx::file_name(bucket, sequence));
        let mut index = Index::load(&path)?;
        for entry in entries {
            // The entry itself, not the first with its key: a key can be
            // indexed twice, and the other one may be the healthy copy.
            if index.remove_entry(&entry) {
                removed += 1;
            }
        }
        index.save_new_generation(data_dir)?;
    }
    Ok(removed)
}

#[derive(Debug, thiserror::Error)]
pub enum VerifyError {
    #[error(transparent)]
    Index(#[from] super::idx::IdxError),

    #[error(transparent)]
    Storage(#[from] StorageError),

    #[error("the scan was cancelled")]
    Cancelled,

    /// Something wrote to the store while it was being read, so what the scan
    /// saw may not be what is on disk. Run it again once the writer is done.
    #[error("the store changed while it was being checked — run the scan again")]
    StoreChanged,

    #[error("{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact::writer::StoreWriter;

    /// A store holding two small files, built the way a download builds one.
    fn store_with(files: &[(u8, &[u8])]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Data").join("data");
        let mut writer = StoreWriter::create(&data).unwrap();
        for (n, bytes) in files {
            let mut key = [0u8; 16];
            key[0] = *n;
            key[15] = *n;
            writer.write(&key, bytes).unwrap();
        }
        writer.finish().unwrap();
        dir
    }

    fn open(dir: &tempfile::TempDir) -> Storage {
        Storage::open(dir.path().join("Data")).unwrap()
    }

    #[test]
    fn a_headerless_alias_is_not_reported_as_damage() {
        // The bytes at an alias's offset are its BLTE payload, not a header.
        let mut bytes = [0u8; CASC_ENTRY_HEADER];
        bytes[..4].copy_from_slice(b"BLTE");
        let alias = Entry { key: [5; 9], archive_id: 4, offset: 1030, size: 100 };
        assert_eq!(fault_in(&bytes, &alias), None);
    }

    #[test]
    fn a_store_the_writer_just_built_verifies_clean() {
        let dir = store_with(&[(1, b"hello"), (2, b"world")]);
        let report = scan(&open(&dir), |_| {});

        assert_eq!(report.checked, 2);
        assert_eq!(report.headers_read, 2);
        assert!(report.is_healthy(), "{:?}", report.damaged);
    }

    #[test]
    fn a_truncated_archive_is_caught_without_reading_it() {
        let dir = store_with(&[(1, b"hello"), (2, b"world")]);
        let archive = dir.path().join("Data").join("data").join(archive_name(0));
        let len = std::fs::metadata(&archive).unwrap().len();
        // Cut the tail off, the way an interrupted write or a full disk does.
        let file = std::fs::OpenOptions::new().write(true).open(&archive).unwrap();
        file.set_len(len - 4).unwrap();

        let report = scan(&open(&dir), |_| {});
        assert!(!report.is_healthy());
        assert_eq!(report.damaged.last().unwrap().fault, Fault::PastEnd);
    }

    #[test]
    fn a_corrupted_header_is_caught_by_its_checksum() {
        let dir = store_with(&[(1, b"hello")]);
        let archive = dir.path().join("Data").join("data").join(archive_name(0));
        let entry = *open(&dir).files().next().unwrap();

        // A bit flipped inside checksum A: the key and size still agree with
        // the index, so this only fails if the checksums are actually checked.
        let mut bytes = std::fs::read(&archive).unwrap();
        bytes[entry.offset as usize + 0x16] ^= 0x40;
        std::fs::write(&archive, &bytes).unwrap();

        let report = scan(&open(&dir), |_| {});
        assert_eq!(report.damaged.len(), 1);
        assert_eq!(report.damaged[0].fault, Fault::BadChecksum);
    }

    #[test]
    fn a_header_naming_another_file_is_caught_before_its_checksum() {
        // What a misindexed store looks like: the offset points at real,
        // internally consistent bytes that belong to a different key.
        let dir = store_with(&[(1, b"hello")]);
        let archive = dir.path().join("Data").join("data").join(archive_name(0));
        let entry = *open(&dir).files().next().unwrap();

        // The header stores its key byte-reversed across offsets 7..16, so
        // byte 15 is the key's first byte, flipping it changes which file
        // the header claims to be, which is checked before any checksum.
        let mut bytes = std::fs::read(&archive).unwrap();
        bytes[entry.offset as usize + 15] ^= 0xff;
        std::fs::write(&archive, &bytes).unwrap();

        let report = scan(&open(&dir), |_| {});
        assert_eq!(report.damaged[0].fault, Fault::KeyMismatch);
    }

    #[test]
    fn repair_removes_the_damaged_entry_so_a_plan_refetches_it() {
        let dir = store_with(&[(1, b"hello"), (2, b"world")]);
        let data = dir.path().join("Data").join("data");

        let damaged = {
            let store = open(&dir);
            let entry = *store.files().next().unwrap();
            vec![Damage { entry, fault: Fault::BadChecksum }]
        };
        assert_eq!(repair(&data, &damaged).unwrap(), 1);

        // Gone from the index, so the planner counts it absent.
        let store = open(&dir);
        assert_eq!(store.files().count(), 1);
        assert!(!store.contains(&{
            let mut k = [0u8; 16];
            k[0] = damaged[0].entry.key[0];
            k
        }));
    }

    #[test]
    fn repairing_nothing_changes_nothing() {
        let dir = store_with(&[(1, b"hello")]);
        let data = dir.path().join("Data").join("data");
        assert_eq!(repair(&data, &[]).unwrap(), 0);
        assert_eq!(open(&dir).files().count(), 1);
    }

    #[test]
    fn progress_reports_the_total_it_is_working_through() {
        let dir = store_with(&[(1, b"a"), (2, b"b"), (3, b"c")]);
        let mut last = Progress::default();
        scan(&open(&dir), |p| last = p);
        assert_eq!((last.checked, last.total), (3, 3));
    }

    /// A chunked BLTE file with uncompressed (`N`) chunks, and the encoding
    /// key a CDN would publish it under: the MD5 of its chunk table.
    fn blte(chunks: &[&[u8]]) -> ([u8; 16], Vec<u8>) {
        let header_size = 12 + 24 * chunks.len();
        let mut out = b"BLTE".to_vec();
        out.extend((header_size as u32).to_be_bytes());
        out.push(0x0f);
        out.extend(&(chunks.len() as u32).to_be_bytes()[1..]);
        let stored: Vec<Vec<u8>> = chunks.iter().map(|c| [b"N", *c].concat()).collect();
        for (chunk, body) in stored.iter().zip(chunks) {
            out.extend((chunk.len() as u32).to_be_bytes());
            out.extend((body.len() as u32).to_be_bytes());
            out.extend(md5::compute(chunk).0);
        }
        let key = md5::compute(&out).0;
        for chunk in &stored {
            out.extend(chunk);
        }
        (key, out)
    }

    /// A store holding real-shaped files, keyed the way a download keys them.
    fn deep_store(files: &[([u8; 16], Vec<u8>)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let mut writer = StoreWriter::create(dir.path().join("Data").join("data")).unwrap();
        for (key, bytes) in files {
            writer.write(key, bytes).unwrap();
        }
        writer.finish().unwrap();
        dir
    }

    /// Flip one byte of the only file's stored bytes, `at` past its header.
    fn flip(dir: &tempfile::TempDir, at: usize) {
        let entry = *open(dir).files().next().unwrap();
        let archive = dir.path().join("Data").join("data").join(archive_name(entry.archive_id));
        let mut bytes = std::fs::read(&archive).unwrap();
        bytes[entry.offset as usize + CASC_ENTRY_HEADER + at] ^= 0x01;
        std::fs::write(&archive, &bytes).unwrap();
    }

    fn deep(dir: &tempfile::TempDir) -> Result<Report, VerifyError> {
        deep_scan(dir.path(), &Cancel::new(), |_| {})
    }

    #[test]
    fn a_deep_scan_verifies_every_byte_of_a_healthy_store() {
        let dir = deep_store(&[blte(&[b"first chunk", b"second"]), blte(&[b"alone"])]);
        let report = deep(&dir).unwrap();

        assert!(report.is_healthy(), "{:?}", report.damaged);
        assert_eq!((report.checked, report.unverifiable), (2, 0));
        assert_eq!(report.bytes_read, open(&dir).stats().bytes);
    }

    #[test]
    fn rot_inside_a_chunk_is_caught_though_the_header_is_intact() {
        // The case the header scan cannot see: the 30 bytes in front are
        // untouched, so only hashing the payload finds it.
        let (key, bytes) = blte(&[b"first chunk", b"second"]);
        let dir = deep_store(&[(key, bytes.clone())]);
        flip(&dir, bytes.len() - 2);

        assert!(scan(&open(&dir), |_| {}).is_healthy());
        let report = deep(&dir).unwrap();
        assert_eq!(report.damaged.len(), 1);
        assert_eq!(report.damaged[0].fault, Fault::BadChunk);
    }

    #[test]
    fn a_damaged_chunk_table_is_caught_against_the_key() {
        // Flipping a checksum in the table would otherwise make a healthy
        // chunk look rotten, or, flipped to match rot, a rotten one healthy.
        let dir = deep_store(&[blte(&[b"payload"])]);
        flip(&dir, 12 + 8);
        assert_eq!(deep(&dir).unwrap().damaged[0].fault, Fault::WrongPayload);
    }

    #[test]
    fn a_raw_patch_manifest_is_unverifiable_not_damaged() {
        // Agent keeps these; the downloader never fetches them, so calling
        // them damaged would make a repair delete them for good.
        let mut key = [0u8; 16];
        key[0] = 0x6f;
        let dir = deep_store(&[(key, b"PA\x02\x10\x10\x10\x10\x00\x01".to_vec())]);
        let report = deep(&dir).unwrap();
        assert!(report.is_healthy(), "{:?}", report.damaged);
        assert_eq!(report.unverifiable, 1);
    }

    #[test]
    fn anything_else_that_is_not_blte_is_damage() {
        let dir = deep_store(&[([7u8; 16], b"not a file at all".to_vec())]);
        assert_eq!(deep(&dir).unwrap().damaged[0].fault, Fault::NotBlte);
    }

    #[test]
    fn a_cancelled_deep_scan_reports_nothing() {
        let dir = deep_store(&[blte(&[b"payload"])]);
        let cancel = Cancel::new();
        cancel.cancel();
        let result = deep_scan(dir.path(), &cancel, |_| {});
        assert!(matches!(result, Err(VerifyError::Cancelled)), "{result:?}");
    }

    #[test]
    fn repair_refuses_a_store_that_changed_since_the_scan() {
        let (key, bytes) = blte(&[b"payload"]);
        let dir = deep_store(&[(key, bytes.clone())]);
        flip(&dir, bytes.len() - 1);
        let report = deep(&dir).unwrap();
        assert!(!report.is_healthy());

        // Another writer lands a file after the scan: the findings are
        // positions, and positions in a store that moved may be healthy now.
        let data = dir.path().join("Data").join("data");
        let mut writer = StoreWriter::create(&data).unwrap();
        writer.write(&[9u8; 16], &blte(&[b"new"]).1).unwrap();
        writer.finish().unwrap();

        let result = repair_unchanged(&data, &report);
        assert!(matches!(result, Err(VerifyError::StoreChanged)), "{result:?}");
        assert_eq!(open(&dir).files().count(), 2, "nothing was dropped");
    }

    #[test]
    fn repair_goes_ahead_on_the_store_it_scanned() {
        let (key, bytes) = blte(&[b"payload"]);
        let dir = deep_store(&[(key, bytes.clone())]);
        flip(&dir, bytes.len() - 1);
        let report = deep(&dir).unwrap();

        let data = dir.path().join("Data").join("data");
        assert_eq!(repair_unchanged(&data, &report).unwrap(), 1);
        assert_eq!(open(&dir).files().count(), 0);
    }

    #[test]
    fn deep_progress_counts_bytes_as_well_as_files() {
        let dir = deep_store(&[blte(&[b"a"]), blte(&[b"b"]), blte(&[b"c"])]);
        let mut last = Progress::default();
        deep_scan(dir.path(), &Cancel::new(), |p| last = p).unwrap();
        assert_eq!((last.checked, last.total), (3, 3));
        assert_eq!(last.bytes, last.bytes_total);
        assert!(last.bytes_total > 0);
    }
}
