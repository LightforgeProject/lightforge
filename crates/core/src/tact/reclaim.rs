//! Giving back the shared store's space when one WoW version is uninstalled.
//!
//! Every version in one install writes into the same CASC store, so deleting
//! a version's folder frees only its executables, its game data stays in
//! `Data/`, interleaved with the others'. This finds the store entries only
//! the departing version uses, drops them from the indices, and deletes any
//! `data.NNN` archive that leaves with nothing live in it.
//!
//! Two rules keep it from ever touching another version's files:
//!
//! * The keep-set comes from the builds that are **installed**: each row's
//!   `Build Key` in `.build.info`: not from what the CDN serves today. A
//!   version a patch behind still needs files its newer build dropped.
//! * If any remaining version's manifest cannot be read, there is no plan.
//!   Nothing is reclaimed on a guess.
//!
//! Entries dropped from an archive that still holds other files become dead
//! space: invisible to every client, but still on disk until something
//! rewrites that archive.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use crate::buildinfo::{BuildInfo, BuildInfoError};
use crate::net::Http;
use crate::product::ProductCode;

use super::cdn::{CdnError, Client};
use super::config::BuildConfig;
use super::idx::Entry;
use super::manifest::DownloadManifest;
use super::storage::{archive_name, Storage, StorageError};
use super::verify::{self, Snapshot, VerifyError};
use super::version::Cdn;

#[derive(Debug, thiserror::Error)]
pub enum ReclaimError {
    #[error(transparent)]
    BuildInfo(#[from] BuildInfoError),
    #[error("reading {product}'s installed build: {source}")]
    Cdn { product: String, source: CdnError },
    #[error("{product}'s installed build: {reason}")]
    Manifest { product: String, reason: String },
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Verify(#[from] VerifyError),
    #[error("cannot remove {path}: {source}")]
    Io { path: PathBuf, source: std::io::Error },
}

/// What reclaiming one version's data would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reclaim {
    /// Store entries only the departing version uses.
    pub entries: Vec<Entry>,
    /// Archives that would hold nothing live afterwards, and so are deleted.
    pub archives: Vec<u16>,
    /// Bytes of disk those archives free.
    pub freed: u64,
    /// Bytes of dropped entries left behind in archives that are kept.
    pub stranded: u64,
    /// The store as planned against; execution refuses if it has moved.
    pub snapshot: Snapshot,
}

/// One build, and where its CDN serves it from.
#[derive(Debug, Clone)]
pub struct Build {
    pub product: ProductCode,
    /// The build config's hash, `.build.info`'s `Build Key`.
    pub build_key: String,
    pub cdn: Cdn,
}

impl Build {
    /// The build `.build.info` says is installed for `product`.
    pub fn installed(info: &BuildInfo, product: &ProductCode) -> Result<Self, ReclaimError> {
        let words = |column: &str| -> Vec<String> {
            info.get(product, column)
                .map(|v| v.split_whitespace().map(str::to_owned).collect())
                .unwrap_or_default()
        };
        Ok(Self {
            product: product.clone(),
            build_key: info.get(product, crate::buildinfo::COL_BUILD_KEY)?.to_owned(),
            cdn: Cdn {
                name: String::new(),
                path: info.get(product, "CDN Path")?.to_owned(),
                hosts: words("CDN Hosts"),
                servers: words("CDN Servers"),
                config_path: String::new(),
            },
        })
    }
}

/// Work out what removing `departing`'s data from the store under `install`
/// would free. Every product still in `.build.info` other than it is kept.
/// Reads the indices and each build's download manifest; changes nothing.
///
/// `departing` is usually [`Build::installed`] read just before the product's
/// row goes; it can also name a build already uninstalled, whose data was
/// left behind.
pub async fn plan(
    http: &Http,
    install: &Path,
    departing: &Build,
) -> Result<Reclaim, ReclaimError> {
    let info = BuildInfo::load(install.join(".build.info"))?;

    let gone = manifest_keys(http, departing).await?;
    let mut keep = HashSet::new();
    for product in info.products().filter(|p| p != &departing.product) {
        keep.extend(manifest_keys(http, &Build::installed(&info, &product)?).await?);
    }
    let departing = gone;

    let store = Storage::open(install)?;
    let snapshot = Snapshot::take(store.data_dir())?;
    Ok(select(store.files(), &departing, &keep, store.data_dir(), snapshot))
}

/// The pure half of [`plan`]: which entries go and which archives empty.
fn select<'a>(
    files: impl Iterator<Item = &'a Entry>,
    departing: &HashSet<[u8; 9]>,
    keep: &HashSet<[u8; 9]>,
    data_dir: &Path,
    snapshot: Snapshot,
) -> Reclaim {
    let mut live: BTreeMap<u16, usize> = BTreeMap::new();
    let mut entries = Vec::new();
    for entry in files {
        *live.entry(entry.archive_id).or_default() += 1;
        if departing.contains(&entry.key) && !keep.contains(&entry.key) {
            entries.push(*entry);
        }
    }

    let mut dropped: BTreeMap<u16, (usize, u64)> = BTreeMap::new();
    for entry in &entries {
        let slot = dropped.entry(entry.archive_id).or_default();
        slot.0 += 1;
        slot.1 += u64::from(entry.size);
    }

    let mut reclaim = Reclaim { entries, snapshot, ..Reclaim::default() };
    for (archive, (count, bytes)) in dropped {
        match live.get(&archive) == Some(&count) {
            true => {
                reclaim.archives.push(archive);
                reclaim.freed += std::fs::metadata(data_dir.join(archive_name(archive)))
                    .map(|m| m.len())
                    .unwrap_or(bytes);
            }
            false => reclaim.stranded += bytes,
        }
    }
    reclaim
}

/// Carry out `reclaim` against the store under `install`, returning the
/// bytes freed.
///
/// Refuses if the store changed since the plan was made, its entries are
/// positions in files, and a store that moved may hold something else there
/// now. An archive is deleted only if, after the indices are rewritten,
/// nothing at all points into it. Never call against a live client.
pub fn execute(install: &Path, reclaim: &Reclaim) -> Result<u64, ReclaimError> {
    if reclaim.entries.is_empty() {
        return Ok(0);
    }
    let data_dir = super::storage::resolve_data_dir(install);
    if Snapshot::take(&data_dir)? != reclaim.snapshot {
        return Err(VerifyError::StoreChanged.into());
    }

    verify::remove_entries(&data_dir, reclaim.entries.iter().copied())?;

    let referenced: BTreeSet<u16> =
        Storage::open(install)?.files().map(|e| e.archive_id).collect();
    let mut freed = 0;
    for archive in &reclaim.archives {
        if referenced.contains(archive) {
            continue;
        }
        let path = data_dir.join(archive_name(*archive));
        let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        match std::fs::remove_file(&path) {
            Ok(()) => freed += bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(source) => return Err(ReclaimError::Io { path, source }),
        }
    }
    Ok(freed)
}

/// Every file an installed build's download manifest lists, as the 9-byte
/// keys the indices store: all of them, regardless of tags, since a keep-set
/// that is too large only costs space, and one too small costs files.
async fn manifest_keys(http: &Http, build: &Build) -> Result<HashSet<[u8; 9]>, ReclaimError> {
    let name = build.product.as_str().to_owned();
    let cdn_err = |source| ReclaimError::Cdn { product: name.clone(), source };
    let bad = |reason: String| ReclaimError::Manifest { product: name.clone(), reason };

    let client = Client::from_cdn(&build.cdn).map_err(cdn_err)?;
    let config = client.fetch_config(http, &build.build_key).await.map_err(cdn_err)?;
    let build = BuildConfig::parse(&config).map_err(|e| bad(e.to_string()))?;
    let pair = build.download.ok_or_else(|| bad("no download manifest".to_owned()))?;
    let ekey = pair.ekey.ok_or_else(|| bad("no download manifest key".to_owned()))?;
    let bytes = client.fetch_loose(http, &ekey, &pair.ckey, None).await.map_err(cdn_err)?;
    let manifest = DownloadManifest::parse(&bytes).map_err(|e| bad(e.to_string()))?;

    Ok(manifest
        .entries()
        .iter()
        .map(|e| e.ekey[..9].try_into().expect("9 of 16 bytes"))
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(key: u8, archive: u16, size: u32) -> Entry {
        let mut k = [0u8; 9];
        k[0] = key;
        Entry { key: k, archive_id: archive, offset: 0, size }
    }

    fn keys(ks: &[u8]) -> HashSet<[u8; 9]> {
        ks.iter()
            .map(|k| {
                let mut key = [0u8; 9];
                key[0] = *k;
                key
            })
            .collect()
    }

    /// A store written the way a download writes one, under `root/Data/data`.
    fn store_with(root: &Path, keys: &[u8]) {
        let data = root.join("Data").join("data");
        let mut writer = crate::tact::writer::StoreWriter::create(&data).unwrap();
        for k in keys {
            let mut key = [0u8; 16];
            key[0] = *k;
            writer.write(&key, &[*k; 64]).unwrap();
        }
        writer.finish().unwrap();
    }

    fn plan_for(root: &Path, departing: &[u8], keep: &[u8]) -> Reclaim {
        let store = Storage::open(root).unwrap();
        let snapshot = Snapshot::take(store.data_dir()).unwrap();
        select(store.files(), &keys(departing), &keys(keep), store.data_dir(), snapshot)
    }

    #[test]
    fn an_archive_left_empty_is_deleted_and_one_still_used_is_kept() {
        let root = tempfile::tempdir().unwrap();
        store_with(root.path(), &[1, 2, 3]);
        let archive = root.path().join("Data/data").join(archive_name(0));

        // 3 is still wanted: its archive stays, only 1 and 2 leave the index.
        let partial = plan_for(root.path(), &[1, 2, 3], &[3]);
        assert!(partial.archives.is_empty());
        assert_eq!(execute(root.path(), &partial).unwrap(), 0);
        assert!(archive.exists());
        let store = Storage::open(root.path()).unwrap();
        let left: Vec<u8> = store.files().map(|e| e.key[0]).collect();
        assert_eq!(left, vec![3]);

        // Now nothing wants it: the archive goes.
        let whole = plan_for(root.path(), &[3], &[]);
        assert_eq!(whole.archives, vec![0]);
        assert!(execute(root.path(), &whole).unwrap() > 0);
        assert!(!archive.exists());
    }

    #[test]
    fn a_store_that_changed_since_the_plan_is_refused() {
        let root = tempfile::tempdir().unwrap();
        store_with(root.path(), &[1, 2]);
        let stale = plan_for(root.path(), &[1], &[]);
        // Another writer lands between plan and execute.
        std::thread::sleep(std::time::Duration::from_millis(20));
        let data = root.path().join("Data/data");
        let mut writer = crate::tact::writer::StoreWriter::create(&data).unwrap();
        writer.write(&[9u8; 16], &[9; 64]).unwrap();
        writer.finish().unwrap();

        assert!(matches!(
            execute(root.path(), &stale),
            Err(ReclaimError::Verify(VerifyError::StoreChanged))
        ));
        assert_eq!(Storage::open(root.path()).unwrap().files().count(), 3);
    }

    #[test]
    fn only_unshared_entries_go_and_only_emptied_archives_are_freed() {
        let files = [
            // Archive 5: all MoP-only, emptied, freed whole.
            entry(1, 5, 100),
            entry(2, 5, 100),
            // Archive 6: one MoP-only file beside a shared one, stranded.
            entry(3, 6, 40),
            entry(4, 6, 60),
            // Archive 7: retail only, untouched.
            entry(9, 7, 500),
        ];
        let departing = keys(&[1, 2, 3, 4]);
        let keep = keys(&[4, 9]);
        let dir = std::env::temp_dir();
        let reclaim = select(files.iter(), &departing, &keep, &dir, Snapshot::default());

        let gone: Vec<u8> = reclaim.entries.iter().map(|e| e.key[0]).collect();
        assert_eq!(gone, vec![1, 2, 3]);
        assert_eq!(reclaim.archives, vec![5]);
        assert_eq!(reclaim.stranded, 40);
    }
}
