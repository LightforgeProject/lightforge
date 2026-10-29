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

/// Install path handed to the installer, in Windows spelling: it goes into
/// `.product.db`.
const WINDOWS_BATTLE_NET_DIR: &str = r"C:\Program Files (x86)\Battle.net";

/// Default UI language passed to the installer.
const DEFAULT_LOCALE: &str = "enUS";

/// How long the installer gets before it is considered stuck.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(600);

/// Gap between filesystem checks while waiting for the install.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// How long to keep watching after a clean installer exit: the setup hands
/// over to a second process and can return before the files land.
const EXIT_GRACE: Duration = Duration::from_secs(60);

/// The same grace after a failed exit, short: the installer has given up.
const FAILED_EXIT_GRACE: Duration = Duration::from_secs(5);

/// Most stderr kept from the installer, in bytes: room for a loader error and
/// a stack of Wine warnings.
pub(super) const STDERR_TAIL: usize = 8 * 1024;

/// How many of those lines the error message shows. The full tail goes into
/// a debug bundle; the message is a GUI banner.
pub(super) const STDERR_LINES_SHOWN: usize = 8;

/// Consecutive successful checks before the install counts as done: the
/// launcher is written before the versioned client directory is complete.
const SETTLE_POLLS: u32 = 3;

/// Download the installer into `cache_dir`, returning its path. Blizzard
/// publishes no checksum for it; HTTPS is the only guarantee.
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

    /// The grace after a clean and a failed exit, for tests.
    #[cfg(test)]
    pub fn with_exit_grace(mut self, clean: Duration, failed: Duration) -> Self {
        self.exit_grace = clean;
        self.failed_exit_grace = failed;
        self
    }

    /// Shorten the polling gap, for tests.
    #[cfg(test)]
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// `--installpath` keeps Battle.net out of `%LOCALAPPDATA%`, where a
    /// prefix wipe would take it along.
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

    /// Run the installer unless Battle.net is already there. Returns with the
    /// prefix's `wineserver` shut down, so the caller can edit registry and
    /// config files.
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
            // stderr is where Proton's loader names a missing 32-bit library.
            // Drained on a thread: an unread pipe blocks the child once full.
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

        // Success leaves a client running, failure a half-finished one.
        let _ = child.kill();
        let _ = child.wait();
        runner.wineserver_kill()?;

        // Joined after the child is gone, so the pipe is closed.
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

    /// Poll until the install settles, the installer exits, or the timeout.
    ///
    /// The files are the main signal (the installer stays up after success,
    /// running the client), but the exit catches an installer that died in
    /// its first second. `child` is optional for the timeout test.
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

            // An exit is not a failure yet, the files may still be landing.
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

/// Why [`Installer::wait_for_install`] stopped waiting. Becomes a
/// [`BattleNetError`] once the installer's stderr is drained.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Wait {
    /// Nothing happened for the whole timeout.
    TimedOut,
    /// The installer process ended and the files never appeared.
    Exited(Option<i32>),
}

/// Read a pipe to the end, so the writer never blocks, keeping only the last
/// [`STDERR_TAIL`] bytes.
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
