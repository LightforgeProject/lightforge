//! Fitting the UI to the screen.
//!
//! Without a DPI from the X server, winit derives a scale from the panel's
//! size: 3.25 on a ROG Ally X, 2.25 on a Deck (headless gamescope says 96 dpi,
//! so tests never saw it). Under gamescope winit is pinned to 1 and the UI
//! scales by height against [`DESIGN_HEIGHT`]: Deck 1.0, Ally X 1.35, 4K 2.7.
//!
//! On a desktop the scale is the chosen interface size on top of the
//! desktop's own, and the window grows with it: the layout is drawn for
//! [`crate::WINDOW`] points, so a larger UI in the same window would not fit.

use iced::{Point, Size};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

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

/// The interface sizes on offer, in percent.
pub const INTERFACE_SIZES: [u16; 7] = [90, 100, 110, 125, 150, 175, 200];

/// How much of the monitor the window may cover: winit reports the whole
/// monitor, panels included, and has no work area.
const SCREEN_SHARE: f32 = 0.9;

/// Below this the text is unreadable; only a bogus monitor size gets here.
const MIN_SCALE: f32 = 0.5;

/// The desktop scale the window opens at, for [`centred`]; `f32` bits.
static OPENING: AtomicU32 = AtomicU32::new(0x3f80_0000);

/// Record the scale the window is about to open at.
pub fn open_at(scale: f32) {
    OPENING.store(scale.to_bits(), Ordering::Relaxed);
}

/// Where the window opens on a monitor `monitor` points big.
///
/// iced centres the size it was given, but opens the window at that size
/// times the app's scale, so its own `Centered` is off by the difference.
pub fn centred(window: Size, monitor: Size) -> Point {
    let window = window * f32::from_bits(OPENING.load(Ordering::Relaxed));
    Point::new(
        ((monitor.width - window.width) / 2.0).max(0.0),
        ((monitor.height - window.height) / 2.0).max(0.0),
    )
}

/// The desktop scale for an interface size of `chosen` on a monitor that is
/// `monitor` points big at the desktop's own scale.
///
/// Less than chosen when the window would not fit, below 1 included: a
/// 1080p screen at 150% has 720 points for an 820-point window.
pub fn desktop_scale(chosen: f32, monitor: Size) -> f32 {
    let window = crate::WINDOW;
    let fits = (monitor.width * SCREEN_SHARE / window.width)
        .min(monitor.height * SCREEN_SHARE / window.height);
    let fits = (fits * 20.0 + 1e-3).floor() / 20.0;
    chosen.min(fits).max(MIN_SCALE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_size_that_fits_is_kept() {
        assert_eq!(desktop_scale(1.0, Size::new(1920.0, 1080.0)), 1.0);
        assert_eq!(desktop_scale(1.5, Size::new(2560.0, 1440.0)), 1.5);
        assert_eq!(desktop_scale(0.9, Size::new(1920.0, 1080.0)), 0.9);
    }

    #[test]
    fn a_size_that_would_outgrow_the_screen_stops_at_it() {
        assert_eq!(desktop_scale(2.0, Size::new(1920.0, 1080.0)), 1.15);
    }

    #[test]
    fn a_screen_smaller_than_the_window_shrinks_it() {
        assert_eq!(desktop_scale(1.0, Size::new(1280.0, 720.0)), 0.75, "1080p at 150%");
        assert_eq!(desktop_scale(1.0, Size::new(1366.0, 768.0)), 0.8);
    }

    #[test]
    fn the_window_opens_centred_at_its_scaled_size() {
        open_at(1.5);
        let at = centred(Size::new(1280.0, 820.0), Size::new(2560.0, 1440.0));
        assert_eq!(at, Point::new(320.0, 105.0));
    }

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
