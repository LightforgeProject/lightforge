//! The work the dashboard hands off: disk inspection, the version service, the
//! asset pipeline, downloads, and launching.
//!
//! Everything here returns owned data and never borrows application state, so
//! each one can be driven by `Task::perform` without the update loop holding a
//! lock across an await.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use lightforge_core::assets::{Branding, Manifest, Pipeline};
use lightforge_core::config::Config;
use lightforge_core::net::Http;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::preflight::{DownloadGuard, Preflight, PreflightError};
use lightforge_core::product::ProductCode;
use lightforge_core::state::Local;
use lightforge_core::tact::download::{Cancel, Downloads, Progress};
use lightforge_core::tact::install::{self, Event, Options as InstallOptions, Summary};
use lightforge_core::tact::version::{self, Region, Version};

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
        let branding = pipeline.cached(&code);
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
) -> HashMap<ProductCode, PathBuf> {
    codes
        .iter()
        .filter_map(|code| {
            let icon = pipeline.cached_icon(manifest, code)?;
            Some((code.clone(), icon.path))
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
    pub branding: Option<Branding>,
    pub icons: HashMap<ProductCode, PathBuf>,
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
            Ok(fresh) => Arc::new(fresh),
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
            icons.insert(product.clone(), icon.path);
        }
    }

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
    /// Bytes of this product already in the store when the run started.
    ///
    /// Without it the bar restarts at zero on every resume: `progress` counts
    /// only what *this* run stored, and a resumed plan excludes everything
    /// already on disk. The download was never starting over; the readout was.
    pub baseline: u64,
    /// Bytes the whole product needs, present ones included.
    pub product_bytes: u64,
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
                    // Tighter than the CLI's 4 GiB. A checkpoint is a few
                    // milliseconds of index writing, and it bounds what is
                    // lost if the app is killed rather than closed — the CLI
                    // is a foreground process someone is watching, a launcher
                    // is not.
                    checkpoint_bytes: 512 * 1024 * 1024,
                    limit_bytes_per_second,
                    ..InstallOptions::default()
                };

                // Coalesce here rather than in core: stages are rare and always
                // worth publishing, progress is not.
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut stage = String::from("preparing");
                // The product's full size, and how much of it the plan still
                // has to fetch — the difference is what the store already had.
                let mut product_bytes = 0u64;
                let mut baseline = 0u64;

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
                            if let Event::Wanted { bytes, .. } = other {
                                product_bytes = *bytes;
                            }
                            if let Event::Planned { bytes, .. } = other {
                                baseline = product_bytes.saturating_sub(*bytes);
                            }
                            stage = describe(other);
                            Progress::default()
                        }
                    };
                    let _ = tx.send(TransferState {
                        stage: stage.clone(),
                        progress,
                        baseline,
                        product_bytes,
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
        Event::Wanted { files, bytes } => {
            format!("{files} files, {:.1} GiB wanted", gib(*bytes))
        }
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
        Event::ProductDb { .. } => "registered with Battle.net".to_owned(),
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
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        let runner = runner_for(&layout, &code)?;
        let mut guard = downloads;
        Preflight::new(Config::default().prefix_user)
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
        let flavor = layout.wow_symlink().join(&*code.install_dir());
        let exe = lightforge_core::paths::game_executable(&flavor, &code);
        if !exe.is_file() {
            return Err(format!("no game at {}", exe.display()));
        }

        let runner = runner_for(&layout, &code)?;
        let status = runner
            .command("run", &exe, Vec::<String>::new())
            .status()
            .map_err(|e| e.to_string())?;
        Ok(format!("game exited ({status})"))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Hand off to Battle.net so the session is SSO'd and the in-game shop works.
pub async fn launch_via_battle_net(
    layout: Layout,
    code: ProductCode,
) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        use lightforge_core::battlenet::{self, Launch};

        let runner = runner_for(&layout, &code)?;
        let launch = Launch::Product { code: code.clone(), exec: None };
        battlenet::launch(&runner, &layout, &launch).map_err(|e| e.to_string())?;
        Ok(format!("Battle.net launching {}", code.as_str()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// What Battle.net reports as installed, keyed by product code.
///
/// Read when the browse panel opens, never on a timer and never to decide
/// what the header shows — it labels a list the user asked to see.
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

        let runner = runner_for(&layout, &code)?;
        battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| e.to_string())?;
        Ok("Battle.net opening".to_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The Proton build a product launches under, with its per-product caches.
fn runner_for(
    layout: &Layout,
    code: &ProductCode,
) -> Result<lightforge_core::prefix::ProtonRunner, String> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, config.effective_proton(code))
        .ok_or_else(|| "no supported Proton-GE build installed".to_owned())?;

    let target = std::fs::read_link(layout.wow_symlink()).unwrap_or_else(|_| layout.wow_install());
    Ok(
        ProtonRunner::new(build, layout, ProtonRunner::steam_client_path(steam.as_ref(), &dirs))
            .with_compat_mount(&target)
            // The prefix is shared, so pooling the shader cache would mean one
            // product's Proton swap discarding another's shaders.
            .with_shader_cache(dirs.shader_cache_dir(code)),
    )
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

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| format!("{url}: {e}"))
}
