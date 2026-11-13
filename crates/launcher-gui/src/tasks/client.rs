//! The Battle.net client kept ready in the background.

use std::sync::atomic::Ordering;
use std::time::SystemTime;

use lightforge_core::battlenet::{self, ClientWindows, Readiness};
use lightforge_core::preflight::{self, NoDownloads};

use super::*;

/// How long a client that came up without signing in is watched for someone
/// signing it in by hand.
const SIGN_IN_PATIENCE: Duration = Duration::from_secs(30 * 60);

/// How long an update gets to show up in the log. It began 2 s after the client
/// came up on Forever; a client still updating itself may make a game wait.
const UPDATE_STARTS_WITHIN: Duration = Duration::from_secs(60);

/// The log gets a progress line about every 1.5 s.
const UPDATE_POLL: Duration = Duration::from_millis(500);

/// Clears [`Prefix::updating`] on every way out of the update.
struct Updating;

impl Updating {
    fn start() -> Self {
        prefix().updating.store(true, Ordering::SeqCst);
        Updating
    }
}

impl Drop for Updating {
    fn drop(&mut self) {
        prefix().updating.store(false, Ordering::SeqCst);
    }
}

/// What the background client's paths read from the prefix and do to it, so
/// their races can be scripted in tests. The client's log stays a real file.
pub(crate) trait ClientDriver {
    fn client_running(&self) -> bool;
    /// The client or Agent, which outlives a client that handed a game off.
    fn client_or_agent_running(&self) -> bool;
    fn games_running(&self) -> Vec<GameProcess>;
    /// Whether one of its windows is open, on screen or minimized. `true` when
    /// they cannot be read: closing one in use loses what someone was doing in it.
    fn window_open(&self) -> bool;
    fn signs_in_by_itself(&self) -> bool;
    /// Pre-flight's sweep, for a client starting on its own.
    fn clear(&self) -> Result<(), Problem>;
    /// Start it in the tray. Returns the moment after which its log answers
    /// for this start.
    fn start(&self) -> Result<SystemTime, Problem>;
    fn quit(&self) -> Result<(), Problem>;
}

struct Live<'a> {
    config: &'a Config,
    layout: &'a Layout,
}

impl ClientDriver for Live<'_> {
    fn client_running(&self) -> bool {
        preflight::client_running(self.layout)
    }

    fn client_or_agent_running(&self) -> bool {
        preflight::scan(self.layout)
            .iter()
            .any(|p| matches!(p.kind, preflight::ProcKind::BattleNet | preflight::ProcKind::Agent))
    }

    fn games_running(&self) -> Vec<GameProcess> {
        preflight::games_running(self.layout)
    }

    fn window_open(&self) -> bool {
        ClientWindows::connect(preflight::client_pids(self.layout))
            .and_then(|windows| windows.any_open())
            .unwrap_or_else(|e| {
                tracing::warn!("Battle.net's windows cannot be read, so it stays up: {}", chain(&e));
                true
            })
    }

    fn signs_in_by_itself(&self) -> bool {
        battlenet::signs_in_by_itself(self.layout, &self.config.prefix_user)
    }

    fn clear(&self) -> Result<(), Problem> {
        Preflight::new(self.config.prefix_user.clone())
            .with_managed(self.config.managed_products())
            .with_background_client(true)
            .run(&lone_client_runner(self.config, self.layout)?, self.layout, None, &mut NoDownloads)
            .map_err(|e| Problem::of(&e))?;
        Ok(())
    }

    fn start(&self) -> Result<SystemTime, Problem> {
        start_client(self.config, &in_tls_container(lone_client_runner(self.config, self.layout)?)?, self.layout, true)
    }

    fn quit(&self) -> Result<(), Problem> {
        preflight::quit_client(&lone_client_runner(self.config, self.layout)?, self.layout, STOP_GRACE)
            .map_err(|e| Problem::of(&e))?;
        Ok(())
    }
}

/// What came of starting the client out of sight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Warmed {
    /// Signed in, with its game list built.
    Ready,
    /// Up, but it never signed in: two-factor, an expired session, an outage.
    NeedsSignIn { started: SystemTime },
    /// Already up, beside a game, or without a saved sign-in.
    NotStarted,
    /// It came up and went away again before it signed in.
    Exited,
}

/// `preflights_seen` is [`preflights`] when the warm-up was asked for.
/// `cancel` is tripped when the mode is switched off: checked under the lock,
/// which a close holds for its whole check-and-close.
pub async fn warm_battle_net(
    config: Config,
    layout: Layout,
    preflights_seen: u64,
    cancel: Cancel,
) -> Result<Warmed, Problem> {
    off_runtime("warm-up", move || {
        warm(&Live { config: &config, layout: &layout }, &layout, &config.prefix_user, preflights_seen, &cancel)
    })
    .await?
}

fn warm(
    client: &impl ClientDriver,
    layout: &Layout,
    user: &str,
    preflights_seen: u64,
    cancel: &Cancel,
) -> Result<Warmed, Problem> {
    let mut prefix = hold_prefix();
    if cancel.is_cancelled() {
        return Ok(Warmed::NotStarted);
    }
    if preflights() != preflights_seen {
        tracing::info!("Battle.net waits in the background: a launch got to the prefix first");
        owe_client();
        return Ok(Warmed::NotStarted);
    }
    // Owed again: the client poll brings it back once the game is gone.
    if !client.games_running().is_empty() {
        owe_client();
        return Ok(Warmed::NotStarted);
    }
    // Without a saved sign-in this would be a form nobody can see.
    if client.client_running() || !client.signs_in_by_itself() {
        return Ok(Warmed::NotStarted);
    }
    tracing::info!("starting Battle.net in the background");
    // Counted, so a pending wind-down leaves this start alone.
    prefix.launches += 1;
    client.clear()?;
    let started = client.start()?;
    drop(prefix);
    Ok(match battlenet::wait_until_ready(layout, user, started, SIGN_IN_TIMEOUT, || client.client_running()) {
        Readiness::SignedIn => Warmed::Ready,
        Readiness::NeedsAttention => Warmed::NeedsSignIn { started },
        Readiness::Exited => Warmed::Exited,
    })
}

/// Wait for someone to sign in the client that [`warm_battle_net`] gave up on.
pub async fn await_sign_in(layout: Layout, user: String, started: SystemTime) -> Result<Warmed, Problem> {
    off_runtime("sign-in", move || {
        match battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_PATIENCE, || preflight::client_running(&layout)) {
            Readiness::SignedIn => Warmed::Ready,
            Readiness::NeedsAttention => Warmed::NeedsSignIn { started },
            Readiness::Exited => Warmed::Exited,
        }
    })
    .await
}

/// What came of following an update through the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientUpdated {
    Completed,
    /// The user stopped watching. The client carries on regardless.
    StoppedWatching,
}

/// Have the client update `uid`, and follow it. Battle.net owns this game's
/// files; Lightforge only starts the client so that it updates, and reads its log.
///
/// [`make_way`] leaves the client be from the call on, not from when the task
/// first runs: downloads resumed in the same message would get there first.
pub fn update_through_client(
    config: Config,
    layout: Layout,
    uid: String,
    progress: Arc<Mutex<lightforge_core::state::Progress>>,
    stop: Cancel,
) -> impl std::future::Future<Output = Result<ClientUpdated, Problem>> {
    use lightforge_core::battlenet::{SessionLog, UpdateState};

    let updating = Updating::start();
    async move {
        off_runtime("client-update", move || -> Result<_, Problem> {
            let _updating = updating;
            let mut prefix = hold_prefix();
            if let Some(game) = preflight::games_running(&layout).first() {
                return Err(format!("close {} first — Battle.net has to restart for this update", game.name).into());
            }
            let user = &config.prefix_user;
            let runner = lone_client_runner(&config, &layout)?;
            // Read when it starts, and written back when it exits.
            preflight::quit_client(&runner, &layout, STOP_GRACE).map_err(|e| Problem::of(&e))?;
            let auto_update = [battlenet::auto_update_tweak(&uid, true)];
            // Noted so the next normal-mode pre-flight puts the user's own
            // choice back; background mode turns it off again anyway.
            battlenet::record_overrides(&layout, user, &auto_update, &[]).map_err(|e| Problem::of(&e))?;
            battlenet::configure(&layout, user, &auto_update).map_err(|e| Problem::of(&e))?;
            tracing::info!("asking Battle.net to update {uid}");
            prefix.launches += 1;

            let started = start_client(&config, &in_tls_container(runner)?, &layout, true)?;
            drop(prefix);
            match battlenet::wait_until_ready(&layout, user, started, SIGN_IN_TIMEOUT, || preflight::client_running(&layout)) {
                Readiness::SignedIn => {}
                Readiness::NeedsAttention => {
                    return Err("Battle.net needs you to sign in — press its button, then try the update again".into());
                }
                Readiness::Exited => return Err("Battle.net closed before it signed in".into()),
            }
            let log = SessionLog::find(&layout, user)
                .ok_or_else(|| Problem::from("Battle.net's log cannot be read"))?;

            let began = Instant::now();
            loop {
                if stop.is_cancelled() {
                    return Ok(ClientUpdated::StoppedWatching);
                }
                match log.update_state(&uid) {
                    UpdateState::Completed => return Ok(ClientUpdated::Completed),
                    UpdateState::Running(now) => {
                        if let Ok(mut shown) = progress.lock() {
                            *shown = now;
                        }
                    }
                    UpdateState::NotStarted if began.elapsed() >= UPDATE_STARTS_WITHIN => {
                        return Err("Battle.net did not start the update — open Battle.net to see why".into());
                    }
                    UpdateState::NotStarted => {}
                }
                if !preflight::client_running(&layout) {
                    return Err("Battle.net closed during the update".into());
                }
                std::thread::sleep(UPDATE_POLL);
            }
        })
        .await?
    }
}

/// Whether the client holds `uid` back for an update, from its log.
pub async fn update_pending(layout: Layout, user: String, uid: String) -> bool {
    tokio::task::spawn_blocking(move || {
        battlenet::SessionLog::find(&layout, &user).is_some_and(|log| log.pending_update(&uid))
    })
    .await
    .unwrap_or(false)
}

/// Close the client unless something is using it: a game, or a window someone
/// opened. `false` when it was left up.
fn close_if_idle(client: &impl ClientDriver) -> Result<bool, Problem> {
    if client.window_open() || !client.games_running().is_empty() {
        return Ok(false);
    }
    client.quit()?;
    Ok(true)
}

/// Clear Battle.net out of the way of work that needs a quiet prefix, and say
/// whether it is. A background client comes back when that work is over. One
/// someone is using stays, and the work's own check says what to close.
///
/// Agent too, in either mode: it outlives the client that handed a game off
/// and keeps its registry in memory, so a download's new version can be
/// written over with the old one (GlitchTip #40: every hand-off after
/// Lightforge updated Forever asked for an update).
pub(crate) fn make_way(config: &Config, layout: &Layout) -> bool {
    give_way(&Live { config, layout }, config.battle_net_in_background)
}

fn give_way(client: &impl ClientDriver, background: bool) -> bool {
    let _prefix = hold_prefix();
    // Before the scan: between its restart and the update the client is
    // briefly not up at all.
    if prefix().updating.load(Ordering::SeqCst) {
        tracing::debug!("Battle.net stays up for work on the game files: it is updating a game");
        return false;
    }
    if !client.client_or_agent_running() {
        return true;
    }
    match close_if_idle(client) {
        Ok(true) => {
            tracing::info!("closed Battle.net for work on the game files");
            if background {
                owe_client();
            }
            true
        }
        Ok(false) => {
            tracing::debug!("Battle.net stays up for work on the game files: a game or its window is open");
            false
        }
        Err(e) => {
            tracing::warn!("could not close Battle.net: {e}");
            false
        }
    }
}

/// The client went away by itself: it is owed a restart if that was to switch
/// to a version it had just updated to.
pub async fn note_self_update(layout: Layout, user: String) {
    let _ = tokio::task::spawn_blocking(move || {
        if battlenet::SessionLog::find(&layout, &user).is_some_and(|log| log.quit_to_update()) {
            tracing::info!("Battle.net quit to switch to the version it updated to");
            owe_client();
        }
    })
    .await;
}


/// For the setting being switched off: a client nobody is using goes with it,
/// and the settings the mode overwrote come back. After it has closed, since it
/// rewrites its files on exit. One that stays up gets them back at the next
/// pre-flight.
pub async fn close_background_client(config: Config, layout: Layout) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || {
        let _prefix = hold_prefix();
        let client = Live { config: &config, layout: &layout };
        if client.client_running() && !close_if_idle(&client)? {
            return Ok("Battle.net stays open: a game or its window is using it".to_owned());
        }
        Ok(match battlenet::restore_overrides(&layout, &config.prefix_user).map_err(|e| Problem::of(&e))? {
            true => "Battle.net closed, its own settings are back".to_owned(),
            false => "Battle.net closed".to_owned(),
        })
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// Close what the background mode left running, as the last thing Lightforge
/// does. Best effort: the next pre-flight clears the prefix anyway.
pub fn stop_background_client_at_exit() {
    let Some(config) = AppDirs::discover().ok().and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
    else {
        return;
    };
    if !config.battle_net_in_background {
        return;
    }
    let Ok(layout) = config.layout() else { return };
    let _prefix = hold_prefix();
    let client = Live { config: &config, layout: &layout };
    if !client.client_running() {
        return;
    }
    match close_if_idle(&client) {
        Ok(true) => tracing::info!("closed the background Battle.net on exit"),
        Ok(false) => tracing::info!("left Battle.net running on exit: a game or its window is using it"),
        Err(e) => tracing::warn!("could not close the background Battle.net on exit: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use std::cell::{Cell, RefCell};

    use super::*;

    /// A prefix the test scripts: what runs in it, and what was done to it.
    #[derive(Default)]
    struct Scripted {
        client: Cell<bool>,
        agent: bool,
        window: bool,
        games: Vec<GameProcess>,
        signs_in: bool,
        /// How many more times a started client answers as running before it dies.
        lives_for: Option<Cell<u32>>,
        done: RefCell<Vec<&'static str>>,
    }

    impl ClientDriver for Scripted {
        fn client_running(&self) -> bool {
            if let Some(left) = self.lives_for.as_ref().filter(|_| self.client.get()) {
                match left.get() {
                    0 => self.client.set(false),
                    n => left.set(n - 1),
                }
            }
            self.client.get()
        }

        fn client_or_agent_running(&self) -> bool {
            self.client.get() || self.agent
        }

        fn games_running(&self) -> Vec<GameProcess> {
            self.games.clone()
        }

        fn window_open(&self) -> bool {
            self.window
        }

        fn signs_in_by_itself(&self) -> bool {
            self.signs_in
        }

        fn clear(&self) -> Result<(), Problem> {
            self.done.borrow_mut().push("clear");
            Ok(())
        }

        fn start(&self) -> Result<SystemTime, Problem> {
            self.done.borrow_mut().push("start");
            self.client.set(true);
            Ok(SystemTime::now())
        }

        fn quit(&self) -> Result<(), Problem> {
            self.done.borrow_mut().push("quit");
            self.client.set(false);
            Ok(())
        }
    }

    fn warm_up(client: &Scripted, preflights_seen: u64, cancel: &Cancel) -> Warmed {
        let scratch = tempfile::tempdir().unwrap();
        warm(client, &Layout::with_root(scratch.path()), "steamuser", preflights_seen, cancel).unwrap()
    }

    #[test]
    fn a_warm_up_stands_down_for_a_launch_that_got_to_the_prefix_first() {
        let client = Scripted { signs_in: true, ..Default::default() };
        let asked = preflights();
        prefix().preflights.fetch_add(1, Ordering::SeqCst);

        assert_eq!(warm_up(&client, asked, &Cancel::new()), Warmed::NotStarted);
        assert!(client.done.borrow().is_empty());
        assert!(take_client_owed(), "it comes back after the launch");
    }

    #[test]
    fn a_warm_up_beside_a_running_game_waits_for_it() {
        let client = Scripted {
            signs_in: true,
            games: vec![GameProcess { pid: 4242, name: "Wow.exe".to_owned() }],
            ..Default::default()
        };
        assert_eq!(warm_up(&client, preflights(), &Cancel::new()), Warmed::NotStarted);
        assert!(client.done.borrow().is_empty());
        assert!(take_client_owed());
    }

    #[test]
    fn a_client_that_dies_while_signing_in_ends_the_warm_up() {
        let client = Scripted { signs_in: true, lives_for: Some(Cell::new(1)), ..Default::default() };
        assert_eq!(warm_up(&client, preflights(), &Cancel::new()), Warmed::Exited);
        assert_eq!(*client.done.borrow(), ["clear", "start"]);
    }

    #[test]
    fn switching_the_mode_off_before_the_warm_up_starts_leaves_the_client_down() {
        let client = Scripted { signs_in: true, ..Default::default() };
        let cancel = Cancel::new();
        cancel.cancel();
        assert_eq!(warm_up(&client, preflights(), &cancel), Warmed::NotStarted);
        assert!(client.done.borrow().is_empty());
        assert!(!take_client_owed());
    }

    #[test]
    fn make_way_closes_an_idle_background_client_and_owes_it() {
        let client = Scripted { client: Cell::new(true), ..Default::default() };
        assert!(give_way(&client, true));
        assert_eq!(*client.done.borrow(), ["quit"]);
        assert!(take_client_owed());
    }

    #[test]
    fn make_way_closes_an_agent_a_hand_off_left_without_owing_a_client() {
        let client = Scripted { agent: true, ..Default::default() };
        assert!(give_way(&client, false));
        assert_eq!(*client.done.borrow(), ["quit"]);
        assert!(!take_client_owed());
    }

    #[test]
    fn make_way_leaves_a_client_whose_window_is_open() {
        let client = Scripted { client: Cell::new(true), window: true, ..Default::default() };
        assert!(!give_way(&client, true));
        assert!(client.done.borrow().is_empty());
    }

    #[test]
    fn make_way_leaves_a_client_that_is_updating_a_game() {
        let client = Scripted { client: Cell::new(true), ..Default::default() };
        let _updating = Updating::start();
        assert!(!give_way(&client, true));
        assert!(client.done.borrow().is_empty());
    }
}
