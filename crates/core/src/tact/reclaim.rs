//! Giving back the shared store's space: when one WoW version is uninstalled,
//! and for data no version uses any more.
//!
//! Every version in an install shares one CASC store, so deleting a
//! version's folder frees only its executables. [`plan`] finds the entries
//! only the departing version uses, [`plan_unused`] the ones no version uses
//! (what patches leave behind). Either drops them from the indices and
//! compacts the archives left mostly dead ([`super::compact`]).
//!
//! Three rules keep other versions' files safe:
//!
//! * The keep-set comes from the builds **on disk**: every `.build.info` row,
//!   every `.product.db` record without one (Agent drops a pre-installed
//!   build's row and keeps its data, as the Forever stress test showed on
//!   2026-10-17), and every pre-downloaded build ([`super::staged`]).
//! * A build keeps everything its build config names too (encoding, root,
//!   `vfs-*`, patch manifests), which no manifest lists.
//! * If any version's manifests cannot be read, there is no plan.
//!
//! Entries in an archive under a quarter dead stay as dead space until a
//! later compaction.

use std::collections::{BTreeSet, HashSet};
use std::path::Path;

use crate::buildinfo::{BuildInfo, BuildInfoError};
use crate::net::Http;
use crate::product::ProductCode;

use super::cdn::{CdnError, Client};
use super::compact::{self, CompactError, Compaction};
use super::config::{self as tact_config, BuildConfig};
use super::encoding::Encoding;
use super::productdb::{self, ProductDbError};
use super::idx::Entry;
use super::manifest::{DownloadManifest, TagSet};
use super::storage::{Storage, StorageError};
#[cfg(test)]
use super::storage::archive_name;
use super::verify::{self, Snapshot, VerifyError};
use super::writer::StoreLock;
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
    #[error(transparent)]
    Compact(#[from] CompactError),
    #[error(transparent)]
    ProductDb(#[from] ProductDbError),
    #[error(transparent)]
    Staged(#[from] super::staged::StagedError),
    #[error(
        "{product} is recorded in .product.db but its build cannot be worked out, \
         so its data cannot be told apart from unused data"
    )]
    Unaccounted { product: String },
    #[error("{product}'s {what} is not in the store, so what it needs cannot be listed")]
    NotInStore { product: String, what: &'static str },
}

/// What reclaiming one version's data would do.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reclaim {
    /// Store entries only the departing version uses.
    pub entries: Vec<Entry>,
    /// Archives compacted afterwards, or deleted when nothing is left in them.
    pub archives: Vec<u16>,
    /// Bytes of other versions' files the compaction copies.
    pub moved: u64,
    /// Bytes of disk freed.
    pub freed: u64,
    /// Dead bytes left in touched archives not worth rewriting.
    pub stranded: u64,
    /// The store as planned against; execution refuses if it has moved.
    pub snapshot: Snapshot,
}

/// One build, and where its CDN serves it from.
#[derive(Debug, Clone, PartialEq, Eq)]
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

    /// A build named by the version service, not `.build.info`: what a
    /// cancelled download was fetching, which has no installed row.
    pub fn remote(product: ProductCode, build_key: String, cdn: Cdn) -> Self {
        Self { product, build_key, cdn }
    }
}

/// Every build whose data lives in the store under `install`: each
/// `.build.info` row, each product `.product.db` records without one, and
/// each pre-downloaded build.
///
/// A record with no build key or no CDN row is an error: skipped, its whole
/// build would count as unused.
pub fn builds(install: &Path) -> Result<Vec<Build>, ReclaimError> {
    let info = BuildInfo::load(install.join(".build.info"))?;
    let mut builds = info
        .products()
        .map(|product| Build::installed(&info, &product))
        .collect::<Result<Vec<_>, _>>()?;

    let records = match productdb::read(&install.join(".product.db")) {
        Ok(records) => records,
        Err(ProductDbError::Io { source, .. }) if source.kind() == std::io::ErrorKind::NotFound => {
            Vec::new()
        }
        Err(e) => return Err(e.into()),
    };
    for record in records.iter().filter(|r| !r.is_infrastructure()) {
        let product: ProductCode = record.product.parse().expect("product codes are infallible");
        if builds.iter().any(|b| b.product == product) {
            continue;
        }
        let unaccounted = || ReclaimError::Unaccounted { product: record.product.clone() };
        let build_key = record.build_key.clone().ok_or_else(unaccounted)?;
        // Every WoW version comes from the same CDN path as its siblings.
        let cdn = builds.first().map(|b| b.cdn.clone()).ok_or_else(unaccounted)?;
        builds.push(Build { product, build_key, cdn });
    }

    for staged in super::staged::load(install)? {
        if builds.iter().any(|b| b.product == staged.product && b.build_key == staged.build_key) {
            continue;
        }
        let cdn = Cdn {
            name: String::new(),
            path: staged.cdn_path,
            hosts: staged.cdn_hosts,
            servers: staged.cdn_servers,
            config_path: String::new(),
        };
        builds.push(Build { product: staged.product, build_key: staged.build_key, cdn });
    }
    Ok(builds)
}

/// Work out what removing `departing`'s data from the store under `install`
/// would free. Every other build in [`builds`] is kept. Reads the indices and
/// each build's manifests; changes nothing.
///
/// `departing` is usually [`Build::installed`] read just before the row goes,
/// or a build already uninstalled whose data stayed.
pub async fn plan(
    http: &Http,
    install: &Path,
    departing: &Build,
) -> Result<Reclaim, ReclaimError> {
    let config = build_config(http, install, departing).await?;
    let gone = manifest_keys(http, departing, &config).await?;

    let mut store = Storage::open(install)?;
    let mut keep = HashSet::new();
    for build in kept(&builds(install)?, departing) {
        keep.extend(keep_keys(http, install, &mut store, build).await?);
    }

    let snapshot = Snapshot::take(store.data_dir())?;
    let gone = |e: &Entry| gone.contains(&e.key) && !keep.contains(&e.key);
    Ok(select(store.files(), gone, store.data_dir(), snapshot))
}

/// Every build in `all` but `departing`, matched on build key too: the same
/// product may be installed at a newer build.
fn kept<'a>(all: &'a [Build], departing: &'a Build) -> impl Iterator<Item = &'a Build> {
    all.iter()
        .filter(move |b| !(b.product == departing.product && b.build_key == departing.build_key))
}

/// Work out what dropping every entry no build in [`builds`] uses would
/// free: what patches, repairs and old builds left behind. Changes nothing.
///
/// Refuses when any build cannot be accounted for. Never while a download is
/// running or paused: what it wrote belongs to no build yet.
pub async fn plan_unused(http: &Http, install: &Path) -> Result<Reclaim, ReclaimError> {
    let mut store = Storage::open(install)?;
    let mut keep = HashSet::new();
    for build in builds(install)? {
        keep.extend(keep_keys(http, install, &mut store, &build).await?);
    }

    let snapshot = Snapshot::take(store.data_dir())?;
    Ok(select(store.files(), |e| !keep.contains(&e.key), store.data_dir(), snapshot))
}

/// Work out what switching one optional download off would free, for the
/// installed build of `product`: only the files that option alone brought in,
/// and only those no other build on disk needs. Changes nothing.
///
/// `remaining` is every other option still opted in to. The tags come from
/// `.build.info`, as the download's did.
pub async fn plan_optional(
    http: &Http,
    install: &Path,
    product: &ProductCode,
    name: &str,
    remaining: &[String],
) -> Result<Reclaim, ReclaimError> {
    let info = BuildInfo::load(install.join(".build.info"))?;
    let target = Build::installed(&info, product)?;
    let base = TagSet::parse(info.get(product, "Tags")?).with_optional(remaining.iter().cloned());

    let config = build_config(http, install, &target).await?;
    let manifest = download_manifest(http, &target, &config).await?;
    let only: HashSet<[u8; 9]> = manifest
        .only_with(&base, name)
        .iter()
        .map(|e| e.ekey[..9].try_into().expect("9 of 16 bytes"))
        .collect();

    let mut store = Storage::open(install)?;
    let mut keep = HashSet::new();
    for build in builds(install)? {
        let mut needs = keep_keys(http, install, &mut store, &build).await?;
        if build.product == target.product && build.build_key == target.build_key {
            needs.retain(|key| !only.contains(key));
        }
        keep.extend(needs);
    }

    let snapshot = Snapshot::take(store.data_dir())?;
    let gone = |e: &Entry| only.contains(&e.key) && !keep.contains(&e.key);
    Ok(select(store.files(), gone, store.data_dir(), snapshot))
}

/// The pure half of [`plan`]: which entries go, and which archives that
/// leaves worth compacting.
fn select<'a>(
    files: impl Iterator<Item = &'a Entry>,
    gone: impl Fn(&Entry) -> bool,
    data_dir: &Path,
    snapshot: Snapshot,
) -> Reclaim {
    let (entries, live): (Vec<Entry>, Vec<Entry>) = files.copied().partition(|e| gone(e));

    // Only archives the dropped files were in.
    let touched: BTreeSet<u16> = entries.iter().map(|e| e.archive_id).collect();
    let usage = compact::usage(data_dir, live.iter(), entries.iter());
    let Compaction { archives, moved, freed, left } =
        Compaction::choose(&usage, |a| touched.contains(&a));

    Reclaim { entries, archives, moved, freed, stranded: left, snapshot }
}

/// Carry out `reclaim` against the store under `install`, returning the
/// bytes freed. `progress` hears the bytes the compaction has copied.
///
/// Refuses if the store changed since the plan: its entries are positions.
/// Never against a live client.
pub fn execute(
    install: &Path,
    reclaim: &Reclaim,
    progress: impl FnMut(u64),
) -> Result<u64, ReclaimError> {
    if reclaim.entries.is_empty() {
        return Ok(0);
    }
    let data_dir = super::storage::resolve_data_dir(install);
    let lock = StoreLock::acquire(&data_dir).map_err(CompactError::from)?;
    if Snapshot::take(&data_dir)? != reclaim.snapshot {
        return Err(VerifyError::StoreChanged.into());
    }

    verify::remove_entries(&data_dir, reclaim.entries.iter().copied())?;
    Ok(compact::rewrite(install, &reclaim.archives, progress, lock)?)
}

/// A build's config: from the store's own `Data/config` when it is there,
/// which is every installed build, else from the CDN.
async fn build_config(http: &Http, install: &Path, build: &Build) -> Result<BuildConfig, ReclaimError> {
    BuildConfig::parse(&config_text(http, install, build).await?)
        .map_err(|e| ReclaimError::Manifest { product: build.product.as_str().to_owned(), reason: e.to_string() })
}

async fn config_text(http: &Http, install: &Path, build: &Build) -> Result<String, ReclaimError> {
    let local = tact_config::config_path(&install.join("Data").join("config"), &build.build_key)
        .ok()
        .and_then(|path| std::fs::read_to_string(path).ok());
    if let Some(text) = local {
        return Ok(text);
    }
    let cdn_err = |source| ReclaimError::Cdn { product: build.product.as_str().to_owned(), source };
    let client = Client::from_cdn(&build.cdn).map_err(cdn_err)?;
    client.fetch_config(http, &build.build_key).await.map_err(cdn_err)
}

/// Everything in the store `build` needs: its download manifest's files, and
/// every key its build config names, with the content-key-only ones (root,
/// patch) resolved through its encoding manifest.
pub(crate) async fn keep_keys(
    http: &Http,
    install: &Path,
    store: &mut Storage,
    build: &Build,
) -> Result<HashSet<[u8; 9]>, ReclaimError> {
    let text = config_text(http, install, build).await?;
    let product = build.product.as_str().to_owned();
    let config = BuildConfig::parse(&text)
        .map_err(|e| ReclaimError::Manifest { product: product.clone(), reason: e.to_string() })?;
    let mut keep = manifest_keys(http, build, &config).await?;

    let named: Vec<[u8; 16]> = text
        .lines()
        .filter_map(|line| line.split_once('='))
        .filter(|(name, _)| !name.trim().ends_with("-size"))
        .flat_map(|(_, value)| value.split_whitespace().filter_map(|h| tact_config::parse_hash(h).ok()))
        .collect();

    let ekey = config.encoding.ekey.ok_or_else(|| ReclaimError::Manifest {
        product: product.clone(),
        reason: "no encoding key".to_owned(),
    })?;
    // Verified: a damaged copy would keep too little.
    let raw = store.read_verified(&ekey, &config.encoding.ckey).map_err(|e| match e {
        StorageError::ContentMismatch { .. } => ReclaimError::Storage(e),
        _ => ReclaimError::NotInStore { product: product.clone(), what: "encoding manifest" },
    })?;
    let encoding = Encoding::parse(&raw)
        .map_err(|e| ReclaimError::Manifest { product: product.clone(), reason: e.to_string() })?;
    drop(raw);

    let short = |k: &[u8; 16]| -> [u8; 9] { k[..9].try_into().expect("9 of 16 bytes") };
    for hash in &named {
        keep.insert(short(hash));
        if let Some(ekey) = encoding.ekey_for(hash) {
            keep.insert(short(&ekey));
        }
    }
    Ok(keep)
}

/// Every file an installed build's download manifest lists, as 9-byte index
/// keys, regardless of tags: too large a keep-set only costs space.
async fn manifest_keys(
    http: &Http,
    build: &Build,
    config: &BuildConfig,
) -> Result<HashSet<[u8; 9]>, ReclaimError> {
    let manifest = download_manifest(http, build, config).await?;
    Ok(manifest
        .entries()
        .iter()
        .map(|e| e.ekey[..9].try_into().expect("9 of 16 bytes"))
        .collect())
}

/// An installed build's download manifest, from the CDN.
async fn download_manifest(
    http: &Http,
    build: &Build,
    config: &BuildConfig,
) -> Result<DownloadManifest, ReclaimError> {
    let name = build.product.as_str().to_owned();
    let cdn_err = |source| ReclaimError::Cdn { product: name.clone(), source };
    let bad = |reason: String| ReclaimError::Manifest { product: name.clone(), reason };

    let client = Client::from_cdn(&build.cdn).map_err(cdn_err)?;
    let pair = config.download.as_ref().ok_or_else(|| bad("no download manifest".to_owned()))?;
    let ekey = pair.ekey.ok_or_else(|| bad("no download manifest key".to_owned()))?;
    let bytes = client.fetch_loose(http, &ekey, &pair.ckey).await.map_err(cdn_err)?;
    DownloadManifest::parse(&bytes).map_err(|e| bad(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(product: ProductCode, key: &str) -> Build {
        let cdn = Cdn {
            name: String::new(),
            path: "tpr/wow".into(),
            hosts: Vec::new(),
            servers: Vec::new(),
            config_path: String::new(),
        };
        Build { product, build_key: key.into(), cdn }
    }

    #[test]
    fn an_old_build_leaving_keeps_the_installed_build_of_the_same_product() {
        // `lightforge reclaim --build-key <old>` while retail is installed at
        // a newer build: everything the two share must stay.
        let installed = build(ProductCode::Retail, "new");
        let era = build(ProductCode::ClassicEra, "era");
        let all = [installed.clone(), era.clone()];

        let departing = build(ProductCode::Retail, "old");
        let staying: Vec<_> = kept(&all, &departing).collect();
        assert_eq!(staying, [&installed, &era]);

        // The build itself leaving is still excluded.
        let staying: Vec<_> = kept(&all, &installed).collect();
        assert_eq!(staying, [&era]);
    }

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
        let (departing, keep) = (keys(departing), keys(keep));
        let gone = |e: &Entry| departing.contains(&e.key) && !keep.contains(&e.key);
        select(store.files(), gone, store.data_dir(), snapshot)
    }

    #[test]
    fn an_archive_left_empty_is_deleted_and_one_still_used_is_kept() {
        let root = tempfile::tempdir().unwrap();
        store_with(root.path(), &[1, 2, 3]);
        let archive = root.path().join("Data/data").join(archive_name(0));

        // 3 is still wanted: its archive stays, only 1 and 2 leave the index.
        // Two thirds of the archive go dead, so it is rewritten around 3.
        let partial = plan_for(root.path(), &[1, 2, 3], &[3]);
        assert_eq!(partial.archives, vec![0]);
        assert_eq!(partial.moved, 94);
        assert!(execute(root.path(), &partial, |_| {}).unwrap() > 0);
        assert!(!archive.exists());
        let mut store = Storage::open(root.path()).unwrap();
        let left: Vec<u8> = store.files().map(|e| e.key[0]).collect();
        assert_eq!(left, vec![3]);
        let mut key = [0u8; 16];
        key[0] = 3;
        let moved = *store.locate(&key).unwrap();
        assert_eq!(store.read_raw(&moved).unwrap()[30..], [3; 64]);

        // Now nothing wants it: its archive goes, nothing copied.
        let whole = plan_for(root.path(), &[3], &[]);
        assert_eq!(whole.archives, vec![moved.archive_id]);
        assert_eq!(whole.moved, 0);
        assert!(execute(root.path(), &whole, |_| {}).unwrap() > 0);
        assert!(!root.path().join("Data/data").join(archive_name(moved.archive_id)).exists());
    }

    /// `.build.info` with one row per `(product, build key)`, served from
    /// `tpr/wow` like every WoW version.
    fn write_build_info(dir: &Path, rows: &[(&str, &str)]) {
        let mut text = "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Path!STRING:0|\
                        CDN Hosts!STRING:0|CDN Servers!STRING:0|Product!STRING:0\n"
            .to_owned();
        for (product, key) in rows {
            text += &format!("eu|1|{key}|tpr/wow|eu.cdn.blizzard.com|http://eu.cdn.blizzard.com|{product}\n");
        }
        std::fs::write(dir.join(".build.info"), text).unwrap();
    }

    fn record(product: &str, build_key: &str) -> productdb::ProductInstall {
        productdb::ProductInstall {
            product: product.into(),
            install_path: "C:/Program Files (x86)/World of Warcraft".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: String::new(),
            geo_country: String::new(),
            flavor_dir: String::new(),
            version: "1.0".into(),
            build_key: build_key.into(),
            tags: String::new(),
            key_ring: None,
            family: "wow".into(),
            installed_at_ms: 0,
        }
    }

    /// The stress test on 2026-10-17: Agent took its `.build.info` row away
    /// and kept its data. Its `.product.db` record is all that says so.
    #[test]
    fn a_pre_installed_build_without_a_row_is_still_kept() {
        let dir = tempfile::tempdir().unwrap();
        write_build_info(dir.path(), &[("wow", &"a".repeat(32))]);
        productdb::write(dir.path(), &[record("wow", &"a".repeat(32)), record("wowf", &"f".repeat(32))])
            .unwrap();

        let builds = builds(dir.path()).unwrap();
        let found: Vec<(&str, &str)> =
            builds.iter().map(|b| (b.product.as_str(), b.build_key.as_str())).collect();
        let (a, f) = ("a".repeat(32), "f".repeat(32));
        assert_eq!(found, vec![("wow", a.as_str()), ("wowf", f.as_str())]);
        // Served from where its sibling is.
        assert_eq!(builds[1].cdn.path, "tpr/wow");
    }

    #[test]
    fn a_pre_downloaded_build_is_kept_alongside_the_installed_one() {
        let dir = tempfile::tempdir().unwrap();
        write_build_info(dir.path(), &[("wow", &"a".repeat(32))]);
        let staged = super::super::staged::Staged {
            product: ProductCode::Retail,
            build_key: "b".repeat(32),
            version: "12.2.0.71000".to_owned(),
            cdn_path: "tpr/wow".to_owned(),
            cdn_hosts: vec!["level3.blizzard.com".to_owned()],
            cdn_servers: Vec::new(),
        };
        super::super::staged::record(dir.path(), staged).unwrap();

        let keys: Vec<String> = builds(dir.path()).unwrap().into_iter().map(|b| b.build_key).collect();
        assert_eq!(keys, vec!["a".repeat(32), "b".repeat(32)]);
    }

    #[test]
    fn a_record_whose_build_cannot_be_named_refuses_the_plan() {
        let dir = tempfile::tempdir().unwrap();
        write_build_info(dir.path(), &[("wow", &"a".repeat(32))]);
        productdb::write(dir.path(), &[record("wowf", "")]).unwrap();
        assert!(matches!(builds(dir.path()), Err(ReclaimError::Unaccounted { .. })));
    }

    #[test]
    fn no_product_db_means_the_rows_alone() {
        let dir = tempfile::tempdir().unwrap();
        write_build_info(dir.path(), &[("wow", &"a".repeat(32))]);
        assert_eq!(builds(dir.path()).unwrap().len(), 1);
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
            execute(root.path(), &stale, |_| {}),
            Err(ReclaimError::Verify(VerifyError::StoreChanged))
        ));
        assert_eq!(Storage::open(root.path()).unwrap().files().count(), 3);
    }

    #[test]
    fn only_unshared_entries_go_and_only_mostly_dead_archives_are_rewritten() {
        let files = [
            // Archive 5: all MoP-only, emptied, freed whole.
            entry(1, 5, 100),
            entry(2, 5, 100),
            // Archive 6: one small MoP-only file beside a large shared one,
            // not worth copying the shared one for: stranded.
            entry(3, 6, 40),
            entry(4, 6, 600),
            // Archive 7: retail only, untouched.
            entry(9, 7, 500),
        ];
        let departing = keys(&[1, 2, 3, 4]);
        let keep = keys(&[4, 9]);
        let dir = std::env::temp_dir();
        let gone = |e: &Entry| departing.contains(&e.key) && !keep.contains(&e.key);
        let reclaim = select(files.iter(), gone, &dir, Snapshot::default());

        let gone: Vec<u8> = reclaim.entries.iter().map(|e| e.key[0]).collect();
        assert_eq!(gone, vec![1, 2, 3]);
        assert_eq!(reclaim.archives, vec![5]);
        assert_eq!(reclaim.moved, 0);
        assert_eq!(reclaim.stranded, 40);
    }
}
