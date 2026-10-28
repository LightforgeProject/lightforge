use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{io, BattleNetError};
use crate::paths::Layout;
use crate::product::ProductCode;

/// Where Battle.net should propose installing games. The WoW symlink lives
/// directly under this directory, so an install it starts itself lands in
/// `games/` without a "locate game" prompt.
const WINDOWS_PROGRAM_FILES: &str = r"C:\Program Files (x86)";

/// One `Battle.net.config` setting, addressed by its path through the JSON.
#[derive(Debug, Clone, PartialEq)]
pub struct Tweak {
    pub path: Vec<String>,
    pub value: Value,
    /// Why it's set, surfaced by the CLI so the config isn't a pile of magic.
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
pub fn default_tweaks(layout: &Layout) -> Vec<Tweak> {
    // The default library's `games/` drive, so a title Battle.net installs
    // itself lands beside WoW on the drive the user chose, rather than inside
    // the prefix on the system disk, which is where `Program Files (x86)`
    // used to put it.
    //
    // This moves the *games*, never the client: Battle.net itself is
    // installed into the prefix, and the prefix is at the Blizzard root
    // whatever the default drive is.
    let install_path = layout
        .default_library()
        .map(|library| library.windows_games_path())
        .unwrap_or_else(|| WINDOWS_PROGRAM_FILES.to_owned());
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
            json!(install_path),
            "propose installs in the default drive's games folder, not inside the prefix",
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
/// only lever there is for starting the client out of the way, Wine cannot
/// minimize a window for us, and nothing on Wayland can do it from outside.
pub const WINDOW_STATE_KEY: &str = "User.Client.Windows.BrowserMainWindow.WindowState";

/// The client's own "launch minimized to the system tray" preference.
///
/// Its settings panel calls this *Launch Battle.net minimized to the system
/// tray*, and it is the key the checkbox writes, `AutoStartMinimized`, beside
/// `HideOnClose` and `DisplayExitPrompt` in `battle.net.dll`, so it lives in
/// the per-account config with the rest of the window preferences.
pub const AUTOSTART_MINIMIZED_KEY: &str = "User.Client.AutoStartMinimized";

/// Start the client minimized, or normally.
///
/// The client logs its decision on every start:
///
/// ```text
/// [Frontend] {Main} Attempting to show main window. isMinimizedOnStartup=false
/// shouldMainWindowShowMinimizedAtAutoStartup=false varAutoStarted=false …
/// ```
///
/// `isMinimizedOnStartup` is the Qt window state below; the preference above
/// is only read when `varAutoStarted` is set, which is what
/// [`Launch::Autostart`] is for. Both are re-applied before each start because
/// the client writes its window state back on every exit.
pub fn window_state_tweaks(minimized: bool) -> Vec<Tweak> {
    vec![
        Tweak::new(
            WINDOW_STATE_KEY,
            json!(match minimized {
                true => "1",
                false => "0",
            }),
            match minimized {
                true => "come up minimized: the hand-off drives the client, nobody needs to see it",
                false => "come up as a normal window",
            },
        ),
        Tweak::new(
            AUTOSTART_MINIMIZED_KEY,
            json!(match minimized {
                true => "true",
                false => "false",
            }),
            match minimized {
                true => "stay minimized when started as an autostart, which a hand-off is",
                false => "show the window when started as an autostart",
            },
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
/// A missing file is normal, Battle.net only writes one on first launch, and
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
/// An absent or unreadable file yields nothing rather than an error, the
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
/// login, so there is nothing to write before the user has signed in once, an
/// empty result is the normal outcome of a fresh setup, not a failure. The
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
    crate::fsutil::write_atomic(path, text.as_bytes()).map_err(io(path))
}

/// Read, merge, and atomically rewrite one config file.
///
/// `seed` decides what a missing file means: the global config is created from
/// nothing, while a per-account file is the client's to invent.
fn merge_file(path: &Path, tweaks: &[Tweak], seed: bool) -> Result<bool, BattleNetError> {
    let mut config = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound && seed => json!({}),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(io(path)(e)),
    };

    apply_tweaks(&mut config, tweaks);
    let mut text = serde_json::to_string_pretty(&config)
        .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?;
    text.push('\n');
    crate::fsutil::write_atomic(path, text.as_bytes()).map_err(io(path))?;
    Ok(true)
}
