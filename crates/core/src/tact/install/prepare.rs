//! Everything before the first byte: the version service, the configs and
//! manifests, what the store already holds, and the plan.

use std::collections::HashSet;
use std::path::Path;

use super::locale::shipped_locale;
use super::{Event, InstallError, Options};
use crate::net::Http;
use crate::product::ProductCode;
use crate::tact::archive::FileIndex;
use crate::tact::cdn::Client;
use crate::tact::config::{BuildConfig, CdnConfig, KeyPair};
use crate::tact::encoding::Encoding;
use crate::tact::indices::{IndexCache, Locations};
use crate::tact::manifest::{DownloadEntry, DownloadManifest, InstallManifest, TagSet};
use crate::tact::plan::{reverse_encoding, Options as PlanOptions, Plan};
use crate::tact::storage::{EKey, Storage};
use crate::tact::version;

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
/// Much cheaper than [`prepare`] (no encoding or install manifest, none of the
/// ~1400 archive indices), and the only way to know which optional content a
/// build offers before downloading. With a `store`, sizes leave out what it
/// already holds, by the same test a real plan applies.
pub async fn offerings(
    http: &Http,
    code: &ProductCode,
    options: &Options,
    store: Option<&StoreProbe>,
) -> Result<Offerings, InstallError> {
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

    let locale = shipped_locale(&options.locale, manifest.tags());
    let base_tags = TagSet::parse(&options.tag_column_for(&locale));
    let tags = base_tags.clone().with_optional(options.optional_tags.clone());

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
    let versions = version::versions(http, options.region, code.as_str()).await?;
    let remote = version::for_region(&versions, options.region, |v| &v.region)
        .cloned()
        .ok_or(InstallError::EmptyTable("versions"))?;
    let cdns = version::cdns(http, options.region, code.as_str()).await?;
    let cdn = version::for_region(&cdns, options.region, |c| &c.name)
        .cloned()
        .ok_or(InstallError::EmptyTable("cdns"))?;
    let client = Client::from_cdn(&cdn)?;

    let build_text = client.fetch_config(http, &remote.build_config).await?;
    let cdn_text = client.fetch_config(http, &remote.cdn_config).await?;
    // Into the store on the way past. These two are the only part of an
    // install that is not inside CASC, the client needs both before it can
    // start, and it fetches them itself when they are missing: about 1.7
    // seconds on the first launch of a new build, for bytes we are holding
    // right here and have already checked against their hashes. Not fatal if
    // it fails, the client's own fetch is the fallback and always was.
    let data_dir = dest.join("Data");
    for (hash, text) in
        [(&remote.build_config, &build_text), (&remote.cdn_config, &cdn_text)]
    {
        if let Err(e) = crate::tact::config::store(&data_dir, hash, text.as_bytes()) {
            tracing::warn!("could not seed {hash} into the store: {e}");
        }
    }
    let build = BuildConfig::parse(&build_text)?;
    let cdn_config = CdnConfig::parse(&cdn_text)?;
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
    let locale = shipped_locale(&options.locale, download_manifest.tags());
    let tag_column = options.tag_column_for(&locale);
    let tags = TagSet::parse(&tag_column).with_optional(options.optional_tags.clone());
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
    if !cache.holds(&file_index_hash, cdn_config.file_index_size) {
        let bytes = client
            .fetch_index(http, &file_index_hash, cdn_config.file_index_size)
            .await?;
        cache.store(&file_index_hash, &bytes)?;
    }
    let loose = FileIndex::parse(&std::fs::read(cache.path_of(&file_index_hash)).map_err(
        |source| InstallError::Io { path: cache.path_of(&file_index_hash), source },
    )?)?;
    on_event(Event::Loose { files: loose.len() });

    let wanted_keys: HashSet<EKey> = wanted.iter().map(|e| e.ekey).collect();
    let ckeys = reverse_encoding(&encoding, &wanted_keys);
    let mut locations = Locations::default();
    for (archive, _) in &archives {
        if !cache.is_cached(archive, None) {
            continue;
        }
        // One archive that could not be fetched intact costs its own files,
        // which then show up as unlocatable, not the whole download.
        match cache.load_archive(archive) {
            Ok(index) => {
                locations.absorb(&index, &wanted_keys);
            }
            Err(e) => tracing::warn!("skipping archive index {archive}: {e}"),
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
