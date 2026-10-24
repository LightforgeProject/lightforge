//! Driving [`lightforge_core::setup`] from the GUI.
//!
//! Same shape as the download plumbing: the work runs off the UI thread and
//! publishes into a `watch` channel the dashboard samples on its timer. Setup
//! is chattier than it is frequent, so the channel carries the whole log rather
//! than one line. It is a handful of strings, and a UI that samples can
//! otherwise miss a line entirely.

use lightforge_core::error::chain;
use crate::tasks::Problem;
use lightforge_core::net::Http;
use lightforge_core::config::Config;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::setup::{self, Event, Status, Step};
use lightforge_core::proton::{self, ProtonRelease};

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

/// Install one Proton build, publishing progress to `tx`.
///
/// Deliberately not [`run`] with a pinned version: that goes on to the prefix
/// and Battle.net, which is not what Install beside a version list means.
pub async fn install_build(
    version: String,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<(), Problem> {
    let dirs = AppDirs::discover().map_err(|e| Problem::of(&e))?;
    setup::install_build(&dirs, &version, reporter(tx))
        .await
        .map(|_| ())
        .map_err(|e| Problem::of(&e))
}

/// Turn setup [`Event`]s into [`Progress`] updates on `tx`.
///
/// Shared by [`run`] and [`install_build`] so the log reads the same whichever
/// started the work.
fn reporter(tx: tokio::sync::watch::Sender<Progress>) -> impl FnMut(Event) {
    let mut progress = Progress::default();
    move |event: Event| {
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
    }
}

/// Provision whatever is missing, publishing progress to `tx`.
pub async fn run(
    layout: Layout,
    config: Config,
    proton_version: Option<String>,
    tx: tokio::sync::watch::Sender<Progress>,
) -> Result<Status, Problem> {
    let dirs = AppDirs::discover().map_err(|e| Problem::of(&e))?;
    setup::run(&layout, &dirs, &config, proton_version.as_deref(), reporter(tx))
        .await
        .map_err(|e| Problem::of(&e))
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

/// What one trip to GitHub tells us: whether a newer build exists, and every
/// build that could be installed.
///
/// Both come out of the same release lists, so they are fetched together, the
/// requests are ETag-revalidated and asking twice would buy nothing.
/// Notify-only, per the policy: nothing here installs anything by itself.
#[derive(Debug, Clone, Default)]
pub struct ProtonReleases {
    /// A newer stable Proton-GE than the installed one, if there is one.
    pub update: Option<String>,
    /// Every installable stable tag per family, newest first. A family whose
    /// list could not be fetched is left out, not shown empty.
    pub available: Vec<(proton::ProtonFamily, Vec<String>)>,
}

pub async fn check_proton(
    http: Option<Http>,
    installed: Option<String>,
) -> Result<ProtonReleases, String> {
    use lightforge_core::net::ETagCache;

    let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
    let http = http.ok_or(crate::tasks::NO_HTTP)?;

    let mut found = ProtonReleases::default();
    let mut first_error = None;
    for family in proton::ProtonFamily::ALL {
        let cache = ETagCache::new(dirs.asset_cache_dir().join(family.cache_file()));
        let releases = match proton::fetch_releases(&http, &cache, family).await {
            Ok(releases) => releases,
            Err(e) => {
                first_error.get_or_insert_with(|| chain(&e));
                continue;
            }
        };

        // Pre-releases are deliberately not offered: `latest_stable` skips
        // them for the update check, and a list that could install one would
        // make the two disagree about what "available" means.
        let tags = releases
            .iter()
            .filter(|release| !release.prerelease && release.tarball().is_some())
            .map(|release| release.tag_name.clone())
            .collect();
        found.available.push((family, tags));

        // The update check follows the family of the build in use.
        if family == proton::ProtonFamily::GloriousEggroll {
            found.update = installed
                .as_deref()
                .filter(|v| proton::ProtonFamily::of_tag(v) == Some(family))
                .and_then(|v| proton::check_for_update(v, &releases))
                .map(|notice| notice.available);
        }
    }

    // One list answering is enough to show something; none is a failure.
    match (found.available.is_empty(), first_error) {
        (true, Some(e)) => Err(e),
        _ => Ok(found),
    }
}

/// Measure every installed build, off the UI thread.
///
/// A Proton build is ~400 MB across thousands of files, so this is a task and
/// its answer is kept, walking it from `view` would cost that on every frame.
pub async fn measure(builds: Vec<std::path::PathBuf>) -> Vec<(std::path::PathBuf, u64)> {
    tokio::task::spawn_blocking(move || {
        builds
            .into_iter()
            .map(|path| {
                let size = lightforge_core::proton::size_on_disk(&path);
                (path, size)
            })
            .collect()
    })
    .await
    .unwrap_or_default()
}

/// Delete a managed build, refusing anything still pinned.
pub async fn remove_proton(config: Config, build: ProtonRelease) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let pinned_by = config.pinned_to(&build.version);
        lightforge_core::proton::remove_build(&build, &dirs.proton_dir(), &pinned_by)
            .map_err(|e| chain(&e))?;
        Ok(format!("removed {}", build.version))
    })
    .await
    .map_err(|e| chain(&e))?
}
