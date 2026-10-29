//! Downloading a build, and the queue guard pre-flight pauses through.

use super::*;

/// What a running transfer publishes for the UI to read.
///
/// Through a `watch` channel, newest value only: the callback runs 1.5
/// million times on retail.
#[derive(Debug, Clone, Default)]
pub struct TransferState {
    pub stage: String,
    pub progress: Progress,
    /// Bytes this run's plan has to fetch; `None` until the plan is made.
    ///
    /// The size of the job, not the product.
    pub remaining: Option<u64>,
    /// Optional content this build offers, as the manifest named it.
    pub optional: Vec<String>,
}

/// Download `code` into its install directory, publishing progress to `tx`.
///
/// A paused transfer is not an error; the next run resumes from the store.
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
    pre_download: bool,
    cancel: Cancel,
    tx: tokio::sync::watch::Sender<TransferState>,
) -> Result<Summary, Problem> {
    let (done_tx, done_rx) = tokio::sync::oneshot::channel();
    let stalls = tx.subscribe();

    // Its own thread and runtime: the fetch future is not `Send`, which
    // `Task::perform` requires.
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
                let watchdog = tokio::spawn(report_stalls(stalls));
                let result = async move {
                // Its own client: connections belong to the runtime that opened them.
                let http = Http::new().map_err(|e| Problem::of(&e))?;
                let dest = layout.wow_install(&code);
                // The library's own Windows path, for `.product.db`.
                let windows_path = layout
                    .library(&code)
                    .map(|library| library.windows_wow_path())
                    .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned());
                let options = InstallOptions {
                    region,
                    locale,
                    // Empty for a new install: its product config decides.
                    flavor: layout.known_flavor(&code).unwrap_or_default().to_owned(),
                    optional_tags: optional_tags.clone(),
                    // Tighter than the CLI's 4 GiB: nobody watches a launcher.
                    checkpoint_bytes: 512 * 1024 * 1024,
                    limit_bytes_per_second,
                    // So Battle.net lists the product as installed instead of
                    // offering to install what is already there.
                    agent_dir: Some(layout.agent_dir()),
                    windows_path: Some(windows_path),
                    stage: pre_download,
                    ..InstallOptions::default()
                };

                // Coalesce here, not in core: stages are rare and always
                // worth publishing, progress is not.
                let mut last = Instant::now() - Duration::from_secs(1);
                let mut stage = String::from("preparing");
                let mut remaining = None;
                let mut optional: Vec<String> = Vec::new();
                // A stage event carries no progress, so the last figures stay.
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
                }
                .await;
                watchdog.abort();
                result
            });
            let _ = done_tx.send(result);
        })
        .map_err(|e| Problem::of(&e))?;

    done_rx.await.map_err(|_| "the download thread stopped unexpectedly".to_owned())?
}

/// Write to the log when a running transfer stops moving.
///
/// A stuck download raises no error. After a minute without progress, then
/// every five minutes.
async fn report_stalls(mut state: tokio::sync::watch::Receiver<TransferState>) {
    const TICK: Duration = Duration::from_secs(15);
    let key = |s: &TransferState| (s.stage.clone(), s.progress.bytes_stored, s.progress.requests_done);
    let mut last = key(&state.borrow_and_update());
    let mut still = Duration::ZERO;
    loop {
        tokio::time::sleep(TICK).await;
        let now = key(&state.borrow_and_update());
        if now != last {
            last = now;
            still = Duration::ZERO;
            continue;
        }
        still += TICK;
        if still == Duration::from_secs(60) || (still.as_secs() > 60 && still.as_secs().is_multiple_of(300)) {
            let progress = state.borrow().progress;
            tracing::warn!(
                "transfer stalled for {}s at \"{}\": {}/{} B, {}/{} requests",
                still.as_secs(),
                last.0,
                progress.bytes_stored,
                progress.bytes_total,
                progress.requests_done,
                progress.requests_total,
            );
        }
    }
}

/// One short line describing a non-progress event, for the status readout.
pub(crate) fn describe(event: &Event) -> String {
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
        Event::Staged { version } => format!("pre-downloaded {version}"),
        Event::ProductDb { .. } => "writing .product.db".to_owned(),
        Event::Registered { .. } => "registered with Battle.net".to_owned(),
        Event::Incomplete { reasons } => format!("incomplete: {}", reasons.join(", ")),
    }
}

pub(crate) fn gib(bytes: u64) -> f64 {
    bytes as f64 / 1024f64.powi(3)
}

/// `Downloads` behind a lock, so pre-flight can pause a transfer the UI owns.
///
#[derive(Debug, Clone, Default)]
pub struct SharedDownloads(pub Arc<Mutex<Downloads>>);

impl SharedDownloads {
    /// Poison is ignored: every method is a single insert or remove.
    fn lock(&self) -> std::sync::MutexGuard<'_, Downloads> {
        self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn register(&self, code: ProductCode, cancel: Cancel) {
        self.lock().register(code, cancel);
    }

    pub fn finished(&self, code: &ProductCode) {
        self.lock().finished(code);
    }

    /// Products a launch paused and owes a restart.
    pub fn pending(&self) -> Vec<ProductCode> {
        self.lock().pending().cloned().collect()
    }
}

impl DownloadGuard for SharedDownloads {
    fn pause(&mut self, code: &ProductCode) -> Result<bool, PreflightError> {
        self.lock().pause(code)
    }

    fn resume(&mut self, code: &ProductCode) -> Result<bool, PreflightError> {
        // The GUI restarts the transfer itself, since resuming means building
        // a fresh plan against what the store now holds.
        self.lock().resume(code)
    }
}

/// Remove what a cancelled download wrote, and nothing else.
///
/// The whole store when nothing else is installed; otherwise entries only
/// this build wants. The build comes from the version service, so offline
/// the files stay.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_poisoned_lock_still_pauses_the_download() {
        let mut downloads = SharedDownloads::default();
        let poisoner = downloads.clone();
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.0.lock().unwrap();
            panic!("poison the downloads lock");
        })
        .join();
        assert!(downloads.0.is_poisoned());

        downloads.register(ProductCode::Classic, Cancel::new());
        assert!(downloads.pause(&ProductCode::Classic).unwrap());
        assert_eq!(downloads.pending(), [ProductCode::Classic]);
    }
}
