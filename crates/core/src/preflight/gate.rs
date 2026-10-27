use std::path::PathBuf;
use std::time::{Duration, Instant};

use super::games::{client_running, games_running};
use super::scan::PrefixProcess;
use super::shutdown::{remove_dir, shutdown_timed, Disposition, Outcome};
use super::{PreflightError, DEFAULT_GRACE};
use crate::battlenet;
use crate::paths::Layout;
use crate::prefix::ProtonRunner;
use crate::product::ProductCode;

/// A downloader that pre-flight can pause before Battle.net starts.
///
/// This is the seam the native downloader plugs into. Both the
/// downloader and an `Agent.exe` verification pass write the same
/// `data.NNN`/`.idx` files, and letting them overlap corrupts CASC storage.
///
/// Implementations are expected to be cheap no-ops when nothing is in flight.
pub trait DownloadGuard {
    /// Pause any transfer writing to `code`'s CASC storage. `Ok(true)` means
    /// something was actually paused and [`DownloadGuard::resume`] is owed.
    /// An `Err` blocks the launch rather than risking the overlap.
    fn pause(&mut self, code: &ProductCode) -> Result<bool, PreflightError>;

    /// Resume what [`DownloadGuard::pause`] stopped, once Battle.net has exited.
    fn resume(&mut self, code: &ProductCode) -> Result<bool, PreflightError>;
}

/// For callers with no downloader of their own: there is never anything
/// to pause.
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
    /// re-disabled on every pass, the client rewrites its config on exit, so
    /// setting this once at install time does not hold.
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

    /// Do not close a Battle.net client that is already up.
    ///
    /// For a direct launch of the game, which has no need of the prefix
    /// being empty: the client is the only thing a scan finds besides its own
    /// helpers, so the pass has nothing else to clean. The caches and
    /// configs it would rewrite belong to that running client, which writes
    /// its config back on exit, so they wait for a launch that finds it
    /// closed. With no client up this changes nothing.
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

    /// Run the full pass. Fails rather than half-cleaning if anything in the
    /// prefix refused to die.
    pub fn run(
        &self,
        runner: &ProtonRunner,
        layout: &Layout,
        code: Option<&ProductCode>,
        downloads: &mut dyn DownloadGuard,
    ) -> Result<Report, PreflightError> {
        let mut timings = Vec::new();

        if self.keep_client && client_running(layout) {
            return self.run_beside_client(layout, code, downloads, timings);
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

        // Everything below this line depends on the prefix being quiet.
        // Agent's cache is `Agent/data/cache` and nothing else: clearing the
        // whole directory would take `product.db` with it, and Battle.net
        // would forget every install it has been shown.
        let agent_cache = remove_dir(&layout.agent_cache_dir())?;
        let staging = remove_dir(&layout.battle_net_staging())?;
        stamp(&mut timings, "clear caches");

        // The per-account file is the client's, not ours: it is recreated on
        // login and after a prefix update, each time without `HideOnClose`. A
        // one-shot write at setup does not survive, so it is re-applied here
        // on every launch.
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

    /// The pass when the client stays up: a game already running is still a
    /// refusal, and the download is still paused.
    pub(super) fn run_beside_client(
        &self,
        layout: &Layout,
        code: Option<&ProductCode>,
        downloads: &mut dyn DownloadGuard,
        mut timings: Vec<Timing>,
    ) -> Result<Report, PreflightError> {
        let at = Instant::now();
        let games = games_running(layout);
        if !games.is_empty() {
            return Err(PreflightError::GameRunning(games));
        }
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
    /// How long each stage took, in the order they ran. A launch that feels
    /// like it has hung is one of these, and which one is not guessable from
    /// the outside.
    pub timings: Vec<Timing>,
}

/// One pre-flight stage and what it cost.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timing {
    pub stage: &'static str,
    pub took: Duration,
}
