//! Driving a whole install: version service -> plan -> download -> store.
//!
//! This is the orchestration both front-ends need, so it lives here rather than
//! in either of them. It reports through an [`Event`] callback instead of
//! printing: the CLI turns events into lines, and the GUI turns them into
//! messages, without either owning the sequence.
//!
//! The ordering is not arbitrary and is the part worth preserving:
//!
//! 1. Archive indices are cached first. Nothing can be *located* without them.
//! 2. Whatever the store already holds is subtracted from the plan, which is
//!    what makes an interrupted download resume rather than start over.
//! 3. `.build.info` is written **last, and only on a genuinely complete run**.
//!    [`crate::state::Local`] treats its presence as proof that a download
//!    finished, so writing it after a partial run would offer PLAY on a broken
//!    game. Every way a run can fall short is accounted for in
//!    [`Summary::complete`], failures, cancellation, unlocatable keys, an
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
use crate::tact::manifest::{DownloadEntry, DownloadManifest, InstallManifest, TagSet};
use crate::tact::plan::{reverse_encoding, Options as PlanOptions, Plan};
use crate::tact::storage::{EKey, Storage};
use crate::tact::version::{self, Region};
use crate::tact::writer::{StoreWriter, Summary as StoreSummary};

/// The locales a WoW build ships, as the download manifest spells them, with
/// something to call each one in a picker.
///
/// Not every product has every one, a Classic Era build ships fewer, but the
/// manifest decides that at plan time: a tag nothing defines selects nothing
/// rather than failing, so offering the full list is safe and the alternative
/// is reading a manifest before the user can pick a language.
pub const LOCALES: &[(&str, &str)] = &[
    ("enUS", "English (US)"),
    ("enGB", "English (EU)"),
    ("deDE", "Deutsch"),
    ("frFR", "Français"),
    ("esES", "Español (EU)"),
    ("esMX", "Español (AL)"),
    ("itIT", "Italiano"),
    ("ptBR", "Português (BR)"),
    ("ptPT", "Português (PT)"),
    ("ruRU", "Русский"),
    ("koKR", "한국어"),
    ("zhCN", "简体中文"),
    ("zhTW", "繁體中文"),
];

/// What a locale code is called, or the code itself when it is not one we know.
pub fn locale_name(code: &str) -> &str {
    LOCALES
        .iter()
        .find(|(locale, _)| *locale == code)
        .map(|(_, name)| *name)
        .unwrap_or(code)
}

/// How a run is tuned. The limits exist for development slices and must never
/// be set by a real install, each of them makes the result deliberately
/// partial, which is why [`Summary::complete`] refuses `.build.info` when any
/// is in play.
#[derive(Debug, Clone)]
pub struct Options {
    pub region: Region,
    /// Client locale, e.g. `enUS`. Selects which of a build's files are wanted.
    pub locale: String,
    /// Optional content to opt in to, by tag name, e.g. `HighRes`.
    ///
    /// Empty matches Battle.net, which installs optional content only when
    /// asked. See [`crate::tact::manifest::TagSet::with_optional`].
    pub optional_tags: Vec<String>,
    /// Flavor directory the install files are extracted into, e.g. `_retail_`.
    pub flavor: String,
    pub concurrency: usize,
    /// Cap on download speed in bytes per second; `0` is no cap.
    ///
    /// Bytes rather than a friendlier unit because this is the layer that
    /// spends them, the UI converts from whatever it shows the user.
    pub limit_bytes_per_second: u64,
    /// Ranges this far apart are merged into one request.
    pub gap_bytes: u32,
    /// Upper bound on the checkpoint interval; scaled down for small products.
    pub checkpoint_bytes: u64,
    /// Extract the install manifest's files (the game executable and friends).
    pub install_files: bool,
    /// Stop after this many archive indices, development only.
    pub limit_archives: Option<usize>,
    /// Truncate the plan to this many coalesced runs, development only.
    pub limit_runs: Option<usize>,
    /// Skip the loose (unarchived) files, development only.
    pub skip_loose: bool,
    /// The install's path as Windows sees it, e.g.
    /// `C:/Program Files (x86)/World of Warcraft`. `None` skips writing
    /// `.product.db`, which only matters for Battle.net adoption.
    pub windows_path: Option<String>,
    /// Agent's own directory, whose `product.db` decides whether Battle.net
    /// shows `Play` or `Install` for a product Lightforge downloaded.
    ///
    /// `None` skips the registration. There is nothing to merge into on a
    /// machine with no Battle.net, and a CLI run against a bare directory has
    /// no prefix to speak of.
    pub agent_dir: Option<PathBuf>,
    /// Account and GeoIP country, as Agent would record them. Empty is fine.
    pub account_country: String,
    pub geo_country: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            region: Region::default(),
            locale: "enUS".to_owned(),
            optional_tags: Vec::new(),
            flavor: String::new(),
            // 32 measured 2.4x the throughput of 8 on a real product: these
            // transfers are request-latency-bound, not bandwidth-bound.
            concurrency: 32,
            limit_bytes_per_second: 0,
            gap_bytes: 64 * 1024,
            checkpoint_bytes: 4096 * 1024 * 1024,
            install_files: true,
            limit_archives: None,
            limit_runs: None,
            skip_loose: false,
            windows_path: Some(crate::paths::WINDOWS_WOW_PATH.to_owned()),
            agent_dir: None,
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
    /// The install root refused a file and its owner-write bit was added so
    /// the run could proceed. Reported because it is a change to a directory
    /// the user owns, made without being asked.
    Repaired {
        dir: PathBuf,
    },
    /// Optional content this build offers, by tag name.
    Optional {
        names: Vec<String>,
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
    /// The product was merged into Agent's registry, so Battle.net lists it
    /// as installed without being asked to look for it.
    Registered {
        code: String,
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
    /// True only if this run produced a whole, launchable install, the same
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
    /// Optional content this build offers, opted in or not, what a UI can
    /// offer the user. Only knowable once the manifest has been read.
    pub optional: Vec<String>,
    /// The build's download manifest, kept so a report can attribute the
    /// selection back to the tags that produced it.
    pub download: DownloadManifest,
    /// Every file this build wants for the selected tags, before the store is
    /// consulted, the input the plan is a subtraction from.
    pub wanted: Vec<DownloadEntry>,
    /// The probe that decided what the store already holds.
    pub probe: StoreProbe,
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

/// The planner's "does the store already hold this?" test.
///
/// Named rather than a closure because it is the one piece of the plan that
/// can be wrong without anything looking wrong: every key it wrongly answers
/// `false` for becomes a file the plan re-fetches. [`crate::tact::explain`]
/// exists to cross-examine it, which means it has to be something a caller can
/// hold on to.
///
/// The set is built once from the store's indices and matched on the 9-byte
/// truncated key the indices store, so a lookup costs no I/O.
pub struct StoreProbe {
    present: HashSet<EKey>,
    /// False when the store could not be opened at all, which is a fresh
    /// install: every key is absent, and legitimately so.
    opened: bool,
}

impl StoreProbe {
    pub fn open(dest: &Path) -> Self {
        match Storage::open(dest) {
            Ok(store) => Self {
                present: store
                    .files()
                    .map(|e| {
                        let mut k = [0u8; 16];
                        k[..9].copy_from_slice(&e.key);
                        k
                    })
                    .collect(),
                opened: true,
            },
            Err(_) => Self { present: HashSet::new(), opened: false },
        }
    }

    pub fn has(&self, ekey: &EKey) -> bool {
        let mut probe = [0u8; 16];
        probe[..9].copy_from_slice(&ekey[..9]);
        self.present.contains(&probe)
    }

    /// Whether the store opened. A plan that fetches everything because the
    /// store would not open is a very different fact from one that fetches
    /// everything because the store is empty.
    pub fn opened(&self) -> bool {
        self.opened
    }

    pub fn len(&self) -> usize {
        self.present.len()
    }

    pub fn is_empty(&self) -> bool {
        self.present.is_empty()
    }
}

/// What a build is, and what it offers, without planning anything.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offerings {
    pub remote: version::Version,
    /// Optional content this build offers, by tag name.
    pub optional: Vec<String>,
    /// Files and encoded bytes the current tag selection wants, so a caller
    /// can say how big this product is without fetching a plan.
    pub wanted_files: usize,
    pub wanted_bytes: u64,
    /// Encoded bytes to fetch for the game with no optional content at all:
    /// all of it without a store, only what the store lacks with one.
    pub base_bytes: u64,
    /// What each optional download adds on top of [`Offerings::base_bytes`],
    /// counted the same way, in the order of [`Offerings::optional`], so an
    /// install dialog can price a switch before it is flipped.
    pub optional_bytes: Vec<(String, u64)>,
}

/// Resolve a build and read its download manifest. Nothing else.
///
/// Deliberately much cheaper than [`prepare`]: no encoding manifest, no
/// install manifest, and none of the ~1400 archive indices. That is the
/// difference between a button someone presses to see what is new and one
/// that downloads a hundred megabytes to answer.
///
/// Optional content is the reason this exists at all. Which extras a build
/// offers is knowable only from its download manifest, so without a call like
/// this the settings panel has nothing to list until a download has run.
///
/// With a `store`, the base and optional sizes leave out what it already
/// holds, the files another WoW version sharing the folder stored, by the
/// same test a real plan applies. The local indices are all it reads.
pub async fn offerings(
    http: &Http,
    code: &ProductCode,
    options: &Options,
    store: Option<&StoreProbe>,
) -> Result<Offerings, InstallError> {
    let base_tags = TagSet::parse(&options.tag_column());
    let tags = base_tags.clone().with_optional(options.optional_tags.clone());

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
    let pair = build.download.ok_or(InstallError::MissingManifest("download"))?;
    let ekey = pair.ekey.ok_or(InstallError::MissingManifest("download"))?;
    let manifest =
        DownloadManifest::parse(&client.fetch_loose(http, &ekey, &pair.ckey, None).await?)?;

    let missing = |ekey: &EKey| store.is_none_or(|probe| !probe.has(ekey));
    let bytes = |tags: &TagSet| {
        manifest.select(tags).iter().filter(|e| missing(&e.ekey)).map(|e| e.size).sum::<u64>()
    };
    let wanted = manifest.select(&tags);
    let optional = manifest.optional_names(&tags);
    let base_bytes = bytes(&base_tags);
    let optional_bytes = optional
        .iter()
        .map(|name| {
            let with = bytes(&base_tags.clone().with_optional([name.clone()]));
            (name.clone(), with.saturating_sub(base_bytes))
        })
        .collect();
    Ok(Offerings {
        remote,
        optional,
        wanted_files: wanted.len(),
        wanted_bytes: wanted.iter().map(|e| e.size).sum(),
        base_bytes,
        optional_bytes,
    })
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
    let tags = TagSet::parse(&tag_column).with_optional(options.optional_tags.clone());

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

    let optional = download_manifest.optional_names(&tags);
    on_event(Event::Optional { names: optional.clone() });

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

    // Skip whatever the store already holds. This is what makes an
    // interrupted download resume instead of starting over.
    let probe = StoreProbe::open(dest);
    let have = |ekey: &EKey| probe.has(ekey);

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

    Ok(Prepared {
        plan,
        optional,
        download: download_manifest,
        wanted,
        probe,
        remote,
        cdn,
        client,
        encoding,
        install,
        locations,
        loose,
        tags,
        tag_column,
    })
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
    let Prepared {
        plan, remote, cdn, client, encoding, install, locations, loose, tags, tag_column, ..
    } = prepared;

    // Before a byte is fetched, not after. The run's very last act is to write
    // `.build.info` *into the install root*, and a root that will not take a
    // new file fails there, hours in, with every downloaded byte already on
    // disk and the update still reported as failed. Writing into `Data/data`
    // and the flavour directory proves nothing about the root: those are
    // separate directories with their own permissions, and an install carried
    // over from another tool or another user is exactly where they differ.
    match crate::paths::ensure_writable(dest) {
        Ok(crate::paths::Repaired::Yes) => on_event(Event::Repaired { dir: dest.to_path_buf() }),
        Ok(crate::paths::Repaired::No) => {}
        Err(source) => return Err(InstallError::Unwritable { dir: dest.to_path_buf(), source }),
    }

    let mut writer = StoreWriter::create(dest.join("Data").join("data"))?;
    // The reader always carries a key service; the downloader must too, or
    // blte cannot even name a missing key properly.
    let keys = download::default_key_service();
    let source = Source { http, client: &client, keys: Some(&keys) };
    let downloader = Downloader::new(DownloadOptions {
        concurrency: options.concurrency,
        limit: download::RateLimit::new(options.limit_bytes_per_second),
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
        info.save(&info_path)
            .map_err(|source| InstallError::Metadata { what: ".build.info", source })?;
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
            let path = crate::tact::productdb::write(dest, std::slice::from_ref(&record))?;
            on_event(Event::ProductDb { path });

            // And into Agent's own registry, which is the list the client
            // builds its library from. Without this the tile reads `Install`
            // until the user asks Battle.net to locate the game, it adopts
            // fine, but only after a scan nobody should have to trigger.
            //
            // Skipped silently when there is no registry to merge into, or
            // when it cannot be read back: see `productdb::register`.
            if let Some(agent_dir) = &options.agent_dir {
                if crate::tact::productdb::register(agent_dir, std::slice::from_ref(&record))? {
                    on_event(Event::Registered { code: code.as_str().to_owned() });
                }
            }
        }
        Some(remote.versions_name.clone())
    } else {
        on_event(Event::Incomplete { reasons });
        None
    };

    let store = writer.finish()?;
    Ok(Summary { outcome, store, missing: plan.missing.len(), complete, version })
}

/// Prepare and execute in one call, the ordinary path.
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

    /// The content is downloaded and extracted and only the bookkeeping is
    /// left. Its own variant because "i/o error at <the install root>" says
    /// nothing about which of the half-dozen things that touch that directory
    /// was being written, and this is the one the user has waited longest for.
    #[error("{dir} will not accept new files, so this install cannot be finished")]
    Unwritable {
        dir: PathBuf,
        #[source]
        source: std::io::Error,
    },

    #[error("the download finished but {what} could not be written")]
    Metadata {
        what: &'static str,
        #[source]
        source: crate::buildinfo::BuildInfoError,
    },

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
