//! The work the dashboard hands off: disk inspection, the version service, the
//! asset pipeline, downloads, and launching.
//!
//! Everything here returns owned data and never borrows application state, so
//! each one can be driven by `Task::perform` without the update loop holding a
//! lock across an await.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::widget::image::Handle;
use lightforge_core::backup;
use lightforge_core::advice::{advise, Advice};
use lightforge_core::error::chain;
use lightforge_core::assets::{Manifest, Pipeline};
use lightforge_core::config::Config;
use lightforge_core::net::Http;
use lightforge_core::news;
use lightforge_core::families::Families;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::preflight::{DownloadGuard, GameProcess, Preflight, PreflightError};
use lightforge_core::product::ProductCode;
use lightforge_core::state::Local;
use lightforge_core::tact::download::{Cancel, Downloads, Progress};
use lightforge_core::tact::install::{self, Event, Options as InstallOptions, Summary};
use lightforge_core::tact::version::{self, Region, Version};
use lightforge_core::tweaks::{HostTools, Tweaks};

use crate::art::Art;

/// An error on its way to the screen: what went wrong, and what to do about
/// it when that is known.
///
/// Built with [`Problem::of`] while the error still has its type, because
/// that is the only point at which [`advise`](lightforge_core::advice::advise)
/// can tell a full drive from a dropped connection. A plain `String` converts
/// into one with no advice, so a path that has not been taught any yet still
/// reports exactly what it did before.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub message: String,
    pub advice: Option<Advice>,
}

impl Problem {
    pub fn of(error: &(dyn std::error::Error + 'static)) -> Self {
        Self { message: chain(error), advice: advise(error) }
    }
}

impl From<String> for Problem {
    fn from(message: String) -> Self {
        Self { message, advice: None }
    }
}

impl From<&str> for Problem {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// The message alone. The advice is for the screen; the status history and
/// a bug report want what actually went wrong.
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// One product's situation, as of a refresh.
#[derive(Debug, Clone)]
pub struct Refreshed {
    pub local: Local,
    pub remote: Option<Version>,
    /// Why the remote half is missing, if it is. Not an error state: an
    /// unreachable version service must not stop a complete install launching.
    pub offline: Option<String>,
}

/// Returned in place of a network error when the shared client never built.
pub const NO_HTTP: &str = "no HTTP client available";

/// Read the disk and, when `check_remote`, ask the version service what is
/// current.
///
/// `check_remote` is `false` for a product Blizzard does not serve us builds
/// for: the probe can only fail there, and would report itself as the version
/// service being down.
pub async fn refresh(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    region: Region,
    check_remote: bool,
) -> Refreshed {
    let local = {
        let layout = layout.clone();
        let code = code.clone();
        // Opening the store to count files is an index read, but on a 123 GB
        // install it is still tens of milliseconds, off the UI thread.
        tokio::task::spawn_blocking(move || Local::inspect(&layout, &code))
            .await
            .unwrap_or_default()
    };

    if !check_remote {
        return Refreshed { local, remote: None, offline: None };
    }

    let (remote, offline) = match &http {
        Some(http) => match version::versions(http, region, code.as_str()).await {
            Ok(rows) => (
                version::for_region(&rows, region, |v| &v.region).cloned(),
                None,
            ),
            Err(e) => (None, Some(chain(&e))),
        },
        None => (None, Some(NO_HTTP.to_owned())),
    };

    Refreshed { local, remote, offline }
}

/// What a build is and what it offers, for the Check for Updates action.
///
/// Much lighter than planning: the download manifest only, so pressing the
/// button costs one manifest rather than the whole prelude a download needs.
pub async fn offerings(
    http: Option<Http>,
    code: ProductCode,
    region: Region,
    optional_tags: Vec<String>,
    store: Option<PathBuf>,
) -> Result<lightforge_core::tact::install::Offerings, String> {
    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let options = InstallOptions {
        region,
        flavor: code.install_dir().into_owned(),
        optional_tags,
        ..InstallOptions::default()
    };
    // Reading the indices is disk work, so it stays off the runtime's
    // threads; a store that will not open is priced as empty.
    let probe = match store {
        Some(dir) => tokio::task::spawn_blocking(move || install::StoreProbe::open(&dir))
            .await
            .ok(),
        None => None,
    };
    install::offerings(&http, &code, &options, probe.as_ref())
        .await
        .map_err(|e| chain(&e))
}

/// What a scan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Scanned {
    pub checked: u64,
    pub damaged: u64,
    /// Entries dropped from the indices, which the next download refetches.
    pub removed: usize,
}

/// Verify the store's entries against their own headers, and drop the bad ones.
///
/// Repair rewrites the indices a running client memory-maps, so the prefix
/// has to be quiet first. A game or an open Battle.net is the user's to
/// close; anything else, a `wineserver` waiting out its timeout or a launch's
/// leftovers, is put to rest here exactly as a launch's pre-flight would.
pub async fn scan_and_repair(
    layout: Layout,
    code: ProductCode,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<Scanned, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::storage::Storage;
        use lightforge_core::tact::verify;

        use lightforge_core::preflight;

        if let Some(game) = preflight::games_running(&layout).first() {
            return Err(format!("close {} first — the store is in use", game.name));
        }
        if preflight::client_running(&layout) {
            return Err("close Battle.net first — the store is in use".to_owned());
        }
        if !preflight::scan(&layout).is_empty() {
            let runner = runner_for(&layout, &code, Runs::Game)?;
            preflight::shutdown(&runner, &layout, preflight::DEFAULT_GRACE)
                .map_err(|e| chain(&e))?;
        }

        let install = layout.wow_install(&code);
        let store = Storage::open(install.join("Data")).map_err(|e| chain(&e))?;
        let report = verify::scan(&store, |progress| {
            let _ = tx.send(progress);
        });

        let removed = match report.is_healthy() {
            true => 0,
            false => verify::repair(&install.join("Data").join("data"), &report.damaged)
                .map_err(|e| chain(&e))?,
        };
        Ok(Scanned {
            checked: report.checked,
            damaged: report.damaged.len() as u64,
            removed,
        })
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Hash every stored byte against its chunk table and key.
///
/// Only reads, so unlike [`scan_and_repair`] it does not wait for the game to
/// close; the caller holds the download queue for as long as it runs, and
/// the core refuses a result from a store that changed underneath it anyway.
pub async fn deep_scan(
    layout: Layout,
    code: ProductCode,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<lightforge_core::tact::verify::Report, String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::tact::verify::deep_scan(layout.wow_install(&code), &cancel, |progress| {
            let _ = tx.send(progress);
        })
        .map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Why a repair did not happen.
#[derive(Debug, Clone)]
pub enum Unrepaired {
    /// Something is alive in the prefix; the repair is still owed.
    InUse,
    /// Anything else, including a store that moved since the scan, whose
    /// findings are no longer worth acting on.
    Failed(String),
}

/// Drop what a deep scan found damaged, if nothing is using the store and it
/// is still the store that was scanned.
pub async fn repair(
    layout: Layout,
    code: ProductCode,
    report: lightforge_core::tact::verify::Report,
) -> Result<usize, Unrepaired> {
    tokio::task::spawn_blocking(move || {
        if !lightforge_core::preflight::scan(&layout).is_empty() {
            return Err(Unrepaired::InUse);
        }
        let data_dir = layout.wow_install(&code).join("Data").join("data");
        lightforge_core::tact::verify::repair_unchanged(&data_dir, &report)
            .map_err(|e| Unrepaired::Failed(chain(&e)))
    })
    .await
    .map_err(|e| Unrepaired::Failed(chain(&e)))?
}

/// Branding straight off disk, for the frame before [`theme`] answers.
///
/// Network-free and typically a few tens of milliseconds, so on every run
/// after the first the dashboard opens already themed instead of showing its
/// unthemed card for the length of a revalidation.
pub async fn cached_theme(
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
    code: ProductCode,
) -> Themed {
    let Some(pipeline) = pipeline else {
        return Themed::default();
    };

    // Reads and hashes every image it returns; off the UI thread.
    tokio::task::spawn_blocking(move || {
        let branding = pipeline.cached(&code).map(Art::decode);
        let manifest = pipeline.cached_manifest();
        let icons = match &manifest {
            Some(manifest) => cached_icons(&pipeline, manifest, &codes),
            None => HashMap::new(),
        };
        let warm = manifest.as_ref().is_some_and(|manifest| {
            let wants_art = manifest.slot(&code, |t| t.logo.as_ref()).is_some()
                && manifest.slot(&code, |t| t.background.as_ref()).is_some();
            let wants_icons = codes
                .iter()
                .filter(|c| manifest.own_theme(c).is_some_and(|t| t.icon.is_some()))
                .count();
            (!wants_art || branding.is_some()) && icons.len() == wants_icons
        });
        Themed { manifest: manifest.map(Arc::new), branding, icons, warm }
    })
    .await
    .unwrap_or_default()
}

/// Every product's cached icon, skipping the ones that aren't there yet.
fn cached_icons(
    pipeline: &Pipeline,
    manifest: &Manifest,
    codes: &[ProductCode],
) -> HashMap<ProductCode, Mark> {
    codes
        .iter()
        .filter_map(|code| {
            let icon = pipeline.cached_icon(manifest, code)?;
            Some((code.clone(), Mark::decode(icon.path)))
        })
        .collect()
}

/// A theming pass: the branding it produced, the manifest it read, and every
/// configured product's bar icon.
///
/// The manifest comes back so the app can hold onto it; see [`theme`]. The
/// icons ride along because the pass already holds the one manifest that names
/// them all, and the product bar needs the whole set, not just the selected
/// product's.
#[derive(Debug, Default, Clone)]
pub struct Themed {
    pub manifest: Option<Arc<Manifest>>,
    pub branding: Option<Art>,
    pub icons: HashMap<ProductCode, Mark>,
    /// Set only by the cache-only pass: everything the first screen draws
    /// was on disk, the selected game's art and every product's mark, so the
    /// splash has nothing left to wait for.
    pub warm: bool,
}

/// A product's decoded mark, beside the file it came from.
///
/// The path is kept for one reason: to tell whether a theming pass produced
/// *different* marks or the same ones again. [`Handle::from_path`] used to
/// make that free, because iced caches those by path and reassigning one costs
/// nothing. A handle built from pixels is a fresh id every time, so assigning
/// an identical set makes the renderer re-upload every texture, which is the
/// band blinking after a switch. Comparing paths answers it exactly, the same
/// way [`Art`] compares its [`lightforge_core::assets::Branding`].
#[derive(Debug, Clone)]
pub struct Mark {
    pub path: PathBuf,
    pub handle: Handle,
}

impl Mark {
    fn decode(path: PathBuf) -> Self {
        let handle = crate::art::mark(&path);
        Self { path, handle }
    }
}

/// Whether two icon sets name the same files, regardless of handle identity.
pub fn same_marks(a: &HashMap<ProductCode, Mark>, b: &HashMap<ProductCode, Mark>) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(code, mark)| b.get(code).is_some_and(|other| other.path == mark.path))
}

/// Fetch every product's artwork into the cache, without decoding any of it.
///
/// The first run's "it looks weird for the first second or two on every game"
/// fix: a panel's background and wordmark are revalidated the first time it is
/// opened, and until they land the card is empty. Pulling them all down while
/// the wizard's first slide is on screen means every panel opens from disk.
///
/// Deliberately not decoded and not kept: decoding thirteen 4K backgrounds to
/// hold them all in memory would cost more than it saves, and [`cached_theme`]
/// reads them off disk per product in tens of milliseconds. Returns how many
/// products now have art cached.
/// Nothing, after a wait. The one thing a `Task` cannot express on its own.
pub async fn after(delay: std::time::Duration) {
    tokio::time::sleep(delay).await;
}

/// [`prefetch`] under a deadline.
///
/// A start must not be hostage to a network that never answers. Whatever
/// landed inside the budget stays cached; the rest is fetched on demand, which
/// is what every start did before the splash existed.
/// Ask GitHub whether a newer Lightforge exists.
///
/// Runs beside the splash's artwork fetch, which is where the launcher is
/// already waiting on the network, and answers `None` for every reason there
/// is not to interrupt someone: the check is switched off, there is no
/// connection, GitHub is unhappy, the list does not parse, or this is already
/// the newest build. A failure here is not worth a word on screen; the user
/// asked to start a launcher, not to audit a release feed.
pub async fn check_for_update(
    http: Option<Http>,
    enabled: bool,
) -> Option<lightforge_core::update::Available> {
    use lightforge_core::net::ETagCache;
    use lightforge_core::update;

    if !enabled {
        return None;
    }
    let http = http?;
    let dirs = AppDirs::discover().ok()?;
    // Shares GitHub's 60-per-hour unauthenticated budget with the Proton-GE
    // check, so the ETag is what keeps a start from costing two of it.
    let cache = ETagCache::new(dirs.asset_cache_dir().join("lightforge-releases.json"));
    let releases = update::fetch(&http, &cache).await.ok()?;
    let found = update::newer_than(env!("CARGO_PKG_VERSION"), &releases);
    crate::trace::mark(match &found {
        Some(available) => format!("update: {} is out", available.latest),
        None => "update: this is the newest build".to_owned(),
    });
    found
}

/// How many products a prefetch fetches at once.
const PREFETCH_AT_ONCE: usize = 4;

pub async fn prefetch_within(
    budget: std::time::Duration,
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
) -> usize {
    match tokio::time::timeout(budget, prefetch(http, pipeline, codes)).await {
        Ok(cached) => cached,
        Err(_) => {
            crate::trace::mark("prefetch: out of time, opening anyway");
            0
        }
    }
}

pub async fn prefetch(
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    codes: Vec<ProductCode>,
) -> usize {
    let (Some(http), Some(pipeline)) = (http, pipeline) else {
        return 0;
    };
    let manifest = match pipeline.manifest(&http).await {
        Ok(manifest) => manifest,
        Err(e) => {
            crate::trace::mark(format_args!("prefetch: no manifest ({e})"));
            return 0;
        }
    };
    use futures_util::StreamExt;

    // A few products at a time. This only runs with art missing, usually a
    // first run, where everything is a real download: one at a time made the
    // splash the sum of every transfer, and all at once would open a dozen
    // connections to one host (no HTTP/2 here) fighting over a slow link.
    //
    // Each future owns its handles: borrowing them across `buffer_unordered`
    // trips the compiler's higher-ranked `Send` check on the task.
    let manifest = Arc::new(manifest);
    let total = codes.len();
    let cached = futures_util::stream::iter(codes)
        .map(|code| {
            let (http, pipeline, manifest) = (http.clone(), pipeline.clone(), Arc::clone(&manifest));
            async move {
                match pipeline.resolve_with(&http, &manifest, &code).await {
                    // Branding carries the icon; asking again is a round trip.
                    Ok(_) => true,
                    // A hand-off has a mark and no art.
                    Err(_) => {
                        let _ = pipeline.icon(&http, &manifest, &code).await;
                        false
                    }
                }
            }
        })
        .buffer_unordered(PREFETCH_AT_ONCE)
        .filter(|resolved| std::future::ready(*resolved))
        .count()
        .await;
    crate::trace::mark(format_args!("prefetched art for {cached}/{total} products"));
    cached
}

/// Resolve a product's branding, or an empty [`Themed`] when theming isn't
/// configured or the manifest can't be read.
///
/// The manifest is fetched once per session rather than once per product
/// switch: one document describes every product, so re-asking for it on each
/// switch buys a round trip and nothing else. Pass the cached copy back in and
/// only the two images are revalidated. The app drops its copy when the asset
/// source changes, which is the only thing that can invalidate it mid-session.
pub async fn theme(
    http: Option<Http>,
    pipeline: Option<Pipeline>,
    manifest: Option<Arc<Manifest>>,
    codes: Vec<ProductCode>,
    code: ProductCode,
) -> Themed {
    let (Some(http), Some(pipeline)) = (http, pipeline) else {
        crate::trace::mark("theme skipped: no client or no asset source configured");
        return Themed::default();
    };

    let manifest = match manifest {
        Some(cached) => cached,
        None => match pipeline.manifest(&http).await {
            Ok(fresh) => {
                // Only on a fresh manifest, which is the only moment the full
                // set of live URLs is known. Published art is
                // content-addressed, so a changed image arrives under a new
                // URL and the old cache file would otherwise sit there
                // forever, a few megabytes per revision.
                let removed = pipeline.prune_cache(&fresh);
                if removed > 0 {
                    crate::trace::mark(format_args!("pruned {removed} stale cached assets"));
                }
                Arc::new(fresh)
            }
            Err(e) => {
                crate::trace::mark(format_args!("manifest fetch failed: {e}"));
                return Themed::default();
            }
        },
    };

    // A product missing from the manifest is not a reason to throw the
    // manifest away, so the branding half fails on its own.
    let branding = match pipeline.resolve_with(&http, &manifest, &code).await {
        Ok(branding) => Some(branding),
        Err(e) => {
            crate::trace::mark(format_args!("branding failed for {}: {e}", code.as_str()));
            None
        }
    };

    // Every product's mark, not just the selected one's: the bar shows them
    // all at once. They are small and revalidate to a 304, so the whole set
    // costs about what one image does.
    let mut icons = HashMap::new();
    for product in &codes {
        if let Ok(Some(icon)) = pipeline.icon(&http, &manifest, product).await {
            // Scaled here rather than handed over as a path: the band draws
            // these at 40pt, and letting the renderer reduce a 256px image
            // that far is what made them look pixelated.
            icons.insert(product.clone(), Mark::decode(icon.path));
        }
    }

    // Decoded here, off the UI thread, so the dashboard never decodes on
    // draw; see [`crate::art`].
    let branding = match branding {
        Some(branding) => tokio::task::spawn_blocking(move || Art::decode(branding)).await.ok(),
        None => None,
    };

    Themed { manifest: Some(manifest), branding, icons, warm: false }
}

/// What a running transfer publishes for the UI to read.
///
/// A `watch` channel deliberately keeps only the newest value: the download
/// calls its progress callback once per stored file, 1.5 million times on
/// retail, and the UI samples at its own rate. Nothing queues, so the
/// transfer can never be slowed by a busy window.
#[derive(Debug, Clone, Default)]
pub struct TransferState {
    pub stage: String,
    pub progress: Progress,
    /// Bytes this run's plan has to fetch; `None` until the plan is made.
    ///
    /// The plan excludes everything already in the store, so this is the
    /// size of the job, not of the product: an update to a 90 GB install
    /// that changes 10 GB plans 10 GB.
    pub remaining: Option<u64>,
    /// Optional content this build offers, as the manifest named it. Carried
    /// back so the settings panel has something real to offer rather than a
    /// hardcoded guess.
    pub optional: Vec<String>,
}

/// Download `code` into its install directory, publishing progress to `tx`.
///
/// Returns the run's summary. A paused transfer is not an error: `pause` trips
/// the same cancel token the guard uses, and the next run resumes from what
/// the store already holds.
// Eight arguments, all of them a distinct input to one transfer; a struct
// here would only move the same list one level out.
#[allow(clippy::too_many_arguments)]
pub async fn download(
    layout: Layout,
    code: ProductCode,
    region: Region,
    locale: String,
    limit_bytes_per_second: u64,
    optional_tags: Vec<String>,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<TransferState>,
) -> Result<Summary, Problem> {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();

    // The download runs on its own thread with its own runtime, and the result
    // comes back over a channel.
    //
    // This is not ceremony: the fetch loop holds borrowed work items across
    // await points, so its future is not provably `Send`, and `Task::perform`
    // requires `Send`. `block_on` imposes no such bound. The CLI has always run
    // the whole fetch as a single task regardless of its runtime's threads, so
    // nothing about throughput changes here.
    std::thread::Builder::new()
        .name("lightforge-download".to_owned())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(e) => {
                    let _ = done_tx.send(Err(Problem::of(&e)));
                    return;
                }
            };

            let result = runtime.block_on(async move {
                // Deliberately not the app's shared client: this runtime is
                // built here and torn down with the thread, and a reqwest
                // client's connections belong to the runtime that opened them.
                let http = Http::new().map_err(|e| Problem::of(&e))?;
                let dest = layout.wow_install(&code);
                // The library's own path, not the default one. `.product.db`
                // records where Battle.net should look, and a product on an
                // added drive lives under that drive's letter; writing
                // `C:\Program Files (x86)\World of Warcraft` for it points
                // Battle.net at the default library, where this install is
                // not, and it answers that the folder holds no WoW files.
                // `uninstall` already resolves it this way.
                let windows_path = layout
                    .library(&code)
                    .map(|library| library.windows_wow_path())
                    .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned());
                let options = InstallOptions {
                    region,
                    locale,
                    flavor: code.install_dir().into_owned(),
                    optional_tags: optional_tags.clone(),
                    // Tighter than the CLI's 4 GiB. A checkpoint is a few
                    // milliseconds of index writing, and it bounds what is
                    // lost if the app is killed rather than closed, the CLI
                    // is a foreground process someone is watching, a launcher
                    // is not.
                    checkpoint_bytes: 512 * 1024 * 1024,
                    limit_bytes_per_second,
                    // So Battle.net lists the product as installed rather than
                    // offering to install what is already there.
                    agent_dir: Some(layout.agent_dir()),
                    windows_path: Some(windows_path),
                    ..InstallOptions::default()
                };

                // Coalesce here rather than in core: stages are rare and always
                // worth publishing, progress is not.
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut stage = String::from("preparing");
                let mut remaining = None;
                let mut optional: Vec<String> = Vec::new();
                // The last figures the downloader actually reported. A stage
                // event carries no progress of its own, and publishing a
                // default one in its place is what emptied the bar and dropped
                // the rate to zero the moment the bytes were in: the tail of
                // every run, extracting, .build.info, .product.db, is
                // nothing but stage events.
                let mut latest = Progress::default();

                let on_event = move |event: Event| {
                    let progress = match &event {
                        Event::Progress(p) => {
                            if last.elapsed() < Duration::from_millis(50) {
                                return;
                            }
                            last = Instant::now();
                            latest = *p;
                            *p
                        }
                        other => {
                            if let Event::Planned { bytes, .. } = other {
                                remaining = Some(*bytes);
                            }
                            if let Event::Optional { names } = other {
                                optional = names.clone();
                            }
                            stage = describe(other);
                            latest
                        }
                    };
                    let _ = tx.send(TransferState {
                        stage: stage.clone(),
                        progress,
                        remaining,
                        optional: optional.clone(),
                    });
                };

                install::run(&http, &dest, &code, &options, &cancel, on_event)
                    .await
                    .map_err(|e| Problem::of(&e))
            });
            let _ = done_tx.send(result);
        })
        .map_err(|e| Problem::of(&e))?;

    done_rx.await.map_err(|_| "the download thread stopped unexpectedly".to_owned())?
}

/// One short line describing a non-progress event, for the status readout.
fn describe(event: &Event) -> String {
    match event {
        Event::Build { versions_name, .. } => format!("build {versions_name}"),
        Event::Repaired { dir } => {
            format!("{} would not take a file; added owner write", dir.display())
        }
        Event::Encoding { entries } => format!("encoding — {entries} entries"),
        Event::Manifests { download, .. } => format!("manifests — {download} entries"),
        // The whole product's size, not the job's, an update would flash
        // "90 GiB" before planning says 10. `Planned` states what is fetched.
        Event::Wanted { files, .. } => format!("checking {files} files against the store"),
        Event::Optional { names } => match names.is_empty() {
            true => "no optional content offered".to_owned(),
            false => format!("optional content available: {}", names.join(", ")),
        },
        Event::Indices(step) => format!("archive indices {}/{}", step.done, step.total),
        Event::IndicesDone { cached, downloaded, .. } => {
            format!("indices ready — {cached} cached, {downloaded} fetched")
        }
        Event::Loose { files } => format!("{files} loose files"),
        Event::Located { located, wanted } => format!("located {located}/{wanted}"),
        Event::Planned { requests, bytes, .. } => {
            format!("planned {requests} requests, {:.1} GiB", gib(*bytes))
        }
        Event::Progress(_) => String::new(),
        Event::InstallFile { path, .. } => format!("extracting {}", path.display()),
        Event::Extracted { files, .. } => format!("extracted {files} install files"),
        Event::BuildInfo { version, .. } => format!("finished — {version}"),
        Event::ProductDb { .. } => "writing .product.db".to_owned(),
        Event::Registered { .. } => "registered with Battle.net".to_owned(),
        Event::Incomplete { reasons } => format!("incomplete: {}", reasons.join(", ")),
    }
}

fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1024f64.powi(3)
}

/// `Downloads` behind a lock, so pre-flight can pause a transfer the UI owns.
///
/// A launch must not let Agent and our downloader write the same CASC
/// files.
#[derive(Debug, Clone, Default)]
pub struct SharedDownloads(pub Arc<Mutex<Downloads>>);

impl SharedDownloads {
    pub fn register(&self, code: ProductCode, cancel: Cancel) {
        if let Ok(mut guard) = self.0.lock() {
            guard.register(code, cancel);
        }
    }

    pub fn finished(&self, code: &ProductCode) {
        if let Ok(mut guard) = self.0.lock() {
            guard.finished(code);
        }
    }

    /// Products a launch paused and owes a restart.
    pub fn pending(&self) -> Vec<ProductCode> {
        self.0
            .lock()
            .map(|g| g.pending().cloned().collect())
            .unwrap_or_default()
    }
}

impl DownloadGuard for SharedDownloads {
    fn pause(&mut self, code: &ProductCode) -> Result<bool, PreflightError> {
        let mut guard = self.0.lock().expect("downloads lock");
        guard.pause(code)
    }

    fn resume(&mut self, code: &ProductCode) -> Result<bool, PreflightError> {
        // The GUI restarts the transfer itself, since resuming means building
        // a fresh plan against what the store now holds.
        let mut guard = self.0.lock().expect("downloads lock");
        guard.resume(code)
    }
}

/// Launch the game directly under Proton, the `[ PLAY ]` path.
///
/// Blocking from start to finish: pre-flight spawns processes and the returned
/// future only resolves when the game exits.
/// Everything that must be settled before a launch: no stale processes in the
/// prefix, no transfer mid-flight.
///
/// Split from the launch itself because the two take wildly different amounts
/// of time, this is seconds, the launch is however long someone plays, and a
/// UI that cannot tell them apart ends up claiming it is still doing pre-flight
/// an hour into a raid.
pub async fn preflight(
    layout: Layout,
    code: ProductCode,
    downloads: SharedDownloads,
    runs: Runs,
) -> Result<(), Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The same build the launch will use. The prefix is shared, so
        // clearing it with one Proton and then launching under another is how
        // you get an unasked-for prefix upgrade between the two.
        let runner = runner_for(&layout, &code, runs)?;
        let mut guard = downloads;
        let config = AppDirs::discover()
            .ok()
            .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
            .unwrap_or_default();
        let started = Instant::now();
        let report = Preflight::new(config.prefix_user.clone())
            .with_managed(config.managed_products())
            .run(&runner, &layout, Some(&code), &mut guard)
            .map_err(|e| Problem::of(&e))?;
        // The report is for the CLI, which prints it. Here the outcome is
        // binary, the prefix is ready or the launch does not happen, but the
        // stage timings are worth tracing: a launch that feels stuck is one
        // of them, and this says which.
        crate::trace::mark(format!(
            "preflight: {} ms total, {} process(es)",
            started.elapsed().as_millis(),
            report.processes.len()
        ));
        for timing in &report.timings {
            crate::trace::mark(format!(
                "preflight:   {:>6} ms  {}",
                timing.took.as_millis(),
                timing.stage
            ));
        }
        Ok(())
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// How long the game itself gets to appear in the prefix before the watch
/// gives up and falls back to waiting on the whole process tree.
///
/// Generous on purpose: a cold start on a mechanical drive, or a first launch
/// that compiles pipelines before it opens a window, is still a launch. The
/// fallback is correct either way. It is only slower to notice.
const GAME_APPEAR_WITHIN: Duration = Duration::from_secs(180);

/// Counts launches, so a wind-down from a finished session can tell whether a
/// newer one has started since.
///
/// The hazard is narrow and real: the tail of session one can still be
/// unwinding while session two is spawning, and a prefix scan taken in the
/// seconds before the new game registers a process looks exactly like an empty
/// one. Killing the server there would take the starting game with it.
static LAUNCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How often the prefix is re-scanned while waiting for the game to appear.
///
/// 60 ms a scan, measured, so this is well under a percent of a core.
const WATCH_POLL: Duration = Duration::from_millis(500);

/// How often it is re-scanned once the game is up.
///
/// This interval *is* the delay between the window closing and PLAY coming
/// back, so it is worth more than the waiting-to-start one is: a scan every
/// 150 ms is about 0.4% of a core while someone plays, which buys back a
/// third of a second of a button that looks stuck.
const EXIT_POLL: Duration = Duration::from_millis(150);

/// Where this game's settings backups go, and how many it keeps.
pub fn settings_backup(layout: &Layout, dirs: &AppDirs, code: &ProductCode) -> backup::Plan {
    let keep = Config::load_or_default(&dirs.config_file())
        .map_or(backup::DEFAULT_KEEP, |config| config.backup_keep);
    backup::Plan::for_product(layout, dirs, code, keep)
}

/// A game's settings backups, newest first.
pub async fn list_backups(layout: Layout, code: ProductCode) -> Result<Vec<backup::Backup>, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        settings_backup(&layout, &dirs, &code).list().map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Back up a game's settings now, answering with the sentence to show.
pub async fn create_backup(layout: Layout, code: ProductCode) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        Ok(match plan.create(backup::Reason::Manual).map_err(|e| chain(&e))? {
            Some(_) => "Backed up.".to_owned(),
            None if plan.account_dir.is_dir() => "Nothing changed since the last backup.".to_owned(),
            None => "Nothing to back up yet: the game has not been started.".to_owned(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Put one backup back. Refused while a game runs: the client writes
/// SavedVariables when it exits and would overwrite the restore.
pub async fn restore_backup(layout: Layout, code: ProductCode, archive: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        if !lightforge_core::preflight::games_running(&layout).is_empty() {
            return Err("Close the game first, it would overwrite the restore when it exits.".to_owned());
        }
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        let backups = plan.list().map_err(|e| chain(&e))?;
        let chosen = backups
            .iter()
            .find(|b| b.path == archive)
            .ok_or("That backup is no longer there.")?;
        plan.restore(chosen).map_err(|e| chain(&e))?;
        Ok(format!("Restored the backup from {}.", chosen.when()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete one backup, answering with the sentence to show.
pub async fn delete_backup(layout: Layout, code: ProductCode, archive: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        let backups = plan.list().map_err(|e| chain(&e))?;
        let chosen = backups
            .iter()
            .find(|b| b.path == archive)
            .ok_or("That backup is no longer there.")?;
        plan.delete(chosen).map_err(|e| chain(&e))?;
        Ok(format!("Deleted the backup from {}.", chosen.when()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The first launch on a new build gets a settings backup first.
///
/// Never fatal, unlike the one before an addon update: an update can wait for
/// a backup, but refusing to start the game over one is the wrong trade. Runs
/// behind the pre-flight gate, with the client closed.
fn back_up_before_launch(layout: &Layout, code: &ProductCode) {
    let Ok(dirs) = AppDirs::discover() else { return };
    match settings_backup(layout, &dirs, code).before_launch() {
        Ok(Some(taken)) => crate::trace::mark(format!("launch: settings backed up to {}", taken.path.display())),
        Ok(None) => {}
        Err(e) => eprintln!("lightforge: could not back up settings before launch: {}", chain(&e)),
    }
}

/// Run the game, returning when *the game* has exited.
///
/// Not when the prefix is quiet. Proton launches the client through its own
/// `steam.exe`, which waits for every process left in the prefix, WoW's voice
/// proxy, the shop's CEF children, then `services.exe` and `winedevice.exe`
/// until wineserver times out. Waiting on that child is what used to keep
/// `[ PLAY ]` dead for tens of seconds after the window had gone.
///
/// So the child is spawned rather than waited on, and the exe we spawned is
/// watched by name in the process table, the same scan the Battle.net
/// hand-off uses, which is 60 ms a pass. The moment it is gone the launch is
/// over as far as the UI is concerned; a detached thread reaps the Proton
/// child so nothing is left a zombie, and the tail cleans itself up. The next
/// launch's pre-flight is what guarantees a quiet prefix, and it does it
/// deterministically with `wineserver -k -w` rather than by hoping the tail
/// finished.
///
/// The fallback path, nothing recognisable ever appeared, keeps the old
/// behaviour, because a launch that produced no game is exactly the case where
/// the child's exit status is the only thing worth reporting.
pub async fn run_game(layout: Layout, code: ProductCode) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The install's native path, *not* the `C:\Program Files (x86)`
        // symlink. This is half of what makes the in-game shop work, and the
        // other half is [`Layout::wow_drive`]. Wine resolves a Unix path to
        // the most specific drive mapped over it, so launching from here puts
        // the game on `D:`; launching through the symlink would keep it on
        // `C:` and the shop's browser would lose its GPU child process the way
        // it always did. Verified end to end 2026-10-02: a full checkout, from
        // a direct launch, with no Battle.net session.
        let flavor = layout.wow_install(&code).join(&*code.install_dir());
        let exe = lightforge_core::paths::game_executable(&flavor, &code);
        if !exe.is_file() {
            return Err(format!("no game at {}", exe.display()).into());
        }

        back_up_before_launch(&layout, &code);

        let (tweaks, tools) = tweaks_for(&code);

        // Reconciled here, not when the toggle was flipped: the client
        // rewrites Config.wtf from memory every time it exits, so a value
        // written once may be gone, or changed by the player in the game's own
        // options, which then stays theirs. Behind the pre-flight gate, with
        // the client closed.
        let cvars = layout.wtf_dir(&code).join("Config.wtf");
        match lightforge_core::cvars::reconcile(&cvars, &tweaks.cvars(), lightforge_core::tweaks::RETIRED) {
            Ok(done) if !done.yielded.is_empty() => crate::trace::mark(format!(
                "launch: left as set in the game: {}",
                done.yielded.join(", ")
            )),
            Ok(_) => {}
            // Not fatal. A settings file Lightforge could not write is a
            // worse game, not an unplayable one, and refusing to launch over
            // it would be the wrong trade.
            Err(e) => eprintln!("lightforge: could not apply client settings: {e}"),
        }

        let runner = runner_for(&layout, &code, Runs::Game)?;
        let mut launch = lightforge_core::tweaks::wrapped(
            runner.command("run", &exe, Vec::<String>::new()),
            &tweaks.wrappers(&tools),
        );

        let generation =
            LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;

        // What the prefix held before the click, so a session already running
        // under another launcher is not mistaken for this one's.
        let before = lightforge_core::preflight::games_running(&layout);
        let mut child = launch.spawn().map_err(|e| Problem::of(&e))?;
        crate::trace::mark(format!("launch: spawned {}", exe.display()));

        let watch = exe.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(watch) = watch else {
            return reap(child);
        };

        match watch_for(&layout, &before, &watch, &mut child)? {
            Watched::Running(pid) => {
                crate::trace::mark(format!("launch: {watch} is up (pid {pid})"));
                watch_until_gone(&layout, pid, &mut child);
                crate::trace::mark(format!("launch: {watch} has exited"));
                // The tail is nobody's business but the kernel's now. Reaped
                // on a thread of its own so the process is not left a zombie,
                // and so this returns while it is still winding down.
                //
                // Settled *first*, before the child is waited on, because the
                // child is Proton's `steam.exe` and its wait is the thing
                // being closed: measured on a retail session, the game's pid
                // went at 10.5 s and that wait did not return until 19.6 s.
                // Nine seconds in which PLAY is armed, the prefix is not
                // empty, and a press pays the whole teardown at pre-flight
                // with two signal graces behind it. Winding down here ends
                // the window instead of waiting it out: `wineserver -k -w` is
                // what `steam.exe` is waiting for anyway.
                let (quiet_layout, quiet_runner) = (layout.clone(), runner);
                std::thread::spawn(move || {
                    settle(&quiet_layout, &quiet_runner, generation);
                    let _ = child.wait();
                    crate::trace::mark("launch: the prefix is quiet");
                });
                Ok("game closed".to_owned())
            }
            // Either the game was never recognisable or it went down with the
            // tree. Both make the child's own status the honest answer.
            Watched::TreeExited(status) => Ok(format!("game exited ({status})")),
            Watched::NeverAppeared => reap(child),
        }
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What [`watch_for`] saw.
enum Watched {
    /// The spawned executable is in the process table, with this pid.
    Running(u32),
    /// The Proton child exited before anything was recognised.
    TreeExited(std::process::ExitStatus),
    /// Nothing by that name appeared inside [`GAME_APPEAR_WITHIN`].
    NeverAppeared,
}

/// Watch the prefix for the executable that was just spawned.
///
/// Matched by file name against the process table rather than by walking the
/// child's descendants: Proton hands the launch to its `steam.exe`, so the
/// game is not our child's child in any dependable way.
fn watch_for(
    layout: &Layout,
    before: &[GameProcess],
    exe: &str,
    child: &mut std::process::Child,
) -> Result<Watched, String> {
    let known: HashSet<u32> = before.iter().map(|p| p.pid).collect();
    let deadline = Instant::now() + GAME_APPEAR_WITHIN;

    loop {
        if let Some(found) = lightforge_core::preflight::games_running(layout)
            .into_iter()
            .find(|p| !known.contains(&p.pid) && p.name.eq_ignore_ascii_case(exe))
        {
            return Ok(Watched::Running(found.pid));
        }
        // Checked after the scan, not before it: a game that starts and exits
        // between two polls still has to be seen, and the scan is the only
        // thing that can see it.
        match child.try_wait().map_err(|e| chain(&e))? {
            Some(status) => return Ok(Watched::TreeExited(status)),
            None if Instant::now() >= deadline => return Ok(Watched::NeverAppeared),
            None => std::thread::sleep(WATCH_POLL),
        }
    }
}

/// Block until `pid` is gone from the prefix, or the whole tree is.
///
/// Watched by pid, so a client that re-execs itself counts as closed. That is
/// the right call here too: the next launch cleans the prefix anyway.
fn watch_until_gone(layout: &Layout, pid: u32, child: &mut std::process::Child) {
    while lightforge_core::preflight::games_running(layout).iter().any(|p| p.pid == pid) {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(EXIT_POLL);
    }
}

/// Put the prefix to rest once a direct launch is over.
///
/// Only when nothing of ours is still using it. Battle.net is the case that
/// matters: it shares the prefix, people leave it open, and killing the server
/// out from under it would close the client they are looking at. A game
/// started from Battle.net is the same prefix too, so both are checked.
///
/// Best effort throughout. A prefix that will not settle is exactly what the
/// next pre-flight is for, and it is still there, doing the same work with the
/// same guarantees. This only tries to make sure it finds nothing to do.
fn settle(
    layout: &Layout,
    runner: &lightforge_core::prefix::ProtonRunner,
    generation: u64,
) {
    use lightforge_core::preflight;

    // A newer launch owns the prefix now. Checked first and by counter rather
    // than by scanning, because a game that has been spawned but has not yet
    // appeared in the process table is invisible to a scan and is exactly the
    // session this must not interrupt.
    if LAUNCHES.load(std::sync::atomic::Ordering::SeqCst) != generation {
        crate::trace::mark("launch: a newer session owns the prefix, leaving it alone");
        return;
    }
    if preflight::client_running(layout) || !preflight::games_running(layout).is_empty() {
        crate::trace::mark("launch: something is still in the prefix, leaving it alone");
        return;
    }
    let started = Instant::now();
    let outcome = runner.wineserver_kill();
    // Microseconds, because the interesting failure looks like success: a
    // `wineserver` that was never found returns before it spawns anything,
    // and at millisecond resolution that is indistinguishable from a prefix
    // that was already quiet.
    crate::trace::mark(format!(
        "launch: prefix settled in {} us{}",
        started.elapsed().as_micros(),
        match &outcome {
            Ok(()) => String::new(),
            Err(e) => format!(" (failed: {})", chain(e)),
        }
    ));
}

/// Wait the Proton child out and report what it said, the old contract, kept
/// for the launches where nothing better is knowable.
fn reap(mut child: std::process::Child) -> Result<String, Problem> {
    let status = child.wait().map_err(|e| Problem::of(&e))?;
    Ok(format!("game exited ({status})"))
}

/// Read the client's cached catalogue, for the game families in it.
///
/// About 25 ms against a real cache, and only at startup: the answer changes
/// when Blizzard publishes a new title, not while the launcher is open.
pub async fn game_families(layout: Layout, user: String, region: Region) -> Families {
    tokio::task::spawn_blocking(move || Families::read(&layout, &user, region))
        .await
        .unwrap_or_default()
}

pub async fn launch_via_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
    family: Option<String>,
    minimized: bool,
) -> Result<Handoff, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch, Readiness};
        use lightforge_core::preflight;

        let runner = runner_for(&layout, &code, Runs::BattleNet)?;
        back_up_before_launch(&layout, &code);

        // Counted too, though nothing here winds the prefix down: a direct
        // launch's wind-down may still be pending, and a client that has been
        // spawned but not yet registered a process is invisible to its scan.
        LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Taken before the client is told anything, so a game that was
        // already playing is not mistaken for this launch's.
        let before = preflight::games_running(&layout);

        // Two phases, because one does not work. A cold client handed
        // `--exec=launch` parses it and then does nothing with it: measured
        // on a real start, the client was signed in after five seconds and
        // the game did not begin for another forty-eight. The arguments have
        // to arrive at a client that is already up, which is what the
        // client's own IPC queue is for, a second invocation hands them to
        // the first.
        if !preflight::client_running(&layout) {
            // Out of the way, but only where it can sign itself in. A
            // sign-in form nobody can see is a launch that quietly never
            // happens, so a prefix without auto-login gets a normal window
            // whatever the setting says.
            let minimize = minimized && battlenet::signs_in_by_itself(&layout, &user);
            let _ = battlenet::configure_accounts(
                &layout,
                &user,
                &battlenet::window_state_tweaks(minimize),
            );

            // `--autostarted` or not: the preference above is only read on an
            // autostart, and the flag without the preference does nothing of
            // its own.
            let how = match minimize {
                true => Launch::Autostart,
                false => Launch::Client,
            };

            let started = std::time::SystemTime::now();
            // Wrappers go here and nowhere else on this path: this
            // invocation becomes the client, and the game inherits from it.
            // The launch below only hands arguments to it over IPC.
            let tools = HostTools::detect();
            let wrappers = config_tweaks().wrappers(&tools);
            battlenet::launch_wrapped(&runner, &layout, &how, &wrappers)
                .map_err(|e| Problem::of(&e))?;

            if battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT)
                == Readiness::NeedsAttention
            {
                // Stop driving. The client is up and is the only thing that
                // can finish this, so it is left running for the user, who
                // has to be told where it is if it was put in the tray.
                return Ok(Handoff::NeedsAttention { minimized: minimize });
            }
        }

        // The client can be gone by now: signing in takes a few seconds, and
        // a window nobody expected is a window somebody closes, with
        // `HideOnClose` off, closing it quits. Sending the launch anyway
        // would start a *second*, cold client, and a cold client drops the
        // `--exec` it was handed. Saying so is the better outcome.
        if !preflight::client_running(&layout) {
            return Ok(Handoff::ClientClosed);
        }

        // Now it lands: the running client picks this up over its IPC queue
        // and starts the game. The family the panel resolved, which for
        // anything but WoW came out of the client's own catalogue;
        // `Launch::command` falls back to the compiled-in table without one.
        let launch = Launch::Product { code: code.clone(), exec: family };
        battlenet::launch(&runner, &layout, &launch).map_err(|e| Problem::of(&e))?;

        Ok(
            match preflight::wait_for_game_start(&layout, &before, HANDOFF_TIMEOUT) {
                Some(game) => Handoff::Playing(game),
                None => Handoff::NothingStarted,
            },
        )
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// Open the client's install flow for a game, dialog and all.
///
/// Three mechanisms were tried against a real client, and only this one
/// works, for one reason: the client always boots to its homepage.
///
/// - `--exec="launch <uid>"` sets the active product and the homepage then
///   draws over it. Nothing visible happens.
/// - `--uri="battlenet://<family>"` is handled properly, by a `UriController`
///   that waits until the client is ready, but all it does is select the
///   game, and the homepage still wins the window.
/// - `--game=<uid> --install` starts the install flow, which has a **view of
///   its own**. It opens over the homepage, with the path picker, which is
///   the screen this button is for.
///
/// One invocation covers both cases: a running client takes it over its IPC
/// queue, and with no client running this *is* the client.
pub async fn install_via_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
    uid: String,
    label: String,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch};

        let runner = runner_for(&layout, &code, Runs::BattleNet)?;

        // Normal window, whatever a launch would have chosen: the next thing
        // that happens is a person choosing where to install.
        let _ = battlenet::configure_accounts(
            &layout,
            &user,
            &battlenet::window_state_tweaks(false),
        );

        battlenet::launch(&runner, &layout, &Launch::InstallFlow { uid })
            .map_err(|e| Problem::of(&e))?;

        Ok(format!("Battle.net is asking where to install {label}"))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What came of a hand-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The game is running.
    Playing(GameProcess),
    /// The client never reported signing in. Someone has to look at it,
    /// which means being told where it is when it was started minimized.
    NeedsAttention { minimized: bool },
    /// It was there and then it was not, closed while we waited.
    ClientClosed,
    /// Signed in, told to launch, and nothing appeared.
    NothingStarted,
}

/// How long the client gets to start and sign itself in.
///
/// A cold start under Proton reaches "Logged into Battle.net successfully" in
/// about five and a half seconds. A minute is for the bad days, a client
/// updating itself, or a slow network, and running out of it is not an
/// error, just the point where a person is needed.
const SIGN_IN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a handed-off game gets to appear/// How long a handed-off game gets to appear before the launcher stops
/// claiming a launch is in progress.
///
/// Generous: a cold Battle.net under Wine has to start, update itself if it
/// feels like it, and may stop at a login screen, and the cost of being
/// wrong in this direction is only an unlocked button.
const HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Block until a handed-off game exits.
///
/// Split from the launch so the screen can say which game is running in
/// between, instead of "handing off…" for the length of a session.
pub async fn await_game_exit(layout: Layout, game: GameProcess) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        lightforge_core::preflight::wait_for_game_exit(&layout, game.pid);
        Ok(format!("{} exited", game.name))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What Battle.net reports as installed, keyed by product code.
///
/// Read when the browse panel opens, never on a timer and never to decide
/// what the header shows, it labels a list the user asked to see.
/// Battle.net's own download limit, in MB/s, or `None` when it has none.
pub async fn battle_net_limit(layout: Layout, user: String) -> Option<u64> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::download_limit(&layout, &user)
            .map(|bps| bps / 1_000_000)
            .filter(|mbps| *mbps > 0)
    })
    .await
    .ok()
    .flatten()
}

/// Set or clear Battle.net's own download limit, in MB/s.
pub async fn set_battle_net_limit(
    layout: Layout,
    user: String,
    mbps: Option<u64>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::set_download_limit(
            &layout,
            &user,
            mbps.map(|mbps| mbps * 1_000_000),
        )
        .map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))?
}

/// The product codes Battle.net's own config lists, so the catalog can use
/// the codes the client uses rather than the ones our table happens to carry.
pub async fn known_codes(layout: Layout, user: String) -> Vec<String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::known_product_codes(&layout, &user)
    })
    .await
    .unwrap_or_default()
}

/// Index Battle.net's registry by every name a record answers to.
///
/// The tile asking "is this installed?" holds the CDN's product code, and
/// this file is keyed by the client's install key. They are the same string
/// for every product but one: Hearthstone is `hs_beta` to Battle.net and
/// `hsb` to the CDN, so a map keyed only by the install key left an installed
/// Hearthstone reading `Install via Battle.net` forever, locating it in the
/// client did not help, because the client was never the thing that was
/// wrong.
///
/// A record with no version is skipped rather than recorded as installed:
/// Agent writes the row before the download finishes, and a tile that offers
/// Play for a game that is still arriving is worse than one that waits.
fn by_every_name(
    installs: Vec<lightforge_core::tact::productdb::Installed>,
) -> HashMap<String, String> {
    let mut found = HashMap::new();
    for install in installs {
        let Some(version) = install.version else { continue };
        if !install.product_code.is_empty() && install.product_code != install.product {
            found.insert(install.product_code, version.clone());
        }
        found.insert(install.product, version);
    }
    found
}

pub async fn installed_versions(layout: Layout) -> HashMap<String, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::productdb;

        by_every_name(productdb::read(&layout.agent_dir().join("product.db")).unwrap_or_default())
    })
    .await
    .unwrap_or_default()
}

/// Open Battle.net with no product: the way to reach everything outside the
/// WoW family.
///
/// Deliberately a hand-off and nothing more. Lightforge does not track,
/// install or update other Blizzard titles, `SPEC.md` is explicit that it is
/// WoW-family-primary and a pass-through for the rest, so this opens the
/// client and stops there.
///
/// Still pre-flighted by the caller: a stale wineserver breaks Battle.net
/// exactly as it breaks a game launch.
pub async fn open_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch};

        let runner = runner_for(&layout, &code, Runs::BattleNet)?;
        // A normal window, whatever the last hand-off left behind: somebody
        // asked for the client itself, so hiding it in the tray would be
        // answering a different request.
        let _ = battlenet::configure_accounts(
            &layout,
            &user,
            &battlenet::window_state_tweaks(false),
        );
        battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| Problem::of(&e))?;
        Ok("Battle.net opening".to_owned())
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// The Proton build a product launches under, with its per-product caches.
/// Which workload a runner is being built for.
///
/// They pin separately: `SPEC.md` asks for a Proton build chosen for
/// Battle.net and Agent specifically, because "latest" is not always best for
/// the client even when it is right for the game. A product's own pin still
/// wins on both paths; see [`Config::handoff_proton`], so a hand-off title
/// is not stuck with whatever the client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// The game itself, under the product's own pin.
    Game,
    /// Battle.net or Agent, starting something for this product.
    BattleNet,
}

impl Runs {
    /// The build this workload resolves to for `code`.
    fn pin<'a>(self, config: &'a Config, code: &ProductCode) -> Option<&'a str> {
        match self {
            Self::Game => config.effective_proton(code),
            Self::BattleNet => config.handoff_proton(code),
        }
    }
}

/// The build a launch would use, and how it sits against the prefix.
///
/// Read before a launch rather than during it: the prefix is shared, Proton
/// only migrates it forward, and an older build opening a prefix a newer one
/// rewrote is the one way this arrangement breaks. The caller decides what to
/// do about it. Nothing here refuses a launch.
pub fn prefix_fit(
    layout: &Layout,
    code: &ProductCode,
    runs: Runs,
) -> (Option<String>, lightforge_core::proton::PrefixFit) {
    use lightforge_core::proton;

    let Ok(dirs) = AppDirs::discover() else {
        return (None, proton::PrefixFit::Unknown);
    };
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let Some(build) = proton::select(&builds, runs.pin(&config, code)) else {
        return (None, proton::PrefixFit::Unknown);
    };
    let built_by = proton::prefix_build(&layout.compat_data());
    let fit = proton::prefix_fit(&build.version, built_by.as_deref());
    (Some(build.version.clone()), fit)
}

fn runner_for(
    layout: &Layout,
    code: &ProductCode,
    runs: Runs,
) -> Result<lightforge_core::prefix::ProtonRunner, String> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, runs.pin(&config, code))
        .ok_or_else(|| "no supported Proton-GE build installed".to_owned())?;

    let target =
        std::fs::read_link(layout.wow_symlink()).unwrap_or_else(|_| layout.wow_install(code));
    let mut runner =
        ProtonRunner::new(build, layout, ProtonRunner::steam_client_path(steam.as_ref(), &dirs))
            .with_compat_mount(&target)
            // The prefix is shared, so pooling the shader cache would mean one
            // product's Proton swap discarding another's shaders.
            .with_shader_cache(dirs.shader_cache_dir(code));

    // Two profiles, because they are two different questions. A direct
    // launch is this product's own; a Battle.net launch is the client's and
    // therefore every game the client starts, the one environment all of
    // them inherit. See `Config::battle_net_tweaks` for why that cannot be
    // per-game.
    let tweaks = match runs {
        Runs::Game => config.product(code).map(|p| p.profile.tweaks.clone()).unwrap_or_default(),
        Runs::BattleNet => config.battle_net_tweaks.clone(),
    };
    for (key, value) in tweaks.environment(&HostTools::detect()) {
        runner = runner.with_env(key, value);
    }
    Ok(runner)
}

/// The shared profile Battle.net and its games launch under.
///
/// Not per-product on purpose; see `Config::battle_net_tweaks`.
fn config_tweaks() -> Tweaks {
    AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .map(|config| config.battle_net_tweaks)
        .unwrap_or_default()
}

/// One product's performance settings, and what this machine can honour.
fn tweaks_for(code: &ProductCode) -> (Tweaks, HostTools) {
    let tweaks = AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .and_then(|config| config.product(code).map(|p| p.profile.tweaks.clone()))
        .unwrap_or_default();
    (tweaks, HostTools::detect())
}

/// The Proton build that would be used, for the debug bundle.
pub fn proton_for(code: &ProductCode) -> Option<lightforge_core::proton::ProtonRelease> {
    use lightforge_core::proton;

    let dirs = AppDirs::discover().ok()?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    proton::select(&builds, config.effective_proton(code)).cloned()
}

/// Open a path in the desktop's file manager.
pub fn open_path(path: PathBuf) -> Result<(), String> {
    open::that_detached(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// What uninstalling a product would do, before anything is touched.
#[derive(Debug, Clone)]
pub struct UninstallPreview {
    pub plan: lightforge_core::tact::uninstall::Plan,
    /// The shared store's space it would give back. `None` when the whole
    /// store goes anyway; an error when what it shares could not be checked,
    /// in which case the store is left as it is.
    pub reclaim: Option<Result<lightforge_core::tact::reclaim::Reclaim, String>>,
}

/// Remove what a cancelled download wrote, and nothing else.
///
/// Two shapes, decided by what else is in the install:
///
/// * nothing else, so the whole store was this download's and it goes with
///   the flavour directory, which is what an uninstall does;
/// * other products share the store, so the only honest answer is to drop the
///   entries this build wants that no remaining product wants. Everything a
///   cancelled run stored is in that set by construction: the plan skips what
///   the store already holds, so a byte it fetched is a byte no installed
///   product had.
///
/// The build is named by the version service rather than by `.build.info`,
/// since a cancelled install never got a row there. That is the one thing
/// this needs the network for, and without it the files have to stay.
pub async fn discard_download(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    region: Region,
) -> Result<u64, String> {
    use lightforge_core::tact::{reclaim, uninstall, version};

    // The same gate a repair and an uninstall go through.
    let alive = layout.clone();
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&alive))
        .await
        .map_err(|e| chain(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name));
    }

    let install = layout.wow_install(&code);
    let plan = uninstall::plan(&install, &code).map_err(|e| chain(&e))?;
    let windows_path = layout
        .library(&code)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned());

    // Whatever belongs to this product on the filesystem: a partial flavour
    // directory, and the store itself when nothing else is reading it.
    let whole = plan.is_whole_install();
    let removed = {
        let (install, code) = (install.clone(), code.clone());
        tokio::task::spawn_blocking(move || {
            uninstall::execute(&install, &code, &plan, &windows_path)
                .map(|removed| removed.bytes)
                .map_err(|e| chain(&e))
        })
        .await
        .map_err(|e| chain(&e))??
    };
    if whole {
        return Ok(removed);
    }

    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let versions = version::versions(&http, region, code.as_str())
        .await
        .map_err(|e| chain(&e))?;
    let remote = version::for_region(&versions, region, |v| &v.region)
        .ok_or_else(|| "the version service lists no build for this region".to_owned())?;
    let cdns = version::cdns(&http, region, code.as_str()).await.map_err(|e| chain(&e))?;
    let cdn = version::for_region(&cdns, region, |c| &c.name)
        .cloned()
        .ok_or_else(|| "the version service lists no CDN for this region".to_owned())?;

    let departing = reclaim::Build::remote(code, remote.build_config.clone(), cdn);
    let reclaim = reclaim::plan(&http, &install, &departing).await.map_err(|e| chain(&e))?;
    let freed = tokio::task::spawn_blocking(move || {
        reclaim::execute(&install, &reclaim, |_| {}).map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))??;

    Ok(removed + freed)
}

/// Plan the store reclaim for `code`, unless the whole store is going.
async fn reclaim_plan(
    http: &Http,
    install: &std::path::Path,
    code: &ProductCode,
    plan: &lightforge_core::tact::uninstall::Plan,
) -> Option<Result<lightforge_core::tact::reclaim::Reclaim, String>> {
    use lightforge_core::tact::reclaim;
    if plan.is_whole_install() || !plan.has_row {
        return None;
    }
    let departing = lightforge_core::buildinfo::BuildInfo::load(install.join(".build.info"))
        .map_err(|e| chain(&e))
        .and_then(|info| reclaim::Build::installed(&info, code).map_err(|e| chain(&e)));
    Some(match departing {
        Ok(build) => reclaim::plan(http, install, &build).await.map_err(|e| chain(&e)),
        Err(e) => Err(e),
    })
}

/// What uninstalling `code` would delete and free. Nothing is touched.
pub async fn uninstall_plan(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
) -> Result<UninstallPreview, String> {
    let install = layout.wow_install(&code);
    let plan = {
        let (install, code) = (install.clone(), code.clone());
        tokio::task::spawn_blocking(move || {
            lightforge_core::tact::uninstall::plan(&install, &code).map_err(|e| chain(&e))
        })
        .await
        .map_err(|e| chain(&e))??
    };
    let reclaim = match &http {
        Some(http) => reclaim_plan(http, &install, &code, &plan).await,
        None => Some(Err(NO_HTTP.to_owned())),
    };
    Ok(UninstallPreview { plan, reclaim })
}

/// Uninstall `code`: its files, its `.build.info` row and `.product.db`
/// record, the shared store's data only it used (or the whole store if it
/// was the last product), and its entry in Battle.net's registry. Returns
/// the bytes freed.
///
/// Refuses while anything is alive in the prefix, the same gate repair uses:
/// deleting files a running client has open, or a registry Agent rewrites on
/// exit, is not something to race.
///
/// Everything is planned again here rather than trusted from when the dialog
/// opened. The reclaim is planned *before* the product's row goes, since the
/// row is what names the build whose files it was.
pub async fn uninstall(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
    copied: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<u64, Problem> {
    use lightforge_core::tact::{productdb, reclaim, uninstall};

    let alive = layout.clone();
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&alive))
        .await
        .map_err(|e| Problem::of(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name).into());
    }

    let install = layout.wow_install(&code);
    let windows_path = layout
        .library(&code)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned());
    let plan = uninstall::plan(&install, &code).map_err(|e| Problem::of(&e))?;
    // A reclaim that cannot be planned is skipped, not fatal: the uninstall
    // itself is still exactly what was asked for.
    let reclaim = match &http {
        Some(http) => reclaim_plan(http, &install, &code, &plan).await.and_then(Result::ok),
        None => None,
    };

    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        let removed = uninstall::execute(&install, &code, &plan, &windows_path)
            .map_err(|e| Problem::of(&e))?;
        let reclaimed = match &reclaim {
            Some(reclaim) => reclaim::execute(&install, reclaim, |bytes| {
                copied.store(bytes, std::sync::atomic::Ordering::Relaxed)
            })
            .map_err(|e| Problem::of(&e))?,
            None => 0,
        };
        productdb::unregister(&layout.agent_dir(), code.as_str()).map_err(|e| Problem::of(&e))?;
        Ok(removed.bytes + reclaimed)
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What freeing a store's unused game data would do: data no installed or
/// pre-installed version uses, plus dead space older passes left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Unused {
    /// Bytes of disk it would free.
    pub freeable: u64,
    /// Bytes of data still in use it copies first.
    pub moved: u64,
}

/// Plan freeing the unused data in the store under `install`. Changes nothing.
pub async fn unused_plan(http: Option<Http>, install: PathBuf) -> Result<Unused, String> {
    use lightforge_core::tact::{compact, reclaim};
    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let prune = reclaim::plan_unused(&http, &install).await.map_err(|e| chain(&e))?;
    // Archives the prune already rewrites are counted once, by the prune.
    let rewritten: HashSet<u16> = prune.archives.iter().copied().collect();
    let (dead, _) =
        tokio::task::spawn_blocking(move || compact::plan_where(&install, |a| !rewritten.contains(&a)))
            .await
            .map_err(|e| chain(&e))?
            .map_err(|e| chain(&e))?;
    Ok(Unused { freeable: prune.freed + dead.freed, moved: prune.moved + dead.moved })
}

/// Free the unused data in the store under `install`: drop what no version
/// uses, then compact. Planned again here rather than trusted from when the
/// page opened. Returns the bytes freed; `copied` hears the copy's progress.
pub async fn free_unused(
    http: Option<Http>,
    layout: Layout,
    install: PathBuf,
    copied: std::sync::Arc<std::sync::atomic::AtomicU64>,
) -> Result<u64, String> {
    use lightforge_core::tact::{compact, reclaim};
    use std::sync::atomic::Ordering;

    let http = http.ok_or_else(|| NO_HTTP.to_owned())?;
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&layout))
        .await
        .map_err(|e| chain(&e))?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name));
    }

    let prune = reclaim::plan_unused(&http, &install).await.map_err(|e| chain(&e))?;
    tokio::task::spawn_blocking(move || {
        let pruned = reclaim::execute(&install, &prune, |bytes| copied.store(bytes, Ordering::Relaxed))
            .map_err(|e| chain(&e))?;
        let base = prune.moved;
        let (dead, snapshot) = compact::plan(&install).map_err(|e| chain(&e))?;
        let compacted = compact::execute(&install, &dead, &snapshot, |bytes| {
            copied.store(base + bytes, Ordering::Relaxed)
        })
        .map_err(|e| chain(&e))?;
        Ok(pruned + compacted)
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Measure Settings > Storage. Walks the whole game install.
pub async fn storage(layout: Layout, dirs: AppDirs) -> lightforge_core::storage::Report {
    let fallback = lightforge_core::storage::Report {
        libraries: Vec::new(),
        app: Vec::new(),
        app_root: dirs.data_dir().to_path_buf(),
        app_disk: None,
    };
    tokio::task::spawn_blocking(move || lightforge_core::storage::report(&layout, &dirs))
        .await
        .unwrap_or(fallback)
}

/// Clear what one library holds that is only a cache; returns bytes freed.
pub async fn clear_library(
    layout: Layout,
    library: lightforge_core::library::Library,
) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::storage::clear_library(&layout, &library).map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Clear Lightforge's own cache, shader caches aside; returns bytes freed.
pub async fn clear_cache(dirs: AppDirs) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || lightforge_core::storage::clear_cache(&dirs))
        .await
        .map_err(|e| chain(&e))?
        .map_err(|e| chain(&e))
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| format!("{url}: {e}"))
}

#[cfg(test)]
mod uninstall_tests {
    use super::*;
    use lightforge_core::tact::productdb;

    /// Uninstalling one version from a throwaway library built from copies of
    /// the real `.build.info` and Agent registry: only that version goes, and
    /// every other row and record stays. Skipped where there is no real
    /// install to copy from.
    #[test]
    fn uninstalling_one_version_leaves_the_rest() {
        let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else { return };
        let real = Layout::with_root(home.join("Games/Blizzard"));
        let (Ok(info), Ok(registry)) = (
            std::fs::read(real.build_info(&ProductCode::ClassicEra)),
            std::fs::read(real.agent_dir().join("product.db")),
        ) else {
            return;
        };

        let root = std::env::temp_dir()
            .join(format!("lightforge-uninstall-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let layout = Layout::with_root(&root);
        let era = layout.product_dir(&ProductCode::ClassicEra);
        std::fs::create_dir_all(era.join("WTF")).unwrap();
        std::fs::write(era.join("WowClassic.exe"), [0u8; 64]).unwrap();
        std::fs::create_dir_all(layout.wow_install(&ProductCode::ClassicEra).join("Data/data")).unwrap();
        std::fs::write(layout.build_info(&ProductCode::ClassicEra), &info).unwrap();
        std::fs::create_dir_all(layout.agent_dir()).unwrap();
        std::fs::write(layout.agent_dir().join("product.db"), &registry).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        // No client: the reclaim is skipped, which is what this test isolates.
        runtime.block_on(uninstall(None, layout.clone(), ProductCode::ClassicEra, Default::default())).unwrap();

        assert!(!era.exists());
        assert!(
            layout.wow_install(&ProductCode::ClassicEra).join("Data").exists(),
            "the shared store stays",
        );
        let left = lightforge_core::buildinfo::BuildInfo::load(layout.build_info(&ProductCode::ClassicEra)).unwrap();
        let before = lightforge_core::buildinfo::BuildInfo::load(real.build_info(&ProductCode::ClassicEra)).unwrap();
        assert_eq!(left.products().count(), before.products().count() - 1);
        assert!(left.products().all(|p| p != ProductCode::ClassicEra));

        let after = std::fs::read(layout.agent_dir().join("product.db")).unwrap();
        let after = productdb::decode(&after);
        let had = productdb::decode(&registry);
        assert_eq!(after.len(), had.len() - 1);
        assert!(after.iter().all(|r| r.product != "wow_classic_era"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

#[cfg(test)]
mod registry_tests {
    use super::*;
    use lightforge_core::tact::productdb::Installed;

    fn record(product: &str, product_code: &str, version: Option<&str>) -> Installed {
        Installed {
            product: product.to_owned(),
            product_code: product_code.to_owned(),
            install_path: "C:/Program Files (x86)/Whatever".to_owned(),
            version: version.map(str::to_owned),
            build_key: None,
            family: String::new(),
        }
    }

    /// Gabriel's bug, 2026-10-10: Hearthstone installed through Battle.net,
    /// located in the client, and the tile still read `Install via
    /// Battle.net`. The registry calls it `hs_beta`; the tile asks for `hsb`.
    #[test]
    fn hearthstone_answers_to_the_name_its_tile_uses() {
        let found = by_every_name(vec![record("hs_beta", "hsb", Some("36.6.3.253932"))]);

        assert_eq!(found.get("hsb").map(String::as_str), Some("36.6.3.253932"));
        // And still under the name the registry itself uses, which is what
        // anything reading Agent's own file will hold.
        assert_eq!(found.get("hs_beta").map(String::as_str), Some("36.6.3.253932"));
    }

    #[test]
    fn a_product_whose_names_agree_is_indexed_once() {
        let found = by_every_name(vec![record("wow", "wow", Some("12.1.0.69933"))]);
        assert_eq!(found.len(), 1);
        assert_eq!(found.get("wow").map(String::as_str), Some("12.1.0.69933"));
    }

    /// A row Agent wrote before the download finished is not an install.
    #[test]
    fn a_record_without_a_version_is_not_installed() {
        let found = by_every_name(vec![record("hs_beta", "hsb", None)]);
        assert!(found.is_empty(), "{found:?}");
    }
}

#[cfg(test)]
mod mark_tests {
    use super::*;

    fn mark(path: &str) -> Mark {
        Mark { path: PathBuf::from(path), handle: Handle::from_rgba(1, 1, vec![0, 0, 0, 0]) }
    }

    /// The regression this guards: icons became decoded handles, and a handle
    /// built from pixels is a fresh id every time. Reassigning an identical
    /// set makes the renderer re-upload every texture, which is the band
    /// blinking a second after each product switch, the same fault the
    /// branding swap was fixed for in October.
    #[test]
    fn identical_icon_sets_compare_equal_despite_new_handles() {
        let retail = ProductCode::from("wow".to_owned());
        let a = HashMap::from([(retail.clone(), mark("/cache/abc.png"))]);
        let b = HashMap::from([(retail.clone(), mark("/cache/abc.png"))]);
        assert!(same_marks(&a, &b), "same file must compare equal");

        let c = HashMap::from([(retail, mark("/cache/def.png"))]);
        assert!(!same_marks(&a, &c), "a different file must swap");
    }

    #[test]
    fn a_changed_set_size_is_not_the_same() {
        let a = HashMap::from([(ProductCode::from("wow".to_owned()), mark("/x.png"))]);
        let b = HashMap::new();
        assert!(!same_marks(&a, &b));
        assert!(!same_marks(&b, &a));
    }
}

/// Fetch and decode the pictures for a set of headlines.
///
/// `have` is what is already drawn: those URLs are skipped entirely, so a
/// Refresh re-downloads nothing and, more importantly, mints no new handle
/// for a picture already on screen. `Handle::from_rgba` gets a fresh id every
/// call, so handing the view an identical-looking handle re-uploads the
/// texture and flashes. Returning only what is genuinely new is the guard.
///
/// Capped: a feed is forty items and forty images at once would saturate the
/// connection the downloader wants.
pub async fn news_images(
    http: Option<Http>,
    dir: PathBuf,
    urls: Vec<String>,
    have: HashSet<String>,
) -> Vec<(String, Handle)> {
    let Some(http) = http else {
        return Vec::new();
    };

    // Before fetching, not after: the pictures this feed no longer names are
    // dead the moment the feed arrives, and clearing them first means the
    // directory never holds two feeds' worth at once.
    {
        let dir = dir.clone();
        let keep = urls.clone();
        let _ = tokio::task::spawn_blocking(move || news::prune_images(&dir, &keep)).await;
    }

    let wanted: Vec<String> = urls.into_iter().filter(|url| !have.contains(url)).collect();

    let mut out = Vec::new();
    for chunk in wanted.chunks(NEWS_IMAGE_BATCH) {
        let fetched = futures_util::future::join_all(chunk.iter().map(|url| {
            let http = http.clone();
            let dir = dir.clone();
            let url = url.clone();
            async move {
                let path = news::image(&http, &dir, &url).await?;
                // Decoding and scaling is CPU-bound; off the UI thread.
                let handle = tokio::task::spawn_blocking(move || crate::art::thumbnail(&path))
                    .await
                    .ok()?;
                Some((url, handle))
            }
        }))
        .await;
        out.extend(fetched.into_iter().flatten());
    }
    out
}

/// How many headline pictures are fetched at once.
const NEWS_IMAGE_BATCH: usize = 6;

/// Decode the News page's bundled backdrop, once per session.
///
/// Off the UI thread: it is a 2560px JPEG, and decoding it on the render
/// thread is a visible stall on the frame that first shows the page.
pub async fn news_background(bytes: &'static [u8]) -> Handle {
    tokio::task::spawn_blocking(move || crate::art::decode_bundled(bytes))
        .await
        .unwrap_or_else(|_| crate::art::placeholder())
}

/// Fetch the News page's headlines.
///
/// Returns `None` only when there is genuinely nothing to show, no network
/// and nothing cached. Anything else, including a proxy that is down while a
/// cached copy exists, comes back as headlines that know whether they are
/// stale.
pub async fn headlines(
    http: Option<Http>,
    origin: news::Origin,
    cache_file: PathBuf,
) -> Option<news::Headlines> {
    let http = http?;
    news::headlines(&http, &origin, &cache_file).await
}
