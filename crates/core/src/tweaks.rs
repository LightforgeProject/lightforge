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
    /// Background frame cap — the client's `maxFPSBk`. `None` leaves the
    /// client's own default, which is 30.
    ///
    /// Wins CPU back for everything else on the machine while the window is
    /// not focused, which on Linux is most of a queue.
    pub fps_cap_background: Option<u32>,
    /// Turn cloth and NPC physics off (`physicsLevel 0`).
    ///
    /// The client's own levels are 0 disabled, 1 player only (the default)
    /// and 2 player and NPC, so this is a step below stock rather than a
    /// feature being withheld.
    pub no_cloth_physics: bool,
    /// Draw a software cursor instead of a hardware one (`HardwareCursor 0`).
    ///
    /// Under Wine the hardware cursor is the one that goes missing, lags the
    /// pointer, or fails to appear in screenshots.
    pub software_cursor: bool,
    /// Keep the pointer inside the game's window (`clipcursor 1`).
    ///
    /// The fix for the cursor walking onto a second monitor mid-fight, which
    /// is a multi-monitor problem rather than a game one.
    pub clip_cursor: bool,
    /// Ignore the desktop's mouse acceleration curve
    /// (`mouseAcceleration 0`).
    pub no_mouse_acceleration: bool,
    /// Show the cursor in relative mode (`MouseHiddenInRelativeMode 0`).
    ///
    /// The client offers this as a workaround in its own words — "disabling
    /// may work around issues in relative mode in some rare cases".
    pub show_cursor_in_relative_mode: bool,
    /// Run on Proton's Wayland backend instead of through XWayland.
    ///
    /// Proton itself ignores this unless `WAYLAND_DISPLAY` is set, and so
    /// does [`Tweaks::environment`] — see [`HostTools::wayland`].
    pub wayland: bool,
    /// Ask for HDR output, which Proton turns into `DXVK_HDR`.
    ///
    /// Only offered alongside [`Tweaks::wayland`]: HDR through XWayland is
    /// not a path anybody should be sent down.
    pub hdr: bool,
    /// Which monitor the game should treat as its primary, by connector name
    /// (`DP-1`). Only honoured on the Wayland backend.
    pub wayland_monitor: Option<String>,
    /// Give the driver's shader cache room to hold a whole expansion.
    ///
    /// Mesa's default ceiling is small enough that a long session evicts
    /// shaders it is about to need again, which is felt as stutter that
    /// never quite goes away.
    pub big_shader_cache: bool,
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
    "useMaxFPS",
    "maxFPSBk",
    "useMaxFPSBk",
    "scriptProfile",
    "RAIDWaterDetail",
    "RAIDweatherDensity",
    "worldPreloadNonCritical",
    "rawMouseEnable",
    "ffxGlow",
    "ffxDeath",
    "farclip",
    "groundEffectDensity",
    "groundEffectDist",
    "physicsLevel",
    "HardwareCursor",
    "clipcursor",
    "mouseAcceleration",
    "MouseHiddenInRelativeMode",
];

/// Names Lightforge used to write and no longer does.
///
/// They stay here because [`Tweaks::cvars`] is the complete description of
/// this product's settings: a name that is neither set nor offered for removal
/// is a value left behind in `Config.wtf` forever. Both of these were written
/// by earlier versions, and both were mistakes found by checking the client's
/// own string table rather than a wiki:
///
/// - `addonProfilerEnabled` is not a console variable at all, in any casing.
///   `scriptProfile` is the real one and is still set.
/// - `SpellQueueWindow` is real, but the client keeps it in the
///   account-synced `WTF/Account/<id>/config-cache.wtf`, not in `Config.wtf`.
///   Writing it here was writing to the wrong file, so the control was
///   dropped rather than left looking like it worked.
const RETIRED: &[&str] = &["addonProfilerEnabled", "SpellQueueWindow"];

/// The lowest frame cap the client accepts, from its own help text: "Set FPS
/// limit. Min 8". A smaller number is clamped rather than refused — the panel
/// should not be able to write a value the client will throw away.
pub const FPS_FLOOR: u32 = 8;

/// What [`Tweaks::big_shader_cache`] raises the driver's cache ceiling to.
///
/// Mesa takes a byte count with an optional `K`/`M`/`G` suffix. Ten gigabytes
/// is chosen to be larger than any one expansion's shaders and still small
/// enough that nobody notices it on a modern disk.
const SHADER_CACHE_SIZE: &str = "10G";

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
        // Both halves, or neither does anything. The client has a separate
        // flag per cap — "Enables or disables FPS limit" beside "Set FPS
        // limit. Min 8" — and writing the number alone is what an earlier
        // version did, which is why the cap never took effect.
        if let Some(cap) = self.fps_cap {
            set.push(("maxFPS", cap.max(FPS_FLOOR).to_string()));
            set.push(("useMaxFPS", "1".to_owned()));
        }
        if let Some(cap) = self.fps_cap_background {
            set.push(("maxFPSBk", cap.max(FPS_FLOOR).to_string()));
            set.push(("useMaxFPSBk", "1".to_owned()));
        }
        if self.skip_addon_profiling {
            set.push(("scriptProfile", "0".to_owned()));
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
        if self.no_cloth_physics {
            set.push(("physicsLevel", "0".to_owned()));
        }
        if self.software_cursor {
            set.push(("HardwareCursor", "0".to_owned()));
        }
        if self.clip_cursor {
            set.push(("clipcursor", "1".to_owned()));
        }
        if self.no_mouse_acceleration {
            set.push(("mouseAcceleration", "0".to_owned()));
        }
        if self.show_cursor_in_relative_mode {
            set.push(("MouseHiddenInRelativeMode", "0".to_owned()));
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
            .chain(RETIRED.iter())
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
        // Proton's own Wayland backend. It ignores the request when
        // `WAYLAND_DISPLAY` is unset, and so do we, so the variable is never
        // handed to a session that cannot use it.
        if self.wayland && tools.wayland {
            env.push(("PROTON_ENABLE_WAYLAND".into(), "1".into()));
            if let Some(name) = &self.wayland_monitor {
                env.push(("PROTON_WAYLAND_MONITOR".into(), name.into()));
            }
            // Gated on the backend rather than offered beside it: Proton
            // turns this into `DXVK_HDR` whatever the driver is, and HDR
            // through XWayland is not a path to send anybody down.
            if self.hdr {
                env.push(("PROTON_ENABLE_HDR".into(), "1".into()));
            }
        }
        if self.big_shader_cache {
            // Mesa's variable, verified against the installed RADV rather
            // than taken from a wiki — it is also the one Mesa itself names
            // when it deprecates `MESA_GLSL_CACHE_MAX_SIZE`. There is no
            // confirmed equivalent for the proprietary NVIDIA driver, so
            // this covers Mesa drivers and says nothing about the rest.
            env.push(("MESA_SHADER_CACHE_MAX_SIZE".into(), SHADER_CACHE_SIZE.into()));
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
    /// Whether this is a Wayland session, which is what decides whether
    /// Proton's Wayland backend can be offered at all.
    pub wayland: bool,
}

impl HostTools {
    pub fn detect() -> Self {
        Self {
            game_performance: on_path("game-performance"),
            gamemode: on_path("gamemoderun"),
            mangohud: on_path("mangohud"),
            wayland: std::env::var_os("WAYLAND_DISPLAY").is_some(),
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
/// trade frames against scenery. It never touches the cursor and mouse
/// settings (personal feel, or a Wine workaround, not speed), nor
/// [`Tweaks::graphics_api`], the two frame caps, [`Tweaks::mangohud`] or
/// [`Tweaks::env`] (a genuine fork, two ceilings, an overlay and an escape
/// hatch — none of them a point on this axis). It leaves the runtime switches
/// alone for the same reason: which graphics backend Proton uses is not a
/// position on the frames-versus-scenery axis either. A preset that silently
/// cleared somebody's environment variables would be a data-loss bug wearing
/// the clothes of a convenience.
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
            Self::Frames => "Raid effects, world preloading, glow and cloth physics off. View distance normal.",
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
                tweaks.no_cloth_physics = true;
                tweaks.max_draw_distance = false;
            }
            Self::Balanced => {
                tweaks.gamemode = false;
                tweaks.reduce_raid_effects = true;
                tweaks.faster_zone_loads = false;
                tweaks.no_screen_effects = false;
                tweaks.no_cloth_physics = false;
                tweaks.max_draw_distance = false;
            }
            Self::Looks => {
                tweaks.gamemode = false;
                tweaks.reduce_raid_effects = false;
                tweaks.faster_zone_loads = false;
                tweaks.no_screen_effects = false;
                tweaks.no_cloth_physics = false;
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
        self.no_cloth_physics = false;
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
    fn governed(&self) -> (bool, bool, bool, bool, bool, bool, bool) {
        (
            self.gamemode,
            self.skip_addon_profiling,
            self.reduce_raid_effects,
            self.faster_zone_loads,
            self.no_screen_effects,
            self.no_cloth_physics,
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
        tweaks.fps_cap_background = Some(10);
        tweaks.clip_cursor = true;
        tweaks.software_cursor = true;
        tweaks.wayland = true;
        tweaks.big_shader_cache = true;
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
            raw_mouse: true,
            graphics_api: Some(GraphicsApi::D3d11),
            game_performance: Some(false),
            ..Default::default()
        };
        tweaks.env.insert("MANGOHUD_CONFIG".to_owned(), "cpu_temp".to_owned());

        Preset::Looks.apply(&mut tweaks);

        assert_eq!(tweaks.fps_cap, Some(120));
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
        assert_eq!(unset.len(), MANAGED.len() + RETIRED.len());
    }

    #[test]
    fn an_enabled_toggle_is_not_also_unset() {
        let tweaks = Tweaks { skip_addon_profiling: true, ..Tweaks::default() };
        let (set, unset) = tweaks.cvars();
        assert!(set.contains(&("scriptProfile", "0".to_owned())));
        // `addonProfilerEnabled` is not a console variable in any casing —
        // see [`RETIRED`]. It is offered for removal, never written.
        assert!(!set.iter().any(|(n, _)| *n == "addonProfilerEnabled"));
        assert!(unset.contains(&"addonProfilerEnabled"));
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
            fps_cap_background: Some(20),
            skip_addon_profiling: true,
            reduce_raid_effects: true,
            faster_zone_loads: true,
            raw_mouse: true,
            no_screen_effects: true,
            no_cloth_physics: true,
            software_cursor: true,
            clip_cursor: true,
            no_mouse_acceleration: true,
            show_cursor_in_relative_mode: true,
            max_draw_distance: true,
            wayland: true,
            hdr: true,
            wayland_monitor: Some("DP-1".to_owned()),
            big_shader_cache: true,
            env: Default::default(),
        };
        let (set, unset) = all.cvars();
        // Only the retired names are left, because nothing sets them any
        // more — and they must still be cleaned out of `Config.wtf`.
        assert_eq!(unset, RETIRED, "only the retired names are left to unset");
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

    // The shape a config written before this batch has on disk. Every new
    // field has to default, or an upgrade silently discards settings somebody
    // chose — including an explicit `game_performance = false`, which is not
    // the same as the default and cannot be re-derived.
    #[test]
    fn a_config_from_before_this_batch_still_loads() {
        let old = r#"
game_performance = false
gamemode = false
mangohud = false
skip_addon_profiling = true
reduce_raid_effects = false
faster_zone_loads = false
raw_mouse = false
no_screen_effects = false
max_draw_distance = false
fps_cap = 144
spell_queue_ms = 80

[env]
DXVK_HUD = "fps"
"#;
        let tweaks: Tweaks = toml::from_str(old).unwrap();

        // Kept, and the explicit "no" is still a no.
        assert_eq!(tweaks.game_performance, Some(false));
        assert!(!tweaks.wants_game_performance());
        assert!(tweaks.skip_addon_profiling);
        assert_eq!(tweaks.fps_cap, Some(144));
        assert_eq!(tweaks.env.get("DXVK_HUD").map(String::as_str), Some("fps"));

        // Defaulted, not refused — an unknown key from a newer or older
        // build must never fail the load, which is what `serde(default)` on
        // the struct buys. `spell_queue_ms` above is exactly that case now.
        assert_eq!(tweaks.fps_cap_background, None);
        assert!(!tweaks.no_cloth_physics);
        assert!(!tweaks.wayland);
        assert!(!tweaks.big_shader_cache);
    }

    // The client's own floor: "Set FPS limit. Min 8". Clamped rather than
    // refused, so the panel cannot write a number the client discards.
    #[test]
    fn a_frame_cap_below_the_clients_floor_is_raised_to_it() {
        let tweaks = Tweaks { fps_cap: Some(3), fps_cap_background: Some(1), ..Tweaks::default() };
        let (set, _) = tweaks.cvars();
        assert!(set.contains(&("maxFPS", "8".to_owned())));
        assert!(set.contains(&("maxFPSBk", "8".to_owned())));
    }

    // Both halves or neither. Writing the number without the flag is what an
    // earlier version did, and it is why the cap never took effect.
    #[test]
    fn a_frame_cap_writes_its_enable_flag_too() {
        let (set, unset) = Tweaks { fps_cap: Some(120), ..Tweaks::default() }.cvars();
        assert!(set.contains(&("maxFPS", "120".to_owned())));
        assert!(set.contains(&("useMaxFPS", "1".to_owned())));
        // The background pair is untouched and therefore cleaned out.
        assert!(unset.contains(&"maxFPSBk"));
        assert!(unset.contains(&"useMaxFPSBk"));
    }

    #[test]
    fn the_background_cap_is_independent_of_the_foreground_one() {
        let (set, unset) =
            Tweaks { fps_cap_background: Some(30), ..Tweaks::default() }.cvars();
        assert!(set.contains(&("maxFPSBk", "30".to_owned())));
        assert!(set.contains(&("useMaxFPSBk", "1".to_owned())));
        assert!(unset.contains(&"maxFPS"));
        assert!(unset.contains(&"useMaxFPS"));
    }

    // Verified against the client's own string table: `HardwareCursor`
    // exists and `gxCursor` does not, and 0 is the software cursor.
    #[test]
    fn the_cursor_and_mouse_fixes_write_what_the_client_calls_them() {
        let tweaks = Tweaks {
            software_cursor: true,
            clip_cursor: true,
            no_mouse_acceleration: true,
            show_cursor_in_relative_mode: true,
            ..Tweaks::default()
        };
        let (set, _) = tweaks.cvars();
        assert!(set.contains(&("HardwareCursor", "0".to_owned())));
        assert!(set.contains(&("clipcursor", "1".to_owned())));
        assert!(set.contains(&("mouseAcceleration", "0".to_owned())));
        assert!(set.contains(&("MouseHiddenInRelativeMode", "0".to_owned())));
    }

    // Proton ignores the Wayland backend without `WAYLAND_DISPLAY`, so an
    // X11 session is never handed a variable that cannot do anything.
    #[test]
    fn the_wayland_backend_needs_a_wayland_session() {
        let tweaks = Tweaks { wayland: true, hdr: true, ..Tweaks::default() };
        assert!(tweaks.environment(&HostTools::default()).is_empty());

        let tools = HostTools { wayland: true, ..HostTools::default() };
        let env = tweaks.environment(&tools);
        assert!(env.contains(&(
            OsString::from("PROTON_ENABLE_WAYLAND"),
            OsString::from("1")
        )));
        assert!(env.contains(&(OsString::from("PROTON_ENABLE_HDR"), OsString::from("1"))));
    }

    // HDR rides on the backend rather than sitting beside it: Proton would
    // happily set `DXVK_HDR` under XWayland, which is not a path to offer.
    #[test]
    fn hdr_alone_sets_nothing() {
        let tweaks = Tweaks { hdr: true, ..Tweaks::default() };
        let tools = HostTools { wayland: true, ..HostTools::default() };
        assert!(tweaks.environment(&tools).is_empty());
    }

    #[test]
    fn the_monitor_is_only_named_on_the_wayland_backend() {
        let tools = HostTools { wayland: true, ..HostTools::default() };

        let named = Tweaks {
            wayland: true,
            wayland_monitor: Some("DP-1".to_owned()),
            ..Tweaks::default()
        };
        assert!(named
            .environment(&tools)
            .contains(&(OsString::from("PROTON_WAYLAND_MONITOR"), OsString::from("DP-1"))));

        // Without the backend the name means nothing, so it is not sent.
        let backendless =
            Tweaks { wayland_monitor: Some("DP-1".to_owned()), ..Tweaks::default() };
        assert!(backendless.environment(&tools).is_empty());
    }

    #[test]
    fn the_shader_cache_ceiling_is_raised_for_any_session() {
        let tweaks = Tweaks { big_shader_cache: true, ..Tweaks::default() };
        // Not gated on a Wayland session or on any binary being present: it
        // is read by the driver, which is always there.
        let env = tweaks.environment(&HostTools::default());
        assert_eq!(
            env,
            vec![(
                OsString::from("MESA_SHADER_CACHE_MAX_SIZE"),
                OsString::from(SHADER_CACHE_SIZE)
            )]
        );
    }

    // The runtime switches are environment-only. A product with no client
    // settings at all must still write nothing into `Config.wtf`.
    #[test]
    fn the_runtime_switches_write_no_client_settings() {
        let tweaks = Tweaks {
            wayland: true,
            hdr: true,
            big_shader_cache: true,
            wayland_monitor: Some("DP-1".to_owned()),
            ..Tweaks::default()
        };
        let (set, _) = tweaks.cvars();
        assert!(set.is_empty());
    }

    // Only the Frames preset gives up cloth physics — it is a frames-for-
    // detail trade, which is exactly what the axis is.
    #[test]
    fn only_the_frames_preset_turns_cloth_physics_off() {
        for preset in Preset::ALL {
            let mut tweaks = Tweaks::default();
            preset.apply(&mut tweaks);
            assert_eq!(
                tweaks.no_cloth_physics,
                preset == Preset::Frames,
                "{preset:?} got cloth physics wrong"
            );
        }
    }

    // Why the Battle.net profile in Settings offers no client settings: a
    // hand-off game has no `Config.wtf`, and the settings that would write
    // one contribute nothing to the environment, which is the only thing
    // Lightforge can give that launch. A control for them would be inert.
    #[test]
    fn client_settings_alone_contribute_no_environment() {
        let tweaks = Tweaks {
            graphics_api: Some(GraphicsApi::D3d12),
            fps_cap: Some(144),
            fps_cap_background: Some(10),
            raw_mouse: true,
            software_cursor: true,
            clip_cursor: true,
            no_cloth_physics: true,
            reduce_raid_effects: true,
            ..Tweaks::default()
        };
        let tools = HostTools { wayland: true, ..HostTools::default() };
        assert!(tweaks.environment(&tools).is_empty());
        // And they are not silently dropped — they are simply the other half.
        assert!(!tweaks.cvars().0.is_empty());
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
