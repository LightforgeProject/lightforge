use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

use super::games::games_running;
use super::gate::Timing;
use super::scan::{belongs_to, collect, owned_prefixes, refresh, scan, PrefixProcess, ProcKind};
use super::{io, PreflightError, POLL};
use crate::paths::Layout;
use crate::prefix::ProtonRunner;

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

/// Shut the prefix down: `wineserver -k` first, which lets the client flush
/// its config and only reaches this prefix, then signals for orphans whose
/// wineserver is gone (the deadlock case).
pub fn shutdown(
    runner: &ProtonRunner,
    layout: &Layout,
    grace: Duration,
) -> Result<Vec<Outcome>, PreflightError> {
    shutdown_timed(runner, layout, grace, &mut Vec::new())
}

/// [`shutdown`], stamping how long each stage took into `timings` (a scan is
/// 60 ms, a signal grace up to five seconds).
pub(super) fn shutdown_timed(
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
    tracing::info!(
        "preflight: stopping {} process(es) in the prefix: {}",
        found.len(),
        found.iter().map(|p| format!("{} ({})", p.name, p.pid)).collect::<Vec<_>>().join(", "),
    );

    // `wineserver -k` would take a running game with it; the CLI could get
    // here mid-raid.
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

    outcomes.extend(terminate(&mut system, layout, remaining, grace));
    stamp(timings, "signals");

    outcomes.sort_by_key(|o| o.process.pid);
    for outcome in outcomes.iter().filter(|o| o.disposition != Disposition::Wineserver) {
        tracing::warn!(
            "preflight: {} ({}) needed more than wineserver -k: {:?}",
            outcome.process.name,
            outcome.process.pid,
            outcome.disposition,
        );
    }
    Ok(outcomes)
}

/// Close the Battle.net client, its Agent and their helpers, and leave any
/// game running: on Windows a game outlives the client that started it.
///
/// With no game up this is [`shutdown`], whose `wineserver -k` lets the client
/// write its config on the way out. With one, only the client's own processes
/// are signalled, as [`super::stop_game`] does for a game.
pub fn quit_client(
    runner: &ProtonRunner,
    layout: &Layout,
    grace: Duration,
) -> Result<Vec<Outcome>, PreflightError> {
    if games_running(layout).is_empty() {
        return shutdown(runner, layout, grace);
    }
    let mut system = System::new();
    refresh(&mut system);
    let client: Vec<PrefixProcess> =
        collect(&system, layout).into_iter().filter(|p| p.kind != ProcKind::WineServer).collect();
    tracing::info!(
        "closing Battle.net beside a running game: {}",
        client.iter().map(|p| format!("{} ({})", p.name, p.pid)).collect::<Vec<_>>().join(", "),
    );
    let mut outcomes = terminate(&mut system, layout, client, grace);
    outcomes.sort_by_key(|o| o.process.pid);
    Ok(outcomes)
}

/// `SIGTERM`, then `SIGKILL` for whatever ignored it.
fn terminate(
    system: &mut System,
    layout: &Layout,
    processes: Vec<PrefixProcess>,
    grace: Duration,
) -> Vec<Outcome> {
    let stubborn = signal_all(system, layout, &pids_of(&processes), Signal::Term, grace);
    let (ignored_term, exited): (Vec<_>, Vec<_>) =
        processes.into_iter().partition(|p| stubborn.contains(&p.pid));
    let mut outcomes: Vec<Outcome> = exited
        .into_iter()
        .map(|process| Outcome { process, disposition: Disposition::Terminated })
        .collect();

    if !ignored_term.is_empty() {
        let survivors = signal_all(system, layout, &pids_of(&ignored_term), Signal::Kill, grace);
        outcomes.extend(ignored_term.into_iter().map(|process| {
            let disposition = if survivors.contains(&process.pid) {
                Disposition::Survived
            } else {
                Disposition::Killed
            };
            Outcome { process, disposition }
        }));
    }
    outcomes
}

fn pids_of(processes: &[PrefixProcess]) -> Vec<u32> {
    processes.iter().map(|p| p.pid).collect()
}

/// Send `signal` to each process, then wait up to `grace` for them all to go
/// away. Returns the pids still alive at the end.
///
/// Each pid is re-attributed right before its signal: it may have been recycled.
pub(super) fn signal_all(
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
        // A zombie has exited; it only awaits its parent.
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
pub(super) fn remove_dir(dir: &Path) -> Result<Option<u64>, PreflightError> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let bytes = crate::fsutil::dir_size(dir);
    std::fs::remove_dir_all(dir).map_err(io(dir))?;
    Ok(Some(bytes))
}
