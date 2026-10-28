//! Automated Battle.net setup inside the shared prefix.
//!
//! Four phases, in this order:
//!
//! 1. fetch `Battle.net-Setup.exe` from Blizzard's downloader,
//! 2. run it under Proton with an explicit install path, waiting for the
//!    install to appear rather than for the process to exit,
//! 3. apply the Wine registry tweaks Battle.net benefits from,
//! 4. inject Lightforge's defaults into `Battle.net.config`.
//!
//! `Battle.net-Setup.exe` is a bootstrapper: it hands off to `Battle.net.exe`
//! and `Agent`, then exits early or never (the client keeps the Wine session
//! alive). So phase 2 watches the filesystem and ends with `wineserver -k`.
//!
//! Phases 3 and 4 run with no Battle.net process alive: the client rewrites
//! `Battle.net.config` on exit and would otherwise clobber the injection.

mod installer;
mod launch;
mod readiness;
mod registry;
mod tweaks;
#[cfg(test)]
mod tests;

pub use installer::{fetch_installer, InstallOutcome, Installer, INSTALLER_FILE, INSTALLER_URL};
pub use launch::{game_uid, game_uri, launch, launch_wrapped, resolve_game_uid, Launch};
pub use readiness::{signs_in_by_itself, wait_until_ready, Readiness};
pub use registry::{apply_registry, registry_tweaks, RegValue};
pub use tweaks::{
    account_tweaks, apply_tweaks, configure, configure_accounts, default_tweaks, download_limit,
    known_product_codes, product_tweaks, set_download_limit, window_state_tweaks, Tweak,
    AUTOSTART_MINIMIZED_KEY, DOWNLOAD_LIMIT_KEY, WINDOW_STATE_KEY,
};

use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};
use crate::product::ProductCode;
use crate::text::tail_lines;
use installer::STDERR_LINES_SHOWN;
// The tests reach the installer's and the log reader's private helpers.
#[cfg(test)]
use {installer::*, readiness::*};

/// What a full [`provision`] run did, in order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupReport {
    pub install: InstallOutcome,
    pub registry_values: usize,
    pub config_path: PathBuf,
    /// Per-account configs found and updated, empty until the first login.
    pub account_configs: Vec<PathBuf>,
}

/// Install Battle.net and configure it, in an existing prefix (see
/// [`crate::prefix::provision`]). Only the installer runs in `container`
/// ([`crate::runtime::tls_container`]); the rest runs on the host.
pub fn provision(
    runner: &ProtonRunner,
    container: Option<&Path>,
    layout: &Layout,
    user: &str,
    locale: &str,
    installer: &Path,
    managed: &[ProductCode],
) -> Result<SetupReport, BattleNetError> {
    if !layout.prefix().join("system.reg").is_file() {
        return Err(BattleNetError::PrefixNotReady(layout.prefix()));
    }

    let install_runner = match container {
        Some(entry) => runner.clone().in_container(entry),
        None => runner.clone(),
    };
    let install = Installer::new(layout, installer).with_locale(locale).install(&install_runner)?;

    let registry = registry_tweaks();
    apply_registry(runner, &registry)?;

    // Last, with nothing running: the client rewrites this file on exit.
    let mut tweaks = default_tweaks(layout);
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
        match stderr.is_empty() { true => String::new(), false => format!("\n{}", tail_lines(stderr, STDERR_LINES_SHOWN)) }
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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> BattleNetError + '_ {
    move |source| BattleNetError::Io { path: path.to_path_buf(), source }
}
