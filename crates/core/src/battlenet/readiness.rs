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
pub(super) const GAMES_GRACE: Duration = Duration::from_secs(20);

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
