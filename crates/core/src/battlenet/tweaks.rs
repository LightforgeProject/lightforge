use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::{io, BattleNetError};
use crate::paths::Layout;
use crate::product::ProductCode;

/// Battle.net's install path when there is no library. The WoW symlink sits
/// directly under it, so an install the client starts needs no "locate game".
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

/// Lightforge's defaults for `Battle.net.config`. Every value is a JSON
/// string: the client writes them that way and ignores a real boolean.
///
/// `background` is [`crate::config::Config::battle_net_in_background`]: the
/// client stays up after a hand-off instead of exiting.
pub fn default_tweaks(layout: &Layout, background: bool) -> Vec<Tweak> {
    // Games Battle.net installs land on the default library's drive. The
    // client itself stays in the prefix.
    let install_path = layout
        .default_library()
        .map(|library| library.windows_games_path())
        .unwrap_or_else(|| WINDOWS_PROGRAM_FILES.to_owned());
    let mut tweaks = vec![
        // The client logs which it took: `Game is now running with game launch
        // behavior set to …`. Measured on 2.53.6: `0` and `1` minimize to the
        // taskbar, `2` exits, `3` closes the window, which `HideOnClose` turns
        // into the tray.
        match background {
            true => Tweak::new(
                "Client.GameLaunchWindowBehavior",
                json!("3"),
                "go back to the tray once the game has been handed off, ready for the next one",
            ),
            false => Tweak::new(
                "Client.GameLaunchWindowBehavior",
                json!("2"),
                "exit Battle.net completely once the game has been handed off",
            ),
        },
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
    ];
    if background {
        tweaks.push(Tweak::new(
            "Client.RestoreWindowOnGameEnd",
            json!("false"),
            "stay in the tray when the game exits: \"Restoring auto closed window\" otherwise",
        ));
    }
    tweaks
}

/// The download limit Battle.net applies to its own transfers, in bytes per
/// second, or `None` when it has none set.
pub const DOWNLOAD_LIMIT_KEY: &str = "Client.Install.LastUserDownloadLimitNextPatchInBps";

/// Battle.net's own download limit. A value that is not a number reads as none.
pub fn download_limit(layout: &Layout, user: &str) -> Option<u64> {
    let text = std::fs::read_to_string(layout.battle_net_config(user)).ok()?;
    let config: Value = serde_json::from_str(&text).ok()?;
    let mut node = &config;
    for key in DOWNLOAD_LIMIT_KEY.split('.') {
        node = node.get(key)?;
    }
    node.as_str()?.parse().ok().filter(|bps| *bps > 0)
}

/// Set or clear Battle.net's own download limit. `None` removes the key, the
/// state the client is in before anyone picks a limit.
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

/// Keep Agent's hands off products Lightforge downloads itself, so two
/// programs never maintain one CASC store. Titles Battle.net installed are
/// left to it.
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

/// Whether the client shows its corner notifications. Per account: the same
/// key in `Battle.net.config` is ignored. It logs `Toasts are disabled in
/// ClientSettings and will not be displayed` (measured 2026-11-04).
pub fn toasts_tweak(on: bool) -> Tweak {
    Tweak::new(
        "User.Client.ToastsEnabled",
        json!(match on {
            true => "true",
            false => "false",
        }),
        "a client kept out of sight does not pop up over whatever is on screen",
    )
}

/// Whether the client updates `uid` by itself. There is no command for starting
/// an update (its `--exec` verbs are `launch`, `launch_uid`, `launch_register`,
/// `dialog`, `logout` and `shutdown`); this, read when it starts, is what makes
/// one begin within seconds (measured on Forever, 2026-11-04).
pub fn auto_update_tweak(uid: &str, on: bool) -> Tweak {
    Tweak::new(
        &format!("Games.{uid}.AutoUpdate"),
        json!(match on {
            true => "true",
            false => "false",
        }),
        "a game Battle.net owns updates when the user asks for it, not behind their back",
    )
}

/// Defaults for a per-account `<hash>.config`. The client stores window
/// behaviour per account and ignores these keys in `Battle.net.config`.
pub fn account_tweaks(background: bool) -> Vec<Tweak> {
    let mut tweaks = vec![
        match background {
            true => Tweak::new(
                "User.Client.HideOnClose",
                json!("true"),
                "closing the window sends it to the tray, where it waits for the next hand-off",
            ),
            false => Tweak::new(
                "User.Client.HideOnClose",
                json!("false"),
                "closing the window quits — no tray icon, no lingering background client",
            ),
        },
        Tweak::new(
            "User.Client.DisplayExitPrompt",
            json!("false"),
            "quit immediately instead of showing the close countdown",
        ),
    ];
    if background {
        tweaks.push(toasts_tweak(false));
    }
    tweaks
}

/// The settings checkbox "Launch Battle.net minimized to the system tray".
/// Read from `Battle.net.config` only: the same key in the per-account file
/// is ignored, and so is the Qt `WindowState` there (client 2.53.6, measured
/// 2026-11-04). The window then lives in the tray alone, with no taskbar
/// entry, and a second plain start brings it back. Wine cannot minimize a
/// window for us, and Wayland lets nothing do it from outside.
pub const AUTOSTART_MINIMIZED_KEY: &str = "Client.AutoStartMinimized";

/// Start the client in the tray, or as a window. For [`configure`]. It logs
/// the decision on every start:
///
/// ```text
/// [Frontend] {Main} Attempting to show main window. isMinimizedOnStartup=true
/// shouldMainWindowShowMinimizedAtAutoStartup=true varAutoStarted=true … needToMinimize=true
/// ```
///
/// Only an autostart honours it, see [`super::Launch::Autostart`]. Re-applied
/// before each start: the client rewrites the file on exit.
pub fn window_state_tweaks(minimized: bool) -> Vec<Tweak> {
    vec![Tweak::new(
        AUTOSTART_MINIMIZED_KEY,
        json!(match minimized {
            true => "true",
            false => "false",
        }),
        match minimized {
            true => "start in the tray when started as an autostart, which a hand-off is",
            false => "show the window when started as an autostart",
        },
    )]
}

/// Merge `tweaks` into a parsed config, creating missing objects and leaving
/// every other key alone: the same file holds the login state.
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
            // A scalar where an object belongs: replace the branch, keep setup going.
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

/// Read, merge and atomically rewrite `Battle.net.config`. The client creates
/// it on first launch, so a missing one starts empty and setup can seed it.
pub fn configure(layout: &Layout, user: &str, tweaks: &[Tweak]) -> Result<PathBuf, BattleNetError> {
    let path = layout.battle_net_config(user);
    merge_file(&path, tweaks, true)?;
    Ok(path)
}

/// Product codes as the client spells them, the keys of `Games` in
/// `Battle.net.config`. They can differ from the CDN's (`hs_beta` vs `hsb`).
/// A missing or unreadable file gives none.
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

/// Apply `tweaks` to every per-account config, returning the files changed.
/// They appear on first sign-in, so none exist after a fresh setup; pre-flight
/// applies them on later runs.
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

/// Delete one dotted key. A missing file or key counts as success.
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

/// Read, merge and atomically rewrite one config. With `seed` a missing file
/// starts empty; without it, the file is the client's to create.
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
