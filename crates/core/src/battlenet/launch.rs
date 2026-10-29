use std::path::{Path, PathBuf};
use std::process::Stdio;

use super::installer::is_installed;
use super::BattleNetError;
use crate::paths::Layout;
use crate::prefix::{PrefixError, ProtonRunner};
use crate::product::ProductCode;

/// How to open Battle.net.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Launch {
    /// Just the client window, the app-level "other Blizzard titles" path.
    Client,
    /// The same start with `--autostarted`, the flag Blizzard's own Run entry
    /// passes. Only then does the client honour [`super::AUTOSTART_MINIMIZED_KEY`]
    /// (`needToMinimize = shouldMainWindowShowMinimizedAtAutoStartup &&
    /// varAutoStarted`).
    Autostart,
    /// Hand straight off into a product. With `GameLaunchWindowBehavior: 2`
    /// the client exits once the game is up.
    Product { code: ProductCode, exec: Option<String> },
    /// Open the client's install flow for a product, by its install uid. An
    /// action, not view state, so the client's boot to its homepage keeps it.
    /// `install` and `game` are option names from the client binary ("Start a
    /// package installation", "Game registration/installation uid").
    InstallFlow { uid: String },
    /// A `battlenet://` link, passed the way Wine registered the protocol
    /// (`Battle.net.exe --uri="%1"`). A command the client acts on, not view
    /// state its startup overwrites.
    Uri(String),
}

impl Launch {
    /// Open a product with the default `launch <uid>` verb.
    pub fn product(code: ProductCode) -> Self {
        Self::Product { code, exec: None }
    }

    /// The executable and arguments this launch needs, relative to
    /// `install_dir`. Plain opens use `Battle.net Launcher.exe`, as Blizzard's
    /// shortcut does, which keeps self-update working. A hand-off goes to
    /// `Battle.net.exe`, which parses `--productcode`/`--exec`.
    pub fn command(&self, install_dir: &Path) -> (PathBuf, Vec<String>) {
        match self {
            Self::Client => (install_dir.join("Battle.net Launcher.exe"), Vec::new()),
            // Through the launcher, like a plain start; it passes its command
            // line on to the client.
            Self::Autostart => (
                install_dir.join("Battle.net Launcher.exe"),
                vec!["--autostarted".to_owned()],
            ),
            Self::InstallFlow { uid } => (
                install_dir.join("Battle.net.exe"),
                vec![format!("--game={uid}"), "--install".to_owned()],
            ),
            Self::Uri(uri) => (
                install_dir.join("Battle.net.exe"),
                vec![format!("--uri={uri}")],
            ),
            Self::Product { code, exec } => match exec.clone().or_else(|| game_uid(code)) {
                Some(uid) => (
                    install_dir.join("Battle.net.exe"),
                    vec![
                        format!("--productcode={code}"),
                        format!("--exec=launch {uid}"),
                    ],
                ),
                // No family id: a guess opens the wrong game, so just open the client.
                None => Self::Client.command(install_dir),
            },
        }
    }

    /// Whether this launch hands off, or only opens the client.
    #[cfg(test)]
    pub fn hands_off(&self) -> bool {
        match self {
            Self::Client | Self::Autostart => false,
            Self::Uri(_) | Self::InstallFlow { .. } => true,
            Self::Product { code, exec } => exec.is_some() || game_uid(code).is_some(),
        }
    }
}

/// The `battlenet://` link for a game family.
pub fn game_uri(family: &str) -> String {
    format!("battlenet://{family}")
}

/// Battle.net's id for a game's tile, as `--exec="launch <uid>"` wants it.
///
/// **Not the TACT product code.** The client works in game families, one per
/// tile, read out of its log (`Selecting game family by id: …`):
///
/// ```text
/// WoW  -> WoWX12_retail
/// WoWC -> WoW_wow_classic_era, WoW_wow_classic
/// WoWF -> WoWPTR_wow_classic_beta, WoW_wowf
/// W3   -> W3_retail
/// ```
///
/// A TACT code here makes the client fall back to its first family, Warcraft
/// III. The WoW families are compiled in; the rest come from
/// [`crate::families`].
pub fn resolve_game_uid(code: &ProductCode, families: &crate::families::Families) -> Option<String> {
    game_uid(code).or_else(|| families.family_of(code.as_str()).map(str::to_owned))
}

pub fn game_uid(code: &ProductCode) -> Option<String> {
    let uid = match code {
        // PTR and beta builds of retail live in the retail tile's version list.
        ProductCode::Retail | ProductCode::Ptr | ProductCode::Ptr2 | ProductCode::Beta => "WoW",
        ProductCode::Classic
        | ProductCode::ClassicEra
        | ProductCode::ClassicPtr
        | ProductCode::EraPtr => "WoWC",
        // Observed 2026-10-14: the Forever tile's stress test logs `WoWF`, then `WoW_wowf`.
        ProductCode::ClassicBeta | ProductCode::Forever => "WoWF",
        // From a real client log. Only observed codes: a guess opens another game.
        ProductCode::Custom(code) => match code.as_str() {
            "hsb" | "hs_beta" => "WTCG",
            "osi" => "OSI",
            _ => return None,
        },
    };
    Some(uid.to_owned())
}

/// Start Battle.net and return the running child without waiting for it.
pub fn launch(
    runner: &ProtonRunner,
    layout: &Layout,
    launch: &Launch,
) -> Result<std::process::Child, BattleNetError> {
    launch_wrapped(runner, layout, launch, &[])
}

/// [`launch`], run under `wrappers` (outermost first). Only for the call that
/// becomes the client, which the games inherit from; a call that hands
/// arguments to a running client exits at once.
pub fn launch_wrapped(
    runner: &ProtonRunner,
    layout: &Layout,
    launch: &Launch,
    wrappers: &[PathBuf],
) -> Result<std::process::Child, BattleNetError> {
    let install_dir = layout.battle_net_dir();
    if !is_installed(&install_dir) {
        return Err(BattleNetError::NotInstalled(install_dir));
    }

    // In a container, a call into an already running prefix must join it, or
    // it waits for the very session it is talking to.
    let joined;
    let runner = match runner.has_container() && !crate::preflight::scan(layout).is_empty() {
        true => {
            joined = runner.clone().joining_session();
            &joined
        }
        false => runner,
    };

    let (exe, args) = launch.command(&install_dir);
    let mut cmd = crate::tweaks::wrapped(runner.command("run", &exe, args), wrappers);
    tracing::info!("starting Battle.net ({launch:?}): {cmd:?}");
    cmd.stdin(Stdio::null())
        .spawn()
        .map_err(|e| BattleNetError::Prefix(PrefixError::Spawn { program: exe, source: e }))
}
