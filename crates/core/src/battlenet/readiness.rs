use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::paths::Layout;

/// Whether the client signs in by itself, which is what makes starting it
/// minimized safe: a hidden sign-in form is a launch that never happens.
pub fn signs_in_by_itself(layout: &Layout, user: &str) -> bool {
    let Ok(text) = std::fs::read_to_string(layout.battle_net_config(user)) else {
        return false;
    };
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let client = config.get("Client");
    let flag = |key: &str| {
        client
            .and_then(|c| c.get(key))
            .and_then(Value::as_str)
            .is_some_and(|value| value.eq_ignore_ascii_case("true"))
    };
    let named = client
        .and_then(|c| c.get("SavedAccountNames"))
        .and_then(Value::as_str)
        .is_some_and(|names| !names.trim().is_empty());

    flag("AutoLogin") && named
}

/// What the client's log said while we waited for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Readiness {
    /// Signed in and idle: a launch command sent now will be acted on.
    SignedIn,
    /// Still not signed in when the wait ran out: two-factor, an outage or a
    /// client update, all of which need a person.
    NeedsAttention,
}

/// The line the client writes once it is signed in.
pub(super) const SIGNED_IN: &str = "Logged into Battle.net successfully";

/// The line that says its game list is built. A launch command sent between
/// sign-in and this line is dropped without a trace (no `LaunchBinary`).
/// Measured 2026-10-24: every launch that worked came after it, both that
/// failed came before, Warcraft III and Hearthstone alike.
pub(super) const GAMES_READY: &str = "GameController initialization complete";

/// How long after sign-in the game list gets before the command goes anyway,
/// so a marker that never comes costs a short wait, not the sign-in timeout.
/// It waits on Agent, which once took 27 s after sign-in (2026-11-04).
pub(super) const GAMES_GRACE: Duration = Duration::from_secs(40);

/// Whether a launch command sent now would be acted on, given the log so far
/// and how long it has said the client is signed in.
pub(super) fn client_ready(log: &str, signed_in_for: Duration) -> bool {
    log.contains(SIGNED_IN) && (log.contains(GAMES_READY) || signed_in_for >= GAMES_GRACE)
}

// No "window has drawn" marker: `End recording load sequence for browser
// content_browser` is missing from one log in six. What must survive the
// client's startup goes as a `battlenet://` URI, which it queues ([`Launch::Uri`]).

/// How often the log is re-read while waiting.
const READY_POLL: Duration = Duration::from_millis(250);

/// Wait until the client says it is signed in and has its games.
///
/// Reads the newest log in [`Layout::battle_net_logs`] written after `since`,
/// so an old session cannot answer, for the client's own marker. Only the
/// log: nothing touches the client's memory.
///
/// ```text
/// I [BNLogin] Logged into Battle.net successfully. |bnet=…
/// ```
///
/// About 5.5 s from process start on a cold Proton start.
pub fn wait_until_ready(
    layout: &Layout,
    user: &str,
    since: SystemTime,
    within: Duration,
) -> Readiness {
    let dir = layout.battle_net_logs(user);
    let deadline = Instant::now() + within;
    // When the sign-in line was first seen, to time the grace from.
    let mut signed_in_at: Option<Instant> = None;

    loop {
        if let Some(path) = newest_log_after(&dir, since) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains(SIGNED_IN) {
                    let seen = *signed_in_at.get_or_insert_with(Instant::now);
                    if client_ready(&text, seen.elapsed()) {
                        tracing::info!("Battle.net is signed in and ready ({})", path.display());
                        return Readiness::SignedIn;
                    }
                }
            }
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                "Battle.net did not sign in within {within:?}; logs in {}",
                dir.display()
            );
            return Readiness::NeedsAttention;
        }
        std::thread::sleep(READY_POLL);
    }
}

/// What the client logs when Agent reports a game it launched as up or gone.
/// It hears of an exit 4 to 6 s late, and a launch it is sent meanwhile only
/// selects the game's tile (measured 2026-11-04).
pub(super) const GAME_RUNNING: &str = "Game is running: ";
pub(super) const GAME_STOPPED: &str = "Game is no longer running: ";

/// What the client logs on taking a launch: starting the game, or updating
/// it first.
pub(super) const LAUNCHING: &str = "[GameLaunchController] {Main} LaunchBinary: uid=";
pub(super) const UPDATING: &str = "[GameController] {Main} Update: uid=";

/// What the client logs when it has updated itself. It then quits on the next
/// command it gets, to switch to the new version, and does not come back
/// (build 17952 to 17986, 2026-11-04).
pub(super) const SELF_UPDATED: &str = "Update completed: battle.net";

/// What the client did with a launch command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Taken {
    Launching,
    /// The game needs an update, which the client started instead.
    Updating,
    /// Nothing within the wait: no launch, no update.
    Ignored,
}

/// Whether the client's log still has `uid` running.
pub(super) fn counts_as_running(log: &str, uid: &str) -> bool {
    let running = format!("{GAME_RUNNING}{uid}");
    let stopped = format!("{GAME_STOPPED}{uid}");
    log.lines()
        .rev()
        .find_map(|line| match line {
            _ if line.ends_with(&running) => Some(true),
            _ if line.ends_with(&stopped) => Some(false),
            _ => None,
        })
        .unwrap_or(false)
}

/// What a log written after a launch command says the client did with it.
/// The command just sent is the only one, so any game's line answers.
pub(super) fn taken_in(log: &str) -> Option<Taken> {
    log.lines().find_map(|line| match line {
        _ if line.contains(LAUNCHING) => Some(Taken::Launching),
        _ if line.contains(UPDATING) => Some(Taken::Updating),
        _ => None,
    })
}

/// The log of the client session running now, read from where it stood when
/// found.
pub struct SessionLog {
    path: PathBuf,
    from: usize,
}

impl SessionLog {
    /// The newest log that signed in. A second invocation writes one of its
    /// own, newer, that only holds its command line.
    pub fn find(layout: &Layout, user: &str) -> Option<Self> {
        let mut logs: Vec<_> = std::fs::read_dir(layout.battle_net_logs(user))
            .ok()?
            .flatten()
            .filter(|entry| entry.file_name().to_string_lossy().starts_with("battle.net-"))
            .filter_map(|entry| Some((entry.metadata().ok()?.modified().ok()?, entry.path())))
            .collect();
        logs.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
        logs.into_iter().take(4).find_map(|(_, path)| {
            let text = std::fs::read_to_string(&path).ok()?;
            text.contains(SIGNED_IN).then_some(Self { path, from: text.len() })
        })
    }

    /// Wait until the client no longer counts `uid` as running. `false` if it
    /// still does after `within`.
    pub fn wait_until_closed(&self, uid: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        loop {
            let text = std::fs::read_to_string(&self.path).unwrap_or_default();
            if !counts_as_running(&text, uid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(READY_POLL);
        }
    }

    /// Whether the client updated itself during this session.
    pub fn updated_itself(&self) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|text| text.contains(SELF_UPDATED))
    }

    /// What the client did with a launch sent after [`SessionLog::find`].
    pub fn taken(&self, within: Duration) -> Taken {
        let deadline = Instant::now() + within;
        loop {
            let text = std::fs::read_to_string(&self.path).unwrap_or_default();
            if let Some(taken) = text.get(self.from..).and_then(taken_in) {
                return taken;
            }
            if Instant::now() >= deadline {
                return Taken::Ignored;
            }
            std::thread::sleep(READY_POLL);
        }
    }
}

/// The most recently written `battle.net-*.log`, if newer than `since`. By
/// mtime: the name carries the prefix's local time, which can differ from the host's.
fn newest_log_after(dir: &Path, since: SystemTime) -> Option<PathBuf> {
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with("battle.net-")
        })
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            (modified >= since).then(|| (modified, entry.path()))
        })
        .max_by_key(|(modified, _)| *modified)
        .map(|(_, path)| path)
}
