//! The local CASC store: bucket indices plus the `data.NNN` archives they
//! point into.
//!
//! This is the read side of [`crate::tact`]. `casc-storage` is deliberately
//! not used for any of it. Its `read` returns an empty `Vec` for every entry
//! in a real install — the BLTE *streaming* reader it calls yields nothing
//! where the one-shot decoder returns correct data — and its index support is
//! read-only anyway, so [`super::idx`] already replaces the other half. What
//! is left after removing both is small enough to own outright: find the
//! newest index per bucket, seek into an archive, decode one BLTE payload.
//!
//! Layout underneath a WoW install:
//!
//! ```text
//! Data/
//! ├── config/          build and CDN configs, plain text
//! └── data/
//!     ├── <bucket><seq>.idx    16 buckets, newest generation wins
//!     └── data.NNN             archives, each with 480 reserved head bytes
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use ngdp_crypto::KeyService;

use super::idx::{self, Entry, Index, IdxError, CASC_ENTRY_HEADER};

/// Full 16-byte encoding key. Indices store only the first 9 bytes.
pub type EKey = [u8; 16];

/// A local CASC store opened for reading.
///
/// Archive handles are cached as they are used, which is why reads take
/// `&mut self`: a bulk verify touches the same handful of archives thousands
/// of times, and reopening each file per entry dominates the cost.
// `KeyService` is not `Debug`, so the derive is hand-written to keep `Storage`
// printable in error paths without dumping decryption keys into a log either.
pub struct Storage {
    data_dir: PathBuf,
    /// Newest generation per bucket, keyed by bucket number.
    indices: HashMap<u8, Index>,
    archives: HashMap<u16, File>,
    keys: KeyService,
}

impl std::fmt::Debug for Storage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Storage")
            .field("data_dir", &self.data_dir)
            .field("buckets", &self.indices.len())
            .field("open_archives", &self.archives.len())
            .finish_non_exhaustive()
    }
}

impl Storage {
    /// Open the store under a WoW install root, or under a `Data` directory,
    /// or a `Data/data` directory directly.
    ///
    /// All three spellings are accepted because callers reasonably hold any of
    /// them: the layout module hands out install roots, while a debugging
    /// session usually has the data directory itself.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StorageError> {
        let data_dir = resolve_data_dir(path.as_ref());
        if !data_dir.is_dir() {
            return Err(StorageError::NotACascStore(data_dir));
        }

        let mut indices = HashMap::new();
        for file in idx::discover(&data_dir)? {
            let index = Index::load(&file)?;
            indices.insert(index.bucket(), index);
        }
        if indices.is_empty() {
            return Err(StorageError::NoIndices(data_dir));
        }

        // Encrypted blocks are normal — Blizzard ships unreleased content in
        // the same archives — so a missing key file is not an error here. The
        // read that needs a key is where it becomes one.
        let mut keys = KeyService::new();
        let _ = keys.load_from_standard_dirs();

        Ok(Self { data_dir, indices, archives: HashMap::new(), keys })
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// The bucket indices, ordered by bucket number.
    pub fn indices(&self) -> Vec<&Index> {
        let mut out: Vec<_> = self.indices.values().collect();
        out.sort_by_key(|i| i.bucket());
        out
    }

    /// Entries naming real files, across every bucket. Archive markers are
    /// excluded — see [`Entry::is_archive_marker`].
    pub fn files(&self) -> impl Iterator<Item = &Entry> {
        self.indices().into_iter().flat_map(|i| i.files())
    }

    /// Locate an encoding key without reading its bytes.
    pub fn locate(&self, ekey: &EKey) -> Option<&Entry> {
        let key: [u8; 9] = ekey[..9].try_into().expect("9 of 16 bytes");
        // The bucket is derived from the key, so only one index is consulted.
        let entry = Entry { key, archive_id: 0, offset: 0, size: 0 };
        self.indices.get(&entry.bucket())?.lookup(&key)
    }

    pub fn contains(&self, ekey: &EKey) -> bool {
        self.locate(ekey).is_some()
    }

    /// Read and decode one file by encoding key.
    pub fn read(&mut self, ekey: &EKey) -> Result<Vec<u8>, StorageError> {
        let entry = *self.locate(ekey).ok_or(StorageError::NotFound(*ekey))?;
        self.read_entry(&entry)
    }

    /// Read and decode the file an index entry points at.
    pub fn read_entry(&mut self, entry: &Entry) -> Result<Vec<u8>, StorageError> {
        if entry.is_archive_marker() {
            return Err(StorageError::IsArchiveMarker { archive_id: entry.archive_id });
        }

        let raw = self.read_raw(entry)?;
        let payload = blte_payload(&raw).ok_or(StorageError::NotBlte {
            archive_id: entry.archive_id,
            offset: entry.offset,
        })?;

        // The one-shot decoder, never the streaming one: the streaming reader
        // in blte 0.4 returns zero bytes for payloads this decodes correctly.
        blte::decompress_blte(payload.to_vec(), Some(&self.keys)).map_err(|source| {
            StorageError::Decode {
                archive_id: entry.archive_id,
                offset: entry.offset,
                source: Box::new(source),
            }
        })
    }

    /// The raw archive bytes for an entry, header and all, undecoded.
    pub fn read_raw(&mut self, entry: &Entry) -> Result<Vec<u8>, StorageError> {
        let data_dir = self.data_dir.clone();
        let file = match self.archives.entry(entry.archive_id) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let path = data_dir.join(archive_name(entry.archive_id));
                let handle = File::open(&path).map_err(|source| StorageError::Io { path, source })?;
                e.insert(handle)
            }
        };

        let path = || data_dir.join(archive_name(entry.archive_id));
        file.seek(SeekFrom::Start(u64::from(entry.offset)))
            .map_err(|source| StorageError::Io { path: path(), source })?;
        let mut raw = vec![0u8; entry.size as usize];
        file.read_exact(&mut raw)
            .map_err(|source| StorageError::Io { path: path(), source })?;
        Ok(raw)
    }

    /// Counts for the UI and `lightforge casc`.
    pub fn stats(&self) -> Stats {
        let indices = self.indices();
        Stats {
            buckets: indices.len(),
            files: indices.iter().map(|i| i.files().count()).sum(),
            entries: indices.iter().map(|i| i.entries().len()).sum(),
            bytes: indices
                .iter()
                .flat_map(|i| i.files())
                .map(|e| u64::from(e.size))
                .sum(),
        }
    }
}

/// Summary of what a store holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stats {
    pub buckets: usize,
    /// Entries naming real files.
    pub files: usize,
    /// Every index entry, archive markers included.
    pub entries: usize,
    /// Archived bytes, counting each file's CASC header.
    pub bytes: u64,
}

/// The BLTE payload inside an archive slice, with or without the CASC entry
/// header in front of it.
///
/// Almost every entry points at a 30-byte CASC header followed by the payload.
/// A few point straight at the payload instead: a real install has 57 pairs of
/// entries that share a key, where the second of the pair has both its offset
/// advanced by 30 and its size reduced by 30 — the same file, described
/// without its header. Assuming the header is always present makes those
/// aliases look like a corrupt archive.
pub(crate) fn blte_payload(raw: &[u8]) -> Option<&[u8]> {
    const MAGIC: &[u8; 4] = b"BLTE";
    if raw.len() >= CASC_ENTRY_HEADER + 4 && &raw[CASC_ENTRY_HEADER..CASC_ENTRY_HEADER + 4] == MAGIC {
        return Some(&raw[CASC_ENTRY_HEADER..]);
    }
    if raw.len() >= 4 && &raw[..4] == MAGIC {
        return Some(raw);
    }
    None
}

/// `data.027` — always three digits, matching what Blizzard writes.
pub fn archive_name(archive_id: u16) -> String {
    format!("data.{archive_id:03}")
}

/// Accept an install root, a `Data` directory, or `Data/data` itself.
pub(crate) fn resolve_data_dir(path: &Path) -> PathBuf {
    for suffix in [["Data", "data"].as_slice(), ["data"].as_slice(), [].as_slice()] {
        let candidate = suffix.iter().fold(path.to_path_buf(), |p, s| p.join(s));
        if candidate.is_dir() && has_index(&candidate) {
            return candidate;
        }
    }
    path.join("Data").join("data")
}

fn has_index(dir: &Path) -> bool {
    std::fs::read_dir(dir).is_ok_and(|mut entries| {
        entries.any(|e| {
            e.is_ok_and(|e| idx::parse_file_name(&e.file_name().to_string_lossy()).is_some())
        })
    })
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("{0} is not a CASC store")]
    NotACascStore(PathBuf),

    #[error("no .idx files under {0} — the install is incomplete")]
    NoIndices(PathBuf),

    #[error("no entry for encoding key {}", hex(.0))]
    NotFound(EKey),

    #[error("that entry is archive {archive_id}'s marker, not a file")]
    IsArchiveMarker { archive_id: u16 },

    #[error("data.{archive_id:03} has no BLTE payload at offset {offset}")]
    NotBlte { archive_id: u16, offset: u32 },

    #[error("could not decode the file in data.{archive_id:03} at offset {offset}")]
    Decode {
        archive_id: u16,
        offset: u32,
        #[source]
        source: Box<blte::Error>,
    },

    #[error(transparent)]
    Idx(#[from] IdxError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn hex(key: &EKey) -> String {
    key.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-storage-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn opening_something_that_is_not_a_store_says_so() {
        let dir = scratch("empty");
        assert!(matches!(
            Storage::open(&dir),
            Err(StorageError::NotACascStore(_) | StorageError::NoIndices(_))
        ));
    }

    #[test]
    fn an_install_root_and_its_data_dir_resolve_to_the_same_place() {
        let root = scratch("resolve");
        let data = root.join("Data").join("data");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::write(data.join(idx::file_name(0, 0)), b"not a real index").unwrap();

        assert_eq!(resolve_data_dir(&root), data);
        assert_eq!(resolve_data_dir(&root.join("Data")), data);
        assert_eq!(resolve_data_dir(&data), data);
    }

    #[test]
    fn a_store_round_trips_an_entry_through_a_real_archive_layout() {
        let dir = scratch("roundtrip");
        let data = dir.join("Data").join("data");
        std::fs::create_dir_all(&data).unwrap();

        // Build an archive the way Blizzard does: reserved head bytes, then a
        // CASC entry header, then a BLTE payload.
        let payload = b"lightforge storage round trip".repeat(40);
        let blte = blte::compress_data_single(payload.clone(), blte::CompressionMode::ZLib, None).unwrap();
        let mut archive = vec![0u8; idx::ARCHIVE_RESERVED];
        let offset = archive.len() as u32;
        archive.extend_from_slice(&[0u8; CASC_ENTRY_HEADER]);
        archive.extend_from_slice(&blte);
        std::fs::write(data.join(archive_name(3)), &archive).unwrap();

        let ekey: EKey = [
            0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff, 0x01, 0x02, 0x03, 0, 0, 0, 0, 0, 0, 0,
        ];
        let key: [u8; 9] = ekey[..9].try_into().unwrap();
        let entry = Entry {
            key,
            archive_id: 3,
            offset,
            size: (CASC_ENTRY_HEADER + blte.len()) as u32,
        };

        let mut index = Index::new(entry.bucket());
        index.insert(entry);
        index.save_new_generation(&data).unwrap();

        let mut storage = Storage::open(&dir).unwrap();
        assert!(storage.contains(&ekey));
        assert_eq!(storage.locate(&ekey).unwrap().archive_id, 3);
        assert_eq!(storage.read(&ekey).unwrap(), payload);

        let stats = storage.stats();
        assert_eq!((stats.buckets, stats.files), (1, 1));
    }

    #[test]
    fn a_missing_key_and_a_marker_are_distinct_errors() {
        let dir = scratch("errors");
        let data = dir.join("Data").join("data");
        std::fs::create_dir_all(&data).unwrap();

        let marker = Entry::archive_marker(7, 0);
        let mut index = Index::new(marker.bucket());
        index.insert(marker);
        index.save_new_generation(&data).unwrap();

        let mut storage = Storage::open(&dir).unwrap();
        assert!(matches!(
            storage.read(&[0x42; 16]),
            Err(StorageError::NotFound(_))
        ));
        // Reading a marker is a caller mistake, not a corrupt install, and the
        // message has to say which — this is the case that made a naive
        // "decode every entry" sweep look like 1968 failures.
        assert!(matches!(
            storage.read_entry(&marker),
            Err(StorageError::IsArchiveMarker { archive_id: 7 })
        ));
    }

    #[test]
    fn a_payload_is_found_with_or_without_the_casc_header() {
        let mut headed = vec![0u8; CASC_ENTRY_HEADER];
        headed.extend_from_slice(b"BLTEpayload");
        assert_eq!(blte_payload(&headed).unwrap(), b"BLTEpayload");

        // The headerless alias form: 57 of these exist in a real install,
        // each sharing a key with a normal entry 30 bytes earlier.
        assert_eq!(blte_payload(b"BLTEpayload").unwrap(), b"BLTEpayload");

        assert!(blte_payload(b"not blte at all").is_none());
        assert!(blte_payload(b"").is_none());
    }

    #[test]
    fn archive_names_are_three_digits() {
        assert_eq!(archive_name(0), "data.000");
        assert_eq!(archive_name(27), "data.027");
        assert_eq!(archive_name(123), "data.123");
    }
}
