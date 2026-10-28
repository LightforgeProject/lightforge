//! Per-product launch settings: host-side wrappers, environment variables and
//! the user's own overrides.
//!
//! **Nothing is assumed installed**: `game-performance` and `mangohud` are
//! looked up on `PATH` and do nothing when absent.
//!
//! **The game's own settings (`Config.wtf`) are not touched**: only a launch
//! wrapper and environment variables.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// What Game Settings can set for one product.
///
/// Older configs' extra fields are skipped on load and dropped on save.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tweaks {
    /// Launch through CachyOS' `game-performance`, which holds the CPU on its
    /// performance profile and inhibits idle for the life of the game.
    ///
    /// `None` follows the default: on wherever the tool exists.
    pub game_performance: Option<bool>,
    /// Show the MangoHud overlay.
    pub mangohud: bool,
    /// Run on Proton's Wayland backend instead of through XWayland.
    ///
    /// Proton itself ignores this unless `WAYLAND_DISPLAY` is set, and so
    /// does [`Tweaks::environment`]; see [`HostTools::wayland`].
    pub wayland: bool,
    /// The user's own environment variables for the launch.
    pub env: BTreeMap<String, String>,
}

impl Tweaks {
    /// Environment variables for the launch, in the order they are applied.
    pub fn environment(&self, tools: &HostTools) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = Vec::new();
        if self.mangohud && tools.mangohud.is_some() {
            env.push(("MANGOHUD".into(), "1".into()));
        }
        // Only with `WAYLAND_DISPLAY` set, as Proton itself requires.
        if self.wayland && tools.wayland {
            env.push(("PROTON_ENABLE_WAYLAND".into(), "1".into()));
        }
        // The user's own overrides go last so they win over anything above.
        for (key, value) in &self.env {
            env.push((key.into(), value.into()));
        }
        env
    }

    /// Whether `game-performance` should be used, accounting for its
    /// default-on-when-present behaviour.
    pub fn wants_game_performance(&self) -> bool {
        self.game_performance.unwrap_or(true)
    }

    /// Wrapper programs to launch through, outermost first.
    pub fn wrappers(&self, tools: &HostTools) -> Vec<PathBuf> {
        match self.wants_game_performance() {
            true => tools.game_performance.clone().into_iter().collect(),
            false => Vec::new(),
        }
    }
}

/// Which optional tools this machine actually has.
///
/// Detected, so the panel can say what is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTools {
    /// CachyOS' performance-governor wrapper.
    pub game_performance: Option<PathBuf>,
    /// The MangoHud binary, taken as evidence its Vulkan layer is installed.
    pub mangohud: Option<PathBuf>,
    /// Whether this is a Wayland session.
    pub wayland: bool,
    /// The proprietary NVIDIA driver's major version, when it is loaded.
    pub nvidia: Option<u32>,
}

impl HostTools {
    pub fn detect() -> Self {
        Self {
            game_performance: on_path("game-performance"),
            mangohud: on_path("mangohud"),
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
            nvidia: crate::debug::nvidia_version()
                .and_then(|version| version.split('.').next()?.parse().ok()),
        }
    }

    /// An NVIDIA driver older than 555, the first with explicit sync, which
    /// a Wayland compositor needs from it to present frames without flicker.
    pub fn nvidia_lacks_explicit_sync(&self) -> bool {
        self.nvidia.is_some_and(|major| major < 555)
    }
}

/// A line of the environment editor that is not `KEY=value`.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("line {line}: {reason}")]
pub struct EnvLineError {
    /// 1-based, as the editor numbers it.
    pub line: usize,
    pub reason: &'static str,
}

/// Read the environment editor's text: one `KEY=value` per line, blank lines
/// and `#` comments skipped, a pair of matching quotes around a value removed.
///
/// Every bad line is reported, not just the first. A repeated key keeps the last.
pub fn parse_env(text: &str) -> (BTreeMap<String, String>, Vec<EnvLineError>) {
    let mut env = BTreeMap::new();
    let mut bad = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = index + 1;
        let trimmed = raw.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let trimmed = trimmed.strip_prefix("export ").unwrap_or(trimmed).trim_start();
        let Some((key, value)) = trimmed.split_once('=') else {
            bad.push(EnvLineError { line, reason: "needs KEY=value" });
            continue;
        };
        let key = key.trim();
        let valid = key.chars().next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_')
            && key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_');
        if !valid {
            bad.push(EnvLineError { line, reason: "the name may only use letters, digits and _" });
            continue;
        }
        let value = value.trim();
        let value = ['"', '\'']
            .iter()
            .find_map(|q| value.strip_prefix(*q).and_then(|rest| rest.strip_suffix(*q)))
            .unwrap_or(value);
        env.insert(key.to_owned(), value.to_owned());
    }
    (env, bad)
}

/// The editor's text for a stored environment: the inverse of [`parse_env`].
pub fn env_text(env: &BTreeMap<String, String>) -> String {
    env.iter().map(|(k, v)| format!("{k}={v}\n")).collect()
}

/// Find an executable on `PATH`.
///
/// Without `which`, which may not be installed.
fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable(candidate))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;

    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

/// Rebuild `cmd` to run under `wrappers`, outermost first.
///
/// [`Command`] cannot be wrapped in place, so it is rebuilt.
pub fn wrapped(cmd: Command, wrappers: &[PathBuf]) -> Command {
    let Some((outer, rest)) = wrappers.split_first() else {
        return cmd;
    };

    let mut out = Command::new(outer);
    for wrapper in rest {
        out.arg(wrapper);
    }
    out.arg(cmd.get_program());
    out.args(cmd.get_args());
    for (key, value) in cmd.get_envs() {
        match value {
            Some(value) => out.env(key, value),
            None => out.env_remove(key),
        };
    }
    if let Some(dir) = cmd.get_current_dir() {
        out.current_dir(dir);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsStr;

    #[test]
    fn a_default_profile_asks_for_nothing() {
        let tweaks = Tweaks::default();
        assert!(tweaks.environment(&HostTools::default()).is_empty());
        assert!(tweaks.wrappers(&HostTools::default()).is_empty());
    }

    #[test]
    fn game_performance_is_on_by_default_but_can_be_refused() {
        assert!(Tweaks::default().wants_game_performance());
        assert!(!Tweaks { game_performance: Some(false), ..Tweaks::default() }
            .wants_game_performance());
    }

    #[test]
    fn game_performance_wraps_only_where_it_exists_and_is_wanted() {
        let tools = HostTools {
            game_performance: Some("/usr/bin/game-performance".into()),
            ..HostTools::default()
        };
        assert_eq!(
            Tweaks::default().wrappers(&tools),
            vec![PathBuf::from("/usr/bin/game-performance")]
        );
        let refused = Tweaks { game_performance: Some(false), ..Tweaks::default() };
        assert!(refused.wrappers(&tools).is_empty());
        assert!(Tweaks::default().wrappers(&HostTools::default()).is_empty());
    }

    #[test]
    fn mangohud_needs_the_binary_to_be_present() {
        let tweaks = Tweaks { mangohud: true, ..Tweaks::default() };
        assert!(tweaks.environment(&HostTools::default()).is_empty());

        let tools =
            HostTools { mangohud: Some("/usr/bin/mangohud".into()), ..HostTools::default() };
        assert_eq!(
            tweaks.environment(&tools),
            vec![(OsString::from("MANGOHUD"), OsString::from("1"))]
        );
    }

    #[test]
    fn the_wayland_backend_needs_a_wayland_session() {
        let tweaks = Tweaks { wayland: true, ..Tweaks::default() };
        assert!(tweaks.environment(&HostTools::default()).is_empty());

        let tools = HostTools { wayland: true, ..HostTools::default() };
        assert_eq!(
            tweaks.environment(&tools),
            vec![(OsString::from("PROTON_ENABLE_WAYLAND"), OsString::from("1"))]
        );
    }

    #[test]
    fn explicit_sync_starts_at_nvidia_555() {
        let nvidia = |major| HostTools { nvidia: Some(major), ..HostTools::default() };
        assert!(nvidia(550).nvidia_lacks_explicit_sync());
        assert!(!nvidia(555).nvidia_lacks_explicit_sync());
        assert!(!HostTools::default().nvidia_lacks_explicit_sync(), "no NVIDIA driver at all");
    }

    #[test]
    fn a_user_override_wins_over_a_toggle() {
        let mut env = BTreeMap::new();
        env.insert("MANGOHUD".to_owned(), "0".to_owned());
        let tweaks = Tweaks { mangohud: true, env, ..Tweaks::default() };
        let tools =
            HostTools { mangohud: Some("/usr/bin/mangohud".into()), ..HostTools::default() };
        // Applied in order, so the last write is the one the child sees.
        let env = tweaks.environment(&tools);
        assert_eq!(env.last().unwrap().1, OsString::from("0"));
    }

    #[test]
    fn wrapping_keeps_the_program_arguments_and_environment() {
        let mut inner = Command::new("/proton/proton");
        inner.arg("run").arg("/games/Wow.exe").env("STEAM_COMPAT_DATA_PATH", "/compat");

        let outer = wrapped(inner, &["/usr/bin/game-performance".into()]);
        assert_eq!(outer.get_program(), "/usr/bin/game-performance");
        let args: Vec<_> = outer.get_args().collect();
        assert_eq!(args, ["/proton/proton", "run", "/games/Wow.exe"]);
        assert!(outer
            .get_envs()
            .any(|(k, v)| k == "STEAM_COMPAT_DATA_PATH" && v == Some(OsStr::new("/compat"))));
    }

    #[test]
    fn no_wrappers_leaves_the_command_untouched() {
        let mut inner = Command::new("/proton/proton");
        inner.arg("run");
        let outer = wrapped(inner, &[]);
        assert_eq!(outer.get_program(), "/proton/proton");
    }

    // A config from the old Game Optimizations page: dropped fields load,
    // and an explicit `game_performance = false` survives.
    #[test]
    fn a_config_from_the_optimizations_page_still_loads() {
        let old = r#"
game_performance = false
gamemode = true
mangohud = true
fps_cap = 144
raw_mouse = true
big_shader_cache = true
graphics_api = "D3d12"
spell_queue_ms = 80

[env]
DXVK_HUD = "fps"
"#;
        let tweaks: Tweaks = toml::from_str(old).unwrap();
        assert_eq!(tweaks.game_performance, Some(false));
        assert!(tweaks.mangohud);
        assert_eq!(tweaks.env.get("DXVK_HUD").map(String::as_str), Some("fps"));
    }

    #[test]
    fn tweaks_round_trip_through_toml() {
        let mut env = BTreeMap::new();
        env.insert("DXVK_HUD".to_owned(), "fps".to_owned());
        let tweaks = Tweaks { mangohud: true, wayland: true, env, ..Tweaks::default() };
        let text = toml::to_string(&tweaks).unwrap();
        assert_eq!(toml::from_str::<Tweaks>(&text).unwrap(), tweaks);
    }

    #[test]
    fn the_env_editor_reads_key_value_lines() {
        let (env, bad) = parse_env(
            "# comment\n\nDXVK_HUD=fps\nexport WINEDEBUG=-all\nNAME=\"two words\"\nQ='x'\nEMPTY=\n",
        );
        assert!(bad.is_empty(), "{bad:?}");
        assert_eq!(env.get("DXVK_HUD").map(String::as_str), Some("fps"));
        assert_eq!(env.get("WINEDEBUG").map(String::as_str), Some("-all"));
        assert_eq!(env.get("NAME").map(String::as_str), Some("two words"));
        assert_eq!(env.get("Q").map(String::as_str), Some("x"));
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
    }

    #[test]
    fn the_env_editor_names_every_bad_line() {
        let (env, bad) = parse_env("GOOD=1\nnonsense\n9BAD=1\nA B=2\nLAST=3\n");
        assert_eq!(env.len(), 2, "good lines survive bad ones");
        let lines: Vec<usize> = bad.iter().map(|e| e.line).collect();
        assert_eq!(lines, [2, 3, 4]);
    }

    #[test]
    fn the_env_text_reads_back_as_the_same_environment() {
        let mut env = BTreeMap::new();
        env.insert("A".to_owned(), "1".to_owned());
        env.insert("B".to_owned(), "x=y z".to_owned());
        let (back, bad) = parse_env(&env_text(&env));
        assert!(bad.is_empty());
        assert_eq!(back, env);
    }
}
