//! Per-product performance settings: host-side wrappers, environment, and the
//! client CVars Lightforge is willing to manage.
//!
//! Two rules shape what is in here.
//!
//! **Nothing is assumed installed.** `game-performance`, `gamemoderun` and
//! `mangohud` are looked up on `PATH` and simply do not exist as options when
//! they are absent, so a toggle can never produce a launch command that fails
//! on a distribution that does not ship them.
//!
//! **No invented values.** Every CVar here either wins CPU back, cuts input
//! latency, or fixes a documented Linux-specific problem. The widely-copied
//! "hidden CVar" lists are mostly the opposite — `shadowMode 4`, `SSAO 4`,
//! `waterDetail 3` *cost* frames — so the ones that maximise visuals are
//! collected behind a single toggle that says so, well away from the rest.
//! `processAffinityMask` is deliberately absent: Blizzard has asked people not
//! to set it.
//!
//! This all stays outside the game: a launch wrapper, environment variables,
//! and a settings file the client itself reads at startup. Nothing reaches
//! into the running process.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

/// Everything the "Game Optimizations" panel can set for one product.
///
/// Defaults are all-off except where a default of on is the point — see
/// [`Tweaks::game_performance`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Tweaks {
    /// Launch through CachyOS' `game-performance`, which holds the CPU on its
    /// performance profile and inhibits idle for the life of the game.
    ///
    /// `None` means "follow the default", which is *on wherever the tool
    /// exists*: it is a system utility, it self-disables when the CPU driver
    /// has no performance profile, and its effect is exactly what someone
    /// launching a game wants. An explicit `Some(false)` turns it off.
    pub game_performance: Option<bool>,
    /// Launch through `gamemoderun` (Feral GameMode), when installed.
    pub gamemode: bool,
    /// Show the MangoHud overlay.
    pub mangohud: bool,
    /// Which Direct3D version the client asks for, and therefore whether the
    /// game runs on DXVK or VKD3D. `None` leaves the client's own choice alone.
    pub graphics_api: Option<GraphicsApi>,
    /// Foreground frame cap. `None` leaves it uncapped.
    ///
    /// The client's own `maxFPS` rather than `DXVK_FRAME_RATE`, because it
    /// applies under VKD3D too — a DXVK-only cap would silently stop working
    /// the moment someone switched the API above to D3D12.
    pub fps_cap: Option<u32>,
    /// `SpellQueueWindow`, in milliseconds. Latency plus ~50 is the usual
    /// advice; `None` leaves the client default.
    pub spell_queue_ms: Option<u32>,
    /// Stop the client profiling addon execution.
    pub skip_addon_profiling: bool,
    /// Drop water and weather detail in raids and battlegrounds.
    pub reduce_raid_effects: bool,
    /// Skip non-critical world preloading, which shortens zone loads.
    pub faster_zone_loads: bool,
    /// Raw mouse input — also the fix for the cursor recentering under Wine.
    pub raw_mouse: bool,
    /// Turn off the full-screen glow and the death desaturation filter.
    pub no_screen_effects: bool,
    /// Push view distance and ground clutter to the engine's ceiling.
    ///
    /// This one costs frames rather than winning them. It is here because
    /// people do want it, not because it is an optimization.
    pub max_draw_distance: bool,
    /// Extra environment variables for the launch, as an escape hatch for
    /// anything this panel does not model.
    pub env: std::collections::BTreeMap<String, String>,
}

/// Which Direct3D version the client requests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphicsApi {
    /// D3D11, translated by DXVK. The safer pick, and usually the faster one
    /// on NVIDIA.
    D3d11,
    /// D3D12, translated by VKD3D-Proton. Typically a small win on AMD.
    D3d12,
}

impl GraphicsApi {
    /// The `gxApi` value the client expects.
    pub fn cvar(self) -> &'static str {
        match self {
            Self::D3d11 => "D3D11",
            Self::D3d12 => "D3D12",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::D3d11 => "D3D11 (DXVK)",
            Self::D3d12 => "D3D12 (VKD3D)",
        }
    }
}

/// Every CVar any toggle in [`Tweaks`] can write.
///
/// Used to unset what is no longer enabled: a toggle switched off has to take
/// its values out of `Config.wtf`, or "off" would still be enforcing them.
const MANAGED: &[&str] = &[
    "gxApi",
    "maxFPS",
    "SpellQueueWindow",
    "scriptProfile",
    "addonProfilerEnabled",
    "RAIDWaterDetail",
    "RAIDweatherDensity",
    "worldPreloadNonCritical",
    "rawMouseEnable",
    "ffxGlow",
    "ffxDeath",
    "farclip",
    "groundEffectDensity",
    "groundEffectDist",
];

impl Tweaks {
    /// Whether anything at all is set, for a panel that wants to say so.
    pub fn is_default(&self) -> bool {
        self == &Self::default()
    }

    /// The CVars to write, and the ones to take back out.
    ///
    /// Both halves come from [`MANAGED`], so the result is a complete
    /// description of this product's client settings as Lightforge sees them —
    /// which is what makes re-asserting it before every launch safe.
    pub fn cvars(&self) -> (Vec<(&'static str, String)>, Vec<&'static str>) {
        let mut set: Vec<(&'static str, String)> = Vec::new();

        if let Some(api) = self.graphics_api {
            set.push(("gxApi", api.cvar().to_owned()));
        }
        if let Some(cap) = self.fps_cap {
            set.push(("maxFPS", cap.to_string()));
        }
        if let Some(ms) = self.spell_queue_ms {
            set.push(("SpellQueueWindow", ms.to_string()));
        }
        if self.skip_addon_profiling {
            set.push(("scriptProfile", "0".to_owned()));
            set.push(("addonProfilerEnabled", "0".to_owned()));
        }
        if self.reduce_raid_effects {
            set.push(("RAIDWaterDetail", "0".to_owned()));
            set.push(("RAIDweatherDensity", "0".to_owned()));
        }
        if self.faster_zone_loads {
            set.push(("worldPreloadNonCritical", "0".to_owned()));
        }
        if self.raw_mouse {
            set.push(("rawMouseEnable", "1".to_owned()));
        }
        if self.no_screen_effects {
            set.push(("ffxGlow", "0".to_owned()));
            set.push(("ffxDeath", "0".to_owned()));
        }
        if self.max_draw_distance {
            // The client clamps these to whatever its own ceiling is, so the
            // values are "as far as this build allows" rather than literal.
            set.push(("farclip", "10000".to_owned()));
            set.push(("groundEffectDensity", "256".to_owned()));
            set.push(("groundEffectDist", "500".to_owned()));
        }

        let unset = MANAGED
            .iter()
            .copied()
            .filter(|name| !set.iter().any(|(n, _)| n == name))
            .collect();
        (set, unset)
    }

    /// Environment variables for the launch, in the order they are applied.
    pub fn environment(&self, tools: &HostTools) -> Vec<(OsString, OsString)> {
        let mut env: Vec<(OsString, OsString)> = Vec::new();
        if self.mangohud && tools.mangohud.is_some() {
            env.push(("MANGOHUD".into(), "1".into()));
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
    ///
    /// `game-performance` goes outside `gamemoderun`: the power profile should
    /// be held for the whole session including GameMode's own setup, and
    /// `game-performance` ends in `exec`, so nesting it outward costs nothing.
    pub fn wrappers(&self, tools: &HostTools) -> Vec<PathBuf> {
        let mut out = Vec::new();
        if self.wants_game_performance() {
            out.extend(tools.game_performance.clone());
        }
        if self.gamemode {
            out.extend(tools.gamemode.clone());
        }
        out
    }
}

/// Which optional performance tools this machine actually has.
///
/// Detected rather than assumed: a toggle for a binary that is not installed
/// is a launch failure waiting to happen, so the panel hides what is missing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostTools {
    /// CachyOS' performance-governor wrapper.
    pub game_performance: Option<PathBuf>,
    /// Feral GameMode's launch wrapper.
    pub gamemode: Option<PathBuf>,
    /// The MangoHud binary, taken as evidence its Vulkan layer is installed.
    pub mangohud: Option<PathBuf>,
}

impl HostTools {
    pub fn detect() -> Self {
        Self {
            game_performance: on_path("game-performance"),
            gamemode: on_path("gamemoderun"),
            mangohud: on_path("mangohud"),
        }
    }

    /// Whether there is anything to show in a host-tools section at all.
    pub fn any(&self) -> bool {
        self.game_performance.is_some() || self.gamemode.is_some() || self.mangohud.is_some()
    }
}

/// Find an executable on `PATH`.
///
/// Hand-rolled rather than shelling out to `which`, which is not guaranteed to
/// be installed and would be one more binary dependency.
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
/// A wrapper has to *become* the program with the original command as its
/// arguments, which no method on [`Command`] can do in place — hence a rebuild
/// carrying the program, arguments and environment across.
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

/// A starting point for the frames-versus-scenery trade.
///
/// The panel is a list of independent switches, which is honest but gives no
/// answer to "just make it fast". A preset is that answer: one press sets the
/// switches that sit on this axis, and every one of them stays individually
/// reachable afterwards.
///
/// **What a preset governs is deliberately narrow** — only the switches that
/// trade frames against scenery. It never touches [`Tweaks::raw_mouse`] or
/// [`Tweaks::spell_queue_ms`] (personal feel, not speed), nor
/// [`Tweaks::graphics_api`], [`Tweaks::fps_cap`], [`Tweaks::mangohud`] or
/// [`Tweaks::env`] (a genuine fork, a ceiling, an overlay and an escape
/// hatch — none of them a point on this axis). A preset that silently cleared
/// somebody's environment variables would be a data-loss bug wearing the
/// clothes of a convenience.
///
/// [`Tweaks::game_performance`] is left alone too, for a different reason: its
/// default already means *on wherever the tool exists*, no preset has a reason
/// to give that up, and overwriting it would turn "follow the default" into an
/// explicit value behind the user's back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Preset {
    /// Everything that wins frames, nothing that spends them.
    Frames,
    /// The free wins only: no scenery given up, no extra wrapper.
    Balanced,
    /// Scenery at the engine's ceiling, frames second.
    Looks,
}

impl Preset {
    pub const ALL: [Preset; 3] = [Self::Frames, Self::Balanced, Self::Looks];

    pub fn label(self) -> &'static str {
        match self {
            Self::Frames => "Frames",
            Self::Balanced => "Balanced",
            Self::Looks => "Looks",
        }
    }

    /// One line on what this choice gives up, for the hint beside the row.
    pub fn summary(self) -> &'static str {
        match self {
            Self::Frames => "Raid effects and world preloading off, glow off, view distance normal.",
            Self::Balanced => "Addon profiling and raid effects off. Nothing visual given up.",
            Self::Looks => "View distance and clutter at the ceiling. Costs frames, by choice.",
        }
    }

    /// Write this preset's switches into `tweaks`, leaving every ungoverned
    /// field exactly as it was.
    pub fn apply(self, tweaks: &mut Tweaks) {
        // True in all three: it removes instrumentation, not detail, so there
        // is no version of "I want it to look nice" that wants the profiler on.
        tweaks.skip_addon_profiling = true;
        match self {
            Self::Frames => {
                tweaks.gamemode = true;
                tweaks.reduce_raid_effects = true;
                tweaks.faster_zone_loads = true;
                tweaks.no_screen_effects = true;
                tweaks.max_draw_distance = false;
            }
            Self::Balanced => {
                tweaks.gamemode = false;
                tweaks.reduce_raid_effects = true;
                tweaks.faster_zone_loads = false;
                tweaks.no_screen_effects = false;
                tweaks.max_draw_distance = false;
            }
            Self::Looks => {
                tweaks.gamemode = false;
                tweaks.reduce_raid_effects = false;
                tweaks.faster_zone_loads = false;
                tweaks.no_screen_effects = false;
                tweaks.max_draw_distance = true;
            }
        }
    }
}

impl Tweaks {
    /// Clear every switch a [`Preset`] governs.
    ///
    /// What the panel's "Custom" button does. Custom is not a fourth preset —
    /// it is the *absence* of one, which is why it cannot be a [`Preset`]
    /// variant: pressing it empties the managed switches, and setting any of
    /// them afterwards still matches no preset, so the panel stays on Custom
    /// rather than jumping off it the moment you tick a box.
    ///
    /// Ungoverned settings are left alone here for the same reason
    /// [`Preset::apply`] leaves them: clearing somebody's environment
    /// variables is data loss, not a reset.
    /// Every switch it clears is one that *writes* something when set, so a
    /// cleared profile is also a silent one: nothing in `Config.wtf`, no
    /// wrapper, nothing. That equivalence is why no switch on this panel may
    /// be drawn inverted — a box whose empty state wrote a cvar would make
    /// "Custom" change the game while claiming to set nothing.
    pub fn clear_managed(&mut self) {
        self.gamemode = false;
        self.skip_addon_profiling = false;
        self.reduce_raid_effects = false;
        self.faster_zone_loads = false;
        self.no_screen_effects = false;
        self.max_draw_distance = false;
    }

    /// Which preset these settings are currently on, or `None` for "Custom".
    ///
    /// Compares only the governed switches, so changing the frame cap or
    /// adding an environment variable does not move a profile off its preset —
    /// those were never part of what the preset decided.
    pub fn preset(&self) -> Option<Preset> {
        Preset::ALL.into_iter().find(|preset| {
            let mut probe = self.clone();
            preset.apply(&mut probe);
            probe.governed() == self.governed()
        })
    }

    /// The switches a [`Preset`] decides, as a tuple for comparison. Kept
    /// beside `Preset::apply` on purpose: a field added to one and forgotten
    /// in the other would make `preset()` answer "Custom" forever.
    fn governed(&self) -> (bool, bool, bool, bool, bool, bool) {
        (
            self.gamemode,
            self.skip_addon_profiling,
            self.reduce_raid_effects,
            self.faster_zone_loads,
            self.no_screen_effects,
            self.max_draw_distance,
        )
    }
}

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    #[test]
    fn each_preset_is_recognised_from_the_settings_it_writes() {
        for preset in Preset::ALL {
            let mut tweaks = Tweaks::default();
            preset.apply(&mut tweaks);
            assert_eq!(tweaks.preset(), Some(preset), "{preset:?} did not recognise itself");
        }
    }

    #[test]
    fn flipping_one_governed_switch_reads_as_custom() {
        let mut tweaks = Tweaks::default();
        Preset::Balanced.apply(&mut tweaks);
        tweaks.max_draw_distance = true;
        assert_eq!(tweaks.preset(), None);
    }

    // The whole point of the narrow definition: these are not on the
    // frames-versus-scenery axis, so changing them must not move the profile
    // off its preset.
    #[test]
    fn ungoverned_settings_do_not_move_a_profile_off_its_preset() {
        let mut tweaks = Tweaks::default();
        Preset::Frames.apply(&mut tweaks);

        tweaks.fps_cap = Some(144);
        tweaks.spell_queue_ms = Some(90);
        tweaks.raw_mouse = true;
        tweaks.mangohud = true;
        tweaks.graphics_api = Some(GraphicsApi::D3d12);
        tweaks.env.insert("DXVK_HUD".to_owned(), "fps".to_owned());

        assert_eq!(tweaks.preset(), Some(Preset::Frames));
    }

    // A preset clearing somebody's environment variables would be data loss
    // wearing the clothes of a convenience.
    #[test]
    fn applying_a_preset_preserves_the_escape_hatch_and_the_fork() {
        let mut tweaks = Tweaks {
            fps_cap: Some(120),
            spell_queue_ms: Some(80),
            raw_mouse: true,
            graphics_api: Some(GraphicsApi::D3d11),
            game_performance: Some(false),
            ..Default::default()
        };
        tweaks.env.insert("MANGOHUD_CONFIG".to_owned(), "cpu_temp".to_owned());

        Preset::Looks.apply(&mut tweaks);

        assert_eq!(tweaks.fps_cap, Some(120));
        assert_eq!(tweaks.spell_queue_ms, Some(80));
        assert!(tweaks.raw_mouse);
        assert_eq!(tweaks.graphics_api, Some(GraphicsApi::D3d11));
        assert_eq!(tweaks.env.get("MANGOHUD_CONFIG").map(String::as_str), Some("cpu_temp"));
        // Left alone on purpose: its default already means "on where it
        // exists", so a preset overwriting it would be a silent change.
        assert_eq!(tweaks.game_performance, Some(false));
    }

    #[test]
    fn every_preset_stops_the_addon_profiler() {
        for preset in Preset::ALL {
            let mut tweaks = Tweaks::default();
            preset.apply(&mut tweaks);
            assert!(tweaks.skip_addon_profiling, "{preset:?} left the profiler on");
        }
    }

    // Custom has to leave nothing behind — including the switches a previous
    // preset turned on.
    #[test]
    fn clearing_empties_every_governed_switch() {
        let mut tweaks = Tweaks::default();
        Preset::Frames.apply(&mut tweaks);
        tweaks.clear_managed();

        assert!(!tweaks.gamemode);
        assert!(!tweaks.skip_addon_profiling);
        assert!(!tweaks.reduce_raid_effects);
        assert!(!tweaks.faster_zone_loads);
        assert!(!tweaks.no_screen_effects);
        assert!(!tweaks.max_draw_distance);
    }

    #[test]
    fn only_the_looks_preset_spends_frames_on_scenery() {
        for preset in Preset::ALL {
            let mut tweaks = Tweaks::default();
            preset.apply(&mut tweaks);
            assert_eq!(
                tweaks.max_draw_distance,
                preset == Preset::Looks,
                "{preset:?} got the draw distance wrong"
            );
        }
    }

    // A stock profile matches no preset, which is what the panel shows as
    // "Custom".
    #[test]
    fn a_default_profile_matches_no_preset() {
        assert_eq!(Tweaks::default().preset(), None);
    }

    // The behaviour he asked for: pressing Custom empties the switches, and
    // setting one afterwards must NOT jump the panel onto a preset.
    #[test]
    fn setting_a_switch_after_clearing_stays_off_every_preset() {
        let mut tweaks = Tweaks::default();
        Preset::Looks.apply(&mut tweaks);

        tweaks.clear_managed();
        assert_eq!(tweaks.preset(), None, "a cleared profile is not a preset");

        tweaks.raw_mouse = true;
        tweaks.max_draw_distance = true;
        tweaks.faster_zone_loads = true;
        assert_eq!(tweaks.preset(), None, "ticking boxes by hand is still Custom");
    }

    // Clearing is not a factory reset: the fork and the escape hatch survive
    // it, exactly as they survive a preset.
    #[test]
    fn clearing_preserves_the_escape_hatch_and_the_fork() {
        let mut tweaks = Tweaks {
            fps_cap: Some(120),
            graphics_api: Some(GraphicsApi::D3d11),
            raw_mouse: true,
            ..Default::default()
        };
        tweaks.env.insert("DXVK_HUD".to_owned(), "fps".to_owned());

        tweaks.clear_managed();

        assert_eq!(tweaks.fps_cap, Some(120));
        assert_eq!(tweaks.graphics_api, Some(GraphicsApi::D3d11));
        assert!(tweaks.raw_mouse);
        assert_eq!(tweaks.env.get("DXVK_HUD").map(String::as_str), Some("fps"));
    }

    #[test]
    fn a_default_profile_writes_nothing_and_unsets_everything() {
        let (set, unset) = Tweaks::default().cvars();
        assert!(set.is_empty());
        // Every managed name is offered for removal, so turning the last
        // toggle off actually cleans the file.
        assert_eq!(unset.len(), MANAGED.len());
    }

    #[test]
    fn an_enabled_toggle_is_not_also_unset() {
        let tweaks = Tweaks { skip_addon_profiling: true, ..Tweaks::default() };
        let (set, unset) = tweaks.cvars();
        assert!(set.contains(&("scriptProfile", "0".to_owned())));
        assert!(set.contains(&("addonProfilerEnabled", "0".to_owned())));
        assert!(!unset.contains(&"scriptProfile"));
        assert!(unset.contains(&"ffxGlow"));
    }

    #[test]
    fn every_cvar_a_toggle_writes_is_declared_managed() {
        // Otherwise switching that toggle back off would leave its value
        // behind in Config.wtf forever.
        let all = Tweaks {
            game_performance: Some(true),
            gamemode: true,
            mangohud: true,
            graphics_api: Some(GraphicsApi::D3d11),
            fps_cap: Some(144),
            spell_queue_ms: Some(100),
            skip_addon_profiling: true,
            reduce_raid_effects: true,
            faster_zone_loads: true,
            raw_mouse: true,
            no_screen_effects: true,
            max_draw_distance: true,
            env: Default::default(),
        };
        let (set, unset) = all.cvars();
        assert!(unset.is_empty(), "nothing left to unset when everything is on");
        for (name, _) in &set {
            assert!(MANAGED.contains(name), "{name} is written but not managed");
        }
    }

    #[test]
    fn the_frame_cap_uses_the_clients_own_variable() {
        let tweaks = Tweaks { fps_cap: Some(120), ..Tweaks::default() };
        let (set, _) = tweaks.cvars();
        assert!(set.contains(&("maxFPS", "120".to_owned())));
    }

    #[test]
    fn game_performance_is_on_by_default_but_can_be_refused() {
        assert!(Tweaks::default().wants_game_performance());
        assert!(!Tweaks { game_performance: Some(false), ..Tweaks::default() }
            .wants_game_performance());
    }

    #[test]
    fn a_tool_that_is_not_installed_contributes_no_wrapper() {
        let tweaks = Tweaks { gamemode: true, ..Tweaks::default() };
        assert!(tweaks.wrappers(&HostTools::default()).is_empty());

        let tools = HostTools {
            game_performance: Some("/usr/bin/game-performance".into()),
            gamemode: Some("/usr/bin/gamemoderun".into()),
            ..HostTools::default()
        };
        assert_eq!(
            tweaks.wrappers(&tools),
            vec![
                PathBuf::from("/usr/bin/game-performance"),
                PathBuf::from("/usr/bin/gamemoderun"),
            ]
        );
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
    fn a_user_override_wins_over_a_toggle() {
        let mut env = std::collections::BTreeMap::new();
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

        let wrappers =
            ["/usr/bin/game-performance".into(), "/usr/bin/gamemoderun".into()];
        let outer = wrapped(inner, &wrappers);
        assert_eq!(outer.get_program(), "/usr/bin/game-performance");
        let args: Vec<_> = outer.get_args().collect();
        assert_eq!(args, ["/usr/bin/gamemoderun", "/proton/proton", "run", "/games/Wow.exe"]);
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

    #[test]
    fn tweaks_round_trip_through_toml() {
        let mut env = std::collections::BTreeMap::new();
        env.insert("DXVK_HUD".to_owned(), "fps".to_owned());
        let tweaks = Tweaks {
            graphics_api: Some(GraphicsApi::D3d12),
            fps_cap: Some(165),
            raw_mouse: true,
            env,
            ..Tweaks::default()
        };
        let text = toml::to_string(&tweaks).unwrap();
        assert_eq!(toml::from_str::<Tweaks>(&text).unwrap(), tweaks);
    }
}
