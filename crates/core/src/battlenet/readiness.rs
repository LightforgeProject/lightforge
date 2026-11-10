use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::paths::Layout;
use crate::state::Progress;

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
    /// The client was up and went away before it signed in.
    Exited,
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

/// Polls in a row the client must be missing for before it counts as gone,
/// about a second, so one scan that misses it does not end the wait.
const GONE_AFTER: u32 = 4;

/// Wait until the client says it is signed in and has its games.
///
/// Reads every log in [`Layout::battle_net_logs`] written after `since`, so an
/// old session cannot answer, for the client's own marker. Every one, not the
/// newest: a second invocation's log, holding only its command line, can be
/// newer than the client's. Only the logs: nothing touches the client's memory.
///
/// ```text
/// I [BNLogin] Logged into Battle.net successfully. |bnet=…
/// ```
///
/// About 5.5 s from process start on a cold Proton start.
///
/// `alive` says whether the client is in the process table. It only ends the
/// wait once the client has been seen: a cold Proton start takes seconds to
/// put it there.
pub fn wait_until_ready(
    layout: &Layout,
    user: &str,
    since: SystemTime,
    within: Duration,
    alive: impl Fn() -> bool,
) -> Readiness {
    let dir = layout.battle_net_logs(user);
    let deadline = Instant::now() + within;
    let mut logs: Vec<LogTail> = Vec::new();
    // When the sign-in line was first seen, to time the grace from.
    let mut signed_in_at: Option<Instant> = None;
    let mut seen = false;
    let mut missing = 0;

    loop {
        for path in logs_after(&dir, since) {
            if !logs.iter().any(|log| log.path == path) {
                logs.push(LogTail::new(path, 0));
            }
        }
        for log in &mut logs {
            log.catch_up();
            if !log.text.contains(SIGNED_IN) {
                continue;
            }
            let signed_in = *signed_in_at.get_or_insert_with(Instant::now);
            if client_ready(&log.text, signed_in.elapsed()) {
                tracing::info!("Battle.net is signed in and ready ({})", log.path.display());
                return Readiness::SignedIn;
            }
        }
        match alive() {
            true => {
                seen = true;
                missing = 0;
            }
            false if seen => {
                missing += 1;
                if missing >= GONE_AFTER {
                    tracing::warn!("Battle.net exited before it signed in; logs in {}", dir.display());
                    return Readiness::Exited;
                }
            }
            false => {}
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

/// What the client logs on taking a launch.
pub(super) const LAUNCHING: &str = "[GameLaunchController] {Main} LaunchBinary: uid=";

/// What the client logs once it has started the game's exe, 200 to 380 ms
/// after [`LAUNCHING`] (wowf, 2026-11-05 to 11-08).
pub(super) const LAUNCHED: &str = "[InstallManager] {Main} Launched ";

/// What the client logs when it has updated itself. It then quits, to switch
/// to the new version, and does not come back: on the next command it got
/// (build 17952 to 17986, 2026-11-04), or 4 s later with none (seqn 23 to 24,
/// 2026-11-06).
pub(super) const SELF_UPDATED: &str = "Update completed: battle.net";

/// How soon after [`SELF_UPDATED`] a client that ends its session counts as
/// gone to switch versions. One quit later than that was someone closing it.
const SWITCH_WITHIN: f64 = 30.0;

/// What the client logs when it shows the user an error, with a support code:
/// `[ErrorManager] {Main} An error occurred: BLZBNTAGT00001393` (2026-11-06).
pub(super) const ERROR_SHOWN: &str = "[ErrorManager] {Main} An error occurred";

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

/// Whether the client's log has `uid` waiting on an update. A launch then only
/// selects the game's tile; nothing starts and nothing is patched (measured on
/// Forever, 2026-11-04). The line comes once with both fields, and again each
/// time the operation changes.
pub(super) fn pending_update(log: &str, uid: &str) -> bool {
    let state = format!("InstallState ({uid}): currentOperation=");
    log.lines()
        .rev()
        .find_map(|line| line.split_once(&state).map(|(_, rest)| rest.starts_with("Update ")))
        .unwrap_or(false)
}

/// Where the client has got with updating a game.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateState {
    NotStarted,
    /// Empty while the log has given no sizes: a bar with no number, never a
    /// guessed one.
    Running(Progress),
    Completed,
}

/// What a client session's log says about updating `uid`. The sizes are on the
/// `InstallState` lines, each carrying only the fields that changed:
///
/// ```text
/// InstallState (hs_beta): currentOperationStatus=Running bytesToDownload=52458135 … downloadRate=1807604 downloadRemaining=52458135 progress=0.3
/// ```
pub(super) fn update_state(log: &str, uid: &str) -> UpdateState {
    let started = format!("Update started: {uid}");
    let completed = format!("Update completed: {uid}");
    let state = format!("InstallState ({uid}): ");
    let mut seen = false;
    let (mut total, mut remaining, mut rate) = (0u64, 0u64, 0u64);
    for line in log.lines() {
        if line.ends_with(&completed) {
            return UpdateState::Completed;
        }
        if line.ends_with(&started) {
            seen = true;
        }
        let Some((_, fields)) = line.split_once(&state) else { continue };
        for (key, value) in fields.split_whitespace().filter_map(|field| field.split_once('=')) {
            let Ok(value) = value.parse::<u64>() else { continue };
            match key {
                // Back to zero once downloaded, with the patch still to apply.
                "bytesToDownload" => total = total.max(value),
                "downloadRemaining" => remaining = value,
                "downloadRate" => rate = value,
                _ => {}
            }
        }
    }
    match seen {
        true => UpdateState::Running(Progress {
            downloaded_bytes: total.saturating_sub(remaining),
            total_bytes: total,
            bytes_per_second: rate,
        }),
        false => UpdateState::NotStarted,
    }
}

/// The log of the client session running now, read from where it stood when
/// found.
pub struct SessionLog {
    path: PathBuf,
    from: u64,
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
            text.contains(SIGNED_IN).then_some(Self { path, from: text.len() as u64 })
        })
    }

    /// Wait until the client no longer counts `uid` as running. `false` if it
    /// still does after `within`.
    pub fn wait_until_closed(&self, uid: &str, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut log = LogTail::new(self.path.clone(), 0);
        loop {
            log.catch_up();
            if !counts_as_running(&log.text, uid) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(READY_POLL);
        }
    }

    /// Whether the session ended to switch to a version the client had just
    /// updated itself to. Told apart by the time on its lines: only the
    /// session's own clock, the prefix's, is on both.
    pub fn quit_to_update(&self) -> bool {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            return false;
        };
        let updated = text.lines().rev().find(|line| line.trim_end().ends_with(SELF_UPDATED)).and_then(line_time);
        let ended = text.lines().rev().find_map(line_time);
        updated.zip(ended).is_some_and(|(updated, ended)| ended - updated <= SWITCH_WITHIN)
    }

    /// Whether the client updated itself during this session.
    pub fn updated_itself(&self) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|text| text.contains(SELF_UPDATED))
    }

    /// Whether the client is holding `uid` back for an update.
    pub fn pending_update(&self, uid: &str) -> bool {
        std::fs::read_to_string(&self.path).is_ok_and(|text| pending_update(&text, uid))
    }

    /// Where the client has got with updating `uid` during this session.
    pub fn update_state(&self, uid: &str) -> UpdateState {
        let text = std::fs::read_to_string(&self.path).unwrap_or_default();
        update_state(&text, uid)
    }

    /// Whether the client started a game's exe since [`SessionLog::find`]. A
    /// client gone without it quit in the middle of the launch.
    pub fn game_launched(&self) -> bool {
        let mut sent = LogTail::new(self.path.clone(), self.from);
        sent.catch_up();
        sent.text.contains(LAUNCHED)
    }

    /// Whether the client took a launch sent after [`SessionLog::find`].
    pub fn launch_taken(&self, within: Duration) -> bool {
        let deadline = Instant::now() + within;
        let mut sent = LogTail::new(self.path.clone(), self.from);
        loop {
            sent.catch_up();
            // The command just sent is the only one, so any game's line answers.
            if sent.text.contains(LAUNCHING) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            std::thread::sleep(READY_POLL);
        }
    }

    /// Follow what the client does with a launch of `uid` sent after
    /// [`SessionLog::find`].
    pub fn follow(&self, uid: &str) -> LaunchWatch {
        LaunchWatch { sent: LogTail::new(self.path.clone(), self.from), running: format!("{GAME_RUNNING}{uid}") }
    }
}

/// How far the client has got with a launch, by its log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchStage {
    /// Nothing logged yet. A window it shows now may be a prompt.
    Sent,
    /// Taken: the window it raises is only the launch's.
    Taken,
    /// It showed an error, which is for the user to read.
    Failed,
    /// The game is up, and the client puts its window away itself.
    Running,
}

/// See [`SessionLog::follow`].
pub struct LaunchWatch {
    sent: LogTail,
    running: String,
}

impl LaunchWatch {
    pub fn stage(&mut self) -> LaunchStage {
        self.sent.catch_up();
        let text = &self.sent.text;
        match () {
            _ if text.contains(ERROR_SHOWN) => LaunchStage::Failed,
            _ if text.lines().any(|line| line.trim_end().ends_with(&self.running)) => LaunchStage::Running,
            _ if text.contains(LAUNCHING) => LaunchStage::Taken,
            _ => LaunchStage::Sent,
        }
    }
}

/// A log read as it grows, so a wait that polls it reads each line once.
/// Only whole lines: one the client is still writing waits for the next read.
struct LogTail {
    path: PathBuf,
    read: u64,
    text: String,
}

impl LogTail {
    fn new(path: PathBuf, from: u64) -> Self {
        Self { path, read: from, text: String::new() }
    }

    fn catch_up(&mut self) {
        let Ok(mut file) = std::fs::File::open(&self.path) else { return };
        let mut fresh = Vec::new();
        if file.seek(SeekFrom::Start(self.read)).is_err() || file.read_to_end(&mut fresh).is_err() {
            return;
        }
        let Some(end) = fresh.iter().rposition(|&byte| byte == b'\n') else { return };
        self.text.push_str(&String::from_utf8_lossy(&fresh[..=end]));
        self.read += end as u64 + 1;
    }
}

/// A line's time in seconds, from `I 2026-11-06 09:55:25.943151 [Main] …`.
fn line_time(line: &str) -> Option<f64> {
    let (date, time) = line.get(2..)?.split_once(' ')?;
    let time = time.split(' ').next()?;
    let mut ymd = date.split('-').map(|part| part.parse::<i64>().ok());
    let (year, month, day) = (ymd.next()??, ymd.next()??, ymd.next()??);
    let mut hms = time.split(':').map(|part| part.parse::<f64>().ok());
    let (hours, minutes, seconds) = (hms.next()??, hms.next()??, hms.next()??);
    // Days since 1970-01-01 (Howard Hinnant's `days_from_civil`).
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;
    Some(days as f64 * 86_400.0 + hours * 3600.0 + minutes * 60.0 + seconds)
}

/// Every `battle.net-*.log` written since `since`. By mtime: the name carries
/// the prefix's local time, which can differ from the host's.
fn logs_after(dir: &Path, since: SystemTime) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("battle.net-"))
        .filter(|entry| entry.metadata().and_then(|meta| meta.modified()).is_ok_and(|modified| modified >= since))
        .map(|entry| entry.path())
        .collect()
}
