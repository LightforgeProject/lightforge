//! Automated, headless Battle.net setup inside the shared prefix.
//!
//! Four phases, in this order:
//!
//! 1. fetch `Battle.net-Setup.exe` from Blizzard's downloader,
//! 2. run it under Proton with an explicit install path, waiting for the
//!    install to appear rather than for the process to exit,
//! 3. apply the Wine registry tweaks Battle.net benefits from,
//! 4. inject Lightforge's defaults into `Battle.net.config`.
//!
//! Phase 2 needs the unusual completion rule: `Battle.net-Setup.exe` is a
//! bootstrapper. It unpacks a launcher, hands off to `Battle.net.exe`/`Agent`
//! and exits — or doesn't exit at all, since the client it spawned keeps the
//! Wine session alive. Waiting on the child's exit status therefore either
//! returns far too early or never returns, so completion is detected from the
//! filesystem and the session is torn down with `wineserver -k`.
//!
//! Phases 3 and 4 run with no Battle.net process alive: the client rewrites
//! `Battle.net.config` on exit and would otherwise clobber the injection.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant, SystemTime};

use serde_json::{json, Value};

use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};
use crate::product::ProductCode;

/// Blizzard's version-agnostic installer endpoint.
pub const INSTALLER_URL: &str =
    "https://downloader.battle.net/download/getInstaller?os=win&installer=Battle.net-Setup.exe";

/// File name the installer is cached under.
pub const INSTALLER_FILE: &str = "Battle.net-Setup.exe";

/// Windows-side install path handed to the installer. It has to be the Windows
/// spelling, not the Linux one — the installer writes it into `.product.db`.
const WINDOWS_BATTLE_NET_DIR: &str = r"C:\Program Files (x86)\Battle.net";

/// Where Battle.net should propose installing games. The WoW symlink lives
/// directly under this directory, so an install it starts itself lands in
/// `games/` without a "locate game" prompt.
const WINDOWS_PROGRAM_FILES: &str = r"C:\Program Files (x86)";

/// Default UI language passed to the installer.
const DEFAULT_LOCALE: &str = "enUS";

/// How long the installer gets before it is considered stuck.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Gap between filesystem checks while waiting for the install.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long to keep watching after the installer process has exited.
///
/// Exiting is not the same as failing: Blizzard's setup bootstraps a second
/// process and the first can return before the files land, so a short grace
/// period after a *clean* exit avoids calling a working install a failure.
const EXIT_GRACE: Duration = Duration::from_secs(60);

/// The same grace after a *failed* exit.
///
/// Kept short deliberately. A non-zero exit is the installer telling us it
/// gave up; the only reason to wait at all is the vanishing chance that the
/// files landed in the same instant.
const FAILED_EXIT_GRACE: Duration = Duration::from_secs(5);

/// Most stderr kept from the installer, in bytes.
///
/// Enough for a dynamic-loader complaint or a stack of Wine warnings, bounded
/// so a chatty installer cannot grow this without limit.
const STDERR_TAIL: usize = 8 * 1024;

/// How many of those lines the error message itself shows.
///
/// Capturing generously and displaying briefly are different jobs: the tail is
/// kept in full for a debug bundle, while the message goes into a banner in
/// the GUI, where eight kilobytes of Wine chatter would bury the one line that
/// matters. The last lines are the right ones — a failing process says why
/// just before it stops.
const STDERR_LINES_SHOWN: usize = 8;

/// Consecutive successful checks required before the install counts as done.
/// The installer writes the launcher before the versioned client directory is
/// fully populated, so one positive sample is not enough.
const SETTLE_POLLS: u32 = 3;

/// Download the installer into `cache_dir`, returning its path.
///
/// Blizzard publishes no checksum for this endpoint, so unlike Proton-GE there
/// is nothing to verify against; the transfer is HTTPS-authenticated and that
/// is all the guarantee available.
pub async fn fetch_installer(
    http: &crate::net::Http,
    cache_dir: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, BattleNetError> {
    let dest = cache_dir.join(INSTALLER_FILE);
    http.download(INSTALLER_URL, &dest, on_progress).await?;
    Ok(dest)
}

/// Runs `Battle.net-Setup.exe` in the prefix and waits for the install to land.
#[derive(Debug, Clone)]
pub struct Installer {
    installer: PathBuf,
    install_dir: PathBuf,
    locale: String,
    timeout: Duration,
    poll_interval: Duration,
    exit_grace: Duration,
    failed_exit_grace: Duration,
}

impl Installer {
    /// Install into the Battle.net directory `layout` describes.
    pub fn new(layout: &Layout, installer: impl Into<PathBuf>) -> Self {
        Self {
            installer: installer.into(),
            install_dir: layout.battle_net_dir(),
            locale: DEFAULT_LOCALE.to_owned(),
            timeout: DEFAULT_TIMEOUT,
            exit_grace: EXIT_GRACE,
            failed_exit_grace: FAILED_EXIT_GRACE,
            poll_interval: POLL_INTERVAL,
        }
    }

    /// Battle.net's UI language, e.g. `deDE`.
    pub fn with_locale(mut self, locale: impl Into<String>) -> Self {
        self.locale = locale.into();
        self
    }

    /// Give up after `timeout` instead of the ten-minute default.
    pub fn with_timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Shorten the polling gap — used by tests, and by nothing else.
    /// How long to keep watching after the installer exits — cleanly, then
    /// with a failure. Exists for the same reason [`Installer::with_timeout`]
    /// does: so a test does not have to wait a real minute.
    pub fn with_exit_grace(mut self, clean: Duration, failed: Duration) -> Self {
        self.exit_grace = clean;
        self.failed_exit_grace = failed;
        self
    }

    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Arguments passed to the installer.
    ///
    /// `--installpath` is what keeps Battle.net out of the default
    /// `%LOCALAPPDATA%` location, where a prefix wipe would take it with it.
    pub fn args(&self) -> Vec<String> {
        vec![
            format!("--lang={}", self.locale),
            format!("--installpath={WINDOWS_BATTLE_NET_DIR}"),
        ]
    }

    /// Whether a usable Battle.net install is already present.
    pub fn is_installed(&self) -> bool {
        is_installed(&self.install_dir)
    }

    /// Run the installer, or return immediately if Battle.net is already there.
    ///
    /// Leaves the prefix quiesced: whatever the installer spawned is killed and
    /// `wineserver` is shut down before returning, so the caller can touch
    /// registry and config files safely.
    pub fn install(&self, runner: &ProtonRunner) -> Result<InstallOutcome, BattleNetError> {
        if self.is_installed() {
            return Ok(InstallOutcome {
                install_dir: self.install_dir.clone(),
                skipped: true,
                waited: Duration::ZERO,
            });
        }
        if !self.installer.is_file() {
            return Err(BattleNetError::InstallerMissing(self.installer.clone()));
        }

        let started = Instant::now();
        let mut child = runner
            .command("run", &self.installer, self.args())
            .stdin(Stdio::null())
            // stdout is noise nobody reads. stderr is the one thing worth
            // having when this goes wrong — it is where Proton's dynamic
            // loader names a missing 32-bit library — so it is piped and
            // drained on a thread. Piping without a reader would eventually
            // block the child on a full pipe, which is why the drain exists
            // rather than a plain `.stderr(piped())`.
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| {
                BattleNetError::Prefix(PrefixError::Spawn {
                    program: self.installer.clone(),
                    source: e,
                })
            })?;

        let drain = child.stderr.take().map(|pipe| std::thread::spawn(move || tail_of(pipe)));

        let waited = self.wait_for_install(started, Some(&mut child));

        // Either way the session has to go: on success the installer left a
        // client running, and on failure a half-finished one.
        let _ = child.kill();
        let _ = child.wait();
        runner.wineserver_kill()?;

        // Only joined after the child is gone, so the pipe is closed and the
        // reader cannot block.
        let stderr = drain.and_then(|handle| handle.join().ok()).unwrap_or_default();

        match waited {
            Ok(outcome) => Ok(outcome),
            Err(Wait::TimedOut) => Err(BattleNetError::InstallTimeout {
                after: self.timeout,
                dir: self.install_dir.clone(),
            }),
            Err(Wait::Exited(code)) => Err(BattleNetError::InstallerExited {
                code,
                dir: self.install_dir.clone(),
                stderr,
            }),
        }
    }

    /// Poll the filesystem until the install settles, the installer gives up,
    /// or the timeout expires.
    ///
    /// Watching the files is still the primary signal — the installer stays
    /// alive after a successful install, having launched the client — but
    /// watching *only* the files means a process that died in the first
    /// second is indistinguishable from one still downloading, and the user
    /// waits the full ten minutes to be told it "timed out". It did not time
    /// out; it failed immediately, and said why on a stderr nobody kept.
    ///
    /// `child` is optional so the timeout path can be tested without one.
    fn wait_for_install(
        &self,
        started: Instant,
        mut child: Option<&mut Child>,
    ) -> Result<InstallOutcome, Wait> {
        let mut settled = 0;
        let mut exit: Option<(Instant, Option<i32>)> = None;

        loop {
            if is_installed(&self.install_dir) {
                settled += 1;
                if settled >= SETTLE_POLLS {
                    return Ok(InstallOutcome {
                        install_dir: self.install_dir.clone(),
                        skipped: false,
                        waited: started.elapsed(),
                    });
                }
            } else {
                settled = 0;
            }

            if exit.is_none() {
                if let Some(child) = child.as_deref_mut() {
                    if let Ok(Some(status)) = child.try_wait() {
                        exit = Some((Instant::now(), status.code()));
                    }
                }
            }

            // An exit is not itself a failure — the files may still be
            // landing — but it bounds how long waiting can be worthwhile.
            if let Some((at, code)) = exit {
                let grace = match code {
                    Some(0) | None => self.exit_grace,
                    Some(_) => self.failed_exit_grace,
                };
                if at.elapsed() >= grace {
                    return Err(Wait::Exited(code));
                }
            }

            if started.elapsed() >= self.timeout {
                return Err(Wait::TimedOut);
            }
            std::thread::sleep(self.poll_interval);
        }
    }
}

/// Why [`Installer::wait_for_install`] stopped waiting.
///
/// Internal: the caller turns these into [`BattleNetError`], which it can only
/// do once it has drained the installer's stderr.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Wait {
    /// Nothing happened for the whole timeout.
    TimedOut,
    /// The installer process ended and the files never appeared.
    Exited(Option<i32>),
}

/// The last `n` non-empty lines of `text`, in order.
fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Read a pipe to the end, keeping only the last [`STDERR_TAIL`] bytes.
///
/// Reads everything so the writer never blocks on a full pipe, but retains a
/// bounded tail — the end is where the error is.
fn tail_of(mut pipe: impl Read) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        match pipe.read(&mut chunk) {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&chunk[..n]);
                if buf.len() > STDERR_TAIL * 2 {
                    buf.drain(..buf.len() - STDERR_TAIL);
                }
            }
        }
    }
    if buf.len() > STDERR_TAIL {
        buf.drain(..buf.len() - STDERR_TAIL);
    }
    String::from_utf8_lossy(&buf).trim().to_owned()
}

/// What [`Installer::install`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallOutcome {
    pub install_dir: PathBuf,
    /// Battle.net was already installed and the installer never ran.
    pub skipped: bool,
    pub waited: Duration,
}

/// A Battle.net install is only usable once the bootstrapper has written both
/// the launcher and the versioned client directory it launches.
fn is_installed(dir: &Path) -> bool {
    dir.join("Battle.net Launcher.exe").is_file() && has_versioned_client(dir)
}

/// The client unpacks itself into a sibling `Battle.net.<build>` directory.
fn has_versioned_client(dir: &Path) -> bool {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return false;
    };
    entries
        .flatten()
        .any(|e| e.path().is_dir() && e.file_name().to_string_lossy().starts_with("Battle.net."))
}

/// One `Battle.net.config` setting, addressed by its path through the JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Tweak {
    pub path: Vec<String>,
    pub value: Value,
    /// Why it's set — surfaced by the CLI so the config isn't a pile of magic.
    pub reason: &'static str,
}

impl Tweak {
    fn new(path: &str, value: Value, reason: &'static str) -> Self {
        Self {
            path: path.split('.').map(str::to_owned).collect(),
            value,
            reason,
        }
    }

    /// Dotted form, for display.
    pub fn key(&self) -> String {
        self.path.join(".")
    }
}

/// Lightforge's defaults for `Battle.net.config`.
///
/// Every value is a JSON *string*: Battle.net writes booleans and numbers that
/// way itself, and a real boolean here is silently ignored by the client.
pub fn default_tweaks() -> Vec<Tweak> {
    vec![
        Tweak::new(
            "Client.GameLaunchWindowBehavior",
            json!("2"),
            "exit Battle.net completely once the game has been handed off",
        ),
        Tweak::new(
            "Client.HardwareAcceleration",
            json!("false"),
            "GPU-accelerated CEF under Wine is the usual cause of storefront rendering glitches",
        ),
        Tweak::new(
            "Client.Install.DefaultInstallPath",
            json!(WINDOWS_PROGRAM_FILES),
            "propose installs next to the WoW symlink, so they land in games/",
        ),
        Tweak::new(
            "Client.GameSearch.BackgroundSearch",
            json!("false"),
            "no background scanning of the filesystem for other Blizzard installs",
        ),
    ]
}

/// The download limit Battle.net applies to its own transfers, in bytes per
/// second, or `None` when it has none set.
pub const DOWNLOAD_LIMIT_KEY: &str = "Client.Install.LastUserDownloadLimitNextPatchInBps";

/// Read Battle.net's own download limit out of its config.
///
/// The client stores it as a string; a value it cannot be read as a number is
/// reported as no limit rather than as an error, since a launcher has nothing
/// useful to do with a malformed one.
pub fn download_limit(layout: &Layout, user: &str) -> Option<u64> {
    let text = std::fs::read_to_string(layout.battle_net_config(user)).ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    let mut node = &config;
    for key in DOWNLOAD_LIMIT_KEY.split('.') {
        node = node.get(key)?;
    }
    node.as_str()?.parse().ok().filter(|bps| *bps > 0)
}

/// Set or clear Battle.net's own download limit.
///
/// `None` removes the key rather than writing a zero: the client wrote this
/// from its own settings dropdown and absent is the shape it uses before
/// anyone has chosen, which is the closest thing to "no limit" that can be
/// written without guessing at a sentinel value.
pub fn set_download_limit(
    layout: &Layout,
    user: &str,
    bytes_per_second: Option<u64>,
) -> Result<(), BattleNetError> {
    match bytes_per_second {
        Some(bps) => {
            let tweak = Tweak::new(
                DOWNLOAD_LIMIT_KEY,
                json!(bps.to_string()),
                "download limit Battle.net applies to its own transfers",
            );
            merge_file(&layout.battle_net_config(user), &[tweak], true)?;
        }
        None => remove_key(&layout.battle_net_config(user), DOWNLOAD_LIMIT_KEY)?,
    }
    Ok(())
}

/// Keep Agent's hands off a product Lightforge downloads itself.
///
/// Only for managed products. Battle.net owns the titles it installed, and
/// switching their updates off would be taking something away rather than
/// protecting anything: the hazard here is specific to a CASC store that two
/// programs both believe they maintain.
pub fn product_tweaks(codes: &[ProductCode]) -> Vec<Tweak> {
    codes
        .iter()
        .flat_map(|code| {
            [
                Tweak::new(
                    &format!("Games.{code}.AutoUpdate"),
                    json!("false"),
                    "Lightforge patches this product; Agent must not race it into the store",
                ),
                Tweak::new(
                    &format!("Games.{code}.DownloadAllUpdates"),
                    json!("false"),
                    "no background pre-fetching into a store Lightforge owns",
                ),
            ]
        })
        .collect()
}

/// Defaults for a per-account `<hash>.config`.
///
/// "Full quit, never tray-minimize" lives here rather than in
/// `Battle.net.config`: the client stores window behavior per account, and
/// keys of this name in the global config are silently ignored.
pub fn account_tweaks() -> Vec<Tweak> {
    vec![
        Tweak::new(
            "User.Client.HideOnClose",
            json!("false"),
            "closing the window quits — no tray icon, no lingering background client",
        ),
        Tweak::new(
            "User.Client.DisplayExitPrompt",
            json!("false"),
            "quit immediately instead of showing the close countdown",
        ),
    ]
}

/// Where the client remembers how its window was last left.
///
/// Qt's own `WindowState` flags, written by the client when it exits and read
/// when it starts: `0` is a normal window, `1` is minimized. Setting it is the
/// only lever there is for starting the client out of the way — Wine cannot
/// minimize a window for us, and nothing on Wayland can do it from outside.
pub const WINDOW_STATE_KEY: &str = "User.Client.Windows.BrowserMainWindow.WindowState";

/// Start the client minimized, or normally.
///
/// Has to be re-applied before each start, because the client writes this key
/// back from its live window every time it exits — the same reason the game's
/// CVars are re-asserted per launch rather than set once.
pub fn window_state_tweaks(minimized: bool) -> Vec<Tweak> {
    vec![Tweak::new(
        WINDOW_STATE_KEY,
        json!(match minimized {
            true => "1",
            false => "0",
        }),
        match minimized {
            true => "come up minimized: the hand-off drives the client, nobody needs to see it",
            false => "come up as a normal window",
        },
    )]
}

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
    /// pending client update — all of them need a person, and none of them
    /// are ours to drive.
    NeedsAttention,
}

/// The line the client writes once it is signed in and the UI is live.
const SIGNED_IN: &str = "Logged into Battle.net successfully";

/// How often the log is re-read while waiting.
const READY_POLL: Duration = Duration::from_millis(250);

/// Wait until the client says it is signed in.
///
/// Watches the newest log in [`Layout::battle_net_logs`] that the client
/// wrote *after* `since`, so a previous session's log cannot answer for this
/// one. Reading a log file is the whole mechanism — nothing is injected,
/// nothing is scraped out of the client's memory, and the marker is a line
/// the client prints for its own benefit:
///
/// ```text
/// I [GSStateTracker] State changed to: Ready
/// I [AccountManager] Account manager state has changed to: Ready
/// I [BNLogin] Logged into Battle.net successfully. |bnet=…
/// ```
///
/// Measured on a cold start under Proton: about five and a half seconds from
/// the process appearing to that last line.
pub fn wait_until_signed_in(
    layout: &Layout,
    user: &str,
    since: SystemTime,
    within: Duration,
) -> Readiness {
    let dir = layout.battle_net_logs(user);
    let deadline = Instant::now() + within;

    loop {
        if let Some(path) = newest_log_after(&dir, since) {
            if let Ok(text) = std::fs::read_to_string(&path) {
                if text.contains(SIGNED_IN) {
                    return Readiness::SignedIn;
                }
            }
        }
        if Instant::now() >= deadline {
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

/// Merge `tweaks` into a parsed config, creating missing objects on the way
/// down and leaving every unrelated key untouched.
///
/// Preserving the rest matters: the client stores login state and per-game
/// settings in the same file, and a wholesale rewrite would log the user out.
pub fn apply_tweaks(config: &mut Value, tweaks: &[Tweak]) {
    if !config.is_object() {
        *config = json!({});
    }
    for tweak in tweaks {
        let Some((last, parents)) = tweak.path.split_last() else {
            continue;
        };
        let mut node = &mut *config;
        for key in parents {
            let next = node
                .as_object_mut()
                .expect("node is an object")
                .entry(key.clone())
                .or_insert_with(|| json!({}));
            // A scalar where an object belongs is a corrupt config; replacing
            // that one branch is better than failing the whole setup.
            if !next.is_object() {
                *next = json!({});
            }
            node = next;
        }
        node.as_object_mut()
            .expect("node is an object")
            .insert(last.clone(), tweak.value.clone());
    }
}

/// Read, merge, and atomically rewrite `Battle.net.config`.
///
/// A missing file is normal — Battle.net only writes one on first launch — and
/// is treated as an empty config rather than an error, so setup can seed the
/// values before the client has ever run.
pub fn configure(layout: &Layout, user: &str, tweaks: &[Tweak]) -> Result<PathBuf, BattleNetError> {
    let path = layout.battle_net_config(user);
    merge_file(&path, tweaks, true)?;
    Ok(path)
}

/// Product codes the client itself records, read from `Battle.net.config`.
///
/// The `Games` object is keyed by the code Battle.net uses for each title it
/// knows about, and that is not always the code Lightforge's own tables carry:
/// `version::KNOWN_PRODUCTS` holds `hsb` for Hearthstone while the client and
/// `product.db` both say `hs_beta`. This is the client's own answer, so it is
/// the one that matches everywhere else on the machine.
///
/// An absent or unreadable file yields nothing rather than an error — the
/// client writes it on first launch, and a machine that has not run it yet is
/// not a broken one.
pub fn known_product_codes(layout: &Layout, user: &str) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(layout.battle_net_config(user)) else {
        return Vec::new();
    };
    let Ok(config) = serde_json::from_str::<Value>(&text) else {
        return Vec::new();
    };
    config
        .get("Games")
        .and_then(Value::as_object)
        .map(|games| games.keys().cloned().collect())
        .unwrap_or_default()
}

/// Apply `tweaks` to every per-account config next to `Battle.net.config`,
/// returning the files changed.
///
/// These files are named after an account hash the client invents on first
/// login, so there is nothing to write before the user has signed in once — an
/// empty result is the normal outcome of a fresh setup, not a failure. M3's
/// pre-flight pass is what applies them on later runs.
pub fn configure_accounts(
    layout: &Layout,
    user: &str,
    tweaks: &[Tweak],
) -> Result<Vec<PathBuf>, BattleNetError> {
    let Some(dir) = layout.battle_net_config(user).parent().map(Path::to_path_buf) else {
        return Ok(Vec::new());
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    let mut changed = Vec::new();
    for path in account_configs(entries) {
        merge_file(&path, tweaks, false)?;
        changed.push(path);
    }
    changed.sort();
    Ok(changed)
}

/// Every `<hash>.config` in the directory, excluding the global config.
fn account_configs(entries: std::fs::ReadDir) -> Vec<PathBuf> {
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension().is_some_and(|e| e == "config")
                && p.file_name().is_some_and(|n| n != "Battle.net.config")
        })
        .collect()
}

/// Read, merge, and atomically rewrite one config file.
///
/// `seed` decides what a missing file means: the global config is created from
/// nothing, while a per-account file is the client's to invent.
/// Delete one dotted key, leaving the rest of the file alone.
///
/// A missing file or a key that was never there is success: the caller asked
/// for it to be absent, and it is.
fn remove_key(path: &Path, key: &str) -> Result<(), BattleNetError> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(());
    };
    let mut config: Value = serde_json::from_str(&text)
        .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?;

    let parts: Vec<&str> = key.split('.').collect();
    let Some((last, parents)) = parts.split_last() else {
        return Ok(());
    };
    let mut node = &mut config;
    for parent in parents {
        match node.get_mut(*parent) {
            Some(next) => node = next,
            None => return Ok(()),
        }
    }
    let Some(object) = node.as_object_mut() else {
        return Ok(());
    };
    if object.remove(*last).is_none() {
        return Ok(());
    }

    let mut text = serde_json::to_string_pretty(&config)
        .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?;
    text.push('\n');
    write_atomic(path, text.as_bytes())
}

fn merge_file(path: &Path, tweaks: &[Tweak], seed: bool) -> Result<bool, BattleNetError> {
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && seed => json!({}),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(BattleNetError::io(path, e)),
    };

    apply_tweaks(&mut config, tweaks);
    let mut text = serde_json::to_string_pretty(&config)
        .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?;
    text.push('\n');
    write_atomic(path, text.as_bytes())?;
    Ok(true)
}


/// Write via a temp file in the destination directory plus a rename, so a
/// crash mid-write can never leave a truncated config behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), BattleNetError> {
    use std::io::Write;

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| BattleNetError::io(dir, e))?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(|e| BattleNetError::io(dir, e))?;
    tmp.write_all(bytes)
        .map_err(|e| BattleNetError::io(tmp.path(), e))?;
    tmp.as_file()
        .sync_all()
        .map_err(|e| BattleNetError::io(tmp.path(), e))?;
    tmp.persist(path)
        .map_err(|e| BattleNetError::io(path, e.error))?;
    Ok(())
}

/// A registry value applied with `reg add`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegValue {
    pub key: &'static str,
    pub name: &'static str,
    pub kind: &'static str,
    pub data: &'static str,
    pub reason: &'static str,
}

impl RegValue {
    /// Arguments for `reg add`, in the order `reg.exe` expects them.
    pub fn args(&self) -> Vec<String> {
        vec![
            "add".to_owned(),
            self.key.to_owned(),
            "/v".to_owned(),
            self.name.to_owned(),
            "/t".to_owned(),
            self.kind.to_owned(),
            "/d".to_owned(),
            self.data.to_owned(),
            "/f".to_owned(),
        ]
    }
}

/// Registry tweaks that specifically benefit the Battle.net client.
///
/// `reg add` is used rather than a generated `.reg` file: no temp file inside
/// the prefix, no path translation, and no guessing at the encoding `regedit`
/// wants for a given Wine build.
pub fn registry_tweaks() -> Vec<RegValue> {
    vec![
        RegValue {
            key: r"HKEY_CURRENT_USER\Software\Wine\DXVA2",
            name: "backend",
            kind: "REG_SZ",
            data: "va",
            reason: "hardware video decode for Battle.net's own video/UI path (VA-API)",
        },
        RegValue {
            key: r"HKEY_CURRENT_USER\Software\Wine\DllOverrides",
            name: "winemenubuilder.exe",
            kind: "REG_SZ",
            data: "",
            reason: "stop the installer writing Windows shortcuts into the desktop's app menu",
        },
        // Subpixel font smoothing, the four values `winetricks fontsmooth=rgb`
        // writes. Wine ships with smoothing off, and the storefront is a CEF
        // browser rendering text under Wine's font stack — unsmoothed it is
        // the thing that most obviously looks broken next to the same page in
        // a native browser.
        //
        // Prefix-wide rather than Battle.net-only because the registry has no
        // per-application scope for these. WoW draws its own text and is
        // unaffected.
        RegValue {
            key: r"HKEY_CURRENT_USER\Control Panel\Desktop",
            name: "FontSmoothing",
            // A string, not a DWORD — Windows stores this one as text, and a
            // DWORD here is silently ignored.
            kind: "REG_SZ",
            data: "2",
            reason: "turn font smoothing on for the CEF storefront",
        },
        RegValue {
            key: r"HKEY_CURRENT_USER\Control Panel\Desktop",
            name: "FontSmoothingType",
            kind: "REG_DWORD",
            data: "2",
            reason: "subpixel rather than greyscale smoothing",
        },
        RegValue {
            key: r"HKEY_CURRENT_USER\Control Panel\Desktop",
            name: "FontSmoothingOrientation",
            kind: "REG_DWORD",
            data: "1",
            reason: "RGB subpixel order, which is what desktop panels use",
        },
        RegValue {
            key: r"HKEY_CURRENT_USER\Control Panel\Desktop",
            name: "FontSmoothingGamma",
            kind: "REG_DWORD",
            data: "1400",
            reason: "the gamma winetricks pairs with subpixel smoothing",
        },
        // Deliberately no `LogPixels`: Wine's default of 96 is right at 1x,
        // writing 96 is a no-op, and any other value needs a display scale
        // that cannot be read from inside the prefix. A wrong one makes the
        // storefront unusable, so it stays a thing the user asks for.
    ]
}

/// Apply registry tweaks, then shut the session down again.
pub fn apply_registry(runner: &ProtonRunner, values: &[RegValue]) -> Result<(), BattleNetError> {
    for value in values {
        runner.run("reg", value.args())?;
    }
    runner.wineserver_kill()?;
    Ok(())
}

/// How to open Battle.net.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Just the client window — the app-level "other Blizzard titles" path.
    Client,
    /// Hand straight off into a product, so the user never clicks through the
    /// Battle.net UI. Paired with `GameLaunchWindowBehavior: 2`, the client
    /// exits by itself once the game is up.
    Product { code: ProductCode, exec: Option<String> },
}

impl Launch {
    /// Open a product with the default `launch <uid>` verb.
    pub fn product(code: ProductCode) -> Self {
        Self::Product { code, exec: None }
    }

    /// The executable and arguments this launch needs, relative to `install_dir`.
    ///
    /// Plain client opens go through `Battle.net Launcher.exe`, which is what
    /// Blizzard's own shortcut runs and what keeps the client's self-update
    /// path intact. A product hand-off goes straight to `Battle.net.exe`, the
    /// binary that actually parses `--productcode`/`--exec`.
    pub fn command(&self, install_dir: &Path) -> (PathBuf, Vec<String>) {
        match self {
            Self::Client => (install_dir.join("Battle.net Launcher.exe"), Vec::new()),
            Self::Product { code, exec } => match exec.clone().or_else(|| game_uid(code)) {
                Some(uid) => (
                    install_dir.join("Battle.net.exe"),
                    vec![
                        format!("--productcode={code}"),
                        format!("--exec=launch {uid}"),
                    ],
                ),
                // No family id for this product, so there is nothing safe to
                // ask for. Opening the client leaves the user one click away
                // from the right tile; guessing puts them on the wrong game.
                None => Self::Client.command(install_dir),
            },
        }
    }

    /// Whether this launch can actually hand off, or would only open the
    /// client. Lets a caller label the button for what it will really do.
    pub fn hands_off(&self) -> bool {
        match self {
            Self::Client => false,
            Self::Product { code, exec } => exec.is_some() || game_uid(code).is_some(),
        }
    }
}

/// Battle.net's own identifier for a game, as used by `--exec="launch <uid>"`.
///
/// **This is not the TACT product code.** The client works in *game family*
/// ids, one per tile in its library, and the flavor within a tile comes from
/// its version dropdown. Read out of the client's own log, where each selection
/// is recorded (`Selecting game family by id: …` followed by
/// `Active product set to: …`):
///
/// ```text
/// WoW  -> WoWX12_retail
/// WoWC -> WoW_wow_classic_era, WoW_wow_classic
/// WoWF -> WoWPTR_wow_classic_beta
/// W3   -> W3_retail
/// ```
///
/// Passing a TACT code here is not a no-op — it is worse. The client fails to
/// resolve it and **falls back to the first family it knows, which is
/// Warcraft III**, so the user lands on a game they may not even own. That was
/// a real bug: `launch wow_classic_era` put Gabriel on the W3 page every time.
pub fn game_uid(code: &ProductCode) -> Option<String> {
    let uid = match code {
        // PTR and beta builds of retail live in the retail tile's version list.
        ProductCode::Retail | ProductCode::Ptr | ProductCode::Ptr2 | ProductCode::Beta => "WoW",
        ProductCode::Classic
        | ProductCode::ClassicEra
        | ProductCode::ClassicPtr
        | ProductCode::EraPtr => "WoWC",
        ProductCode::ClassicBeta => "WoWF",
        // Families read out of a real client log, same as the WoW ones above.
        // Only codes actually observed are listed: a guess here does not
        // degrade to "nothing happens", it opens somebody else's game.
        ProductCode::Custom(code) => match code.as_str() {
            "hsb" | "hs_beta" => "WTCG",
            "osi" => "OSI",
            _ => return None,
        },
    };
    Some(uid.to_owned())
}

/// Start Battle.net and hand the caller the running child.
///
/// Deliberately does not wait: the CLI blocks on the child itself, and the GUI
/// wants the process to outlive the click that started it.
pub fn launch(
    runner: &ProtonRunner,
    layout: &Layout,
    launch: &Launch,
) -> Result<std::process::Child, BattleNetError> {
    let install_dir = layout.battle_net_dir();
    if !is_installed(&install_dir) {
        return Err(BattleNetError::NotInstalled(install_dir));
    }

    let (exe, args) = launch.command(&install_dir);
    runner
        .command("run", &exe, args)
        .stdin(Stdio::null())
        .spawn()
        .map_err(|e| BattleNetError::Prefix(PrefixError::Spawn { program: exe, source: e }))
}

/// What a full [`provision`] run did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub install: InstallOutcome,
    pub registry_values: usize,
    pub config_path: PathBuf,
    /// Per-account configs found and updated — empty until the first login.
    pub account_configs: Vec<PathBuf>,
}

/// Install Battle.net and configure it: the whole of M2 step 4 in one call.
///
/// Assumes the prefix already exists — [`crate::prefix::provision`] is the
/// step before this one, not part of it.
pub fn provision(
    runner: &ProtonRunner,
    layout: &Layout,
    user: &str,
    installer: &Path,
    managed: &[ProductCode],
) -> Result<SetupReport, BattleNetError> {
    if !layout.prefix().join("system.reg").is_file() {
        return Err(BattleNetError::PrefixNotReady(layout.prefix()));
    }

    let install = Installer::new(layout, installer).install(runner)?;

    let registry = registry_tweaks();
    apply_registry(runner, &registry)?;

    // Last, and only with nothing running: the client rewrites this file when
    // it exits, so an injection before the teardown above would be lost.
    let mut tweaks = default_tweaks();
    tweaks.extend(product_tweaks(managed));
    let config_path = configure(layout, user, &tweaks)?;
    let account_configs = configure_accounts(layout, user, &account_tweaks())?;

    Ok(SetupReport {
        install,
        registry_values: registry.len(),
        config_path,
        account_configs,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum BattleNetError {
    #[error("no Battle.net installer at {0} — fetch it first")]
    InstallerMissing(PathBuf),

    #[error("the prefix at {0} has not been created yet — run prefix setup first")]
    PrefixNotReady(PathBuf),

    #[error("no Battle.net install at {0} — run setup first")]
    NotInstalled(PathBuf),

    #[error("Battle.net did not finish installing into {dir} within {}s", after.as_secs())]
    InstallTimeout { after: Duration, dir: PathBuf },

    #[error(
        "the Battle.net installer exited{} without installing into {dir}{}",
        match code { Some(c) => format!(" with code {c}"), None => " on a signal".to_owned() },
        match stderr.is_empty() { true => String::new(), false => format!("\n{}", last_lines(stderr, STDERR_LINES_SHOWN)) }
    )]
    InstallerExited {
        code: Option<i32>,
        dir: PathBuf,
        stderr: String,
    },

    #[error("{path} is not valid JSON")]
    MalformedConfig {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },

    #[error(transparent)]
    Prefix(#[from] PrefixError),

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl BattleNetError {
    fn io(path: impl Into<PathBuf>, source: std::io::Error) -> Self {
        Self::Io {
            path: path.into(),
            source,
        }
    }
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_window_state_key_is_qts_own_and_is_written_as_a_string() {
        // The client stores every one of these as a string, and a real
        // integer here is a value it reads back as zero.
        let minimized = window_state_tweaks(true);
        assert_eq!(minimized.len(), 1);
        assert_eq!(minimized[0].path, WINDOW_STATE_KEY.split('.').collect::<Vec<_>>());
        assert_eq!(minimized[0].value, json!("1"));
        assert_eq!(window_state_tweaks(false)[0].value, json!("0"));
    }

    #[test]
    fn minimizing_is_only_safe_where_the_client_signs_itself_in() {
        let dir = scratch("auto-login");
        let layout = Layout::with_root(&dir);
        let path = layout.battle_net_config("steamuser");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        let write = |value: Value| std::fs::write(&path, value.to_string()).unwrap();

        // Both halves are needed: a remembered account with auto-login off
        // still stops at the sign-in form, and auto-login with nothing
        // remembered has nobody to sign in as.
        write(json!({"Client": {"AutoLogin": "true", "SavedAccountNames": "a@b.c"}}));
        assert!(signs_in_by_itself(&layout, "steamuser"));

        write(json!({"Client": {"AutoLogin": "false", "SavedAccountNames": "a@b.c"}}));
        assert!(!signs_in_by_itself(&layout, "steamuser"));

        write(json!({"Client": {"AutoLogin": "true", "SavedAccountNames": ""}}));
        assert!(!signs_in_by_itself(&layout, "steamuser"));

        // A prefix with no client installed yet answers no, not a panic.
        let _ = std::fs::remove_file(&path);
        assert!(!signs_in_by_itself(&layout, "steamuser"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn readiness_is_read_from_the_log_this_start_wrote() {
        let dir = scratch("signed-in");
        let layout = Layout::with_root(&dir);
        let logs = layout.battle_net_logs("steamuser");
        std::fs::create_dir_all(&logs).unwrap();

        // A previous session's log, saying it signed in, must not answer for
        // this start — that is how a hand-off fires at a client that is
        // still booting.
        let old = logs.join("battle.net-20260101T000000.000000.log");
        std::fs::write(&old, format!("I [BNLogin] {SIGNED_IN}. |bnet=1:0:x\n")).unwrap();

        let since = SystemTime::now() + Duration::from_secs(60);
        assert_eq!(
            wait_until_signed_in(&layout, "steamuser", since, Duration::from_millis(1)),
            Readiness::NeedsAttention,
            "a log older than this start says nothing about it"
        );

        // And one written now does answer.
        assert_eq!(
            wait_until_signed_in(
                &layout,
                "steamuser",
                SystemTime::UNIX_EPOCH,
                Duration::from_millis(1),
            ),
            Readiness::SignedIn
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
    #[test]
    fn game_uids_are_client_family_ids_not_tact_codes() {
        use super::*;

        // Verified against the client's log: these ids select these tiles.
        let uid = |code| game_uid(&code).unwrap();
        assert_eq!(uid(ProductCode::Retail), "WoW");
        assert_eq!(uid(ProductCode::ClassicEra), "WoWC");
        assert_eq!(uid(ProductCode::Classic), "WoWC");
        assert_eq!(uid(ProductCode::ClassicBeta), "WoWF");

        // The bug this prevents: a TACT code is unresolvable and the client
        // falls back to Warcraft III.
        assert_ne!(uid(ProductCode::ClassicEra), "wow_classic_era");
    }

    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-bnet-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn the_installer_is_told_the_windows_install_path() {
        let layout = Layout::with_root("/games/Blizzard");
        let args = Installer::new(&layout, "/cache/Battle.net-Setup.exe")
            .with_locale("deDE")
            .args();
        assert_eq!(
            args,
            [
                "--lang=deDE",
                r"--installpath=C:\Program Files (x86)\Battle.net",
            ]
        );
    }

    #[test]
    fn an_install_needs_both_the_launcher_and_a_versioned_client() {
        let dir = scratch("detect");
        assert!(!is_installed(&dir));

        std::fs::write(dir.join("Battle.net Launcher.exe"), b"MZ").unwrap();
        // The launcher alone is what a half-finished bootstrap leaves behind.
        assert!(!is_installed(&dir));

        std::fs::create_dir_all(dir.join("Battle.net.15000")).unwrap();
        assert!(is_installed(&dir));
    }

    /// The Debian regression: no 32-bit runtime, so the installer dies in
    /// under a second. Before this, the filesystem poll ignored the dead
    /// child and sat there for the full ten minutes, then blamed a timeout.
    #[test]
    fn an_installer_that_dies_is_reported_as_exited_not_timed_out() {
        let dir = scratch("exited");
        let layout = Layout::with_root(&dir);
        let installer = Installer::new(&layout, dir.join("setup.exe"))
            // Deliberately generous: the point is that it does NOT wait this
            // long once the process is gone.
            .with_timeout(Duration::from_secs(60))
            .with_poll_interval(Duration::from_millis(10))
            .with_exit_grace(Duration::from_millis(50), Duration::from_millis(20));

        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 3")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");

        let started = Instant::now();
        let err = installer
            .wait_for_install(started, Some(&mut child))
            .unwrap_err();

        assert_eq!(err, Wait::Exited(Some(3)));
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "a dead installer must not be waited out: {:?}",
            started.elapsed()
        );
    }

    /// Exiting is not failing. Blizzard's setup bootstraps a second process,
    /// so a clean exit with the files still landing has to stay a success.
    #[test]
    fn files_landing_after_a_clean_exit_still_count() {
        let dir = scratch("late-files");
        let layout = Layout::with_root(&dir);
        let install_dir = layout.battle_net_dir();
        let installer = Installer::new(&layout, dir.join("setup.exe"))
            .with_timeout(Duration::from_secs(60))
            .with_poll_interval(Duration::from_millis(10))
            .with_exit_grace(Duration::from_secs(10), Duration::from_millis(20));

        let mut child = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg("exit 0")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn");

        let writer = {
            let install_dir = install_dir.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(80));
                std::fs::create_dir_all(install_dir.join("Battle.net.15000")).unwrap();
                std::fs::write(install_dir.join("Battle.net Launcher.exe"), b"MZ").unwrap();
            })
        };

        let outcome = installer
            .wait_for_install(Instant::now(), Some(&mut child))
            .expect("files arriving within the grace period is a success");
        assert!(!outcome.skipped);
        writer.join().unwrap();
    }

    #[test]
    fn the_message_shows_the_end_of_stderr_not_all_of_it() {
        let noise: String = (0..200).map(|i| format!("warn {i}\n")).collect();
        let err = BattleNetError::InstallerExited {
            code: Some(127),
            dir: PathBuf::from("/tmp/bn"),
            stderr: format!("{noise}libfreetype.so.6: cannot open shared object file"),
        };

        let rendered = err.to_string();
        assert!(rendered.contains("libfreetype.so.6"), "the real error must survive");
        assert!(rendered.contains("code 127"));
        assert!(
            rendered.lines().count() <= STDERR_LINES_SHOWN + 1,
            "a banner cannot show 200 lines of Wine chatter:\n{rendered}"
        );
        assert!(!rendered.contains("warn 0\n"), "the start is the least useful part");
    }

    #[test]
    fn stderr_is_kept_from_the_end_and_bounded() {
        let noise = "x".repeat(STDERR_TAIL * 3);
        let text = format!("{noise}libfreetype.so.6: cannot open shared object file");
        let kept = tail_of(text.as_bytes());

        assert!(kept.len() <= STDERR_TAIL, "kept {} bytes", kept.len());
        assert!(
            kept.ends_with("cannot open shared object file"),
            "the end is where the error is"
        );
    }

    #[test]
    fn a_stalled_installer_times_out_instead_of_hanging() {
        let dir = scratch("timeout");
        let layout = Layout::with_root(&dir);
        let installer = Installer::new(&layout, dir.join("setup.exe"))
            .with_timeout(Duration::from_millis(60))
            .with_poll_interval(Duration::from_millis(10));

        let err = installer.wait_for_install(Instant::now(), None).unwrap_err();
        assert_eq!(err, Wait::TimedOut);
    }

    #[test]
    fn tweaks_create_missing_objects_without_touching_anything_else() {
        let mut config = json!({
            "Client": { "Sound": { "Enabled": "true" } },
            "Games": { "wow": { "LastPlayed": "1" } }
        });
        apply_tweaks(&mut config, &default_tweaks());

        assert_eq!(config["Client"]["GameLaunchWindowBehavior"], json!("2"));
        assert_eq!(config["Client"]["HardwareAcceleration"], json!("false"));
        assert_eq!(
            config["Client"]["Install"]["DefaultInstallPath"],
            json!(r"C:\Program Files (x86)")
        );
        // Login state and per-game settings share this file; losing them would
        // log the user out.
        assert_eq!(config["Client"]["Sound"]["Enabled"], json!("true"));
        assert_eq!(config["Games"]["wow"]["LastPlayed"], json!("1"));
    }

    #[test]
    fn an_existing_value_is_overwritten_not_appended() {
        let mut config = json!({ "Client": { "GameLaunchWindowBehavior": "0" } });
        apply_tweaks(&mut config, &default_tweaks());
        assert_eq!(config["Client"]["GameLaunchWindowBehavior"], json!("2"));
    }

    #[test]
    fn a_scalar_where_an_object_belongs_is_replaced() {
        // Battle.net has shipped schema changes before; a stale scalar must not
        // abort setup.
        let mut config = json!({ "Client": { "Install": "C:\\somewhere" } });
        apply_tweaks(&mut config, &default_tweaks());
        assert_eq!(
            config["Client"]["Install"]["DefaultInstallPath"],
            json!(r"C:\Program Files (x86)")
        );
    }

    #[test]
    fn hide_on_close_is_written_to_every_account_config_but_not_the_global_one() {
        let dir = scratch("accounts");
        let layout = Layout::with_root(&dir);
        let bnet = layout.battle_net_config("steamuser").parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&bnet).unwrap();
        std::fs::write(bnet.join("1c858370.config"), r#"{"User":{"Client":{"HideOnClose":"true"}}}"#)
            .unwrap();
        std::fs::write(bnet.join("beef0001.config"), "{}").unwrap();

        configure(&layout, "steamuser", &default_tweaks()).unwrap();
        let changed = configure_accounts(&layout, "steamuser", &account_tweaks()).unwrap();
        assert_eq!(changed.len(), 2);

        for name in ["1c858370.config", "beef0001.config"] {
            let v: Value =
                serde_json::from_str(&std::fs::read_to_string(bnet.join(name)).unwrap()).unwrap();
            assert_eq!(v["User"]["Client"]["HideOnClose"], json!("false"));
            assert_eq!(v["User"]["Client"]["DisplayExitPrompt"], json!("false"));
        }

        // The global config is the client's own; account keys must not leak in.
        let global: Value = serde_json::from_str(
            &std::fs::read_to_string(layout.battle_net_config("steamuser")).unwrap(),
        )
        .unwrap();
        assert!(global.get("User").is_none());
        assert!(global["Client"].get("MinimizeToTray").is_none());
    }

    #[test]
    fn a_prefix_with_no_account_yet_is_a_no_op() {
        let dir = scratch("noaccount");
        let layout = Layout::with_root(&dir);
        configure(&layout, "steamuser", &default_tweaks()).unwrap();

        // Only Battle.net.config exists before the first login.
        assert!(configure_accounts(&layout, "steamuser", &account_tweaks()).unwrap().is_empty());
    }

    #[test]
    fn configure_seeds_a_config_that_does_not_exist_yet() {
        let dir = scratch("seed");
        let layout = Layout::with_root(&dir);

        let path = configure(&layout, "steamuser", &default_tweaks()).unwrap();
        assert_eq!(path, layout.battle_net_config("steamuser"));

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["Client"]["GameLaunchWindowBehavior"], json!("2"));
    }

    #[test]
    fn configure_is_idempotent_and_leaves_no_temp_files() {
        let dir = scratch("idempotent");
        let layout = Layout::with_root(&dir);

        let path = configure(&layout, "steamuser", &default_tweaks()).unwrap();
        let first = std::fs::read_to_string(&path).unwrap();
        configure(&layout, "steamuser", &default_tweaks()).unwrap();
        assert_eq!(first, std::fs::read_to_string(&path).unwrap());

        let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(leftovers, 1);
    }

    #[test]
    fn a_corrupt_config_is_reported_rather_than_silently_replaced() {
        let dir = scratch("corrupt");
        let layout = Layout::with_root(&dir);
        let path = layout.battle_net_config("steamuser");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{ not json").unwrap();

        assert!(matches!(
            configure(&layout, "steamuser", &default_tweaks()),
            Err(BattleNetError::MalformedConfig { .. })
        ));
    }

    #[test]
    fn dxva2_is_set_through_reg_add() {
        let dxva2 = &registry_tweaks()[0];
        assert_eq!(
            dxva2.args(),
            [
                "add",
                r"HKEY_CURRENT_USER\Software\Wine\DXVA2",
                "/v",
                "backend",
                "/t",
                "REG_SZ",
                "/d",
                "va",
                "/f",
            ]
        );
    }

    #[test]
    fn a_plain_open_runs_the_launcher_with_no_arguments() {
        let dir = Path::new("/prefix/Battle.net");
        let (exe, args) = Launch::Client.command(dir);
        assert_eq!(exe, dir.join("Battle.net Launcher.exe"));
        assert!(args.is_empty());
    }

    /// The failure this guards is not "nothing happens": the client cannot
    /// resolve an unknown uid and falls back to the first family it knows,
    /// which is Warcraft III. Opening the client is the honest alternative.
    #[test]
    fn registry_tweaks_are_well_formed_reg_add_arguments() {
        for value in registry_tweaks() {
            let args = value.args();
            assert_eq!(args[0], "add", "{}", value.name);
            assert!(args.contains(&"/f".to_owned()), "{} is not forced", value.name);
            assert!(!value.reason.is_empty(), "{} has no reason", value.name);

            // `reg add /t REG_DWORD` takes its data as digits; anything else
            // is rejected by reg.exe at runtime, where nobody is watching.
            if value.kind == "REG_DWORD" {
                assert!(
                    value.data.chars().all(|c| c.is_ascii_digit()),
                    "{} is a DWORD but its data is {:?}",
                    value.name,
                    value.data
                );
            }
        }
    }

    #[test]
    fn font_smoothing_is_on_and_subpixel() {
        let by_name = |name: &str| {
            registry_tweaks()
                .into_iter()
                .find(|v| v.name == name)
                .unwrap_or_else(|| panic!("{name} is not set"))
        };

        // The one that is a string rather than a DWORD. Getting this wrong
        // leaves smoothing off with no error anywhere.
        let on = by_name("FontSmoothing");
        assert_eq!((on.kind, on.data), ("REG_SZ", "2"));

        assert_eq!(by_name("FontSmoothingType").data, "2");
        assert_eq!(by_name("FontSmoothingOrientation").data, "1");
        assert_eq!(by_name("FontSmoothingGamma").data, "1400");

        // DPI is deliberately not written — see the comment beside the list.
        assert!(registry_tweaks().iter().all(|v| v.name != "LogPixels"));
    }

    #[test]
    fn only_managed_products_have_their_updates_switched_off() {
        let tweaks = product_tweaks(&[ProductCode::Retail, ProductCode::ClassicEra]);
        let keys: Vec<String> = tweaks.iter().map(Tweak::key).collect();

        assert!(keys.contains(&"Games.wow.AutoUpdate".to_owned()));
        assert!(keys.contains(&"Games.wow.DownloadAllUpdates".to_owned()));
        assert!(keys.contains(&"Games.wow_classic_era.AutoUpdate".to_owned()));
        assert!(tweaks.iter().all(|t| t.value == json!("false")));

        // Nothing is generated for a product nobody asked about — the caller
        // decides what is managed, and a hand-off title never reaches here.
        assert!(product_tweaks(&[]).is_empty());
    }

    #[test]
    fn the_download_limit_round_trips_and_clears() {
        let dir = scratch("download-limit");
        let layout = Layout::with_root(&dir);
        let path = layout.battle_net_config("steamuser");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();

        // A file the client wrote, with settings that must survive.
        std::fs::write(
            &path,
            r#"{"Client":{"Install":{"LastUserDownloadLimitNextPatchInBps":"1000000"},
               "AutoLogin":"true"}}"#,
        )
        .unwrap();
        assert_eq!(download_limit(&layout, "steamuser"), Some(1_000_000));

        set_download_limit(&layout, "steamuser", Some(5_000_000)).unwrap();
        assert_eq!(download_limit(&layout, "steamuser"), Some(5_000_000));

        // Cleared means the key is gone, not zero — and the neighbours stay.
        set_download_limit(&layout, "steamuser", None).unwrap();
        assert_eq!(download_limit(&layout, "steamuser"), None);
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(!text.contains("LastUserDownloadLimitNextPatchInBps"), "{text}");
        assert!(text.contains("AutoLogin"), "unrelated settings were lost: {text}");

        // Clearing again is success, not an error.
        set_download_limit(&layout, "steamuser", None).unwrap();
    }

    #[test]
    fn a_limit_on_a_machine_with_no_config_is_simply_absent() {
        let layout = Layout::with_root(scratch("no-config"));
        assert_eq!(download_limit(&layout, "steamuser"), None);
        // And clearing one that was never there does not fail.
        set_download_limit(&layout, "steamuser", None).unwrap();
    }

    #[test]
    fn a_product_with_no_known_family_opens_the_client_instead_of_guessing() {
        let dir = Path::new("/prefix/Battle.net");
        let unknown = Launch::product(ProductCode::Custom("turtle_wow".into()));

        assert!(!unknown.hands_off());
        assert_eq!(unknown.command(dir), Launch::Client.command(dir));
    }

    #[test]
    fn the_families_read_from_a_real_client_log_hand_off() {
        let dir = Path::new("/prefix/Battle.net");
        for (code, uid) in [
            ("hs_beta", "WTCG"),
            ("hsb", "WTCG"),
            ("osi", "OSI"),
        ] {
            let launch = Launch::product(ProductCode::Custom(code.into()));
            assert!(launch.hands_off(), "{code}");
            let (exe, args) = launch.command(dir);
            assert_eq!(exe, dir.join("Battle.net.exe"), "{code}");
            assert!(args.contains(&format!("--exec=launch {uid}")), "{code}: {args:?}");
            assert!(args.contains(&format!("--productcode={code}")), "{code}: {args:?}");
        }
    }

    #[test]
    fn a_product_hand_off_goes_to_battle_net_exe_with_productcode_and_exec() {
        let dir = Path::new("/prefix/Battle.net");
        let (exe, args) = Launch::product(ProductCode::Retail).command(dir);
        assert_eq!(exe, dir.join("Battle.net.exe"));
        // The exec uid is the client's family id, not the TACT code.
        assert_eq!(args, ["--productcode=wow", "--exec=launch WoW"]);

        let (_, classic) = Launch::product(ProductCode::Classic).command(dir);
        // Every Classic-line product shares one tile.
        assert_eq!(classic, ["--productcode=wow_classic", "--exec=launch WoWC"]);
    }

    #[test]
    fn an_explicit_exec_verb_overrides_the_derived_uid() {
        let (_, args) = Launch::Product { code: ProductCode::Retail, exec: Some("WoW".into()) }
            .command(Path::new("/prefix/Battle.net"));
        assert_eq!(args, ["--productcode=wow", "--exec=launch WoW"]);
    }

    #[test]
    fn launching_without_an_install_is_an_error_not_a_spawn_attempt() {
        let dir = scratch("nolaunch");
        let layout = Layout::with_root(&dir);
        let release = crate::proton::ProtonRelease {
            version: "GE-Proton11-7".into(),
            flavor: crate::proton::ProtonFlavor::GloriousEggroll,
            source: crate::proton::ProtonSource::Managed,
            path: dir.join("GE-Proton11-7"),
            built_at: None,
        };
        let runner = ProtonRunner::new(&release, &layout, dir.join("Steam"));

        assert!(matches!(
            launch(&runner, &layout, &Launch::Client),
            Err(BattleNetError::NotInstalled(_))
        ));
    }

    #[test]
    fn provisioning_refuses_to_run_before_the_prefix_exists() {
        let dir = scratch("noprefix");
        let layout = Layout::with_root(&dir);
        let release = crate::proton::ProtonRelease {
            version: "GE-Proton11-7".into(),
            flavor: crate::proton::ProtonFlavor::GloriousEggroll,
            source: crate::proton::ProtonSource::Managed,
            path: dir.join("GE-Proton11-7"),
            built_at: None,
        };
        let runner = ProtonRunner::new(&release, &layout, dir.join("Steam"));

        assert!(matches!(
            provision(&runner, &layout, "steamuser", &dir.join("setup.exe"), &[]),
            Err(BattleNetError::PrefixNotReady(_))
        ));
    }
}

