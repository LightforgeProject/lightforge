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
//! intact reads as healthy here. Catching that means decoding every file and
//! hashing it against the encoding manifest — a different order of cost, and a
//! different job.
//!
//! Repair is deliberately not a separate mechanism: a damaged entry is removed
//! from the index, which makes the planner see the file as absent, which makes
//! the ordinary download path fetch it again.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use super::entry::Header;
use super::idx::{Entry, Index, CASC_ENTRY_HEADER};
use super::storage::{archive_name, Storage};

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
}

impl Report {
    pub fn is_healthy(&self) -> bool {
        self.damaged.is_empty()
    }
}

/// Verify every file entry in `store`.
///
/// Read-only. `on_progress` is called periodically rather than per entry —
/// there are over a million of them, and a callback per entry costs more than
/// the check does.
pub fn scan(store: &Storage, mut on_progress: impl FnMut(Progress)) -> Report {
    let data_dir = store.data_dir().to_path_buf();
    let mut report = Report::default();

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
        if report.checked % 8192 == 0 {
            on_progress(Progress {
                checked: report.checked,
                total,
                damaged: report.damaged.len() as u64,
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
    });
    report
}

/// Check one header against the index entry that pointed at it.
fn fault_in(header: &[u8], entry: &Entry) -> Option<Fault> {
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

/// Remove damaged entries from the store's indices.
///
/// This *is* the repair: the planner asks the index what the store holds, so
/// an entry that is gone becomes a file the next download fetches again. The
/// archives are not touched — the bad bytes stay where they are and are
/// written over or orphaned, which is exactly what the writer already does
/// with superseded content.
///
/// Returns how many entries were removed. Never call it against a live client;
/// it rewrites the files the client memory-maps, which is what pre-flight
/// exists to prevent.
pub fn repair(data_dir: &Path, damaged: &[Damage]) -> Result<usize, VerifyError> {
    let mut by_bucket: BTreeMap<u8, Vec<Entry>> = BTreeMap::new();
    for damage in damaged {
        by_bucket.entry(damage.entry.bucket()).or_default().push(damage.entry);
    }

    let mut removed = 0;
    for (bucket, entries) in by_bucket {
        let Some(sequence) = super::idx::latest_sequence(data_dir, bucket)? else {
            continue;
        };
        let path = data_dir.join(super::idx::file_name(bucket, sequence));
        let mut index = Index::load(&path)?;
        for entry in entries {
            if index.remove(&entry.key).is_some() {
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
        // byte 15 is the key's first byte — flipping it changes which file
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
}
