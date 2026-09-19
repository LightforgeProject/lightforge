//! Driving [`lightforge_core::setup`] from the GUI.
//!
//! Same shape as the download plumbing: the work runs off the UI thread and
//! publishes into a `watch` channel the dashboard samples on its timer. Setup
//! is chattier than it is frequent, so the channel carries the whole log rather
//! than one line — it is a handful of strings, and a UI that samples can
//! otherwise miss a line entirely.

use lightforge_core::config::Config;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::setup::{self, Event, Status, Step};

/// What setup publishes while it runs.
#[derive(Debug, Clone, Default)]
pub struct Progress {
    /// Step currently running, if any.
    pub step: Option<Step>,
    /// Finished steps, newest last, already rendered for display.
    pub log: Vec<String>,
    pub done: u64,
    pub total: Option<u64>,
}

impl Progress {
    /// Fraction of the current step's download, when it has a known size.
    pub fn fraction(&self) -> Option<f32> {
        let total = self.total?;
        (total > 0).then(|| (self.done as f64 / total as f64).clamp(0.0, 1.0) as f32)
    }
}

/// Provision whatever is missing, publishing progress to `tx`.
pub async fn run(
    layout: Layout,
    config: Config,
    proton_version: Option<String>,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Status, String> {
    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let mut progress = Progress::default();

    let on_event = move |event: Event| {
        match event {
            Event::Started(step) => {
                progress.step = Some(step);
                progress.done = 0;
                progress.total = None;
                progress.log.push(format!("{} — {}", step.label(), step.detail()));
            }
            Event::Progress { done, total } => {
                progress.done = done;
                progress.total = total;
            }
            Event::Finished { step, skipped, note } => {
                progress.step = None;
                progress.done = 0;
                progress.total = None;
                progress.log.push(match skipped {
                    true => format!("{} — already done ({note})", step.label()),
                    false => format!("{} — {note}", step.label()),
                });
            }
        }
        let _ = tx.send(progress.clone());
    };

    setup::run(&layout, &dirs, &config, proton_version.as_deref(), on_event)
        .await
        .map_err(|e| e.to_string())
}

/// Read what is already provisioned, off the UI thread.
pub async fn status(layout: Layout, config: Config) -> Option<Status> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().ok()?;
        Some(Status::inspect(&layout, &dirs, &config))
    })
    .await
    .ok()
    .flatten()
}

/// Ask GitHub whether a newer Proton-GE exists. Notify-only, per the policy:
/// nothing here installs anything by itself.
pub async fn check_proton(installed: String) -> Result<Option<String>, String> {
    use lightforge_core::net::{ETagCache, Http};
    use lightforge_core::proton;

    let dirs = AppDirs::discover().map_err(|e| e.to_string())?;
    let http = Http::new().map_err(|e| e.to_string())?;
    let cache = ETagCache::new(dirs.asset_cache_dir().join("proton-releases.json"));
    let releases = proton::fetch_releases(&http, &cache)
        .await
        .map_err(|e| e.to_string())?;

    Ok(proton::check_for_update(&installed, &releases).map(|notice| notice.available))
}
