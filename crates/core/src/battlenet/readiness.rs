use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::paths::Layout;

/// Whether the client is set up to sign in without being asked.
///
/// Decides whether it is safe to start minimized. Without auto-login the
/// client comes up at a sign-in form, and a sign-in form nobody can see is a
/// launch that quietly never happens.
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
    /// Still not signed in when the wait ran out. Two-factor, an outage, a
    /// pending client update: all of them need a person, and none of them
    /// are ours to drive.
    NeedsAttention,
}

/// The line the client writes once it is signed in.
pub(super) const SIGNED_IN: &str = "Logged into Battle.net successfully";

/// The line that says its game list is built, the only moment a launch
/// command is acted on. Sign-in comes first and the list a beat later, or
/// several seconds later on a slow start, and a command that arrives in
/// between is consumed without a trace: no `LaunchBinary`, nothing starts.
/// Measured on 2026-10-24: every launch that worked arrived after this line,
/// both that failed arrived before it, Warcraft III and Hearthstone alike.
pub(super) const GAMES_READY: &str = "GameController initialization complete";

/// How long after sign-in the game list gets before the command goes anyway.
/// The client's own log is the only witness, and a line that never comes
/// (the window-drawn marker was missing from one log in six) must cost a
/// short wait, not the whole sign-in timeout.
pub(super) const GAMES_GRACE: Duration = Duration::from_secs(20);

/// Whether a launch command sent now would be acted on, given the log so far
/// and how long it has said the client is signed in.
pub(super) fn client_ready(log: &str, signed_in_for: Duration) -> bool {
    log.contains(SIGNED_IN) && (log.contains(GAMES_READY) || signed_in_for >= GAMES_GRACE)
}

// There is deliberately no "the window has drawn" marker here. The obvious
// candidate, `End recording load sequence for browser content_browser`: is
// missing from one full-client log in six, so waiting for it stalls for the
// whole grace period on a client that is perfectly ready, long enough for
// someone to close the window out from under the launch. Anything that must
// survive the client's startup is sent as a `battlenet://` URI instead, which
// the client queues and runs when it is ready. See [`Launch::Uri`].

/// How often the log is re-read while waiting.
const READY_POLL: Duration = Duration::from_millis(250);

/// Wait until the client says it is signed in and has its games.
///
/// Reads the newest log in [`Layout::battle_net_logs`] written after `since`
/// (so an old session cannot answer) for the client's own marker. Nothing is
/// injected or read from its memory.
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

/// The most recently written `battle.net-*.log`, if it is newer than `since`.
///
/// By modification time rather than by the timestamp in the name: the name is
/// the client's local time, and a prefix does not have to agree with the host
/// about what that is.
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
