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

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};

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
}

impl Installer {
    /// Install into the Battle.net directory `layout` describes.
    pub fn new(layout: &Layout, installer: impl Into<PathBuf>) -> Self {
        Self {
            installer: installer.into(),
            install_dir: layout.battle_net_dir(),
            locale: DEFAULT_LOCALE.to_owned(),
            timeout: DEFAULT_TIMEOUT,
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
            // The installer is chatty and nothing reads its output; piping it
            // without a reader would eventually block the child on a full pipe.
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                BattleNetError::Prefix(PrefixError::Spawn {
                    program: self.installer.clone(),
                    source: e,
                })
            })?;

        let outcome = self.wait_for_install(started);

        // Either way the session has to go: on success the installer left a
        // client running, and on failure a half-finished one.
        let _ = child.kill();
        let _ = child.wait();
        runner.wineserver_kill()?;

        outcome
    }

    /// Poll the filesystem until the install settles, times out, or the
    /// installer is confirmed to have produced nothing.
    fn wait_for_install(&self, started: Instant) -> Result<InstallOutcome, BattleNetError> {
        let mut settled = 0;
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

            if started.elapsed() >= self.timeout {
                return Err(BattleNetError::InstallTimeout {
                    after: self.timeout,
                    dir: self.install_dir.clone(),
                });
            }
            std::thread::sleep(self.poll_interval);
        }
    }
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
        Tweak::new(
            "Client.MinimizeToTray",
            json!("false"),
            "close means quit — no tray icon and no lingering background client",
        ),
        Tweak::new(
            "Client.CloseToTray",
            json!("false"),
            "close means quit — no tray icon and no lingering background client",
        ),
    ]
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

    let mut config = match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| BattleNetError::MalformedConfig {
            path: path.clone(),
            source: e,
        })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => json!({}),
        Err(e) => return Err(BattleNetError::io(&path, e)),
    };

    apply_tweaks(&mut config, tweaks);
    let mut text =
        serde_json::to_string_pretty(&config).map_err(|e| BattleNetError::MalformedConfig {
            path: path.clone(),
            source: e,
        })?;
    text.push('\n');
    write_atomic(&path, text.as_bytes())?;
    Ok(path)
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

/// What a full [`provision`] run did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub install: InstallOutcome,
    pub registry_values: usize,
    pub config_path: PathBuf,
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
) -> Result<SetupReport, BattleNetError> {
    if !layout.prefix().join("system.reg").is_file() {
        return Err(BattleNetError::PrefixNotReady(layout.prefix()));
    }

    let install = Installer::new(layout, installer).install(runner)?;

    let registry = registry_tweaks();
    apply_registry(runner, &registry)?;

    // Last, and only with nothing running: the client rewrites this file when
    // it exits, so an injection before the teardown above would be lost.
    let config_path = configure(layout, user, &default_tweaks())?;

    Ok(SetupReport {
        install,
        registry_values: registry.len(),
        config_path,
    })
}

#[derive(Debug, thiserror::Error)]
pub enum BattleNetError {
    #[error("no Battle.net installer at {0} — fetch it first")]
    InstallerMissing(PathBuf),

    #[error("the prefix at {0} has not been created yet — run prefix setup first")]
    PrefixNotReady(PathBuf),

    #[error("Battle.net did not finish installing into {dir} within {}s", after.as_secs())]
    InstallTimeout { after: Duration, dir: PathBuf },

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

    #[test]
    fn a_stalled_installer_times_out_instead_of_hanging() {
        let dir = scratch("timeout");
        let layout = Layout::with_root(&dir);
        let installer = Installer::new(&layout, dir.join("setup.exe"))
            .with_timeout(Duration::from_millis(60))
            .with_poll_interval(Duration::from_millis(10));

        let err = installer.wait_for_install(Instant::now()).unwrap_err();
        assert!(matches!(err, BattleNetError::InstallTimeout { .. }));
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
    fn configure_seeds_a_config_that_does_not_exist_yet() {
        let dir = scratch("seed");
        let layout = Layout::with_root(&dir);

        let path = configure(&layout, "steamuser", &default_tweaks()).unwrap();
        assert_eq!(path, layout.battle_net_config("steamuser"));

        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["Client"]["MinimizeToTray"], json!("false"));
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
            provision(&runner, &layout, "steamuser", &dir.join("setup.exe")),
            Err(BattleNetError::PrefixNotReady(_))
        ));
    }
}
