use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::games::{client_running, games_running};
use super::scan::{scan, PrefixProcess, ProcKind};
use super::shutdown::{remove_dir, shutdown_timed, Disposition, Outcome};
use super::{PreflightError, DEFAULT_GRACE};
use crate::battlenet;
use crate::paths::Layout;
use crate::prefix::ProtonRunner;
use crate::product::ProductCode;

/// A downloader pre-flight pauses before Battle.net starts: it and Agent
/// write the same `data.NNN`/`.idx` files, and overlapping corrupts the store.
pub trait DownloadGuard {
    /// Pause any transfer writing to `code`'s CASC storage. `Ok(true)` means
    /// [`DownloadGuard::resume`] is owed; `Err` blocks the launch.
    fn pause(&mut self, code: &ProductCode) -> Result<bool, PreflightError>;

    /// Resume what [`DownloadGuard::pause`] stopped, once Battle.net has exited.
    fn resume(&mut self, code: &ProductCode) -> Result<bool, PreflightError>;
}

/// For callers with no downloader of their own.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoDownloads;

impl DownloadGuard for NoDownloads {
    fn pause(&mut self, _code: &ProductCode) -> Result<bool, PreflightError> {
        Ok(false)
    }

    fn resume(&mut self, _code: &ProductCode) -> Result<bool, PreflightError> {
        Ok(false)
    }
}

/// What a pre-flight pass is allowed to do.
#[derive(Debug, Clone)]
pub struct Preflight {
    /// Windows user inside the prefix, for locating the config files.
    user: String,
    /// Products Lightforge downloads itself, whose Battle.net auto-update is
    /// switched off again on every pass: the client rewrites its config on exit.
    managed: Vec<ProductCode>,
    grace: Duration,
    /// Leave a running Battle.net client (and its Agent) alone.
    keep_client: bool,
}

impl Preflight {
    pub fn new(user: impl Into<String>) -> Self {
        Self {
            user: user.into(),
            managed: Vec::new(),
            grace: DEFAULT_GRACE,
            keep_client: false,
        }
    }

    /// Do not close a Battle.net client that is already up, for a direct game
    /// launch. Cache and config work waits for a launch that finds it closed.
    pub fn keeping_client(mut self, keep: bool) -> Self {
        self.keep_client = keep;
        self
    }

    /// Products whose Battle.net auto-update should stay off.
    pub fn with_managed(mut self, managed: Vec<ProductCode>) -> Self {
        self.managed = managed;
        self
    }

    /// How long a process gets between `SIGTERM` and `SIGKILL`.
    pub fn with_grace(mut self, grace: Duration) -> Self {
        self.grace = grace;
        self
    }

    /// Run the full pass. Fails, without cleaning, if anything refused to die.
    pub fn run(
        &self,
        runner: &ProtonRunner,
        layout: &Layout,
        code: Option<&ProductCode>,
        downloads: &mut dyn DownloadGuard,
    ) -> Result<Report, PreflightError> {
        let mut timings = Vec::new();

        // A game already up keeps the prefix as it is: this launch goes beside
        // it, as Battle.net starts a second game, or not at all.
        let games = games_running(layout);
        if !games.is_empty() {
            if !served_by(runner, layout) {
                return Err(PreflightError::OtherBuild(games));
            }
            return self.run_beside(code, downloads, timings);
        }
        // A client under another build is closed instead: it would share a
        // wineserver with this launch.
        if self.keep_client && client_running(layout) && served_by(runner, layout) {
            return self.run_beside(code, downloads, timings);
        }
        let processes = shutdown_timed(runner, layout, self.grace, &mut timings)?;
        let mut at = Instant::now();
        let mut stamp = |timings: &mut Vec<Timing>, stage: &'static str| {
            timings.push(Timing { stage, took: at.elapsed() });
            at = Instant::now();
        };

        let survivors: Vec<PrefixProcess> = processes
            .iter()
            .filter(|o| o.disposition == Disposition::Survived)
            .map(|o| o.process.clone())
            .collect();
        if !survivors.is_empty() {
            return Err(PreflightError::StillRunning(survivors));
        }

        // The prefix is quiet from here. Only `Agent/data/cache`: the rest of
        // Agent's directory holds `product.db`, every install it knows.
        let agent_cache = remove_dir(&layout.agent_cache_dir())?;
        let staging = remove_dir(&layout.battle_net_staging())?;
        stamp(&mut timings, "clear caches");

        // The client recreates the per-account file on login and after a
        // prefix update, without `HideOnClose`, so it is re-applied every launch.
        let mut tweaks = battlenet::default_tweaks(layout);
        tweaks.extend(battlenet::product_tweaks(&self.managed));
        let mut configs = vec![battlenet::configure(layout, &self.user, &tweaks)?];
        configs.extend(battlenet::configure_accounts(
            layout,
            &self.user,
            &battlenet::account_tweaks(),
        )?);

        stamp(&mut timings, "battle.net config");

        let download_paused = match code {
            Some(code) => downloads.pause(code)?,
            None => false,
        };
        stamp(&mut timings, "pause transfers");

        Ok(Report {
            timings,
            processes,
            agent_cache_removed: agent_cache,
            staging_removed: staging,
            configs,
            download_paused,
        })
    }

    /// The pass when the prefix stays up, for a running client or game: the
    /// download is still paused, and nothing else is touched.
    pub(super) fn run_beside(
        &self,
        code: Option<&ProductCode>,
        downloads: &mut dyn DownloadGuard,
        mut timings: Vec<Timing>,
    ) -> Result<Report, PreflightError> {
        let at = Instant::now();
        let download_paused = match code {
            Some(code) => downloads.pause(code)?,
            None => false,
        };
        timings.push(Timing { stage: "pause transfers", took: at.elapsed() });
        Ok(Report {
            timings,
            processes: Vec::new(),
            agent_cache_removed: None,
            staging_removed: None,
            configs: Vec::new(),
            download_paused,
        })
    }
}

/// Whether the prefix's live `wineserver`, if any, is this runner's. Two Wine
/// builds cannot share one server. One whose binary cannot be read is let be.
pub(super) fn served_by(runner: &ProtonRunner, layout: &Layout) -> bool {
    let ours = runner.dist().join("bin").join("wineserver");
    let ours = ours.canonicalize().unwrap_or(ours);
    scan(layout)
        .into_iter()
        .filter(|p| p.kind == ProcKind::WineServer)
        .filter_map(|p| p.exe)
        .all(|exe| exe == ours)
}

/// Everything a pre-flight pass did, for the CLI and the UI's status line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Report {
    /// Processes found and how each was made to exit. Empty on a clean system.
    pub processes: Vec<Outcome>,
    /// Bytes reclaimed from Agent's directory cache, if it existed.
    pub agent_cache_removed: Option<u64>,
    /// Bytes reclaimed from the installer's staging directory, if it existed.
    pub staging_removed: Option<u64>,
    /// Config files re-applied, global first.
    pub configs: Vec<PathBuf>,
    /// Whether a download was paused and is owed a resume.
    pub download_paused: bool,
    /// How long each stage took, in order: which one a slow launch sits in.
    pub timings: Vec<Timing>,
}

/// One pre-flight stage and what it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub stage: &'static str,
    pub took: Duration,
}
