use std::collections::HashSet;
use std::time::{Duration, Instant};

use sysinfo::{Signal, System};

use super::scan::{
    belongs_to, executable_name, is_cef_child_arg, is_windows_exe, owned_prefixes, refresh, scan,
    ProcKind,
};
use super::shutdown::{signal_all, Disposition};
use crate::paths::Layout;

/// How often the prefix is re-scanned while waiting on a handed-off game.
///
/// Slower than [`super::POLL`]: that one is a shutdown the user is waiting through,
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
pub(super) fn names_a_game(argv: &[std::ffi::OsString], fallback: &str) -> Option<String> {
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
