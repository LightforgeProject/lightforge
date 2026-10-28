//! The design system: tokens first, components built from them.
//!
//! 1. **Nothing hard-codes a colour, size or gap**: everything comes from the
//!    scales below.
//! 2. **The action button's colour is state**: blue install, violet update,
//!    gold play, the only saturated colours.
//! 3. **Machine text is monospaced**: versions, paths, ids.

use iced::{Color, Font, Shadow, Theme, Vector};

mod buttons;
mod containers;
mod inputs;

#[cfg(test)]
mod tests;

pub use buttons::*;
pub use containers::*;
pub use inputs::*;

// ---------------------------------------------------------------- palette --

/// Page background, near black, slightly blue, so art laid behind it reads as
/// part of the page rather than a picture on a grey wall.
pub const BG: Color = rgb(0x0B, 0x0D, 0x10);

/// A panel sitting on the page.
pub const SURFACE: Color = rgb(0x14, 0x17, 0x1C);

/// A panel on a panel, list rows, inputs.
pub const RAISED: Color = rgb(0x1B, 0x20, 0x27);

/// Hairline. Always a translucent white so it works over art too.
pub const BORDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);

/// Hairline for a panel laid over artwork, where a hairline meant for a flat
/// surface disappears. Draws the edge that [`LIFT`]'s shadow cannot, see
/// [`floating`].
pub const BORDER_OVER_ART: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.16);

pub const TEXT: Color = rgb(0xE8, 0xED, 0xF2);

/// Secondary text. Contrast-checked against [`SURFACE`], not against black.
pub const MUTED: Color = rgb(0x8A, 0x94, 0xA3);

/// Blizzard blue, the single accent, and the Battle.net hand-off colour.
pub const ACCENT: Color = rgb(0x00, 0xB4, 0xFF);

pub const INSTALL: Color = rgb(0x4F, 0xC3, 0xF7);

/// An update waiting: violet, distinct at a glance from [`PLAY`]'s gold in
/// the same spot.
pub const UPDATE: Color = rgb(0xA7, 0x8B, 0xFA);

/// The launch button, and nothing else.
///
/// The midpoint of the wordmark's two golds (`#FABD2F`, `#D79921`).
pub const PLAY: Color = rgb(0xE9, 0xAB, 0x28);

/// Done, healthy, present: a status, never an action.
pub const OK: Color = rgb(0x4C, 0xAF, 0x50);

pub const PAUSE: Color = rgb(0x78, 0x86, 0x96);

pub const ERROR: Color = rgb(0xE5, 0x73, 0x73);

/// Panel over artwork. Opaque enough to read against a bright background,
/// translucent enough that the art still shows through.
pub const PANEL: Color = Color::from_rgba(0.043, 0.055, 0.070, 0.88);

// ------------------------------------------------------------------ scales --

/// Height of a primary control, so the halves of a split button match exactly.
///
/// Pinned: text and an 18px icon would otherwise size the halves differently.
pub const CONTROL_HEIGHT: f32 = 56.0;

/// Spacing scale. Four values cover every gap on every screen; a fifth would
/// only invite eyeballing.
pub const XS: f32 = 4.0;

pub const SM: f32 = 8.0;

pub const MD: f32 = 16.0;

pub const LG: f32 = 24.0;

pub const XL: f32 = 32.0;

/// Type scale.
pub const DISPLAY: f32 = 34.0;

pub const TITLE: f32 = 22.0;

pub const HEADING: f32 = 16.0;

pub const BODY: f32 = 14.0;

pub const LABEL: f32 = 12.0;

pub const CAPTION: f32 = 11.0;

/// The box a product's wordmark is drawn in, under the app title.
///
/// A limit, not a size ([`logo_size`] sizes); only StarCraft's marks reach it.
pub const LOGO_BOX_WIDTH: f32 = 450.0;

pub const LOGO_BOX_HEIGHT: f32 = 280.0;

/// The width a wordmark is centred against before it is left-aligned.
///
/// The eye places a block's middle, so a narrower mark is nudged in by half
/// the difference. Roughly the median drawn width.
pub const LOGO_ANCHOR_WIDTH: f32 = 350.0;

/// How far a mark of `width` is nudged in from the box's left edge.
pub fn logo_offset(width: f32) -> f32 {
    ((LOGO_ANCHOR_WIDTH - width) / 2.0).max(0.0)
}

/// How far in from the page's own inset the wordmark sits.
///
/// Small: the marks are trimmed to their ink and would touch the edge.
pub const LOGO_INSET: f32 = XL;

/// How much of the page one product wordmark covers, in square points.
///
/// Area is what the eye reads as size, across marks from 5:1 to square. The
/// largest that fits every mark in the box; StarCraft's sets it.
const LOGO_AREA: f32 = 56_000.0;

/// The least of a logo [`logo_size`] will believe is the mark rather than its
/// glow.
///
/// A floor on the divisor; the lowest real one is Retail's 0.59.
const LOGO_MIN_INK: f32 = 0.4;

/// How wide and tall to draw one wordmark, from the shape of its trimmed ink.
///
/// [`LOGO_AREA`] by `aspect`, grown by `1 / ink`
/// ([`crate::art::Art::logo_ink`]), then scaled down to fit the box.
pub fn logo_size(aspect: f32, ink: f32) -> (f32, f32) {
    let aspect = match aspect.is_finite() && aspect > 0.0 {
        true => aspect,
        false => 1.0,
    };
    let ink = match ink.is_finite() {
        true => ink.clamp(LOGO_MIN_INK, 1.0),
        false => 1.0,
    };
    let area = LOGO_AREA / ink;
    let (width, height) = ((area * aspect).sqrt(), (area / aspect).sqrt());
    let fit = (LOGO_BOX_WIDTH / width).min(LOGO_BOX_HEIGHT / height).min(1.0);
    (width * fit, height * fit)
}

/// Edge of a product tile in the header's product bar. Square, so this is
/// both dimensions; the art inside is drawn to fit.
///
pub const TILE: f32 = 40.0;

/// Drawn for a Steam Deck or Steam Machine. Set by the view each frame, so
/// the builders below need not be passed it.
static CONSOLE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_console(on: bool) {
    CONSOLE.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn console() -> bool {
    CONSOLE.load(std::sync::atomic::Ordering::Relaxed)
}

/// A product tile's edge: [`TILE`], or a thumb's worth more on a console.
pub fn tile() -> f32 {
    match console() {
        true => 52.0,
        false => TILE,
    }
}

/// Padding for something pressed, `[vertical, horizontal]`: about 42pt tall
/// on a console, as Steam's Game Mode buttons are.
pub fn tap(padding: [f32; 2]) -> [f32; 2] {
    match console() {
        true => [padding[0] + 5.0, padding[1] + XS],
        false => padding,
    }
}

/// Text on something pressed: a step up on a console, where the screen is
/// seven inches across or across the room.
pub fn tap_text(size: f32) -> f32 {
    match console() {
        true => size + 2.0,
        false => size,
    }
}

// A manifest's per-art `height` is parsed but ignored: [`logo_size`] sizes
// by area.

/// Corner radius. One value for panels, one for controls.
pub const RADIUS: f32 = 10.0;

pub const RADIUS_SM: f32 = 6.0;

/// The one drop shadow, for a surface that floats above the page.
///
/// One shadow: a panel is part of the page or over it. Black at low alpha
/// (grey reads as fog on [`BG`]), offset less than the blur.
pub const LIFT: Shadow = Shadow {
    color: Color::from_rgba(0.0, 0.0, 0.0, 0.45),
    offset: Vector::new(0.0, 4.0),
    blur_radius: 20.0,
};

/// Face for versions, paths and ids.
pub const MONO: Font = Font::MONOSPACE;

/// The emphasis weight, for the words that name a place rather than describe
/// one: the page tabs and the band's label.
///
pub const BOLD: Font = Font {
    family: iced::font::Family::SansSerif,
    weight: iced::font::Weight::Bold,
    stretch: iced::font::Stretch::Normal,
    style: iced::font::Style::Normal,
};

// The scales are invariants, so they are checked when the crate compiles
// rather than when a test happens to run.
const _: () = assert!(XS < SM && SM < MD && MD < LG && LG < XL);

const _: () = assert!(CAPTION < LABEL && LABEL < BODY && BODY < HEADING);

const _: () = assert!(HEADING < TITLE && TITLE < DISPLAY);

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// The base iced theme, so built-in widgets (inputs, pick lists, scrollbars)
/// share the palette instead of being restyled one by one.
pub fn base() -> Theme {
    Theme::custom(
        "Lightforge".to_owned(),
        iced::theme::Palette {
            background: BG,
            text: TEXT,
            primary: ACCENT,
            success: OK,
            // 0.14 added a warning slot. Ours is the update colour: an update
            // waiting is the only thing in Lightforge that warns.
            warning: UPDATE,
            danger: ERROR,
        },
    )
}

/// `a` laid over `b` at `amount`, resolved now rather than by the compositor.
///
/// So both backends agree: a translucent gradient stop blends against clear.
const fn mix(a: Color, b: Color, amount: f32) -> Color {
    Color {
        r: b.r + (a.r - b.r) * amount,
        g: b.g + (a.g - b.g) * amount,
        b: b.b + (a.b - b.b) * amount,
        a: 1.0,
    }
}

/// Black or white text, whichever the background can carry.
fn on(background: Color) -> Color {
    let luma = 0.299 * background.r + 0.587 * background.g + 0.114 * background.b;
    match luma > 0.6 {
        true => rgb(0x0B, 0x0D, 0x10),
        false => Color::WHITE,
    }
}

fn lighten(color: Color, amount: f32) -> Color {
    Color {
        r: (color.r + amount).clamp(0.0, 1.0),
        g: (color.g + amount).clamp(0.0, 1.0),
        b: (color.b + amount).clamp(0.0, 1.0),
        a: color.a,
    }
}
