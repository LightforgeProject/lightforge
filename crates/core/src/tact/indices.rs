//! The archive index cache: where an encoding key actually lives.
//!
//! An encoding key's address is in the index beside its archive: for 12.1.0,
//! **1406 indices, 119 MiB**, plus the file index. The combined
//! `archive-group` answers **403**; Agent builds it locally.
//!
//! Cached in `Data/indices/<hash>.index`, where Agent keeps them, so Agent
//! reuses them.
//!
//! No usable declared size ([`super::config::CdnConfig::archives_index_size`]),
//! so each index is checked by its geometry at parse time. The file index
//! has a single `file-index-size`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use futures_util::stream::{self, StreamExt};

use super::archive::{ArchiveError, ArchiveIndex};
use super::cdn::{CdnError, Client};
use crate::net::Http;

/// A directory of `.index` files, fetched on demand and kept.
#[derive(Debug, Clone)]
pub struct IndexCache {
    dir: PathBuf,
}

/// What happened to one index during [`IndexCache::ensure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fetched {
    /// Already on disk at the right size; nothing was transferred.
    Cached,
    /// Fetched and written.
    Downloaded { bytes: u64 },
    /// Every endpoint failed; the run continues without that archive's files.
    Failed { detail: String },
}

/// One index's outcome, as reported to a progress callback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Step {
    pub hash: String,
    pub outcome: Fetched,
    /// 1-based position in the run, for a "347 of 1406" style display.
    pub done: usize,
    pub total: usize,
}

/// The result of a whole prelude.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    pub cached: usize,
    pub downloaded: usize,
    pub bytes: u64,
    /// Hash and reason for each index that could not be fetched.
    pub failed: Vec<(String, String)>,
}

impl Report {
    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

impl IndexCache {
    /// Open the cache in `dir`, e.g. `<install>/Data/indices`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path_of(&self, hash: &str) -> PathBuf {
        self.dir.join(format!("{hash}.index"))
    }

    /// Whether `hash` is cached at its declared size, or at all without one.
    pub fn is_cached(&self, hash: &str, expect_size: Option<u64>) -> bool {
        match std::fs::metadata(self.path_of(hash)) {
            Ok(meta) => expect_size.is_none_or(|want| meta.len() == want),
            Err(_) => false,
        }
    }

    /// Whether a copy of `hash` is cached and every checksum in it holds:
    /// a damaged copy of the right size is fetched again.
    pub fn holds(&self, hash: &str, expect_size: Option<u64>) -> bool {
        self.is_cached(hash, expect_size)
            && std::fs::read(self.path_of(hash)).is_ok_and(|bytes| super::archive::verify(&bytes, hash).is_ok())
    }

    pub fn load_archive(&self, hash: &str) -> Result<ArchiveIndex, IndexError> {
        let path = self.path_of(hash);
        let bytes = std::fs::read(&path).map_err(io(&path))?;
        ArchiveIndex::parse(&bytes, hash).map_err(|source| IndexError::Parse { path, source })
    }

    /// Fetch every index in `wanted` (from
    /// [`super::config::CdnConfig::archive_indices`]) not already cached.
    /// `progress` is called per index. Failures are listed, not fatal.
    pub async fn ensure(
        &self,
        http: &Http,
        client: &Client,
        wanted: &[(&str, Option<u64>)],
        concurrency: usize,
        mut progress: impl FnMut(Step),
    ) -> Result<Report, IndexError> {
        std::fs::create_dir_all(&self.dir).map_err(io(&self.dir))?;

        let total = wanted.len();
        let mut report = Report::default();
        let mut done = 0;

        let mut stream = stream::iter(wanted.iter().copied())
            .map(|(hash, size)| async move {
                if self.holds(hash, size) {
                    return (hash, Fetched::Cached);
                }
                match client.fetch_index(http, hash, size).await {
                    Ok(bytes) => match self.store(hash, &bytes) {
                        Ok(()) => (hash, Fetched::Downloaded { bytes: bytes.len() as u64 }),
                        Err(e) => (hash, Fetched::Failed { detail: e.to_string() }),
                    },
                    Err(e) => (hash, Fetched::Failed { detail: e.to_string() }),
                }
            })
            .buffer_unordered(concurrency.max(1));

        while let Some((hash, outcome)) = stream.next().await {
            done += 1;
            match &outcome {
                Fetched::Cached => report.cached += 1,
                Fetched::Downloaded { bytes } => {
                    report.downloaded += 1;
                    report.bytes += bytes;
                }
                Fetched::Failed { detail } => report.failed.push((hash.to_owned(), detail.clone())),
            }
            progress(Step { hash: hash.to_owned(), outcome, done, total });
        }

        Ok(report)
    }

    /// Write an index atomically: a torn one of the right size would pass checks.
    pub(crate) fn store(&self, hash: &str, bytes: &[u8]) -> Result<(), IndexError> {
        let path = self.path_of(hash);
        // 0644, as Agent's own cached indices are.
        crate::fsutil::write_atomic_with_mode(&path, bytes, 0o644).map_err(io(&path))
    }
}

/// Where each wanted encoding key lives, built by streaming each archive
/// index past the wanted set.
#[derive(Debug, Clone, Default)]
pub struct Locations {
    /// ekey -> (archive index into `archives`, offset, size)
    entries: HashMap<super::storage::EKey, (u32, u32, u32)>,
    archives: Vec<String>,
}

impl Locations {
    /// Fold one archive's index in, keeping only keys `wanted` contains.
    ///
    /// Returns how many were kept. A key in two archives keeps the first.
    pub fn absorb(
        &mut self,
        index: &ArchiveIndex,
        wanted: &std::collections::HashSet<super::storage::EKey>,
    ) -> usize {
        let archive = match self.archives.iter().position(|a| a == index.archive()) {
            Some(i) => i as u32,
            None => {
                self.archives.push(index.archive().to_owned());
                (self.archives.len() - 1) as u32
            }
        };

        let mut kept = 0;
        for entry in index.entries() {
            if wanted.contains(&entry.ekey) {
                self.entries
                    .entry(entry.ekey)
                    .or_insert_with(|| {
                        kept += 1;
                        (archive, entry.offset, entry.size)
                    });
            }
        }
        kept
    }

    /// Where a key lives: its archive hash and the bytes to request.
    pub fn get(&self, ekey: &super::storage::EKey) -> Option<(&str, super::archive::Located)> {
        let (archive, offset, size) = *self.entries.get(ekey)?;
        Some((
            self.archives[archive as usize].as_str(),
            super::archive::Located { ekey: *ekey, size, offset },
        ))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn archives(&self) -> &[String] {
        &self.archives
    }

    /// Every located key, as `(archive hash, location)`.
    pub fn iter(&self) -> impl Iterator<Item = (&str, super::archive::Located)> + '_ {
        self.entries.iter().map(|(ekey, (archive, offset, size))| {
            (
                self.archives[*archive as usize].as_str(),
                super::archive::Located { ekey: *ekey, size: *size, offset: *offset },
            )
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("{path} is not a usable index")]
    Parse {
        path: PathBuf,
        #[source]
        source: ArchiveError,
    },

    #[error(transparent)]
    Cdn(#[from] CdnError),
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> IndexError + '_ {
    move |source| IndexError::Io { path: path.to_path_buf(), source }
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-indices-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn key(n: u8) -> super::super::storage::EKey {
        let mut k = [0u8; 16];
        k[0] = n;
        k
    }

    /// An archive index with the real container shape around the entries.
    fn index_bytes(entries: &[(super::super::storage::EKey, u32, u32)]) -> Vec<u8> {
        let (block_size, entry_size) = (4096usize, 24usize);
        let per_block = block_size / entry_size;
        let blocks = entries.len().div_ceil(per_block).max(1);

        let mut out = vec![0u8; blocks * block_size];
        for (i, (ekey, size, offset)) in entries.iter().enumerate() {
            let at = (i / per_block) * block_size + (i % per_block) * entry_size;
            out[at..at + 16].copy_from_slice(ekey);
            out[at + 16..at + 20].copy_from_slice(&size.to_be_bytes());
            out[at + 20..at + 24].copy_from_slice(&offset.to_be_bytes());
        }
        for b in 0..blocks {
            let last = ((b + 1) * per_block - 1).min(entries.len().saturating_sub(1));
            out.extend_from_slice(&entries.get(last).map_or([0u8; 16], |e| e.0));
        }
        out.extend(std::iter::repeat_n(0u8, blocks * 8));

        let mut footer = vec![0u8; 8];
        footer.extend_from_slice(&[1, 0, 0, (block_size / 1024) as u8, 4, 4, 16, 8]);
        footer.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        footer.extend_from_slice(&[0u8; 8]);
        out.extend_from_slice(&footer);
        super::super::archive::seal(&mut out);
        out
    }

    #[test]
    fn a_cached_index_is_recognised_only_at_the_declared_size() {
        let dir = scratch("cached");
        let cache = IndexCache::new(&dir);
        std::fs::write(cache.path_of("abcd"), b"0123456789").unwrap();

        assert!(cache.is_cached("abcd", Some(10)));
        assert!(cache.is_cached("abcd", None));
        // A truncated mirror is exactly what the declared size is there to catch.
        assert!(!cache.is_cached("abcd", Some(11)));
        assert!(!cache.is_cached("missing", None));
    }

    #[test]
    fn a_damaged_cached_index_of_the_right_size_is_not_held() {
        let dir = scratch("damaged");
        let cache = IndexCache::new(&dir);
        let mut bytes = index_bytes(&[(key(1), 100, 30)]);
        let name = super::super::archive::seal(&mut bytes);
        cache.store(&name, &bytes).unwrap();
        assert!(cache.holds(&name, Some(bytes.len() as u64)));

        bytes[20] ^= 0xff;
        cache.store(&name, &bytes).unwrap();
        assert!(cache.is_cached(&name, Some(bytes.len() as u64)), "the size alone still matches");
        assert!(!cache.holds(&name, Some(bytes.len() as u64)));
    }

    #[test]
    fn an_index_round_trips_through_the_cache_under_agents_own_name() {
        let dir = scratch("roundtrip");
        let cache = IndexCache::new(&dir);
        let bytes = index_bytes(&[(key(1), 100, 30), (key(2), 200, 130)]);
        cache.store("deadbeef", &bytes).unwrap();

        assert_eq!(cache.path_of("deadbeef"), dir.join("deadbeef.index"));
        let index = cache.load_archive("deadbeef").unwrap();
        assert_eq!(index.archive(), "deadbeef");
        assert_eq!(index.len(), 2);
        assert_eq!(index.lookup(&key(2)).unwrap().offset, 130);

        // Nothing but the index is left behind.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1);
    }

    #[test]
    fn a_corrupt_index_is_an_error_naming_the_file() {
        let dir = scratch("corrupt");
        let cache = IndexCache::new(&dir);
        std::fs::write(cache.path_of("bad"), vec![0u8; 4096]).unwrap();

        let err = cache.load_archive("bad").unwrap_err();
        assert!(matches!(err, IndexError::Parse { .. }));
        assert!(err.to_string().contains("bad.index"));
    }

    #[test]
    fn locations_keep_only_wanted_keys() {
        let index = ArchiveIndex::parse(
            &index_bytes(&[(key(1), 10, 0), (key(2), 20, 10), (key(3), 30, 30)]),
            "archive-a",
        )
        .unwrap();
        let wanted: HashSet<_> = [key(1), key(3)].into_iter().collect();

        let mut locations = Locations::default();
        assert_eq!(locations.absorb(&index, &wanted), 2);
        assert_eq!(locations.len(), 2);

        let (archive, located) = locations.get(&key(3)).unwrap();
        assert_eq!(archive, "archive-a");
        assert_eq!((located.offset, located.size), (30, 30));
        assert!(locations.get(&key(2)).is_none());
    }

    #[test]
    fn a_key_in_two_archives_keeps_the_first() {
        let first = ArchiveIndex::parse(&index_bytes(&[(key(1), 10, 0)]), "archive-a").unwrap();
        let second = ArchiveIndex::parse(&index_bytes(&[(key(1), 99, 500)]), "archive-b").unwrap();
        let wanted: HashSet<_> = [key(1)].into_iter().collect();

        let mut locations = Locations::default();
        locations.absorb(&first, &wanted);
        assert_eq!(locations.absorb(&second, &wanted), 0);

        let (archive, located) = locations.get(&key(1)).unwrap();
        assert_eq!(archive, "archive-a");
        assert_eq!(located.size, 10);
        assert_eq!(locations.archives().len(), 2);
    }

    #[test]
    fn a_report_is_incomplete_exactly_when_something_failed() {
        let mut report = Report { cached: 3, downloaded: 2, bytes: 100, failed: Vec::new() };
        assert!(report.is_complete());
        report.failed.push(("abc".into(), "connection reset".into()));
        assert!(!report.is_complete());
    }
}
