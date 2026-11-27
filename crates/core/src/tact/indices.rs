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

    /// A cached copy of `hash`, read once: at its declared size, named by its
    /// footer, and parsed by `parse`, which walks every checksum. `None` for
    /// one missing or damaged: fetch it again. Checked and then loaded, it was
    /// a second 119 MiB read and hashing pass on every prepare of retail.
    pub fn cached<T>(
        &self,
        hash: &str,
        expect_size: Option<u64>,
        parse: impl FnOnce(&[u8]) -> Result<T, super::archive::ArchiveError>,
    ) -> Option<T> {
        if !self.is_cached(hash, expect_size) {
            return None;
        }
        let bytes = std::fs::read(self.path_of(hash)).ok()?;
        super::archive::check_name(&bytes, hash).ok()?;
        parse(&bytes).ok()
    }

    /// Every index in `wanted` (from
    /// [`super::config::CdnConfig::archive_indices`]), from the cache or
    /// fetched, handed to `absorb` in that order, so a key two archives hold
    /// is always located in the same one. `progress` is called per index.
    /// Failures are listed, not fatal.
    pub async fn ensure(
        &self,
        http: &Http,
        client: &Client,
        wanted: &[(&str, Option<u64>)],
        concurrency: usize,
        mut progress: impl FnMut(Step),
        mut absorb: impl FnMut(ArchiveIndex),
    ) -> Result<Report, IndexError> {
        std::fs::create_dir_all(&self.dir).map_err(io(&self.dir))?;

        let total = wanted.len();
        let mut report = Report::default();
        let mut done = 0;

        let mut stream = stream::iter(wanted.iter().copied())
            .map(|(hash, size)| async move {
                if let Some(index) = self.cached(hash, size, |bytes| ArchiveIndex::parse(bytes, hash)) {
                    return (hash, Fetched::Cached, Some(index));
                }
                let failed = |detail: String| (hash, Fetched::Failed { detail }, None);
                let bytes = match client.fetch_index(http, hash, size).await {
                    Ok(bytes) => bytes,
                    Err(e) => return failed(e.to_string()),
                };
                if let Err(e) = self.store(hash, &bytes) {
                    return failed(e.to_string());
                }
                match ArchiveIndex::parse(&bytes, hash) {
                    Ok(index) => (hash, Fetched::Downloaded { bytes: bytes.len() as u64 }, Some(index)),
                    Err(e) => failed(e.to_string()),
                }
            })
            .buffered(concurrency.max(1));

        while let Some((hash, outcome, index)) = stream.next().await {
            if let Some(index) = index {
                absorb(index);
            }
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
        #[allow(clippy::cast_possible_truncation, reason = "archive ids number in the thousands")]
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

/// CDN config keys whose values name a file in `Data/indices`.
const INDEX_KEYS: [&str; 6] =
    ["archives", "patch-archives", "file-index", "patch-file-index", "archive-group", "patch-archive-group"];

/// Delete the `.index` files in `install`'s cache that no product in its
/// `.build.info` names, returning the bytes freed. Agent and Lightforge only
/// ever add to it: on a real retail store (2026-11-22) 21 of 1,504 files were
/// named by nothing and held 470 of its 630 MiB.
///
/// Touches nothing when what is needed is not known for certain: a product
/// whose CDN config is not in `Data/config`, a pre-downloaded build, or a
/// download not yet finished.
pub fn prune_unnamed(install: &Path) -> Result<u64, IndexError> {
    let Ok(info) = crate::buildinfo::BuildInfo::load(install.join(".build.info")) else {
        return Ok(0);
    };
    let ahead = super::staged::load(install).and_then(|staged| Ok((staged, super::staged::fetching(install)?)));
    if !ahead.is_ok_and(|(staged, fetching)| staged.is_empty() && fetching.is_empty()) {
        return Ok(0);
    }
    let mut named = std::collections::HashSet::new();
    for code in info.products() {
        let Ok(cdn_key) = info.get(&code, crate::buildinfo::COL_CDN_KEY) else {
            return Ok(0);
        };
        let Some(text) = super::config::config_path(&install.join("Data").join("config"), cdn_key)
            .ok()
            .and_then(|path| std::fs::read_to_string(path).ok())
        else {
            return Ok(0);
        };
        for line in text.lines() {
            let Some((key, values)) = line.split_once('=') else { continue };
            if INDEX_KEYS.contains(&key.trim()) {
                named.extend(values.split_whitespace().map(str::to_ascii_lowercase));
            }
        }
    }
    if named.is_empty() {
        return Ok(0);
    }

    let dir = install.join("Data").join("indices");
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(0),
        Err(e) => return Err(io(&dir)(e)),
    };
    let mut freed = 0;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_ascii_lowercase();
        let Some(hash) = name.strip_suffix(".index") else { continue };
        if named.contains(hash) {
            continue;
        }
        let path = entry.path();
        let len = entry.metadata().map_or(0, |m| m.len());
        std::fs::remove_file(&path).map_err(io(&path))?;
        freed += len;
    }
    Ok(freed)
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// An install whose one product's CDN config names `named`, with an
    /// index cache holding `cached`.
    fn install_naming(named: &str, cached: &[&str]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let cdn_key = "ab".repeat(16);
        let mut info = crate::buildinfo::BuildInfo::with_default_schema();
        info.set_product(&crate::product::ProductCode::ClassicEra, &[(crate::buildinfo::COL_CDN_KEY, &cdn_key)]).unwrap();
        info.save(dir.path().join(".build.info")).unwrap();
        super::super::config::store(&dir.path().join("Data"), &cdn_key, named.as_bytes()).unwrap();
        let indices = dir.path().join("Data/indices");
        std::fs::create_dir_all(&indices).unwrap();
        for hash in cached {
            std::fs::write(indices.join(format!("{hash}.index")), [0u8; 100]).unwrap();
        }
        dir
    }

    fn cached(dir: &tempfile::TempDir) -> HashSet<String> {
        std::fs::read_dir(dir.path().join("Data/indices"))
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn indices_no_build_names_are_pruned() {
        let dir = install_naming("# CDN Configuration\narchives = aa11 bb22\nfile-index = cc33\n", &["aa11", "bb22", "cc33", "dd44"]);
        assert_eq!(prune_unnamed(dir.path()).unwrap(), 100);
        assert_eq!(cached(&dir), HashSet::from(["aa11.index".into(), "bb22.index".into(), "cc33.index".into()]));
    }

    #[test]
    fn nothing_is_pruned_when_a_product_s_cdn_config_is_missing() {
        let dir = install_naming("archives = aa11\n", &["aa11", "dd44"]);
        let config = dir.path().join("Data/config");
        std::fs::remove_dir_all(&config).unwrap();
        assert_eq!(prune_unnamed(dir.path()).unwrap(), 0);
        assert_eq!(cached(&dir).len(), 2);
    }

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
    fn a_damaged_cached_index_of_the_right_size_is_fetched_again() {
        let dir = scratch("damaged");
        let cache = IndexCache::new(&dir);
        let mut bytes = index_bytes(&[(key(1), 100, 30)]);
        let name = super::super::archive::seal(&mut bytes);
        cache.store(&name, &bytes).unwrap();
        let size = Some(bytes.len() as u64);
        assert!(cache.cached(&name, size, |b| ArchiveIndex::parse(b, &name)).is_some());

        bytes[20] ^= 0xff;
        cache.store(&name, &bytes).unwrap();
        assert!(cache.is_cached(&name, size), "the size alone still matches");
        assert!(cache.cached(&name, size, |b| ArchiveIndex::parse(b, &name)).is_none());
    }

    #[test]
    fn an_index_round_trips_through_the_cache_under_agents_own_name() {
        let dir = scratch("roundtrip");
        let cache = IndexCache::new(&dir);
        let mut bytes = index_bytes(&[(key(1), 100, 30), (key(2), 200, 130)]);
        let name = super::super::archive::seal(&mut bytes);
        cache.store(&name, &bytes).unwrap();

        assert_eq!(cache.path_of(&name), dir.join(format!("{name}.index")));
        let index = cache.cached(&name, None, |b| ArchiveIndex::parse(b, &name)).unwrap();
        assert_eq!(index.archive(), name);
        assert_eq!(index.len(), 2);
        assert_eq!(index.lookup(&key(2)).unwrap().offset, 130);

        // Nothing but the index is left behind.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left.len(), 1);
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
