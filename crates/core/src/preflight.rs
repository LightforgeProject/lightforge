//! Pre-flight: everything that has to be true before Battle.net is spawned.
//!
//! The problem this solves is the "Initializing Data…" deadlock. A previous
//! session leaves `Agent.exe`, or the whole client, alive inside the prefix;
//! the next launch finds Agent's directory cache describing a world that no
//! longer exists, and Agent responds by scanning 126 GB of CASC storage.
//!
//! The pass, in order:
//!
//! 1. [`scan`] the process table for anything belonging to *this* prefix,
//! 2. `wineserver -k` for a graceful, prefix-scoped shutdown,
//! 3. re-scan and signal the stragglers, `SIGTERM` then `SIGKILL`,
//! 4. with nothing alive: delete Agent's directory *cache* and the installer's
//!    staging directory, never Agent's own directory, which holds the
//!    `product.db` registry of installed games,
//! 5. re-apply the Battle.net config defaults,
//! 6. pause any download writing to CASC storage.
//!
//! A process is only ever signalled when its own environment names
//! Lightforge's prefix (`WINEPREFIX` or `STEAM_COMPAT_DATA_PATH`). Someone
//! running Battle.net out of Lutris or Steam has processes with identical
//! names and a different prefix, so killing on name alone would be a
//! data-loss bug. A process whose environment cannot be read is left alone.
//!
//! Steps 4 and 5 are gated on step 3 succeeding: the client rewrites its
//! configs on exit and Agent memory-maps its cache, so writing into a live
//! prefix loses the write at best.
//!
//! The same process table answers the question at the *other* end of a
//! launch; see [`wait_for_game_start`]. Handing a game to Battle.net gives
//! nothing to wait on, because the client exits as soon as the game is up.

use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System, UpdateKind};

use crate::battlenet::{self, BattleNetError};
use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};
use crate::product::ProductCode;

/// How long a process gets to exit on `SIGTERM` before `SIGKILL` follows.
pub const DEFAULT_GRACE: Duration = Duration::from_secs(5);

/// Polling interval while waiting for signalled processes to disappear.
const POLL: Duration = Duration::from_millis(100);

/// Environment variables that identify which prefix a process belongs to.
const PREFIX_VARS: [&str; 2] = ["WINEPREFIX", "STEAM_COMPAT_DATA_PATH"];

/// What a process found inside the prefix is.
///
/// The distinction is not cosmetic: [`ProcKind::Agent`] is the one that causes
/// the repair scan, and [`ProcKind::WineServer`] is the one that must die last,
/// because everything else dies with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcKind {
    /// `Battle.net.exe` or `Battle.net Launcher.exe`.
    BattleNet,
    /// `Agent.exe`: the update/repair service.
    Agent,
    /// A CEF or crash-handler child of the client.
    Helper,
    /// The prefix's own `wineserver`.
    WineServer,
}

impl ProcKind {
    /// Classify a process from its command line.
    ///
    /// `argv[0]` is the only dependable source here. Linux reports a process's
    /// name from `comm`, which is the *thread* name, and the client is CEF,
    /// so a live `Battle.net.exe` shows up as `CrBrowserMain`, `CrGpuMain` or
    /// `CrRendererMain` and a name-based match misses the one process this
    /// whole module exists to clean up. `argv[0]` is the Windows path Wine was
    /// handed, e.g. `C:\Program Files (x86)\Battle.net\Battle.net.exe`.
    ///
    /// Returns `None` for anything Lightforge has no business touching.
    fn classify(argv: &[std::ffi::OsString], name: &str) -> Option<Self> {
        // Matching is case-insensitive: Wine's spelling of an executable is
        // not dependable across builds.
        let exe = executable_name(argv, name).to_ascii_lowercase();
        let kind = match exe.trim_end_matches(".exe") {
            "battle.net" | "battle.net launcher" => Self::BattleNet,
            "agent" => Self::Agent,
            "blizzarderror" | "bnetcrashhandler" => Self::Helper,
            "wineserver" => Self::WineServer,
            _ => return None,
        };

        // CEF's own children re-exec the same binary with `--type=`. They are
        // helpers, not the client, and go first so the browser process isn't
        // left rendering into dead children while it shuts down.
        if kind == Self::BattleNet && argv.iter().skip(1).any(is_cef_child_arg) {
            return Some(Self::Helper);
        }
        Some(kind)
    }

    /// Termination order: children before the server that owns them.
    fn kill_order(self) -> u8 {
        match self {
            Self::Helper => 0,
            Self::Agent => 1,
            Self::BattleNet => 2,
            Self::WineServer => 3,
        }
    }
}

/// The executable's file name, from `argv[0]` where there is one.
///
/// Splits on both separators: `argv[0]` is a Windows path for anything running
/// under Wine and a POSIX one for `wineserver` itself.
fn executable_name(argv: &[std::ffi::OsString], fallback: &str) -> String {
    let Some(argv0) = argv.first().map(|a| a.to_string_lossy()) else {
        return fallback.to_owned();
    };
    if argv0.is_empty() {
        return fallback.to_owned();
    }
    argv0
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&argv0)
        .to_owned()
}

/// `C:\...` or anything ending `.exe`: what Wine puts in `argv[0]`.
fn is_windows_exe(argv0: &std::ffi::OsString) -> bool {
    let argv0 = argv0.to_string_lossy();
    let bytes = argv0.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || argv0.to_ascii_lowercase().ends_with(".exe")
}

fn is_cef_child_arg(arg: &std::ffi::OsString) -> bool {
    arg.to_string_lossy().starts_with("--type=")
}

/// A process proven to belong to Lightforge's prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixProcess {
    pub pid: u32,
    pub name: String,
    pub kind: ProcKind,
    pub exe: Option<PathBuf>,
}

/// How a process was made to exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Gone after `wineserver -k`, before any signal was sent.
    Wineserver,
    /// Exited on `SIGTERM`.
    Terminated,
    /// Needed `SIGKILL`.
    Killed,
    /// Still alive after both. The launch must not proceed.
    Survived,
}

/// One process and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub process: PrefixProcess,
    pub disposition: Disposition,
}

/// Find every process belonging to the prefix described by `layout`.
///
/// Returns them in termination order. An empty result is the normal state of a
/// healthy system, not a failure to detect anything.
pub fn scan(layout: &Layout) -> Vec<PrefixProcess> {
    let mut system = System::new();
    refresh(&mut system);
    collect(&system, layout)
}

/// Refresh only what ownership and classification need. The default refresh
/// walks disks, networks and CPU state as well, which on a machine with a
/// 126 GB install is a measurable stall for data nobody here reads.
fn refresh(system: &mut System) {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
}

fn collect(system: &System, layout: &Layout) -> Vec<PrefixProcess> {
    let prefixes = owned_prefixes(layout);

    let mut found: Vec<PrefixProcess> = system
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            // Threads share their process's environment and command line, so
            // they match every test below, and signalling one is not how a
            // process is stopped. Only thread-group leaders are candidates.
            if proc.thread_kind().is_some() {
                return None;
            }
            let fallback = proc.name().to_string_lossy();
            let kind = ProcKind::classify(proc.cmd(), &fallback)?;
            if !belongs_to(proc.environ(), &prefixes) {
                return None;
            }
            Some(PrefixProcess {
                pid: pid.as_u32(),
                name: executable_name(proc.cmd(), &fallback),
                kind,
                exe: proc.exe().map(Path::to_path_buf),
            })
        })
        .collect();

    found.sort_by_key(|p| (p.kind.kill_order(), p.pid));
    found
}

/// Every spelling of "our prefix" a process's environment might carry.
///
/// Both the compat-data directory and the `pfx` inside it count, because
/// Proton exports the first and Wine exports the second. Canonical forms are
/// included so a prefix reached through a symlinked home still matches.
fn owned_prefixes(layout: &Layout) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    for path in [layout.compat_data(), layout.prefix()] {
        if let Ok(real) = path.canonicalize() {
            set.insert(real);
        }
        set.insert(path);
    }
    set
}

/// Whether a process's environment names one of `prefixes`.
///
/// Unreadable or absent, the case for a process owned by another user, or one
/// that exited mid-scan, means "not ours". Refusing to signal a process we
/// cannot attribute is the whole safety property of this module.
fn belongs_to(environ: &[std::ffi::OsString], prefixes: &HashSet<PathBuf>) -> bool {
    environ.iter().any(|entry| {
        let Some(value) = PREFIX_VARS.iter().find_map(|var| strip_var(entry, var)) else {
            return false;
        };
        let path = PathBuf::from(value);
        prefixes.contains(&path)
            || matches!(path.canonicalize(), Ok(real) if prefixes.contains(&real))
    })
}

/// `KEY=value` -> `value`, for one key.
fn strip_var<'a>(entry: &'a OsStr, var: &str) -> Option<&'a OsStr> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = entry.as_bytes();
    let prefix = var.as_bytes();
    let rest = bytes.strip_prefix(prefix)?;
    let rest = rest.strip_prefix(b"=")?;
    Some(OsStr::from_bytes(rest))
}

/// Shut the prefix down: `wineserver -k` first, then signals for whatever
/// ignored it.
///
/// `wineserver -k` is preferred because it is the shutdown Wine itself defines,
/// the client gets to flush `Battle.net.config` instead of being killed
/// mid-write, and because it is scoped to one `WINEPREFIX` by construction.
/// Signals are the fallback for a genuinely orphaned process whose wineserver
/// is already gone, which is precisely the case that produces the deadlock.
pub fn shutdown(
    runner: &ProtonRunner,
    layout: &Layout,
    grace: Duration,
) -> Result<Vec<Outcome>, PreflightError> {
    shutdown_timed(runner, layout, grace, &mut Vec::new())
}

/// [`shutdown`], stamping how long each stage took into `timings`.
///
/// The stages are wildly different in cost, a scan is 60 ms and a signal grace
/// is up to five seconds, and when a launch feels like it has hung this is the
/// only thing that says which one it is sitting in.
fn shutdown_timed(
    runner: &ProtonRunner,
    layout: &Layout,
    grace: Duration,
    timings: &mut Vec<Timing>,
) -> Result<Vec<Outcome>, PreflightError> {
    let mut at = Instant::now();
    let mut stamp = |timings: &mut Vec<Timing>, stage: &'static str| {
        timings.push(Timing { stage, took: at.elapsed() });
        at = Instant::now();
    };

    let found = scan(layout);
    stamp(timings, "scan");
    if found.is_empty() {
        return Ok(Vec::new());
    }

    // `wineserver -k` below takes every process in the prefix with it, and a
    // running game is in the prefix too. The GUI never gets here mid-session,
    // but the CLI's `play` and Battle.net launch would end a raid without a
    // logout or a SavedVariables write.
    let games = games_running(layout);
    if !games.is_empty() {
        return Err(PreflightError::GameRunning(games));
    }

    // Idempotent and near-instant on a prefix with no server running.
    runner.wineserver_kill()?;
    stamp(timings, "wineserver -k -w");

    let mut system = System::new();
    refresh(&mut system);
    let remaining = collect(&system, layout);
    stamp(timings, "rescan");

    let mut outcomes: Vec<Outcome> = found
        .iter()
        .filter(|p| !remaining.iter().any(|r| r.pid == p.pid))
        .map(|p| Outcome { process: p.clone(), disposition: Disposition::Wineserver })
        .collect();

    if remaining.is_empty() {
        outcomes.sort_by_key(|o| o.process.pid);
        return Ok(outcomes);
    }

    let stubborn = signal_all(&mut system, layout, &pids_of(&remaining), Signal::Term, grace);
    stamp(timings, "SIGTERM");
    let (ignored_term, exited): (Vec<_>, Vec<_>) =
        remaining.into_iter().partition(|p| stubborn.contains(&p.pid));
    outcomes.extend(exited.into_iter().map(|process| Outcome {
        process,
        disposition: Disposition::Terminated,
    }));

    if !ignored_term.is_empty() {
        let survivors = signal_all(&mut system, layout, &pids_of(&ignored_term), Signal::Kill, grace);
        stamp(timings, "SIGKILL");
        outcomes.extend(ignored_term.into_iter().map(|process| {
            let disposition = if survivors.contains(&process.pid) {
                Disposition::Survived
            } else {
                Disposition::Killed
            };
            Outcome { process, disposition }
        }));
    }

    outcomes.sort_by_key(|o| o.process.pid);
    Ok(outcomes)
}

fn pids_of(processes: &[PrefixProcess]) -> Vec<u32> {
    processes.iter().map(|p| p.pid).collect()
}

/// Send `signal` to each process, then wait up to `grace` for them all to go
/// away. Returns the pids still alive at the end.
///
/// Every pid is re-attributed against the prefix immediately before it is
/// signalled. Between the scan and the signal a pid can be recycled onto an
/// unrelated process, and signalling that one would be indefensible.
fn signal_all(
    system: &mut System,
    layout: &Layout,
    targets: &[u32],
    signal: Signal,
    grace: Duration,
) -> HashSet<u32> {
    let prefixes = owned_prefixes(layout);
    let mut pending: HashSet<u32> = HashSet::new();

    for &target in targets {
        let pid = Pid::from_u32(target);
        let Some(proc) = system.process(pid) else {
            continue;
        };
        if !belongs_to(proc.environ(), &prefixes) {
            continue;
        }
        if proc.kill_with(signal).is_some() {
            pending.insert(target);
        }
    }

    let deadline = Instant::now() + grace;
    while !pending.is_empty() && Instant::now() < deadline {
        std::thread::sleep(POLL);
        let pids: Vec<Pid> = pending.iter().map(|p| Pid::from_u32(*p)).collect();
        system.refresh_processes_specifics(
            ProcessesToUpdate::Some(&pids),
            true,
            ProcessRefreshKind::nothing(),
        );
        // A zombie has exited and is only waiting to be reaped by its parent;
        // counting it would report a dead process as one that survived.
        pending.retain(|pid| {
            system
                .process(Pid::from_u32(*pid))
                .is_some_and(|p| p.status() != sysinfo::ProcessStatus::Zombie)
        });
    }
    pending
}

/// Delete a directory, reporting the bytes reclaimed. A missing directory is
/// success with nothing reclaimed.
fn remove_dir(dir: &Path) -> Result<Option<u64>, PreflightError> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let bytes = crate::storage::dir_size(dir);
    std::fs::remove_dir_all(dir)
        .map_err(|source| PreflightError::Io { path: dir.to_path_buf(), source })?;
    Ok(Some(bytes))
}

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
}

impl Preflight {
    /// The pass when the client stays up: a game already running is still a
    /// refusal, and the download is still paused.
    fn run_beside_client(
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

// ---------------------------------------------------------------------------
// Watching a hand-off
// ---------------------------------------------------------------------------

/// How often the prefix is re-scanned while waiting on a handed-off game.
///
/// Slower than [`POLL`]: that one is a shutdown the user is waiting through,
/// this one runs for the length of a gaming session.
const HANDOFF_POLL: Duration = Duration::from_millis(500);

/// Wine's own processes, which live in every prefix and are nobody's game.
///
/// The before-snapshot is the primary filter, these are already running when
/// the hand-off starts, but Wine spawns services on demand too, and a
/// `winedevice.exe` that appears two seconds after the click would otherwise
/// be reported as the game and "played" for a quarter of a second.
///
/// The last four outlive the game: `steam.exe` is the shim every `proton run`
/// goes through, `xalia.exe` the accessibility helper GE-Proton starts beside
/// it, `lsass.exe` is started on demand by a login and `rundll32.exe` by the
/// shell. Counted as a game they kept the prefix "busy" for good: every later
/// Play was refused as a game still running.
const WINE_SERVICES: [&str; 16] = [
    "services",
    "winedevice",
    "plugplay",
    "explorer",
    "rpcss",
    "svchost",
    "conhost",
    "start",
    "wineboot",
    "winemenubuilder",
    "tabtip",
    "msiexec",
    "steam",
    "xalia",
    "lsass",
    "rundll32",
];

/// A process in the prefix that is neither Battle.net's nor Wine's own.
///
/// After a hand-off there is exactly one thing that fits: the game Battle.net
/// was asked to start. Identifying it this way rather than by name is what
/// makes the hand-off work for a title Lightforge has never heard of.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GameProcess {
    pub pid: u32,
    pub name: String,
}

/// Whether a Battle.net client is already up in this prefix.
///
/// The question a hand-off asks first: a client that is already running and
/// signed in can be handed a launch command straight away, and starting a
/// second one would only be told to go away by the first.
pub fn client_running(layout: &Layout) -> bool {
    scan(layout).iter().any(|p| p.kind == ProcKind::BattleNet)
}

/// Processes in the prefix that are not Battle.net's and not Wine's own.
pub fn games_running(layout: &Layout) -> Vec<GameProcess> {
    let mut system = System::new();
    refresh(&mut system);
    collect_games(&system, layout)
}

fn collect_games(system: &System, layout: &Layout) -> Vec<GameProcess> {
    let prefixes = owned_prefixes(layout);

    system
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            if proc.thread_kind().is_some() {
                return None;
            }
            let fallback = proc.name().to_string_lossy();
            let name = names_a_game(proc.cmd(), &fallback)?;
            if !belongs_to(proc.environ(), &prefixes) {
                return None;
            }
            Some(GameProcess { pid: pid.as_u32(), name })
        })
        .collect()
}

/// The executable's name, when a process could be a game at all.
///
/// Ownership is checked separately; this is the "what is it" half. Three
/// things are not games: Battle.net's own processes, Wine's plumbing, and a
/// browser child re-execing its parent.
fn names_a_game(argv: &[std::ffi::OsString], fallback: &str) -> Option<String> {
    // Anything Battle.net's is accounted for elsewhere; what is left is
    // either Wine's own or the game.
    if ProcKind::classify(argv, fallback).is_some() {
        return None;
    }
    // Proton's own `python3`, and any wrapper such as `gamemoderun`, carries
    // the prefix in its environment too, but a game under Wine always has a
    // Windows executable as `argv[0]`.
    if argv.first().is_some_and(|a| !a.is_empty() && !is_windows_exe(a)) {
        return None;
    }
    let name = executable_name(argv, fallback);
    let stem = name.to_ascii_lowercase();
    if WINE_SERVICES.contains(&stem.trim_end_matches(".exe")) {
        return None;
    }
    // A CEF or Electron child re-execs its parent, and the parent is the
    // process that represents the game.
    if argv.iter().skip(1).any(is_cef_child_arg) {
        return None;
    }
    Some(name)
}

/// A game process and how it was made to stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stopped {
    pub game: GameProcess,
    pub disposition: Disposition,
}

/// Stop the running game, and only the game.
///
/// Not [`shutdown`]: `wineserver -k` takes the whole prefix with it, which is
/// Battle.net and any other game started from it. This signals the processes
/// [`games_running`] names, `SIGTERM` first so Wine can tear its windows
/// down, `SIGKILL` for whatever is still there after `grace`. Each pid is
/// re-attributed to the prefix right before it is signalled, as in
/// [`signal_all`]. Lifecycle only: nothing here reads or writes the game.
///
/// The prefix is left for the next pre-flight to settle, exactly as after a
/// game that closed by itself.
pub fn stop_games(layout: &Layout, grace: Duration) -> Vec<Stopped> {
    let mut system = System::new();
    refresh(&mut system);
    let games = collect_games(&system, layout);
    if games.is_empty() {
        return Vec::new();
    }

    let pids: Vec<u32> = games.iter().map(|g| g.pid).collect();
    let stubborn = signal_all(&mut system, layout, &pids, Signal::Term, grace);
    let stubborn_pids: Vec<u32> = stubborn.iter().copied().collect();
    let survivors = match stubborn_pids.is_empty() {
        true => HashSet::new(),
        false => signal_all(&mut system, layout, &stubborn_pids, Signal::Kill, grace),
    };

    games
        .into_iter()
        .map(|game| {
            let disposition = match (stubborn.contains(&game.pid), survivors.contains(&game.pid)) {
                (false, _) => Disposition::Terminated,
                (true, false) => Disposition::Killed,
                (true, true) => Disposition::Survived,
            };
            Stopped { game, disposition }
        })
        .collect()
}

/// Wait for a handed-off game to start.
///
/// `before` is what [`games_running`] returned *prior* to the hand-off, so a
/// game already playing does not count as this launch's. `None` means nothing
/// started in time, almost always a client sitting at its login screen,
/// which is not an error, but is the point at which the launcher has to stop
/// claiming a launch is in progress.
///
/// This is the half of a hand-off that Battle.net does not give back: the
/// client exits as soon as the game is up (`GameLaunchWindowBehavior: 2`), so
/// the child process says nothing about whether anything is playing.
pub fn wait_for_game_start(
    layout: &Layout,
    before: &[GameProcess],
    appear_within: Duration,
) -> Option<GameProcess> {
    let known: HashSet<u32> = before.iter().map(|p| p.pid).collect();
    let deadline = Instant::now() + appear_within;

    loop {
        if let Some(found) = games_running(layout).into_iter().find(|p| !known.contains(&p.pid)) {
            return Some(found);
        }
        if Instant::now() >= deadline {
            return None;
        }
        std::thread::sleep(HANDOFF_POLL);
    }
}

/// Block until a game seen by [`wait_for_game_start`] is gone.
///
/// Watched by pid: a title that re-execs itself becomes a different process,
/// and calling the session over at that point is better than hanging forever
/// on a pid that is never coming back.
pub fn wait_for_game_exit(layout: &Layout, pid: u32) {
    while games_running(layout).iter().any(|p| p.pid == pid) {
        std::thread::sleep(HANDOFF_POLL);
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PreflightError {
    #[error(
        "{} in the prefix survived SIGKILL: {}",
        if .0.len() == 1 { "a process" } else { "processes" },
        .0.iter().map(|p| format!("{} (pid {})", p.name, p.pid)).collect::<Vec<_>>().join(", ")
    )]
    StillRunning(Vec<PrefixProcess>),

    #[error(
        "a game is running in the prefix ({}); close it first, cleaning the prefix would end it",
        .0.iter().map(|g| format!("{} (pid {})", g.name, g.pid)).collect::<Vec<_>>().join(", ")
    )]
    GameRunning(Vec<GameProcess>),

    #[error("a download for `{0}` is writing to CASC storage and could not be paused")]
    DownloadBusy(ProductCode),

    #[error(transparent)]
    Prefix(#[from] PrefixError),

    #[error(transparent)]
    BattleNet(#[from] BattleNetError),

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
    use std::ffi::OsString;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-preflight-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn environ(vars: &[&str]) -> Vec<OsString> {
        vars.iter().map(OsString::from).collect()
    }

    /// `argv` as Wine presents it, plus the `comm` Linux reports.
    fn argv(parts: &[&str]) -> Vec<OsString> {
        parts.iter().map(OsString::from).collect()
    }

    const BNET: &str = r"C:\Program Files (x86)\Battle.net\Battle.net.exe";
    const AGENT: &str = r"C:\ProgramData\Battle.net\Agent\Agent.9775\Agent.exe";

    #[test]
    fn a_pass_beside_the_client_leaves_it_running_and_touches_nothing() {
        use std::process::{Command, Stdio};

        let layout = Layout::with_root("/games/Blizzard");
        // A process that presents itself as the client, the way Wine does.
        let mut client = Command::new("bash")
            .args(["-c", r#"exec -a 'C:\Program Files (x86)\Battle.net\Battle.net.exe' sleep 30"#])
            .env("WINEPREFIX", layout.prefix())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        std::thread::sleep(Duration::from_millis(300));
        assert!(client_running(&layout), "the stand-in is recognised as the client");

        let report = Preflight::new("steamuser")
            .keeping_client(true)
            .run_beside_client(&layout, None, &mut NoDownloads, Vec::new())
            .unwrap();
        assert!(report.processes.is_empty() && report.configs.is_empty());
        assert!(report.agent_cache_removed.is_none() && report.staging_removed.is_none());
        assert!(client.try_wait().unwrap().is_none(), "the client is still up");
        let _ = client.kill();
        let _ = client.wait();
    }

    #[test]
    fn a_process_that_ignores_sigterm_is_killed_and_a_stranger_is_left_alone() {
        use std::process::{Command, Stdio};

        let layout = Layout::with_root("/games/Blizzard");
        let prefix = layout.prefix();
        let stubborn = |env: Option<&Path>| {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            if let Some(prefix) = env {
                cmd.env("WINEPREFIX", prefix);
            }
            cmd.spawn().unwrap()
        };
        let mut ours = stubborn(Some(&prefix));
        let mut stranger = stubborn(None);
        std::thread::sleep(Duration::from_millis(200));

        let mut system = System::new();
        refresh(&mut system);
        let grace = Duration::from_millis(400);
        let ignored = signal_all(&mut system, &layout, &[ours.id(), stranger.id()], Signal::Term, grace);
        assert_eq!(ignored, HashSet::from([ours.id()]), "SIGTERM is ignored, and the stranger was never signalled");
        let survivors = signal_all(&mut system, &layout, &[ours.id()], Signal::Kill, Duration::from_secs(2));
        assert!(survivors.is_empty());

        assert!(ours.wait().is_ok());
        // Still running: it was not ours to signal.
        assert!(stranger.try_wait().unwrap().is_none());
        let _ = stranger.kill();
        let _ = stranger.wait();
    }

    #[test]
    fn a_handed_off_game_is_whatever_is_not_battle_net_or_wine() {
        // The point of the rule: a title nobody wrote a table entry for is
        // still recognised, which is what makes the hand-off work for
        // anything Blizzard ships.
        assert_eq!(
            names_a_game(&argv([r"C:\Program Files (x86)\Hearthstone\Hearthstone.exe"].as_ref()), "Hearthstone.e"),
            Some("Hearthstone.exe".to_owned())
        );
        assert_eq!(
            names_a_game(&argv(&[r"C:\...\_retail_\Wow.exe"]), "Wow.exe"),
            Some("Wow.exe".to_owned())
        );

        // Battle.net's own are somebody else's problem, including the CEF
        // children that report themselves as CrBrowserMain.
        assert_eq!(names_a_game(&argv(&[BNET, "--from-launcher"]), "CrBrowserMain"), None);
        assert_eq!(names_a_game(&argv(&[AGENT]), "Agent.exe"), None);

        // Wine runs these in every prefix, and one of them appearing a
        // second after the click would otherwise be "the game", played for
        // a quarter of a second.
        for service in ["services.exe", "explorer.exe", "winedevice.exe", "rpcss.exe"] {
            assert_eq!(
                names_a_game(&argv(&[&format!(r"C:\windows\system32\{service}")]), service),
                None,
                "{service} is Wine's, not a game"
            );
        }

        // What a GE-Proton launch leaves behind after the game has gone,
        // from a real report: none of it is a game, and all of it counted as
        // one refused every Play after the first.
        for (exe, comm) in [
            (r"C:\windows\system32\steam.exe", "steam.exe"),
            (r"C:\windows\system32\xalia.exe", "xalia.exe"),
            (r"C:\windows\system32\lsass.exe", "lsass.exe"),
            (r"C:\windows\system32\rundll32.exe", "rundll32.exe"),
            ("python3", "python3"),
            ("/usr/bin/gamemoderun", "gamemoderun"),
        ] {
            assert_eq!(names_a_game(&argv(&[exe, "run"]), comm), None, "{exe} is not a game");
        }

        // A browser child of the game is not a second game.
        assert_eq!(
            names_a_game(
                &argv(&[r"C:\Games\Something.exe", "--type=renderer"]),
                "CrRendererMain"
            ),
            None
        );
    }

    #[test]
    fn only_the_processes_that_cause_the_deadlock_are_classified() {
        assert_eq!(
            ProcKind::classify(&argv(&[AGENT, "--session=4409"]), "Agent.exe"),
            Some(ProcKind::Agent)
        );
        assert_eq!(
            ProcKind::classify(&argv(&[BNET, "--from-launcher"]), "CrBrowserMain"),
            Some(ProcKind::BattleNet)
        );
        assert_eq!(
            ProcKind::classify(
                &argv(&[r"C:\Program Files (x86)\Battle.net\Battle.net Launcher.exe"]),
                "Battle.net Laun"
            ),
            Some(ProcKind::BattleNet)
        );
        assert_eq!(
            ProcKind::classify(
                &argv(&["/home/u/proton/files/bin/wineserver", "-p"]),
                "wineserver"
            ),
            Some(ProcKind::WineServer)
        );

        // The game itself is never touched. That is the legal guardrail, and
        // it is enforced here rather than by remembering not to.
        assert_eq!(
            ProcKind::classify(&argv(&[r"C:\...\_retail_\Wow.exe"]), "Wow.exe"),
            None
        );
        assert_eq!(
            ProcKind::classify(&argv(&["/usr/bin/firefox"]), "firefox"),
            None
        );
    }

    #[test]
    fn the_client_is_recognised_despite_cef_renaming_its_threads() {
        // Live finding: Linux reports `comm`, and the Battle.net client is
        // CEF: every one of its processes is named after a Chromium thread,
        // never after the executable. Classifying on the name alone silently
        // misses the client entirely, which is the one process pre-flight
        // exists to clear.
        for comm in ["CrBrowserMain", "CrGpuMain", "CrRendererMain", "CrUtilityMain", "Agent"] {
            assert!(
                ProcKind::classify(&argv(&[BNET, "--from-launcher"]), comm).is_some(),
                "{comm} was not recognised as the client"
            );
        }
    }

    #[test]
    fn cef_children_are_helpers_and_die_before_the_browser_process() {
        let child = ProcKind::classify(
            &argv(&[BNET, "--type=renderer", "--log-severity=error"]),
            "CrRendererMain",
        );
        assert_eq!(child, Some(ProcKind::Helper));
        assert!(ProcKind::Helper.kill_order() < ProcKind::BattleNet.kill_order());
    }

    #[test]
    fn a_windows_argv0_splits_on_backslashes() {
        assert_eq!(executable_name(&argv(&[BNET]), "x"), "Battle.net.exe");
        assert_eq!(executable_name(&argv(&["/usr/bin/wineserver"]), "x"), "wineserver");
        // A process with no readable command line still has its name.
        assert_eq!(executable_name(&[], "wineserver"), "wineserver");
        assert_eq!(executable_name(&argv(&[""]), "wineserver"), "wineserver");
    }

    #[test]
    fn wineserver_dies_after_everything_it_owns() {
        let mut kinds = [
            ProcKind::WineServer,
            ProcKind::BattleNet,
            ProcKind::Helper,
            ProcKind::Agent,
        ];
        kinds.sort_by_key(|k| k.kill_order());
        assert_eq!(
            kinds,
            [
                ProcKind::Helper,
                ProcKind::Agent,
                ProcKind::BattleNet,
                ProcKind::WineServer
            ]
        );
    }

    #[test]
    fn a_process_in_another_prefix_is_never_ours() {
        let layout = Layout::with_root("/games/Blizzard");
        let prefixes = owned_prefixes(&layout);

        // Gabriel's Lutris Battle.net: same executable name, different prefix.
        assert!(!belongs_to(
            &environ(&["WINEPREFIX=/home/u/Games/battlenet"]),
            &prefixes
        ));
        assert!(belongs_to(
            &environ(&["WINEPREFIX=/games/Blizzard/prefix/pfx"]),
            &prefixes
        ));
        // Proton exports the compat-data dir rather than the prefix itself.
        assert!(belongs_to(
            &environ(&["STEAM_COMPAT_DATA_PATH=/games/Blizzard/prefix"]),
            &prefixes
        ));
    }

    #[test]
    fn an_unreadable_environment_means_hands_off() {
        let prefixes = owned_prefixes(&Layout::with_root("/games/Blizzard"));
        assert!(!belongs_to(&[], &prefixes));
        assert!(!belongs_to(&environ(&["PATH=/usr/bin"]), &prefixes));
    }

    #[test]
    fn a_variable_that_merely_starts_with_the_name_does_not_match() {
        let prefixes = owned_prefixes(&Layout::with_root("/games/Blizzard"));
        // `WINEPREFIXES` is not `WINEPREFIX`, and a value-only match would be
        // a way to get an unrelated process signalled.
        assert!(!belongs_to(
            &environ(&["WINEPREFIXES=/games/Blizzard/prefix/pfx"]),
            &prefixes
        ));
    }

    #[test]
    fn scanning_a_prefix_nothing_runs_in_finds_nothing() {
        // Lightforge's own test process is alive and has neither variable set,
        // so this also asserts the scan doesn't match on name alone.
        assert!(scan(&Layout::with_root("/nonexistent/Blizzard")).is_empty());
    }

    #[test]
    fn cleanup_reports_what_it_reclaimed_and_is_idempotent() {
        let root = scratch("cleanup");
        let layout = Layout::with_root(&root);

        let cache = layout.agent_cache_dir();
        std::fs::create_dir_all(cache.join("data")).unwrap();
        std::fs::write(cache.join("data").join("blob"), vec![0u8; 2048]).unwrap();

        assert_eq!(remove_dir(&cache).unwrap(), Some(2048));
        assert!(!cache.exists());
        // A second pass is the normal case on a healthy system.
        assert_eq!(remove_dir(&cache).unwrap(), None);
    }

    #[test]
    fn clearing_the_cache_never_touches_agents_product_registry() {
        // The bug this guards against cost a real user their install
        // registration on every launch: `agent_cache_dir` pointed at Agent's
        // whole directory, so clearing it deleted `product.db`: the list of
        // games Battle.net knows are installed, and the client then asked to
        // locate the game again, every single time.
        let root = scratch("agent-registry");
        let layout = Layout::with_root(&root);

        let agent = layout.agent_dir();
        std::fs::create_dir_all(layout.agent_cache_dir()).unwrap();
        std::fs::write(layout.agent_cache_dir().join("blob"), vec![0u8; 512]).unwrap();
        std::fs::write(agent.join("product.db"), b"registry").unwrap();
        std::fs::write(agent.join("Agent.exe"), b"binary").unwrap();

        remove_dir(&layout.agent_cache_dir()).unwrap();

        assert!(!layout.agent_cache_dir().exists(), "the cache goes");
        assert!(agent.join("product.db").is_file(), "the registry stays");
        assert!(agent.join("Agent.exe").is_file(), "Agent itself stays");
    }

    #[test]
    fn the_cache_is_inside_agents_directory_but_is_not_it() {
        let layout = Layout::with_root("/games/Blizzard");
        assert!(layout.agent_cache_dir().starts_with(layout.agent_dir()));
        assert_ne!(layout.agent_cache_dir(), layout.agent_dir());
        assert!(layout.agent_cache_dir().ends_with("Agent/data/cache"));
    }

    #[test]
    fn the_installer_staging_directory_is_the_one_inside_the_install() {
        let layout = Layout::with_root("/games/Blizzard");
        assert!(layout
            .battle_net_staging()
            .starts_with(layout.battle_net_dir()));
        assert!(layout.battle_net_staging().ends_with(".battle.net/.extract"));
    }

    #[test]
    fn a_surviving_process_names_itself_in_the_error() {
        let err = PreflightError::StillRunning(vec![PrefixProcess {
            pid: 4242,
            name: "Agent.exe".into(),
            kind: ProcKind::Agent,
            exe: None,
        }]);
        let text = err.to_string();
        assert!(text.contains("Agent.exe"), "{text}");
        assert!(text.contains("4242"), "{text}");
    }

    #[test]
    fn the_no_downloads_guard_never_claims_a_pause() {
        let mut guard = NoDownloads;
        assert!(!guard.pause(&ProductCode::Retail).unwrap());
        assert!(!guard.resume(&ProductCode::Retail).unwrap());
    }
}
