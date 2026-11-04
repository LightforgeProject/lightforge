//! The Battle.net client kept ready in the background.

use std::time::SystemTime;

use lightforge_core::battlenet::{self, ClientWindows, Readiness};
use lightforge_core::preflight::{self, NoDownloads};

use super::*;

/// How long a client that came up without signing in is watched for someone
/// signing it in by hand.
const SIGN_IN_PATIENCE: Duration = Duration::from_secs(30 * 60);

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

pub async fn warm_battle_net(layout: Layout, user: String) -> Result<Warmed, Problem> {
    tokio::task::spawn_blocking(move || -> Result<_, Problem> {
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

/// Close the client unless something is using it: a game, or a window someone
/// opened. `false` when it was left up.
fn close_if_idle(layout: &Layout) -> Result<bool, Problem> {
    let shown = ClientWindows::connect(preflight::client_pids(layout))
        .and_then(|windows| windows.any_shown())
        .unwrap_or(false);
    if shown || !preflight::games_running(layout).is_empty() {
        return Ok(false);
    }
    let runner = lone_client_runner(layout)?;
    preflight::quit_client(&runner, layout, STOP_GRACE).map_err(|e| Problem::of(&e))?;
    Ok(true)
}

/// For the setting being switched off.
pub async fn close_background_client(layout: Layout) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || {
        Ok(match close_if_idle(&layout)? {
            true => "Battle.net closed".to_owned(),
            false => "Battle.net stays open: a game or its window is using it".to_owned(),
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
    if !preflight::client_running(&layout) {
        return;
    }
    match close_if_idle(&layout) {
        Ok(true) => tracing::info!("closed the background Battle.net on exit"),
        Ok(false) => tracing::info!("left Battle.net running on exit: a game or its window is using it"),
        Err(e) => tracing::warn!("could not close the background Battle.net on exit: {e}"),
    }
}
