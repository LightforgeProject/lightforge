use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::time::{Duration, Instant};

use super::BattleNetError;
use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};

/// Blizzard's version-agnostic installer endpoint.
pub const INSTALLER_URL: &str =
    "https://downloader.battle.net/download/getInstaller?os=win&installer=Battle.net-Setup.exe";

/// File name the installer is cached under.
pub const INSTALLER_FILE: &str = "Battle.net-Setup.exe";

/// Windows-side install path handed to the installer. It has to be the Windows
/// spelling, not the Linux one, the installer writes it into `.product.db`.
const WINDOWS_BATTLE_NET_DIR: &str = r"C:\Program Files (x86)\Battle.net";

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
pub(super) const STDERR_TAIL: usize = 8 * 1024;

/// How many of those lines the error message itself shows.
///
/// Capturing generously and displaying briefly are different jobs: the tail is
/// kept in full for a debug bundle, while the message goes into a banner in
/// the GUI, where eight kilobytes of Wine chatter would bury the one line that
/// matters. The last lines are the right ones, a failing process says why
/// just before it stops.
pub(super) const STDERR_LINES_SHOWN: usize = 8;

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

    /// How long to keep watching after the installer exits, cleanly, then
    /// with a failure. Exists for the same reason [`Installer::with_timeout`]
    /// does: so a test does not have to wait a real minute.
    #[cfg(test)]
    pub fn with_exit_grace(mut self, clean: Duration, failed: Duration) -> Self {
        self.exit_grace = clean;
        self.failed_exit_grace = failed;
        self
    }

    /// Shorten the polling gap, used by tests, and by nothing else.
    #[cfg(test)]
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
        tracing::info!(
            "running the Battle.net installer {} into {} (timeout {:?})",
            self.installer.display(),
            self.install_dir.display(),
            self.timeout,
        );
        let mut child = runner
            .command("run", &self.installer, self.args())
            .stdin(Stdio::null())
            // stdout is noise nobody reads. stderr is the one thing worth
            // having when this goes wrong. It is where Proton's dynamic
            // loader names a missing 32-bit library, so it is piped and
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

        match &waited {
            Ok(_) => tracing::info!("Battle.net installer finished after {:.0}s", started.elapsed().as_secs_f64()),
            Err(Wait::TimedOut) => tracing::error!("Battle.net installer timed out after {:?}", self.timeout),
            Err(Wait::Exited(code)) => tracing::error!(
                "Battle.net installer exited with {code:?} after {:.0}s; stderr tail: {}",
                started.elapsed().as_secs_f64(),
                stderr.trim(),
            ),
        }
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
    /// Watching the files is still the primary signal, the installer stays
    /// alive after a successful install, having launched the client, but
    /// watching *only* the files means a process that died in the first
    /// second is indistinguishable from one still downloading, and the user
    /// waits the full ten minutes to be told it "timed out". It did not time
    /// out; it failed immediately, and said why on a stderr nobody kept.
    ///
    /// `child` is optional so the timeout path can be tested without one.
    pub(super) fn wait_for_install(
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

            // An exit is not itself a failure, the files may still be
            // landing, but it bounds how long waiting can be worthwhile.
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
pub(super) enum Wait {
    /// Nothing happened for the whole timeout.
    TimedOut,
    /// The installer process ended and the files never appeared.
    Exited(Option<i32>),
}

/// The last `n` non-empty lines of `text`, in order.
pub(super) fn last_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// Read a pipe to the end, keeping only the last [`STDERR_TAIL`] bytes.
///
/// Reads everything so the writer never blocks on a full pipe, but retains a
/// bounded tail, the end is where the error is.
pub(super) fn tail_of(mut pipe: impl Read) -> String {
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
pub(super) fn is_installed(dir: &Path) -> bool {
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
