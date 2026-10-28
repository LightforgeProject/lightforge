use std::collections::HashSet;
use std::path::Path;
use std::time::{Duration, Instant};

use sysinfo::{Pid, ProcessRefreshKind, ProcessesToUpdate, Signal, System};

use super::games::games_running;
use super::gate::Timing;
use super::scan::{belongs_to, collect, owned_prefixes, refresh, scan, PrefixProcess};
use super::{PreflightError, POLL};
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

fn pids_of(processes: &[PrefixProcess]) -> Vec<u32> {
    processes.iter().map(|p| p.pid).collect()
}

/// Send `signal` to each process, then wait up to `grace` for them all to go
/// away. Returns the pids still alive at the end.
///
/// Every pid is re-attributed against the prefix immediately before it is
/// signalled. Between the scan and the signal a pid can be recycled onto an
/// unrelated process, and signalling that one would be indefensible.
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
pub(super) fn remove_dir(dir: &Path) -> Result<Option<u64>, PreflightError> {
    if !dir.is_dir() {
        return Ok(None);
    }
    let bytes = crate::fsutil::dir_size(dir);
    std::fs::remove_dir_all(dir)
        .map_err(|source| PreflightError::Io { path: dir.to_path_buf(), source })?;
    Ok(Some(bytes))
}
