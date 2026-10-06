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
        // install it is still tens of milliseconds — off the UI thread.
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
            Err(e) => (None, Some(e.to_string())),
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
        .map_err(|e| e.to_string())
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
/// Repair rewrites the indices a running client memory-maps, so this refuses
/// to run while anything is alive in the prefix — the same gate a launch and a
/// download go through.
pub async fn scan_and_repair(
    layout: Layout,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<Scanned, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::storage::Storage;
        use lightforge_core::tact::verify;

        let alive = lightforge_core::preflight::scan(&layout);
        if let Some(process) = alive.first() {
            return Err(format!(
                "close {} first — the store is in use",
                process.name
            ));
        }

        let install = layout.wow_install();
        let store = Storage::open(install.join("Data")).map_err(|e| e.to_string())?;
        let report = verify::scan(&store, |progress| {
            let _ = tx.send(progress);
        });

        let removed = match report.is_healthy() {
            true => 0,
            false => verify::repair(&install.join("Data").join("data"), &report.damaged)
                .map_err(|e| e.to_string())?,
        };
        Ok(Scanned {
            checked: report.checked,
            damaged: report.damaged.len() as u64,
            removed,
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Hash every stored byte against its chunk table and key.
///
/// Only reads, so unlike [`scan_and_repair`] it does not wait for the game to
/// close; the caller holds the download queue for as long as it runs, and
/// the core refuses a result from a store that changed underneath it anyway.
pub async fn deep_scan(
    layout: Layout,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<lightforge_core::tact::verify::Progress>,
) -> Result<lightforge_core::tact::verify::Report, String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::tact::verify::deep_scan(layout.wow_install(), &cancel, |progress| {
            let _ = tx.send(progress);
        })
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Why a repair did not happen.
#[derive(Debug, Clone)]
pub enum Unrepaired {
    /// Something is alive in the prefix; the repair is still owed.
    InUse,
    /// Anything else — including a store that moved since the scan, whose
    /// findings are no longer worth acting on.
    Failed(String),
}

/// Drop what a deep scan found damaged, if nothing is using the store and it
/// is still the store that was scanned.
pub async fn repair(
    layout: Layout,
    report: lightforge_core::tact::verify::Report,
) -> Result<usize, Unrepaired> {
    tokio::task::spawn_blocking(move || {
        if !lightforge_core::preflight::scan(&layout).is_empty() {
            return Err(Unrepaired::InUse);
        }
        let data_dir = layout.wow_install().join("Data").join("data");
        lightforge_core::tact::verify::repair_unchanged(&data_dir, &report)
            .map_err(|e| Unrepaired::Failed(e.to_string()))
    })
    .await
    .map_err(|e| Unrepaired::Failed(e.to_string()))?
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
        Themed { manifest: manifest.map(Arc::new), branding, icons }
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
/// The manifest comes back so the app can hold onto it — see [`theme`]. The
/// icons ride along because the pass already holds the one manifest that names
/// them all, and the product bar needs the whole set, not just the selected
/// product's.
#[derive(Debug, Default, Clone)]
pub struct Themed {
    pub manifest: Option<Arc<Manifest>>,
    pub branding: Option<Art>,
    pub icons: HashMap<ProductCode, Mark>,
}

/// A product's decoded mark, beside the file it came from.
///
/// The path is kept for one reason: to tell whether a theming pass produced
/// *different* marks or the same ones again. [`Handle::from_path`] used to
/// make that free, because iced caches those by path and reassigning one costs
/// nothing. A handle built from pixels is a fresh id every time, so assigning
/// an identical set makes the renderer re-upload every texture — which is the
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
    // draw — see [`crate::art`].
    let branding = match branding {
        Some(branding) => tokio::task::spawn_blocking(move || Art::decode(branding)).await.ok(),
        None => None,
    };

    Themed { manifest: Some(manifest), branding, icons }
}

/// What a running transfer publishes for the UI to read.
///
/// A `watch` channel deliberately keeps only the newest value: the download
/// calls its progress callback once per stored file — 1.5 million times on
/// retail — and the UI samples at its own rate. Nothing queues, so the
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
pub async fn download(
    layout: Layout,
    code: ProductCode,
    region: Region,
    limit_bytes_per_second: u64,
    optional_tags: Vec<String>,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<TransferState>,
) -> Result<Summary, String> {
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
                    let _ = done_tx.send(Err(e.to_string()));
                    return;
                }
            };

            let result = runtime.block_on(async move {
                // Deliberately not the app's shared client: this runtime is
                // built here and torn down with the thread, and a reqwest
                // client's connections belong to the runtime that opened them.
                let http = Http::new().map_err(|e| e.to_string())?;
                let dest = layout.wow_install();
                let options = InstallOptions {
                    region,
                    flavor: code.install_dir().into_owned(),
                    optional_tags: optional_tags.clone(),
                    // Tighter than the CLI's 4 GiB. A checkpoint is a few
                    // milliseconds of index writing, and it bounds what is
                    // lost if the app is killed rather than closed — the CLI
                    // is a foreground process someone is watching, a launcher
                    // is not.
                    checkpoint_bytes: 512 * 1024 * 1024,
                    limit_bytes_per_second,
                    // So Battle.net lists the product as installed rather than
                    // offering to install what is already there.
                    agent_dir: Some(layout.agent_dir()),
                    ..InstallOptions::default()
                };

                // Coalesce here rather than in core: stages are rare and always
                // worth publishing, progress is not.
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut stage = String::from("preparing");
                let mut remaining = None;
                let mut optional: Vec<String> = Vec::new();

                let on_event = move |event: Event| {
                    let progress = match &event {
                        Event::Progress(p) => {
                            if last.elapsed() < Duration::from_millis(50) {
                                return;
                            }
                            last = Instant::now();
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
                            Progress::default()
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
                    .map_err(|e| e.to_string())
            });
            let _ = done_tx.send(result);
        })
        .map_err(|e| e.to_string())?;

    done_rx.await.map_err(|_| "the download thread stopped unexpectedly".to_owned())?
}

/// One short line describing a non-progress event, for the status readout.
fn describe(event: &Event) -> String {
    match event {
        Event::Build { versions_name, .. } => format!("build {versions_name}"),
        Event::Encoding { entries } => format!("encoding — {entries} entries"),
        Event::Manifests { download, .. } => format!("manifests — {download} entries"),
        // The whole product's size, not the job's — an update would flash
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
/// This is the seam Milestone 3 defined and left as a no-op: a launch must not
/// let Agent and our downloader write the same CASC files.
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

/// Launch the game directly under Proton — the `[ PLAY ]` path.
///
/// Blocking from start to finish: pre-flight spawns processes and the returned
/// future only resolves when the game exits.
/// Everything that must be settled before a launch: no stale processes in the
/// prefix, no transfer mid-flight.
///
/// Split from the launch itself because the two take wildly different amounts
/// of time — this is seconds, the launch is however long someone plays — and a
/// UI that cannot tell them apart ends up claiming it is still doing pre-flight
/// an hour into a raid.
pub async fn preflight(
    layout: Layout,
    code: ProductCode,
    downloads: SharedDownloads,
    runs: Runs,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        // The same build the launch will use. The prefix is shared, so
        // clearing it with one Proton and then launching under another is how
        // you get an unasked-for prefix upgrade between the two.
        let runner = runner_for(&layout, &code, runs)?;
        let mut guard = downloads;
        let config = AppDirs::discover()
            .ok()
            .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
            .unwrap_or_default();
        Preflight::new(Config::default().prefix_user)
            .with_managed(config.managed_products())
            .run(&runner, &layout, Some(&code), &mut guard)
            // The report is for the CLI, which prints it. Here the outcome is
            // binary: the prefix is ready, or the launch does not happen.
            .map(|_report| ())
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Run the game, blocking until it exits.
///
/// "Exits" is the whole wine process tree winding down, not the moment the
/// window disappears, so this returns a little after the game looks closed.
/// That tail is wanted — it is what makes the *next* launch's pre-flight find
/// a clean prefix — but it has to be visible as winding down rather than as a
/// dead button.
pub async fn run_game(layout: Layout, code: ProductCode) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        // The install's native path, *not* the `C:\Program Files (x86)`
        // symlink — this is half of what makes the in-game shop work, and the
        // other half is [`Layout::wow_drive`]. Wine resolves a Unix path to
        // the most specific drive mapped over it, so launching from here puts
        // the game on `D:`; launching through the symlink would keep it on
        // `C:` and the shop's browser would lose its GPU child process the way
        // it always did. Verified end to end 2026-10-02: a full checkout, from
        // a direct launch, with no Battle.net session.
        let flavor = layout.wow_install().join(&*code.install_dir());
        let exe = lightforge_core::paths::game_executable(&flavor, &code);
        if !exe.is_file() {
            return Err(format!("no game at {}", exe.display()));
        }

        let (tweaks, tools) = tweaks_for(&code);

        // Re-asserted here, not when the toggle was flipped: the client
        // rewrites Config.wtf from memory every time it exits, so a value
        // written once is a value the next session may well have thrown away.
        // This runs behind the pre-flight gate, with the client closed.
        let cvars = layout.wtf_dir(&code).join("Config.wtf");
        let (set, unset) = tweaks.cvars();
        if let Err(e) = lightforge_core::cvars::apply(&cvars, &set, &unset) {
            // Not fatal. A settings file Lightforge could not write is a
            // worse game, not an unplayable one, and refusing to launch over
            // it would be the wrong trade.
            eprintln!("lightforge: could not apply client settings: {e}");
        }

        let runner = runner_for(&layout, &code, Runs::Game)?;
        let mut launch = lightforge_core::tweaks::wrapped(
            runner.command("run", &exe, Vec::<String>::new()),
            &tweaks.wrappers(&tools),
        );
        let status = launch.status().map_err(|e| e.to_string())?;
        Ok(format!("game exited ({status})"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Hand off to Battle.net so the session is SSO'd and the in-game shop works.
/// Read the client's cached catalogue, for the game families in it.
///
/// About 25 ms against a real cache, and only at startup: the answer changes
/// when Blizzard publishes a new title, not while the launcher is open.
pub async fn game_families(layout: Layout, user: String) -> Families {
    tokio::task::spawn_blocking(move || Families::read(&layout, &user))
        .await
        .unwrap_or_default()
}

pub async fn launch_via_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
    family: Option<String>,
) -> Result<Handoff, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::battlenet::{self, Launch, Readiness};
        use lightforge_core::preflight;

        let runner = runner_for(&layout, &code, Runs::BattleNet)?;

        // Taken before the client is told anything, so a game that was
        // already playing is not mistaken for this launch's.
        let before = preflight::games_running(&layout);

        // Two phases, because one does not work. A cold client handed
        // `--exec=launch` parses it and then does nothing with it: measured
        // on a real start, the client was signed in after five seconds and
        // the game did not begin for another forty-eight. The arguments have
        // to arrive at a client that is already up, which is what the
        // client's own IPC queue is for — a second invocation hands them to
        // the first.
        if !preflight::client_running(&layout) {
            // Out of the way, but only where it can sign itself in. A
            // sign-in form nobody can see is a launch that quietly never
            // happens, so a prefix without auto-login gets a normal window.
            let minimize = battlenet::signs_in_by_itself(&layout, &user);
            let _ = battlenet::configure_accounts(
                &layout,
                &user,
                &battlenet::window_state_tweaks(minimize),
            );

            let started = std::time::SystemTime::now();
            battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| e.to_string())?;

            if battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT)
                == Readiness::NeedsAttention
            {
                // Stop driving. The client is up and is the only thing that
                // can finish this, so it is left running for the user.
                return Ok(Handoff::NeedsAttention);
            }
        }

        // The client can be gone by now: signing in takes a few seconds, and
        // a window nobody expected is a window somebody closes — with
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
        battlenet::launch(&runner, &layout, &launch).map_err(|e| e.to_string())?;

        Ok(
            match preflight::wait_for_game_start(&layout, &before, HANDOFF_TIMEOUT) {
                Some(game) => Handoff::Playing(game),
                None => Handoff::NothingStarted,
            },
        )
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Open the client's install flow for a game, dialog and all.
///
/// Three mechanisms were tried against a real client, and only this one
/// works, for one reason: the client always boots to its homepage.
///
/// - `--exec="launch <uid>"` sets the active product and the homepage then
///   draws over it. Nothing visible happens.
/// - `--uri="battlenet://<family>"` is handled properly, by a `UriController`
///   that waits until the client is ready — but all it does is select the
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
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
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
            .map_err(|e| e.to_string())?;

        Ok(format!("Battle.net is asking where to install {label}"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What came of a hand-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The game is running.
    Playing(GameProcess),
    /// The client never reported signing in. Someone has to look at it.
    NeedsAttention,
    /// It was there and then it was not — closed while we waited.
    ClientClosed,
    /// Signed in, told to launch, and nothing appeared.
    NothingStarted,
}

/// How long the client gets to start and sign itself in.
///
/// A cold start under Proton reaches "Logged into Battle.net successfully" in
/// about five and a half seconds. A minute is for the bad days — a client
/// updating itself, or a slow network — and running out of it is not an
/// error, just the point where a person is needed.
const SIGN_IN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a handed-off game gets to appear/// How long a handed-off game gets to appear before the launcher stops
/// claiming a launch is in progress.
///
/// Generous: a cold Battle.net under Wine has to start, update itself if it
/// feels like it, and may stop at a login screen — and the cost of being
/// wrong in this direction is only an unlocked button.
const HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Block until a handed-off game exits.
///
/// Split from the launch so the screen can say which game is running in
/// between, instead of "handing off…" for the length of a session.
pub async fn await_game_exit(layout: Layout, game: GameProcess) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::preflight::wait_for_game_exit(&layout, game.pid);
        Ok(format!("{} exited", game.name))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What Battle.net reports as installed, keyed by product code.
///
/// Read when the browse panel opens, never on a timer and never to decide
/// what the header shows — it labels a list the user asked to see.
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
        .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
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

pub async fn installed_versions(layout: Layout) -> HashMap<String, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::tact::productdb;

        productdb::read(&layout.agent_dir().join("product.db"))
            .unwrap_or_default()
            .into_iter()
            .filter_map(|i| Some((i.product, i.version?)))
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Open Battle.net with no product: the way to reach everything outside the
/// WoW family.
///
/// Deliberately a hand-off and nothing more. Lightforge does not track,
/// install or update other Blizzard titles — `SPEC.md` is explicit that it is
/// WoW-family-primary and a pass-through for the rest — so this opens the
/// client and stops there.
///
/// Still pre-flighted by the caller: a stale wineserver breaks Battle.net
/// exactly as it breaks a game launch.
pub async fn open_battle_net(layout: Layout, code: ProductCode) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::battlenet::{self, Launch};

        let runner = runner_for(&layout, &code, Runs::BattleNet)?;
        battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| e.to_string())?;
        Ok("Battle.net opening".to_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The Proton build a product launches under, with its per-product caches.
/// Which workload a runner is being built for.
///
/// They pin separately: `SPEC.md` asks for a Proton build chosen for
/// Battle.net and Agent specifically, because "latest" is not always best for
/// the client even when it is right for the game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// The game itself, under the product's own pin.
    Game,
    /// Battle.net or Agent, under the Battle.net pin.
    BattleNet,
}

fn runner_for(
    layout: &Layout,
    code: &ProductCode,
    runs: Runs,
) -> Result<lightforge_core::prefix::ProtonRunner, String> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let pin = match runs {
        Runs::Game => config.effective_proton(code),
        Runs::BattleNet => config.battle_net_proton_version(),
    };
    let build = proton::select(&builds, pin)
        .ok_or_else(|| "no supported Proton-GE build installed".to_owned())?;

    let target = std::fs::read_link(layout.wow_symlink()).unwrap_or_else(|_| layout.wow_install());
    let mut runner =
        ProtonRunner::new(build, layout, ProtonRunner::steam_client_path(steam.as_ref(), &dirs))
            .with_compat_mount(&target)
            // The prefix is shared, so pooling the shader cache would mean one
            // product's Proton swap discarding another's shaders.
            .with_shader_cache(dirs.shader_cache_dir(code));

    // Only the game: Battle.net and Agent are a different workload, and a
    // frame overlay or a power profile held for the storefront is not what the
    // toggle asked for.
    if runs == Runs::Game {
        let tweaks =
            config.product(code).map(|p| p.profile.tweaks.clone()).unwrap_or_default();
        for (key, value) in tweaks.environment(&HostTools::detect()) {
            runner = runner.with_env(key, value);
        }
    }
    Ok(runner)
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
        .map_err(|e| e.to_string())
        .and_then(|info| reclaim::Build::installed(&info, code).map_err(|e| e.to_string()));
    Some(match departing {
        Ok(build) => reclaim::plan(http, install, &build).await.map_err(|e| e.to_string()),
        Err(e) => Err(e),
    })
}

/// What uninstalling `code` would delete and free — nothing is touched.
pub async fn uninstall_plan(
    http: Option<Http>,
    layout: Layout,
    code: ProductCode,
) -> Result<UninstallPreview, String> {
    let install = layout.wow_install();
    let plan = {
        let (install, code) = (install.clone(), code.clone());
        tokio::task::spawn_blocking(move || {
            lightforge_core::tact::uninstall::plan(&install, &code).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| e.to_string())??
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
/// Refuses while anything is alive in the prefix — the same gate repair uses:
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
) -> Result<u64, String> {
    use lightforge_core::tact::{productdb, reclaim, uninstall};

    let alive = layout.clone();
    let busy = tokio::task::spawn_blocking(move || lightforge_core::preflight::scan(&alive))
        .await
        .map_err(|e| e.to_string())?;
    if let Some(process) = busy.first() {
        return Err(format!("close {} first — the game files are in use", process.name));
    }

    let install = layout.wow_install();
    let plan = uninstall::plan(&install, &code).map_err(|e| e.to_string())?;
    // A reclaim that cannot be planned is skipped, not fatal: the uninstall
    // itself is still exactly what was asked for.
    let reclaim = match &http {
        Some(http) => reclaim_plan(http, &install, &code, &plan).await.and_then(Result::ok),
        None => None,
    };

    tokio::task::spawn_blocking(move || {
        let removed = uninstall::execute(&install, &code, &plan).map_err(|e| e.to_string())?;
        let reclaimed = match &reclaim {
            Some(reclaim) => reclaim::execute(&install, reclaim).map_err(|e| e.to_string())?,
            None => 0,
        };
        productdb::unregister(&layout.agent_dir(), code.as_str()).map_err(|e| e.to_string())?;
        Ok(removed.bytes + reclaimed)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Measure Settings > Storage. Walks the whole game install.
pub async fn storage(layout: Layout, dirs: AppDirs) -> lightforge_core::storage::Report {
    let fallback = lightforge_core::storage::Report {
        root: layout.root().to_path_buf(),
        disk: None,
        usage: Vec::new(),
    };
    tokio::task::spawn_blocking(move || lightforge_core::storage::report(&layout, &dirs))
        .await
        .unwrap_or(fallback)
}

/// Clear Lightforge's own cache, shader caches aside; returns bytes freed.
pub async fn clear_cache(dirs: AppDirs) -> Result<u64, String> {
    tokio::task::spawn_blocking(move || lightforge_core::storage::clear_cache(&dirs))
        .await
        .map_err(|e| e.to_string())?
        .map_err(|e| e.to_string())
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
            std::fs::read(real.build_info()),
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
        std::fs::create_dir_all(layout.wow_install().join("Data/data")).unwrap();
        std::fs::write(layout.build_info(), &info).unwrap();
        std::fs::create_dir_all(layout.agent_dir()).unwrap();
        std::fs::write(layout.agent_dir().join("product.db"), &registry).unwrap();

        let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
        // No client: the reclaim is skipped, which is what this test isolates.
        runtime.block_on(uninstall(None, layout.clone(), ProductCode::ClassicEra)).unwrap();

        assert!(!era.exists());
        assert!(layout.wow_install().join("Data").exists(), "the shared store stays");
        let left = lightforge_core::buildinfo::BuildInfo::load(layout.build_info()).unwrap();
        let before = lightforge_core::buildinfo::BuildInfo::load(real.build_info()).unwrap();
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
mod mark_tests {
    use super::*;

    fn mark(path: &str) -> Mark {
        Mark { path: PathBuf::from(path), handle: Handle::from_rgba(1, 1, vec![0, 0, 0, 0]) }
    }

    /// The regression this guards: icons became decoded handles, and a handle
    /// built from pixels is a fresh id every time. Reassigning an identical
    /// set makes the renderer re-upload every texture, which is the band
    /// blinking a second after each product switch — the same fault the
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
/// Refresh re-downloads nothing and — more importantly — mints no new handle
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
        .unwrap_or_else(|_| Handle::from_bytes(bytes))
}

/// Fetch the News page's headlines.
///
/// Returns `None` only when there is genuinely nothing to show — no network
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
