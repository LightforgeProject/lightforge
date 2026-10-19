//! Fitting the UI to the screen when running under gamescope.
//!
//! Steam's Game Mode runs Lightforge as an X11 client of gamescope, and when
//! the X server sets no DPI, winit derives a scale factor from the panel's
//! physical size. On a 7" handheld that is enormous: 3.25 on a ROG Ally X
//! (1920x1080), 2.25 on a Steam Deck (1280x800), which leaves a layout of
//! under 600x360 points and a dashboard squeezed until its controls lose
//! their text. Headless gamescope reports a nominal 96 dpi, which is why no
//! test rig ever showed it.
//!
//! So under gamescope Lightforge pins winit to 1 and scales the UI itself, by
//! the window's height against [`DESIGN_HEIGHT`], the height the console
//! layout was built and checked at. A Deck lands on exactly 1.0, an Ally X
//! on 1.35, a 4K television on 2.7, and every one of them sees the same
//! layout.

use std::sync::atomic::{AtomicBool, Ordering};

/// The window height, in points, the console layout is laid out for.
pub const DESIGN_HEIGHT: f32 = 800.0;

/// Set when [`take_over_scale`] pinned winit, read by the app from then on.
static OWNED: AtomicBool = AtomicBool::new(false);

/// Whether this process is a gamescope client.
///
/// gamescope exports both to every client it starts (checked by running a
/// child under it). Either one is enough: a session script may overwrite
/// `XDG_CURRENT_DESKTOP`.
pub fn under_gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
}

/// Pin winit's scale to 1 under gamescope, unless the user set one.
///
/// Must run before anything starts a thread: it writes the environment, and
/// winit reads it when the window is created. A `WINIT_X11_SCALE_FACTOR`
/// someone set in Steam's launch options is theirs and is left alone, with
/// the app's own scale then staying at 1.
pub fn take_over_scale() {
    if !under_gamescope() || std::env::var_os("WINIT_X11_SCALE_FACTOR").is_some() {
        return;
    }
    std::env::set_var("WINIT_X11_SCALE_FACTOR", "1");
    OWNED.store(true, Ordering::Relaxed);
}

/// Whether the app scales itself, see [`take_over_scale`].
pub fn owned() -> bool {
    OWNED.load(Ordering::Relaxed)
}

/// The UI scale for a window `physical_height` pixels tall.
///
/// Never below 1: a window shorter than the design height (a per-game
/// resolution someone chose in Steam) gets a tighter layout rather than
/// text smaller than the desktop's. Rounded *down* to a twentieth, so the
/// layout is never shorter than the design height, and with a little slack,
/// so the height read back through the previous scale cannot flip it.
pub fn scale_for(physical_height: f32) -> f32 {
    ((physical_height / DESIGN_HEIGHT * 20.0 + 1e-3).floor() / 20.0).max(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_handheld_gets_the_design_height() {
        assert_eq!(scale_for(800.0), 1.0, "Steam Deck");
        assert_eq!(scale_for(1080.0), 1.35, "ROG Ally X, Legion Go S");
        assert_eq!(scale_for(1600.0), 2.0, "Legion Go");
        assert_eq!(scale_for(2160.0), 2.7, "a 4K television");
        assert_eq!(scale_for(720.0), 1.0, "never below the desktop's size");
        // A windowed launcher at its own size stays as it is designed.
        assert_eq!(scale_for(820.0), 1.0);
    }

    #[test]
    fn the_scale_settles_when_read_back_through_itself() {
        // The window's height arrives in points, divided by the scale in
        // effect; multiplying back must land on the same answer.
        for pixels in [800.0, 1080.0, 1200.0, 1600.0, 2160.0] {
            let first = scale_for(pixels);
            assert_eq!(scale_for(pixels / first * first), first, "{pixels}");
        }
    }
}
