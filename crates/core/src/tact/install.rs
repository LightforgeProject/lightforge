//! Driving a whole install: version service -> plan -> download -> store.
//!
//! This is the orchestration both front-ends need, so it lives here rather than
//! in either of them. It reports through an [`Event`] callback instead of
//! printing: the CLI turns events into lines, and the GUI turns them into
//! messages, without either owning the sequence.
//!
//! The ordering is not arbitrary and is the part worth preserving:
//!
//! 1. Archive indices are cached first — nothing can be *located* without them.
//! 2. Whatever the store already holds is subtracted from the plan, which is
//!    what makes an interrupted download resume rather than start over.
//! 3. `.build.info` is written **last, and only on a genuinely complete run**.
//!    [`crate::state::Local`] treats its presence as proof that a download
//!    finished, so writing it after a partial run would offer PLAY on a broken
//!    game. Every way a run can fall short is accounted for in
//!    [`Summary::complete`] — failures, cancellation, unlocatable keys, an
//!    incomplete install-file extraction, and the limit options, which
//!    truncate the plan deliberately.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use crate::buildinfo::BuildInfo;
use crate::net::Http;
use crate::product::ProductCode;
use crate::tact::archive::FileIndex;
use crate::tact::cdn::Client;
use crate::tact::config::{BuildConfig, CdnConfig, KeyPair};
use crate::tact::download::{
    self, extract_install, Cancel, Catalog, Downloader, Options as DownloadOptions, Progress,
    Source,
};
use crate::tact::encoding::Encoding;
use crate::tact::indices::{IndexCache, Locations, Step};
use crate::tact::manifest::{DownloadManifest, InstallManifest, TagSet};
use crate::tact::plan::{reverse_encoding, Options as PlanOptions, Plan};
use crate::tact::storage::{EKey, Storage};
use crate::tact::version::{self, Region};
use crate::tact::writer::{StoreWriter, Summary as StoreSummary};

/// How a run is tuned. The limits exist for development slices and must never
/// be set by a real install — each of them makes the result deliberately
/// partial, which is why [`Summary::complete`] refuses `.build.info` when any
/// is in play.
#[derive(Debug, Clone)]
pub struct Options {
    pub region: Region,
    /// Client locale, e.g. `enUS`. Selects which of a build's files are wanted.
    pub locale: String,
    /// Flavor directory the install files are extracted into, e.g. `_retail_`.
    pub flavor: String,
    pub concurrency: usize,
    /// Ranges this far apart are merged into one request.
    pub gap_bytes: u32,
    /// Upper bound on the checkpoint interval; scaled down for small products.
    pub checkpoint_bytes: u64,
    /// Extract the install manifest's files (the game executable and friends).
    pub install_files: bool,
    /// Stop after this many archive indices — development only.
    pub limit_archives: Option<usize>,
    /// Truncate the plan to this many coalesced runs — development only.
    pub limit_runs: Option<usize>,
    /// Skip the loose (unarchived) files — development only.
    pub skip_loose: bool,
    /// The install's path as Windows sees it, e.g.
    /// `C:/Program Files (x86)/World of Warcraft`. `None` skips writing
    /// `.product.db`, which only matters for Battle.net adoption.
    pub windows_path: Option<String>,
    /// Account and GeoIP country, as Agent would record them. Empty is fine.
    pub account_country: String,
    pub geo_country: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            region: Region::default(),
            locale: "enUS".to_owned(),
            flavor: String::new(),
            // 32 measured 2.4x the throughput of 8 on a real product: these
            // transfers are request-latency-bound, not bandwidth-bound.
            concurrency: 32,
            gap_bytes: 64 * 1024,
            checkpoint_bytes: 4096 * 1024 * 1024,
            install_files: true,
            limit_archives: None,
            limit_runs: None,
            skip_loose: false,
            windows_path: Some(crate::paths::WINDOWS_WOW_PATH.to_owned()),
            account_country: String::new(),
            geo_country: String::new(),
        }
    }
}

impl Options {
    /// The tag expression a `.build.info` carries for this machine.
    pub fn tag_column(&self) -> String {
        let r = self.region.as_str().to_uppercase();
        format!(
            "Windows x86_64 {r} {locale} speech:Windows x86_64 {r} {locale} text",
            locale = self.locale
        )
    }

    /// True when any option deliberately truncates the plan.
    pub fn limited(&self) -> bool {
        self.limit_archives.is_some() || self.limit_runs.is_some() || self.skip_loose
    }
}

/// What the run is doing, as it happens.
#[derive(Debug, Clone)]
pub enum Event {
    Build {
        versions_name: String,
        build_config: String,
        archives: usize,
    },
    Encoding {
        entries: usize,
    },
    Manifests {
        download: usize,
        install: usize,
        unknown_tags: Vec<String>,
    },
    Wanted {
        files: usize,
        bytes: u64,
    },
    /// One step of the archive-index prelude; `done`/`total` count indices.
    Indices(Step),
    IndicesDone {
        cached: usize,
        downloaded: usize,
        bytes: u64,
    },
    Loose {
        files: usize,
    },
    Located {
        located: usize,
        wanted: usize,
    },
    Planned {
        requests: usize,
        files: usize,
        bytes: u64,
        padding: u64,
        already_present: usize,
        missing: usize,
    },
    /// Transfer progress. Called once per stored file, so a UI consumer must
    /// coalesce rather than redraw on each one.
    Progress(Progress),
    InstallFile {
        path: PathBuf,
        bytes: u64,
    },
    Extracted {
        files: usize,
        bytes: u64,
    },
    BuildInfo {
        path: PathBuf,
        version: String,
    },
    ProductDb {
        path: PathBuf,
    },
    /// The run finished short of a complete install, and why.
    Incomplete {
        reasons: Vec<String>,
    },
}

/// What a run achieved.
#[derive(Debug, Clone)]
pub struct Summary {
    pub outcome: download::Outcome,
    pub store: StoreSummary,
    /// Keys the plan could not locate in any archive or the file index.
    pub missing: usize,
    /// True only if this run produced a whole, launchable install — the same
    /// condition that gates `.build.info`.
    pub complete: bool,
    /// Present when `complete`; the version `.build.info` now names.
    pub version: Option<String>,
}

/// Everything resolved before a byte is fetched: what the build is, what is
/// wanted, and where each key lives.
///
/// Split out so a caller can show a plan (a dry run) without committing to the
/// download, and so the GUI can present sizes before asking the user.
pub struct Prepared {
    pub plan: Plan,
    pub remote: version::Version,
    pub cdn: version::Cdn,
    pub client: Client,
    pub encoding: Encoding,
    pub install: InstallManifest,
    pub locations: Locations,
    pub loose: FileIndex,
    pub tags: TagSet,
    pub tag_column: String,
}

/// Resolve the build and build the fetch plan, fetching nothing else.
pub async fn prepare(
    http: &Http,
    dest: &Path,
    code: &ProductCode,
    options: &Options,
    on_event: &mut impl FnMut(Event),
) -> Result<Prepared, InstallError> {
    let tag_column = options.tag_column();
    let tags = TagSet::parse(&tag_column);

    let versions = version::versions(http, options.region, code.as_str()).await?;
    let remote = version::for_region(&versions, options.region, |v| &v.region)
        .cloned()
        .ok_or(InstallError::EmptyTable("versions"))?;
    let cdns = version::cdns(http, options.region, code.as_str()).await?;
    let cdn = version::for_region(&cdns, options.region, |c| &c.name)
        .cloned()
        .ok_or(InstallError::EmptyTable("cdns"))?;
    let client = Client::from_cdn(&cdn)?;

    let build = BuildConfig::parse(&client.fetch_config(http, &remote.build_config).await?)?;
    let cdn_config = CdnConfig::parse(&client.fetch_config(http, &remote.cdn_config).await?)?;
    on_event(Event::Build {
        versions_name: remote.versions_name.clone(),
        build_config: remote.build_config.clone(),
        archives: cdn_config.archives.len(),
    });

    // Manifests are the loose kind: one plain GET, decoded and checked against
    // the content key the build config names them by.
    let fetch_manifest = |pair: Option<KeyPair>, what: &'static str| {
        let client = &client;
        async move {
            let pair = pair.ok_or(InstallError::MissingManifest(what))?;
            let ekey = pair.ekey.ok_or(InstallError::MissingManifest(what))?;
            Ok::<_, InstallError>(client.fetch_loose(http, &ekey, &pair.ckey, None).await?)
        }
    };

    let encoding = Encoding::parse(&fetch_manifest(Some(build.encoding), "encoding").await?)?;
    on_event(Event::Encoding { entries: encoding.len() });

    let download_manifest =
        DownloadManifest::parse(&fetch_manifest(build.download, "download").await?)?;
    let install = InstallManifest::parse(&fetch_manifest(build.install, "install").await?)?;
    on_event(Event::Manifests {
        download: download_manifest.len(),
        install: install.len(),
        unknown_tags: tags
            .unknown_names(download_manifest.tags())
            .into_iter()
            .map(str::to_owned)
            .collect(),
    });

    let wanted = download_manifest.select(&tags);
    on_event(Event::Wanted {
        files: wanted.len(),
        bytes: wanted.iter().map(|e| e.size).sum(),
    });

    // The archive indices. Without these nothing can be located.
    let cache = IndexCache::new(dest.join("Data").join("indices"));
    let mut archives = cdn_config.archive_indices();
    if let Some(limit) = options.limit_archives {
        archives.truncate(limit);
    }
    let report = cache
        .ensure(http, &client, &archives, options.concurrency, |step| {
            on_event(Event::Indices(step))
        })
        .await?;
    on_event(Event::IndicesDone {
        cached: report.cached,
        downloaded: report.downloaded,
        bytes: report.bytes,
    });

    let file_index_hash = cdn_config
        .file_index
        .clone()
        .ok_or(InstallError::MissingManifest("file index"))?;
    if !cache.is_cached(&file_index_hash, cdn_config.file_index_size) {
        let bytes = client
            .fetch_index(http, &file_index_hash, cdn_config.file_index_size)
            .await?;
        std::fs::write(cache.path_of(&file_index_hash), &bytes).map_err(|source| {
            InstallError::Io { path: cache.path_of(&file_index_hash), source }
        })?;
    }
    let loose = FileIndex::parse(&std::fs::read(cache.path_of(&file_index_hash)).map_err(
        |source| InstallError::Io { path: cache.path_of(&file_index_hash), source },
    )?)?;
    on_event(Event::Loose { files: loose.len() });

    let wanted_keys: HashSet<EKey> = wanted.iter().map(|e| e.ekey).collect();
    let ckeys = reverse_encoding(&encoding, &wanted_keys);
    let mut locations = Locations::default();
    for (archive, _) in &archives {
        if cache.is_cached(archive, None) {
            locations.absorb(&cache.load_archive(archive)?, &wanted_keys);
        }
    }
    on_event(Event::Located { located: locations.len(), wanted: wanted.len() });

    // Skip whatever the store already holds — this is what makes an
    // interrupted download resume instead of starting over.
    let present: HashSet<EKey> = match Storage::open(dest) {
        Ok(store) => store
            .files()
            .map(|e| {
                let mut k = [0u8; 16];
                k[..9].copy_from_slice(&e.key);
                k
            })
            .collect(),
        Err(_) => HashSet::new(),
    };
    let have = |ekey: &EKey| {
        let mut probe = [0u8; 16];
        probe[..9].copy_from_slice(&ekey[..9]);
        present.contains(&probe)
    };

    let mut plan = Plan::build(
        &wanted,
        &locations,
        &loose,
        &ckeys,
        have,
        PlanOptions { max_gap: options.gap_bytes, ..PlanOptions::default() },
    );
    if options.skip_loose {
        plan.loose.clear();
    }
    if let Some(limit) = options.limit_runs {
        plan.runs.truncate(limit);
    }
    on_event(Event::Planned {
        requests: plan.requests(),
        files: plan.files(),
        bytes: plan.bytes(),
        padding: plan.padding(),
        already_present: plan.already_present,
        missing: plan.missing.len(),
    });

    Ok(Prepared { plan, remote, cdn, client, encoding, install, locations, loose, tags, tag_column })
}

/// Run a prepared plan: fetch, extract the install files, and finish the store.
pub async fn execute(
    http: &Http,
    dest: &Path,
    code: &ProductCode,
    options: &Options,
    prepared: Prepared,
    cancel: &Cancel,
    on_event: &mut impl FnMut(Event),
) -> Result<Summary, InstallError> {
    let Prepared { plan, remote, cdn, client, encoding, install, locations, loose, tags, tag_column } =
        prepared;

    let mut writer = StoreWriter::create(dest.join("Data").join("data"))?;
    // The reader always carries a key service; the downloader must too, or
    // blte cannot even name a missing key properly.
    let keys = download::default_key_service();
    let source = Source { http, client: &client, keys: Some(&keys) };
    let downloader = Downloader::new(DownloadOptions {
        concurrency: options.concurrency,
        checkpoint_bytes: options.checkpoint_bytes,
    });

    let outcome = downloader
        .run(source, &plan, &mut writer, cancel, |p| on_event(Event::Progress(p)))
        .await?;

    let mut install_complete = true;
    if options.install_files {
        let selected = install.select(&tags);
        let catalog = Catalog { encoding: &encoding, locations: &locations, loose: &loose };
        let report = extract_install(
            source,
            &selected,
            catalog,
            &dest.join(&options.flavor),
            cancel,
            |e| on_event(Event::InstallFile { path: e.path.clone(), bytes: e.bytes }),
        )
        .await?;
        on_event(Event::Extracted { files: report.written.len(), bytes: report.bytes() });
        install_complete = report.is_complete();

        // Agent writes this and the install manifest does not list it; without
        // it the client cannot tell which product it is.
        //
        // Only on a complete extraction, though: a `.flavor.info` sitting in a
        // directory with no game in it claims a product that is not installed.
        // A cancelled run used to leave exactly that behind, and Battle.net
        // scanning the install then sees a flavor it cannot account for.
        if install_complete {
            let flavor_dir = dest.join(&options.flavor);
            download::write_flavor_info(&flavor_dir, code)
                .map_err(|source| InstallError::Io { path: flavor_dir, source })?;
        }
    }

    let mut reasons = Vec::new();
    if !outcome.failed.is_empty() {
        reasons.push(format!("{} files failed", outcome.failed.len()));
    }
    if !plan.missing.is_empty() {
        reasons.push(format!("{} keys unlocatable", plan.missing.len()));
    }
    if outcome.cancelled {
        reasons.push("cancelled".to_owned());
    }
    if options.limited() {
        reasons.push("limit options truncated the plan".to_owned());
    }
    if options.install_files && !install_complete {
        reasons.push("install files incomplete".to_owned());
    }
    let complete = reasons.is_empty();

    let version = if complete {
        let info_path = dest.join(".build.info");
        let mut info = BuildInfo::load(&info_path).unwrap_or_else(|_| BuildInfo::with_default_schema());
        info.set_product(
            code,
            &[
                ("Branch", options.region.as_str()),
                ("Active", "1"),
                ("Build Key", &remote.build_config),
                ("CDN Key", &remote.cdn_config),
                ("CDN Path", &cdn.path),
                ("CDN Hosts", &cdn.hosts.join(" ")),
                ("CDN Servers", &cdn.servers.join(" ")),
                ("Tags", &tag_column),
                ("Version", &remote.versions_name),
            ],
        )?;
        info.save(&info_path)?;
        on_event(Event::BuildInfo {
            path: info_path,
            version: remote.versions_name.clone(),
        });

        // Battle.net will not adopt an install without `.product.db`: its
        // Locate flow reads the file and answers error 2800 when it is absent.
        // Written from `.build.info`, so the two can never disagree.
        if let Some(windows_path) = &options.windows_path {
            let record = crate::tact::productdb::ProductInstall::from_build_info(
                &info,
                code,
                windows_path,
            )?
            .with_countries(&options.account_country, &options.geo_country);
            let path = crate::tact::productdb::write(dest, &[record])?;
            on_event(Event::ProductDb { path });
        }
        Some(remote.versions_name.clone())
    } else {
        on_event(Event::Incomplete { reasons });
        None
    };

    let store = writer.finish()?;
    Ok(Summary { outcome, store, missing: plan.missing.len(), complete, version })
}

/// Prepare and execute in one call — the ordinary path.
pub async fn run(
    http: &Http,
    dest: &Path,
    code: &ProductCode,
    options: &Options,
    cancel: &Cancel,
    mut on_event: impl FnMut(Event),
) -> Result<Summary, InstallError> {
    let prepared = prepare(http, dest, code, options, &mut on_event).await?;
    execute(http, dest, code, options, prepared, cancel, &mut on_event).await
}

#[derive(Debug, thiserror::Error)]
pub enum InstallError {
    #[error(transparent)]
    Version(#[from] crate::tact::version::VersionError),

    #[error(transparent)]
    Cdn(#[from] crate::tact::cdn::CdnError),

    #[error(transparent)]
    Config(#[from] crate::tact::config::ConfigError),

    #[error(transparent)]
    Manifest(#[from] crate::tact::manifest::ManifestError),

    #[error(transparent)]
    Encoding(#[from] crate::tact::encoding::EncodingError),

    #[error(transparent)]
    Indices(#[from] crate::tact::indices::IndexError),

    #[error(transparent)]
    Archive(#[from] crate::tact::archive::ArchiveError),

    #[error(transparent)]
    Download(#[from] crate::tact::download::DownloadError),

    #[error(transparent)]
    Writer(#[from] crate::tact::writer::WriteError),

    #[error(transparent)]
    BuildInfo(#[from] crate::buildinfo::BuildInfoError),

    #[error(transparent)]
    ProductDb(#[from] crate::tact::productdb::ProductDbError),

    #[error("the {0} table is empty for this region")]
    EmptyTable(&'static str),

    #[error("the build config has no {0} manifest")]
    MissingManifest(&'static str),

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

    #[test]
    fn the_tag_column_matches_what_a_real_build_info_carries() {
        let options = Options { region: Region::Eu, locale: "enUS".into(), ..Options::default() };
        assert_eq!(
            options.tag_column(),
            "Windows x86_64 EU enUS speech:Windows x86_64 EU enUS text"
        );
    }

    #[test]
    fn any_limit_option_marks_the_run_as_deliberately_partial() {
        assert!(!Options::default().limited());
        assert!(Options { skip_loose: true, ..Options::default() }.limited());
        assert!(Options { limit_runs: Some(1), ..Options::default() }.limited());
        assert!(Options { limit_archives: Some(1), ..Options::default() }.limited());
    }
}
