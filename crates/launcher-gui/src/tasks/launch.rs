//! Starting games: directly, through Battle.net, and the pre-flight before both.

use super::*;

/// Everything that must be settled before a launch: no stale processes in the
/// prefix, no transfer mid-flight.
///
/// Split from the launch: seconds against a whole session.
pub async fn preflight(
    layout: Layout,
    code: ProductCode,
    downloads: SharedDownloads,
    runs: Runs,
) -> Result<(), Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The launch's own build, or the shared prefix gets upgraded in between.
        let runner = runner_for(&layout, &code, runs)?;
        let mut guard = downloads;
        let config = AppDirs::discover()
            .ok()
            .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
            .unwrap_or_default();
        let started = Instant::now();
        tracing::info!("preflight for {} ({runs:?})", code.as_str());
        let report = Preflight::new(config.prefix_user.clone())
            .with_managed(config.managed_products())
            // PLAY only: anything that goes through the client starts it afresh.
            .keeping_client(runs == Runs::Game && !config.close_battle_net_on_play)
            .run(&runner, &layout, Some(&code), &mut guard)
            .map_err(|e| Problem::of(&e))?;
        // The stage timings say where a launch that feels stuck sits.
        tracing::info!(
            "preflight done in {} ms, {} process(es) found: {}",
            started.elapsed().as_millis(),
            report.processes.len(),
            report.timings.iter().map(|t| format!("{} {} ms", t.stage, t.took.as_millis())).collect::<Vec<_>>().join(", "),
        );
        crate::trace::mark(format!(
            "preflight: {} ms total, {} process(es)",
            started.elapsed().as_millis(),
            report.processes.len()
        ));
        for timing in &report.timings {
            crate::trace::mark(format!(
                "preflight:   {:>6} ms  {}",
                timing.took.as_millis(),
                timing.stage
            ));
        }
        Ok(())
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// How long the game itself gets to appear in the prefix before the watch
/// gives up and falls back to waiting on the whole process tree.
///
/// Generous: a cold start on a hard drive compiling pipelines still counts.
pub(crate) const GAME_APPEAR_WITHIN: Duration = Duration::from_secs(180);

/// Counts launches, so a wind-down from a finished session can tell whether a
/// newer one has started since.
///
/// A new game not yet in the process table looks like an empty prefix, and
/// winding down then would kill it.
pub(crate) static LAUNCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How often the prefix is re-scanned while waiting for the game (60 ms a scan).
pub(crate) const WATCH_POLL: Duration = Duration::from_millis(500);

/// How often it is re-scanned once the game is up: the delay before PLAY
/// returns, about 0.4% of a core.
pub(crate) const EXIT_POLL: Duration = Duration::from_millis(150);

/// The first launch on a new build gets a settings backup first.
///
/// Never fatal, unlike the one before an addon update.
pub(crate) fn back_up_before_launch(layout: &Layout, code: &ProductCode) {
    let Ok(dirs) = AppDirs::discover() else { return };
    match settings_backup(layout, &dirs, code).before_launch() {
        Ok(Some(taken)) => crate::trace::mark(format!("launch: settings backed up to {}", taken.path.display())),
        Ok(None) => {}
        Err(e) => tracing::warn!("could not back up settings before launch: {}", chain(&e)),
    }
}

/// What came of a direct launch.
#[derive(Debug, Clone)]
pub enum Started {
    /// The game is up; [`await_game_exit`] watches the rest of its session.
    /// `generation` is this launch's count in [`LAUNCHES`].
    Up { game: GameProcess, generation: u64 },
    /// It ended before it was ever seen, with this to say.
    Over(String),
}

/// Start the game, returning once *the game* is in the process table.
///
/// The exe is watched by name: the game is not reliably a descendant of the
/// Proton child. If it never appears, the child's exit status is reported.
pub async fn start_game(layout: Layout, code: ProductCode) -> Result<Started, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The native path, not the `C:` symlink, so Wine puts the game on its
        // own drive ([`Layout::wow_drive`]) and the shop's browser keeps its
        // GPU child. Verified 2026-10-02, a full checkout.
        let exe = layout.wow_exe(&code);
        if !exe.is_file() {
            return Err(format!("no game at {}", exe.display()).into());
        }

        back_up_before_launch(&layout, &code);

        let (tweaks, tools) = tweaks_for(&code);

        let runner = runner_for(&layout, &code, Runs::Game)?;
        let mut launch = lightforge_core::tweaks::wrapped(
            runner.command("run", &exe, Vec::<String>::new()),
            &tweaks.wrappers(&tools),
        );

        let generation =
            LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;

        // What the prefix held before the click, so a session already running
        // under another launcher is not mistaken for this one's.
        let before = lightforge_core::preflight::games_running(&layout);
        // The whole command, environment included: what to reproduce a launch
        // by hand with.
        tracing::info!("launching {}: {launch:?}", code.as_str());
        let mut child = launch.spawn().map_err(|e| Problem::of(&e))?;
        crate::trace::mark(format!("launch: spawned {}", exe.display()));
        let session = Instant::now();

        let watch = exe.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(watch) = watch else {
            return reap(child).map(Started::Over);
        };

        match watch_for(&layout, &before, &watch, &mut child)? {
            Watched::Running(pid) => {
                crate::trace::mark(format!("launch: {watch} is up (pid {pid})"));
                tracing::info!("{watch} is up (pid {pid}) after {:.1}s", session.elapsed().as_secs_f64());
                // Not waited on for the session: Proton's `steam.exe` waits
                // for every process in the prefix, which held PLAY for tens
                // of seconds, and outlived the game by nine on a measured one.
                reap_later(child);
                Ok(Started::Up { game: GameProcess { pid, name: watch }, generation })
            }
            // Either the game was never recognisable or it went down with the
            // tree. Both make the child's own status the honest answer.
            Watched::TreeExited(status) => {
                tracing::warn!(
                    "Proton exited ({status}) after {:.1}s before {watch} was seen",
                    session.elapsed().as_secs_f64()
                );
                Ok(Started::Over(format!("game exited ({status})")))
            }
            Watched::NeverAppeared => {
                tracing::warn!("{watch} never appeared within {GAME_APPEAR_WITHIN:?}; waiting on Proton");
                reap(child).map(Started::Over)
            }
        }
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What [`watch_for`] saw.
pub(crate) enum Watched {
    /// The spawned executable is in the process table, with this pid.
    Running(u32),
    /// The Proton child exited before anything was recognised.
    TreeExited(std::process::ExitStatus),
    /// Nothing by that name appeared inside [`GAME_APPEAR_WITHIN`].
    NeverAppeared,
}

/// Watch the prefix for the executable that was just spawned.
///
/// By file name: the game is not reliably a descendant of our child.
pub(crate) fn watch_for(
    layout: &Layout,
    before: &[GameProcess],
    exe: &str,
    child: &mut std::process::Child,
) -> Result<Watched, String> {
    let known: HashSet<u32> = before.iter().map(|p| p.pid).collect();
    let deadline = Instant::now() + GAME_APPEAR_WITHIN;

    loop {
        if let Some(found) = lightforge_core::preflight::games_running(layout)
            .into_iter()
            .find(|p| !known.contains(&p.pid) && p.name.eq_ignore_ascii_case(exe))
        {
            return Ok(Watched::Running(found.pid));
        }
        // After the scan, so a game that came and went is still seen.
        match child.try_wait().map_err(|e| chain(&e))? {
            Some(status) => return Ok(Watched::TreeExited(status)),
            None if Instant::now() >= deadline => return Ok(Watched::NeverAppeared),
            None => std::thread::sleep(WATCH_POLL),
        }
    }
}

/// Put the prefix to rest once a direct launch is over.
///
/// Only when no Battle.net or game of its is using it. Best effort: the next
/// pre-flight does the same work.
pub(crate) fn settle(
    layout: &Layout,
    runner: &lightforge_core::prefix::ProtonRunner,
    generation: u64,
) {
    use lightforge_core::preflight;

    // A newer launch owns the prefix; by counter, since a scan cannot see it yet.
    if LAUNCHES.load(std::sync::atomic::Ordering::SeqCst) != generation {
        crate::trace::mark("launch: a newer session owns the prefix, leaving it alone");
        return;
    }
    if preflight::client_running(layout) || !preflight::games_running(layout).is_empty() {
        crate::trace::mark("launch: something is still in the prefix, leaving it alone");
        return;
    }
    let started = Instant::now();
    let outcome = runner.wineserver_kill();
    // Microseconds: a missing `wineserver` returns as fast as a quiet prefix.
    crate::trace::mark(format!(
        "launch: prefix settled in {} us{}",
        started.elapsed().as_micros(),
        match &outcome {
            Ok(()) => String::new(),
            Err(e) => format!(" (failed: {})", chain(e)),
        }
    ));
}

/// Wait the Proton child out and report what it said, the old contract, kept
/// for the launches where nothing better is knowable.
pub(crate) fn reap(mut child: std::process::Child) -> Result<String, Problem> {
    let status = child.wait().map_err(|e| Problem::of(&e))?;
    Ok(format!("game exited ({status})"))
}

/// Wait on a Battle.net invocation off-thread. A dropped `Child` is never
/// waited on, and each hand-off would stay a `<defunct>` until the GUI exits.
fn reap_later(mut child: std::process::Child) {
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

pub async fn launch_via_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
    family: Option<String>,
    minimized: bool,
) -> Result<Handoff, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch, Readiness};
        use lightforge_core::preflight;

        tracing::info!("hand-off to Battle.net for {} (family {family:?})", code.as_str());
        let runner = client_runner(&layout, &code)?;
        back_up_before_launch(&layout, &code);

        // Counted, so a pending wind-down leaves this launch alone.
        LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Taken before the client is told anything, so a game that was
        // already playing is not mistaken for this launch's.
        let before = preflight::games_running(&layout);

        // Two phases: a cold client ignores `--exec=launch` (measured: signed
        // in at 5 s, game at 53 s). A second invocation hands it over IPC.
        if !preflight::client_running(&layout) {
            // Minimized only where it signs itself in.
            let minimize = minimized && battlenet::signs_in_by_itself(&layout, &user);
            let _ = battlenet::configure_accounts(
                &layout,
                &user,
                &battlenet::window_state_tweaks(minimize),
            );

            // The preference is only read on an autostart.
            let how = match minimize {
                true => Launch::Autostart,
                false => Launch::Client,
            };

            let started = std::time::SystemTime::now();
            // Wrappers here only: this invocation becomes the client.
            let tools = HostTools::detect();
            let wrappers = config_tweaks().wrappers(&tools);
            let client = battlenet::launch_wrapped(&runner, &layout, &how, &wrappers)
                .map_err(|e| Problem::of(&e))?;
            reap_later(client);

            if battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT)
                == Readiness::NeedsAttention
            {
                // Left to the user, who is told where the client is.
                return Ok(Handoff::NeedsAttention { minimized: minimize });
            }
        }

        // Closed meanwhile: sending now would start a cold client that drops it.
        if !preflight::client_running(&layout) {
            return Ok(Handoff::ClientClosed);
        }

        // Over IPC to the running client, with the family the panel resolved.
        let launch = Launch::Product { code: code.clone(), exec: family };
        reap_later(battlenet::launch(&runner, &layout, &launch).map_err(|e| Problem::of(&e))?);

        let handoff = match preflight::wait_for_game_start(&layout, &before, HANDOFF_TIMEOUT) {
            Some(game) => Handoff::Playing(game),
            None => Handoff::NothingStarted,
        };
        match &handoff {
            Handoff::NothingStarted => {
                tracing::warn!("Battle.net hand-off for {}: no game started within {HANDOFF_TIMEOUT:?}", code.as_str());
            }
            _ => tracing::info!("Battle.net hand-off for {}: game started", code.as_str()),
        }
        Ok(handoff)
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// Open the client's install flow for a game, dialog and all.
///
/// The client always boots to its homepage, which draws over `--exec="launch
/// <uid>"` and over `--uri=battlenet://<family>` (that only selects the game).
/// `--game=<uid> --install` has a view of its own, with the path picker, so
/// it shows. A running client takes it over IPC; otherwise this starts one.
pub async fn install_via_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
    uid: String,
    label: String,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch};

        let runner = client_runner(&layout, &code)?;

        // Normal window, whatever a launch would have chosen: the next thing
        // that happens is a person choosing where to install.
        let _ = battlenet::configure_accounts(
            &layout,
            &user,
            &battlenet::window_state_tweaks(false),
        );

        let child = battlenet::launch(&runner, &layout, &Launch::InstallFlow { uid })
            .map_err(|e| Problem::of(&e))?;
        reap_later(child);

        Ok(format!("Battle.net is asking where to install {label}"))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What came of a hand-off.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Handoff {
    /// The game is running.
    Playing(GameProcess),
    /// The client never reported signing in. Someone has to look at it,
    /// which means being told where it is when it was started minimized.
    NeedsAttention { minimized: bool },
    /// It was there and then it was not, closed while we waited.
    ClientClosed,
    /// Signed in, told to launch, and nothing appeared.
    NothingStarted,
}

/// How long the client gets to start and sign itself in.
///
/// A cold start signs in after about 5.5 s; the rest is for bad days.
pub(crate) const SIGN_IN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a handed-off game gets to appear before the launcher stops
/// claiming a launch is in progress.
pub(crate) const HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Block until a game exits, by pid: a title that re-execs itself ends the
/// session early instead of hanging.
///
/// `settle_after` is a direct launch's [`Started::Up`] generation: the prefix is
/// put to rest after it, unless something else still runs there.
pub async fn await_game_exit(
    layout: Layout,
    code: ProductCode,
    game: GameProcess,
    settle_after: Option<u64>,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        while lightforge_core::preflight::games_running(&layout).iter().any(|p| p.pid == game.pid) {
            std::thread::sleep(EXIT_POLL);
        }
        crate::trace::mark(format!("launch: {} has exited", game.name));
        // Best effort: the next pre-flight does the same work.
        if let Some(generation) = settle_after {
            if let Ok(runner) = runner_for(&layout, &code, Runs::Game) {
                settle(&layout, &runner, generation);
            }
        }
        Ok(format!("{} exited", game.name))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// How long a game gets to close on `SIGTERM` before it is killed outright.
const STOP_GRACE: Duration = Duration::from_secs(5);

/// Stop one running game, leaving Battle.net, other games and the rest of
/// the prefix be.
pub async fn stop_game(layout: Layout, game: GameProcess) -> Result<String, Problem> {
    use lightforge_core::preflight::{self, Disposition};

    let stopped = tokio::task::spawn_blocking(move || preflight::stop_game(&layout, game.pid, STOP_GRACE))
        .await
        .map_err(|e| Problem::of(&e))?;
    match stopped {
        Some(stuck) if stuck.disposition == Disposition::Survived => {
            Err(format!("{} would not stop", stuck.game.name).into())
        }
        Some(_) => Ok("game stopped".to_owned()),
        None => Ok(format!("{} had already exited", game.name)),
    }
}

/// Battle.net's own download limit, in MB/s, or `None` when it has none.
pub async fn battle_net_limit(layout: Layout, user: String) -> Option<u64> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::download_limit(&layout, &user)
            .map(|bps| bps / 1_000_000)
            .filter(|mbps| *mbps > 0)
    })
    .await
    .ok()
    .flatten()
}

/// Set or clear Battle.net's own download limit, in MB/s.
pub async fn set_battle_net_limit(
    layout: Layout,
    user: String,
    mbps: Option<u64>,
) -> Result<(), String> {
    tokio::task::spawn_blocking(move || {
        lightforge_core::battlenet::set_download_limit(
            &layout,
            &user,
            mbps.map(|mbps| mbps * 1_000_000),
        )
        .map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| chain(&e))?
}

/// Open Battle.net with no product: the way to reach everything outside the
/// WoW family.
///
/// The caller still runs pre-flight: a stale wineserver breaks Battle.net too.
pub async fn open_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch};

        let runner = client_runner(&layout, &code)?;
        // A normal window: the client itself was asked for.
        let _ = battlenet::configure_accounts(
            &layout,
            &user,
            &battlenet::window_state_tweaks(false),
        );
        reap_later(battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| Problem::of(&e))?);
        Ok("Battle.net opening".to_owned())
    })
    .await
    .map_err(|e| Problem::of(&e))?
}
