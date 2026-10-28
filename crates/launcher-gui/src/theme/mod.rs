//! The design system: tokens first, components built from them.
//!
//! Three rules hold the four screens together:
//!
//! 1. **Nothing hard-codes a colour, size or gap.** Everything comes from the
//!    scales below, so a change lands everywhere at once instead of in one
//!    screen.
//! 2. **The action button's colour is state, not decoration**: light blue
//!    install, orange update, green play. Those four are the only saturated
//!    colours on screen, which is what makes them readable at a glance.
//! 3. **Machine text is monospaced**: versions, paths, ids. It stops a
//!    version number from being mistaken for prose, and makes columns line up
//!    without a table.
//!
//! iced gives no cascade, so "restraint" here means type, spacing and one
//! accent rather than effects.

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

/// An update waiting. Violet rather than the orange it used to be: orange and
/// [`PLAY`]'s gold are the same hue to the eye at a glance, and Update and Play
/// are the two buttons that occupy the same spot on the panel, a player who
/// reads the colour before the word has to be able to tell them apart. Violet
/// is the one strong hue the palette had not already spent.
pub const UPDATE: Color = rgb(0xA7, 0x8B, 0xFA);

/// The launch button, and nothing else.
///
/// Taken from the wordmark: it is the midpoint of the two golds the mark is
/// drawn in (`#FABD2F` and `#D79921`), so the one button the eye goes to first
/// is wearing the app's own colour. It replaced a green that read as a traffic
/// light next to a gold mark.
pub const PLAY: Color = rgb(0xE9, 0xAB, 0x28);

/// Done, healthy, present, a status, never an action. This is the green
/// [`PLAY`] used to be, kept for the things that only report: the "installed"
/// pill, a finished setup step, the dot that says the install is shared.
pub const OK: Color = rgb(0x4C, 0xAF, 0x50);

pub const PAUSE: Color = rgb(0x78, 0x86, 0x96);

pub const ERROR: Color = rgb(0xE5, 0x73, 0x73);

/// Panel over artwork. Opaque enough to read against a bright background,
/// translucent enough that the art still shows through.
pub const PANEL: Color = Color::from_rgba(0.043, 0.055, 0.070, 0.88);

// ------------------------------------------------------------------ scales --

/// Height of a primary control, so the halves of a split button match exactly.
///
/// Pinned rather than left to content: the action half is sized by a line of
/// HEADING text and the menu half by an 18px icon, and letting each shrink to
/// its own content leaves the tab visibly short of the button it belongs to.
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
/// A limit, not a size: [`logo_size`] sizes the mark, and only the two
/// StarCraft marks reach this width (equal area would make StarCraft 601pt
/// against Retail's 349, which reads as louder). Nothing reaches the height;
/// Retail's, the tallest, is 270.
pub const LOGO_BOX_WIDTH: f32 = 450.0;

pub const LOGO_BOX_HEIGHT: f32 = 280.0;

/// The width a wordmark is centred against before it is left-aligned.
///
/// Left-aligned alone, every mark starts on the same line but the narrow ones
/// read as *further* left than the wide ones, because what the eye places is
/// the middle of the block and not its edge: Forever, the narrowest in the
/// catalogue at 263, sits its centre a good 90pt left of StarCraft's. So a
/// mark narrower than this is nudged in by half the difference, and one at
/// least this wide simply starts at the edge. Roughly the median drawn width,
/// which is what leaves most marks untouched.
pub const LOGO_ANCHOR_WIDTH: f32 = 350.0;

/// How far a mark of `width` is nudged in from the box's left edge.
pub fn logo_offset(width: f32) -> f32 {
    ((LOGO_ANCHOR_WIDTH - width) / 2.0).max(0.0)
}

/// How far in from the page's own inset the wordmark sits.
///
/// Small: the marks are trimmed to their own ink, so flush with the page edge
/// meant *touching* it, and this is the margin that stops that, not a place
/// the wordmark was moved to. The box is centred on what this leaves, so a
/// narrow mark still sits further in than a wide one starts.
pub const LOGO_INSET: f32 = XL;

/// How much of the page one product wordmark covers, in square points.
///
/// Area, because that is what the eye reads as size: the marks are trimmed
/// to their ink and range from five times wider than tall to nearly square,
/// so one bounding box made the square ones look half the size. The number
/// is the largest that keeps every mark inside [`LOGO_BOX_WIDTH`] by
/// [`LOGO_BOX_HEIGHT`]; StarCraft's sets it.
const LOGO_AREA: f32 = 56_000.0;

/// The least of a logo [`logo_size`] will believe is the mark rather than its
/// glow.
///
/// Dividing by the ink fraction is dividing by a number below one, so a
/// mistaken reading, art that is genuinely almost all soft edge, would ask
/// for a wordmark several times the size of every other. A floor costs
/// nothing on the art we have, where the lowest is Retail's 0.59.
const LOGO_MIN_INK: f32 = 0.4;

/// How wide and tall to draw one wordmark, from the shape of its trimmed ink.
///
/// [`LOGO_AREA`] decides the size; `aspect` decides how that area is spent;
/// `ink` is the share of the image that is the mark rather than its halo (see
/// [`crate::art::Art::logo_ink`]), and the box is grown by the inverse of it
/// so that the *mark* ends up at the standard area with its glow spilling
/// past.
///
/// The result is then scaled down, never up, if it would leave the box, so
/// a mark of some shape nobody has published yet is made smaller rather than
/// allowed off the page.
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
/// Sized for a row of its own under the tabs rather than a seat in them: when
/// the band shared the ribbon it had to stay near the height of a button, and
/// it no longer does. Not as large as that freedom allows, though, the band
/// is a card over the artwork now, and a card is sized by what it holds.
pub const TILE: f32 = 40.0;

/// Drawn for a Steam Deck or Steam Machine: held in a hand or read from a
/// sofa, pressed with a thumb or a controller. Set by the view at the start
/// of each frame, from the answer setup asked for on SteamOS, so the shared
/// builders below can read it without every page passing it down.
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

/// Padding for something pressed, `[vertical, horizontal]`. On a console a
/// button grows to about 42 points tall, the size Steam's own Game Mode
/// buttons are on the same screen, from the 32 a mouse is happy with.
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

// A manifest may still carry a `height` per art set. Nothing reads it any
// more, and the field is kept only so older manifests still parse.
//
// It meant "draw this art set's logo at this many pixels tall", from when
// every logo was drawn on its publisher's own canvas and a height was the
// only lever there was for making two of them look alike. It never worked,
// a height is not a size when the shapes differ, which is why every row in
// the published table carries the same number, and against [`logo_size`] it
// does active harm: capping Retail at the table's 210 undoes most of what
// measuring its mark rather than its halo just bought it.

/// Corner radius. One value for panels, one for controls.
pub const RADIUS: f32 = 10.0;

pub const RADIUS_SM: f32 = 6.0;

/// The one drop shadow, for a surface that floats above the page.
///
/// A single value rather than a scale, because depth here is binary: a panel
/// is either part of the page or laid over it. Two shadows would imply a
/// hierarchy among floating things that the four screens do not have.
///
/// Tuned for a near-black page, pure black at a low alpha, since a grey
/// shadow over [`BG`] reads as fog rather than depth, and a downward offset
/// smaller than the blur so the panel lifts without appearing to tilt.
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
/// Spelled out rather than built from [`Font::DEFAULT`], because struct update
/// syntax is not allowed in a `const`.
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
/// A gradient paints the whole window on its own, so a translucent stop would
/// be asking the renderer to blend against a clear colour it has already
/// drawn. Mixing the two here keeps the result the same on both backends.
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
