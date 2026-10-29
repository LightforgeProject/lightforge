//! Fitting the UI to the screen when running under gamescope.
//!
//! Without a DPI from the X server, winit derives a scale from the panel's
//! size: 3.25 on a ROG Ally X, 2.25 on a Deck (headless gamescope says 96 dpi,
//! so tests never saw it). Under gamescope winit is pinned to 1 and the UI
//! scales by height against [`DESIGN_HEIGHT`]: Deck 1.0, Ally X 1.35, 4K 2.7.

use std::sync::atomic::{AtomicBool, Ordering};

/// The window height, in points, the console layout is laid out for.
pub const DESIGN_HEIGHT: f32 = 800.0;

/// Set when [`take_over_scale`] pinned winit, read by the app from then on.
static OWNED: AtomicBool = AtomicBool::new(false);

/// Whether this process is a gamescope client.
///
/// Either variable: a session script may overwrite `XDG_CURRENT_DESKTOP`.
pub fn under_gamescope() -> bool {
    std::env::var_os("GAMESCOPE_WAYLAND_DISPLAY").is_some()
        || std::env::var("XDG_CURRENT_DESKTOP").is_ok_and(|d| d.eq_ignore_ascii_case("gamescope"))
}

/// Pin winit's scale to 1 under gamescope, unless the user set one.
///
/// Before any thread starts: it writes the environment. A user's own
/// `WINIT_X11_SCALE_FACTOR` is left alone.
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
/// Never below 1. Rounded down to a twentieth, with slack so reading the
/// height back through the old scale cannot flip it.
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
