//! Starting games: directly, through Battle.net, and the pre-flight before both.

use super::*;

/// Launch the game directly under Proton, the `[ PLAY ]` path.
///
/// Blocking from start to finish: pre-flight spawns processes and the returned
/// future only resolves when the game exits.
/// Everything that must be settled before a launch: no stale processes in the
/// prefix, no transfer mid-flight.
///
/// Split from the launch itself because the two take wildly different amounts
/// of time, this is seconds, the launch is however long someone plays, and a
/// UI that cannot tell them apart ends up claiming it is still doing pre-flight
/// an hour into a raid.
pub async fn preflight(
    layout: Layout,
    code: ProductCode,
    downloads: SharedDownloads,
    runs: Runs,
) -> Result<(), Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The same build the launch will use. The prefix is shared, so
        // clearing it with one Proton and then launching under another is how
        // you get an unasked-for prefix upgrade between the two.
        let runner = runner_for(&layout, &code, runs)?;
        let mut guard = downloads;
        let config = AppDirs::discover()
            .ok()
            .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
            .unwrap_or_default();
        let started = Instant::now();
        let report = Preflight::new(config.prefix_user.clone())
            .with_managed(config.managed_products())
            .run(&runner, &layout, Some(&code), &mut guard)
            .map_err(|e| Problem::of(&e))?;
        // The report is for the CLI, which prints it. Here the outcome is
        // binary, the prefix is ready or the launch does not happen, but the
        // stage timings are worth tracing: a launch that feels stuck is one
        // of them, and this says which.
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
/// Generous on purpose: a cold start on a mechanical drive, or a first launch
/// that compiles pipelines before it opens a window, is still a launch. The
/// fallback is correct either way. It is only slower to notice.
pub(crate) const GAME_APPEAR_WITHIN: Duration = Duration::from_secs(180);

/// Counts launches, so a wind-down from a finished session can tell whether a
/// newer one has started since.
///
/// The hazard is narrow and real: the tail of session one can still be
/// unwinding while session two is spawning, and a prefix scan taken in the
/// seconds before the new game registers a process looks exactly like an empty
/// one. Killing the server there would take the starting game with it.
pub(crate) static LAUNCHES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// How often the prefix is re-scanned while waiting for the game to appear.
///
/// 60 ms a scan, measured, so this is well under a percent of a core.
pub(crate) const WATCH_POLL: Duration = Duration::from_millis(500);

/// How often it is re-scanned once the game is up.
///
/// This interval *is* the delay between the window closing and PLAY coming
/// back, so it is worth more than the waiting-to-start one is: a scan every
/// 150 ms is about 0.4% of a core while someone plays, which buys back a
/// third of a second of a button that looks stuck.
pub(crate) const EXIT_POLL: Duration = Duration::from_millis(150);

/// The first launch on a new build gets a settings backup first.
///
/// Never fatal, unlike the one before an addon update: an update can wait for
/// a backup, but refusing to start the game over one is the wrong trade. Runs
/// behind the pre-flight gate, with the client closed.
pub(crate) fn back_up_before_launch(layout: &Layout, code: &ProductCode) {
    let Ok(dirs) = AppDirs::discover() else { return };
    match settings_backup(layout, &dirs, code).before_launch() {
        Ok(Some(taken)) => crate::trace::mark(format!("launch: settings backed up to {}", taken.path.display())),
        Ok(None) => {}
        Err(e) => eprintln!("lightforge: could not back up settings before launch: {}", chain(&e)),
    }
}

/// Run the game, returning when *the game* has exited.
///
/// Not when the prefix is quiet: Proton's `steam.exe` waits for every process
/// left in it, which kept PLAY dead for tens of seconds. So the exe is
/// watched by name in the process table (60 ms a pass), a detached thread
/// reaps the Proton child, and the next pre-flight guarantees a quiet prefix
/// with `wineserver -k -w`. If no game ever appears, the child's exit status
/// is reported, as before.
pub async fn run_game(layout: Layout, code: ProductCode) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        // The install's native path, *not* the `C:\Program Files (x86)`
        // symlink. This is half of what makes the in-game shop work, and the
        // other half is [`Layout::wow_drive`]. Wine resolves a Unix path to
        // the most specific drive mapped over it, so launching from here puts
        // the game on `D:`; launching through the symlink would keep it on
        // `C:` and the shop's browser would lose its GPU child process the way
        // it always did. Verified end to end 2026-10-02: a full checkout, from
        // a direct launch, with no Battle.net session.
        let flavor = layout.wow_install(&code).join(&*code.install_dir());
        let exe = lightforge_core::paths::game_executable(&flavor, &code);
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
        let mut child = launch.spawn().map_err(|e| Problem::of(&e))?;
        crate::trace::mark(format!("launch: spawned {}", exe.display()));

        let watch = exe.file_name().map(|n| n.to_string_lossy().into_owned());
        let Some(watch) = watch else {
            return reap(child);
        };

        match watch_for(&layout, &before, &watch, &mut child)? {
            Watched::Running(pid) => {
                crate::trace::mark(format!("launch: {watch} is up (pid {pid})"));
                watch_until_gone(&layout, pid, &mut child);
                crate::trace::mark(format!("launch: {watch} has exited"));
                // Reaped on its own thread so nothing is left a zombie. Settled
                // first: the child is Proton's `steam.exe`, whose wait outlived
                // the game by nine seconds on a measured retail session, a
                // window where PLAY was armed but the prefix not empty.
                // `wineserver -k -w` is what that wait is waiting for anyway.
                let (quiet_layout, quiet_runner) = (layout.clone(), runner);
                std::thread::spawn(move || {
                    settle(&quiet_layout, &quiet_runner, generation);
                    let _ = child.wait();
                    crate::trace::mark("launch: the prefix is quiet");
                });
                Ok("game closed".to_owned())
            }
            // Either the game was never recognisable or it went down with the
            // tree. Both make the child's own status the honest answer.
            Watched::TreeExited(status) => Ok(format!("game exited ({status})")),
            Watched::NeverAppeared => reap(child),
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
/// Matched by file name against the process table rather than by walking the
/// child's descendants: Proton hands the launch to its `steam.exe`, so the
/// game is not our child's child in any dependable way.
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
        // Checked after the scan, not before it: a game that starts and exits
        // between two polls still has to be seen, and the scan is the only
        // thing that can see it.
        match child.try_wait().map_err(|e| chain(&e))? {
            Some(status) => return Ok(Watched::TreeExited(status)),
            None if Instant::now() >= deadline => return Ok(Watched::NeverAppeared),
            None => std::thread::sleep(WATCH_POLL),
        }
    }
}

/// Block until `pid` is gone from the prefix, or the whole tree is.
///
/// Watched by pid, so a client that re-execs itself counts as closed. That is
/// the right call here too: the next launch cleans the prefix anyway.
pub(crate) fn watch_until_gone(layout: &Layout, pid: u32, child: &mut std::process::Child) {
    while lightforge_core::preflight::games_running(layout).iter().any(|p| p.pid == pid) {
        if matches!(child.try_wait(), Ok(Some(_)) | Err(_)) {
            return;
        }
        std::thread::sleep(EXIT_POLL);
    }
}

/// Put the prefix to rest once a direct launch is over.
///
/// Only when nothing of ours is still using it. Battle.net is the case that
/// matters: it shares the prefix, people leave it open, and killing the server
/// out from under it would close the client they are looking at. A game
/// started from Battle.net is the same prefix too, so both are checked.
///
/// Best effort throughout. A prefix that will not settle is exactly what the
/// next pre-flight is for, and it is still there, doing the same work with the
/// same guarantees. This only tries to make sure it finds nothing to do.
pub(crate) fn settle(
    layout: &Layout,
    runner: &lightforge_core::prefix::ProtonRunner,
    generation: u64,
) {
    use lightforge_core::preflight;

    // A newer launch owns the prefix now. Checked first and by counter rather
    // than by scanning, because a game that has been spawned but has not yet
    // appeared in the process table is invisible to a scan and is exactly the
    // session this must not interrupt.
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
    // Microseconds, because the interesting failure looks like success: a
    // `wineserver` that was never found returns before it spawns anything,
    // and at millisecond resolution that is indistinguishable from a prefix
    // that was already quiet.
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

        let runner = client_runner(&layout, &code)?;
        back_up_before_launch(&layout, &code);

        // Counted too, though nothing here winds the prefix down: a direct
        // launch's wind-down may still be pending, and a client that has been
        // spawned but not yet registered a process is invisible to its scan.
        LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        // Taken before the client is told anything, so a game that was
        // already playing is not mistaken for this launch's.
        let before = preflight::games_running(&layout);

        // Two phases, because one does not work. A cold client handed
        // `--exec=launch` parses it and then does nothing with it: measured
        // on a real start, the client was signed in after five seconds and
        // the game did not begin for another forty-eight. The arguments have
        // to arrive at a client that is already up, which is what the
        // client's own IPC queue is for, a second invocation hands them to
        // the first.
        if !preflight::client_running(&layout) {
            // Out of the way, but only where it can sign itself in. A
            // sign-in form nobody can see is a launch that quietly never
            // happens, so a prefix without auto-login gets a normal window
            // whatever the setting says.
            let minimize = minimized && battlenet::signs_in_by_itself(&layout, &user);
            let _ = battlenet::configure_accounts(
                &layout,
                &user,
                &battlenet::window_state_tweaks(minimize),
            );

            // `--autostarted` or not: the preference above is only read on an
            // autostart, and the flag without the preference does nothing of
            // its own.
            let how = match minimize {
                true => Launch::Autostart,
                false => Launch::Client,
            };

            let started = std::time::SystemTime::now();
            // Wrappers go here and nowhere else on this path: this
            // invocation becomes the client, and the game inherits from it.
            // The launch below only hands arguments to it over IPC.
            let tools = HostTools::detect();
            let wrappers = config_tweaks().wrappers(&tools);
            battlenet::launch_wrapped(&runner, &layout, &how, &wrappers)
                .map_err(|e| Problem::of(&e))?;

            if battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT)
                == Readiness::NeedsAttention
            {
                // Stop driving. The client is up and is the only thing that
                // can finish this, so it is left running for the user, who
                // has to be told where it is if it was put in the tray.
                return Ok(Handoff::NeedsAttention { minimized: minimize });
            }
        }

        // The client can be gone by now: signing in takes a few seconds, and
        // a window nobody expected is a window somebody closes, with
        // `HideOnClose` off, closing it quits. Sending the launch anyway
        // would start a *second*, cold client, and a cold client drops the
        // `--exec` it was handed. Saying so is the better outcome.
        if !preflight::client_running(&layout) {
            return Ok(Handoff::ClientClosed);
        }

        // Now it lands: the running client picks this up over its IPC queue
        // and starts the game. The family the panel resolved, which for
        // anything but WoW came out of the client's own catalogue;
        // `Launch::command` falls back to the compiled-in table without one.
        let launch = Launch::Product { code: code.clone(), exec: family };
        battlenet::launch(&runner, &layout, &launch).map_err(|e| Problem::of(&e))?;

        Ok(
            match preflight::wait_for_game_start(&layout, &before, HANDOFF_TIMEOUT) {
                Some(game) => Handoff::Playing(game),
                None => Handoff::NothingStarted,
            },
        )
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

        battlenet::launch(&runner, &layout, &Launch::InstallFlow { uid })
            .map_err(|e| Problem::of(&e))?;

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
/// A cold start under Proton reaches "Logged into Battle.net successfully" in
/// about five and a half seconds. A minute is for the bad days, a client
/// updating itself, or a slow network, and running out of it is not an
/// error, just the point where a person is needed.
pub(crate) const SIGN_IN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// How long a handed-off game gets to appear/// How long a handed-off game gets to appear before the launcher stops
/// claiming a launch is in progress.
///
/// Generous: a cold Battle.net under Wine has to start, update itself if it
/// feels like it, and may stop at a login screen, and the cost of being
/// wrong in this direction is only an unlocked button.
pub(crate) const HANDOFF_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(180);

/// Block until a handed-off game exits.
///
/// Split from the launch so the screen can say which game is running in
/// between, instead of "handing off…" for the length of a session.
pub async fn await_game_exit(layout: Layout, game: GameProcess) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        lightforge_core::preflight::wait_for_game_exit(&layout, game.pid);
        Ok(format!("{} exited", game.name))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What Battle.net reports as installed, keyed by product code.
///
/// Read when the browse panel opens, never on a timer and never to decide
/// what the header shows, it labels a list the user asked to see.
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
/// Deliberately a hand-off and nothing more. Lightforge does not track,
/// install or update other Blizzard titles, `SPEC.md` is explicit that it is
/// WoW-family-primary and a pass-through for the rest, so this opens the
/// client and stops there.
///
/// Still pre-flighted by the caller: a stale wineserver breaks Battle.net
/// exactly as it breaks a game launch.
pub async fn open_battle_net(
    layout: Layout,
    code: ProductCode,
    user: String,
) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        use lightforge_core::battlenet::{self, Launch};

        let runner = client_runner(&layout, &code)?;
        // A normal window, whatever the last hand-off left behind: somebody
        // asked for the client itself, so hiding it in the tray would be
        // answering a different request.
        let _ = battlenet::configure_accounts(
            &layout,
            &user,
            &battlenet::window_state_tweaks(false),
        );
        battlenet::launch(&runner, &layout, &Launch::Client).map_err(|e| Problem::of(&e))?;
        Ok("Battle.net opening".to_owned())
    })
    .await
    .map_err(|e| Problem::of(&e))?
}
