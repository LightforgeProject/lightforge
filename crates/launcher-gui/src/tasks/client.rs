//! The Battle.net client kept ready in the background.

use std::sync::atomic::{AtomicBool, Ordering};
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

/// The background client went away for a reason that passes, and is owed a
/// restart once nothing needs the prefix. A static because the work that
/// closes it runs far from the app's state; the client poll reads it.
static CLIENT_OWED: AtomicBool = AtomicBool::new(false);

pub(crate) fn owe_client() {
    CLIENT_OWED.store(true, Ordering::SeqCst);
}

pub(crate) fn take_client_owed() -> bool {
    CLIENT_OWED.swap(false, Ordering::SeqCst)
}

/// Set while the client updates a game for us: [`make_way`] leaves it be,
/// or the update dies with it.
static CLIENT_UPDATING: AtomicBool = AtomicBool::new(false);

/// Clears [`CLIENT_UPDATING`] on every way out of the update.
struct Updating;

impl Updating {
    fn start() -> Self {
        CLIENT_UPDATING.store(true, Ordering::SeqCst);
        Updating
    }
}

impl Drop for Updating {
    fn drop(&mut self) {
        CLIENT_UPDATING.store(false, Ordering::SeqCst);
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
}

/// `preflights_seen` is [`preflights`] when the warm-up was asked for.
pub async fn warm_battle_net(layout: Layout, user: String, preflights_seen: u64) -> Result<Warmed, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        let prefix = hold_prefix();
        if preflights() != preflights_seen {
            tracing::info!("Battle.net waits in the background: a launch got to the prefix first");
            owe_client();
            return Ok(Warmed::NotStarted);
        }
        // Without a saved sign-in this would be a form nobody can see.
        if preflight::client_running(&layout)
            || !preflight::games_running(&layout).is_empty()
            || !battlenet::signs_in_by_itself(&layout, &user)
        {
            return Ok(Warmed::NotStarted);
        }
        tracing::info!("starting Battle.net in the background");
        // Counted, so a pending wind-down leaves this start alone.
        LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let runner = lone_client_runner(&layout)?;
        let managed = AppDirs::discover()
            .ok()
            .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
            .map(|config| config.managed_products())
            .unwrap_or_default();
        Preflight::new(user.clone())
            .with_managed(managed)
            .with_background_client(true)
            .run(&runner, &layout, None, &mut NoDownloads)
            .map_err(|e| Problem::of(&e))?;

        let started = start_client(&in_tls_container(runner)?, &layout, &user, true)?;
        drop(prefix);
        Ok(match battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT) {
            Readiness::SignedIn => Warmed::Ready,
            Readiness::NeedsAttention => Warmed::NeedsSignIn { started },
        })
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// Wait for someone to sign in the client that [`warm_battle_net`] gave up on.
pub async fn await_sign_in(layout: Layout, user: String, started: SystemTime) -> Result<Warmed, Problem> {
    tokio::task::spawn_blocking(move || {
        match battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_PATIENCE) {
            Readiness::SignedIn => Warmed::Ready,
            Readiness::NeedsAttention => Warmed::NeedsSignIn { started },
        }
    })
    .await
    .map_err(|e| Problem::of(&e))
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
pub async fn update_through_client(
    layout: Layout,
    user: String,
    uid: String,
    progress: Arc<Mutex<lightforge_core::state::Progress>>,
    stop: Cancel,
) -> Result<ClientUpdated, Problem> {
    use lightforge_core::battlenet::{SessionLog, UpdateState};

    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
        let _updating = Updating::start();
        let prefix = hold_prefix();
        if let Some(game) = preflight::games_running(&layout).first() {
            return Err(format!("close {} first — Battle.net has to restart for this update", game.name).into());
        }
        let runner = lone_client_runner(&layout)?;
        // Read when it starts, and written back when it exits.
        preflight::quit_client(&runner, &layout, STOP_GRACE).map_err(|e| Problem::of(&e))?;
        battlenet::configure(&layout, &user, &[battlenet::auto_update_tweak(&uid, true)])
            .map_err(|e| Problem::of(&e))?;
        tracing::info!("asking Battle.net to update {uid}");
        LAUNCHES.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

        let started = start_client(&in_tls_container(runner)?, &layout, &user, true)?;
        drop(prefix);
        if battlenet::wait_until_ready(&layout, &user, started, SIGN_IN_TIMEOUT) == Readiness::NeedsAttention {
            return Err("Battle.net needs you to sign in — press its button, then try the update again".into());
        }
        let log = SessionLog::find(&layout, &user)
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
    .await
    .map_err(|e| Problem::of(&e))?
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
/// opened. `false` when it was left up, as it is when its windows cannot be
/// read: closing one in use loses what someone was doing in it.
fn close_if_idle(layout: &Layout) -> Result<bool, Problem> {
    let open = ClientWindows::connect(preflight::client_pids(layout))
        .and_then(|windows| windows.any_open())
        .unwrap_or_else(|e| {
            tracing::warn!("Battle.net's windows cannot be read, so it stays up: {}", chain(&e));
            true
        });
    if open || !preflight::games_running(layout).is_empty() {
        return Ok(false);
    }
    let runner = lone_client_runner(layout)?;
    preflight::quit_client(&runner, layout, STOP_GRACE).map_err(|e| Problem::of(&e))?;
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
pub(crate) fn make_way(layout: &Layout) -> bool {
    let _prefix = hold_prefix();
    // Before the scan: between its restart and the update the client is
    // briefly not up at all.
    if CLIENT_UPDATING.load(Ordering::SeqCst) {
        tracing::debug!("Battle.net stays up for work on the game files: it is updating a game");
        return false;
    }
    let up = preflight::scan(layout)
        .iter()
        .any(|p| matches!(p.kind, preflight::ProcKind::BattleNet | preflight::ProcKind::Agent));
    if !up {
        return true;
    }
    let background = AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .is_some_and(|config| config.battle_net_in_background);
    match close_if_idle(layout) {
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
        if battlenet::SessionLog::find(&layout, &user).is_some_and(|log| log.updated_itself()) {
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
pub async fn close_background_client(layout: Layout, user: String) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || {
        let _prefix = hold_prefix();
        if preflight::client_running(&layout) && !close_if_idle(&layout)? {
            return Ok("Battle.net stays open: a game or its window is using it".to_owned());
        }
        Ok(match battlenet::restore_overrides(&layout, &user).map_err(|e| Problem::of(&e))? {
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
    if !preflight::client_running(&layout) {
        return;
    }
    match close_if_idle(&layout) {
        Ok(true) => tracing::info!("closed the background Battle.net on exit"),
        Ok(false) => tracing::info!("left Battle.net running on exit: a game or its window is using it"),
        Err(e) => tracing::warn!("could not close the background Battle.net on exit: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn make_way_leaves_a_client_that_is_updating_a_game() {
        let scratch = tempfile::tempdir().unwrap();
        let _updating = Updating::start();
        assert!(!make_way(&Layout::with_root(scratch.path())));
    }
}
