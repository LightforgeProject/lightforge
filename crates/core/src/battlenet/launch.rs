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
    /// The same client start, told that it was started automatically.
    ///
    /// `--autostarted` is the flag Blizzard's own Run entry passes, the
    /// client writes it as `"%1" --autostarted`: and it is the only condition
    /// under which the client honours
    /// [`AUTOSTART_MINIMIZED_KEY`]: its startup decision reads
    /// `needToMinimize` from `shouldMainWindowShowMinimizedAtAutoStartup &&
    /// varAutoStarted`. A hand-off is exactly an automatic start: nobody asked
    /// for the client, they asked for the game.
    Autostart,
    /// Hand straight off into a product, so the user never clicks through the
    /// Battle.net UI. Paired with `GameLaunchWindowBehavior: 2`, the client
    /// exits by itself once the game is up.
    Product { code: ProductCode, exec: Option<String> },
    /// Open the client's install flow for a product, by its install uid.
    ///
    /// The one thing that is neither view state nor a link: an action with a
    /// view of its own, which is why it is the only candidate that can
    /// survive the client's boot straight to its homepage.
    ///
    /// `install` and `game` are the client's own option names, pooled beside
    /// `repair` and `modify` in its binary against the descriptions "Start a
    /// package installation" and "Game registration/installation uid".
    InstallFlow { uid: String },
    /// A `battlenet://` link, handed over the way Wine registered the protocol
    /// (`Battle.net.exe --uri="%1"`). It is how Blizzard's own web buttons
    /// drive the client, and a command the client acts on rather than view
    /// state its startup can overwrite.
    Uri(String),
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
            // Through the launcher, like a plain start: it is what Blizzard's
            // shortcut runs, it keeps the client's self-update path intact,
            // and it passes its command line on to the client.
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
                // No family id for this product, so there is nothing safe to
                // ask for. Opening the client leaves the user one click away
                // from the right tile; guessing puts them on the wrong game.
                None => Self::Client.command(install_dir),
            },
        }
    }

    /// Whether this launch can actually hand off, or would only open the
    /// client. Lets a caller label the button for what it will really do.
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
/// III, which is where `launch wow_classic_era` used to land. The compiled-in
/// WoW families come first (a PTR and a beta live in the retail tile, Forever
/// has its own); everything else comes from [`crate::families`].
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
        // Observed 2026-10-14: picking the stress test in the Forever tile's
        // dropdown logs `WoWF` and then `WoW_wowf`, one tile for both.
        ProductCode::ClassicBeta | ProductCode::Forever => "WoWF",
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
    launch_wrapped(runner, layout, launch, &[])
}

/// [`launch`], run under `wrappers` (outermost first).
///
/// Separate because almost nothing wants them. A wrapper belongs on the
/// invocation that *becomes* the client, the one whose process the games
/// inherit from, and not on the invocations that only hand arguments to a
/// client already running: those exit at once, so a power profile or a
/// GameMode session held around them would be held for nothing.
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

    // In a container, a call that finds the prefix already running must join
    // it: the client's hand-off, or a second open of the client, arrives that
    // way and would otherwise wait for the session it is talking to to end.
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
