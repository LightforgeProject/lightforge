//! The design system: tokens first, components built from them.
//!
//! Three rules hold the four screens together:
//!
//! 1. **Nothing hard-codes a colour, size or gap.** Everything comes from the
//!    scales below, so a change lands everywhere at once instead of in one
//!    screen.
//! 2. **The action button's colour is state, not decoration** — light blue
//!    install, orange update, green play. Those four are the only saturated
//!    colours on screen, which is what makes them readable at a glance.
//! 3. **Machine text is monospaced** — versions, paths, ids. It stops a
//!    version number from being mistaken for prose, and makes columns line up
//!    without a table.
//!
//! iced gives no cascade, so "restraint" here means type, spacing and one
//! accent rather than effects.

use iced::widget::{button, container, overlay, pick_list, text_input};
use iced::{Background, Border, Color, Font, Theme};

// ---------------------------------------------------------------- palette --

/// Page background — near black, slightly blue, so art laid behind it reads as
/// part of the page rather than a picture on a grey wall.
pub const BG: Color = rgb(0x0B, 0x0D, 0x10);
/// A panel sitting on the page.
pub const SURFACE: Color = rgb(0x14, 0x17, 0x1C);
/// A panel on a panel — list rows, inputs.
pub const RAISED: Color = rgb(0x1B, 0x20, 0x27);
/// Hairline. Always a translucent white so it works over art too.
pub const BORDER: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.08);
/// Hairline for a panel laid over artwork, where a hairline meant for a flat
/// surface disappears. Carries the separation a drop shadow used to — see
/// [`floating`].
pub const BORDER_OVER_ART: Color = Color::from_rgba(1.0, 1.0, 1.0, 0.16);

pub const TEXT: Color = rgb(0xE8, 0xED, 0xF2);
/// Secondary text. Contrast-checked against [`SURFACE`], not against black.
pub const MUTED: Color = rgb(0x8A, 0x94, 0xA3);

/// Blizzard blue — the single accent, and the Battle.net hand-off colour.
pub const ACCENT: Color = rgb(0x00, 0xB4, 0xFF);
pub const INSTALL: Color = rgb(0x4F, 0xC3, 0xF7);
pub const UPDATE: Color = rgb(0xFF, 0x98, 0x00);
pub const PLAY: Color = rgb(0x4C, 0xAF, 0x50);
pub const PAUSE: Color = rgb(0x78, 0x86, 0x96);
pub const ERROR: Color = rgb(0xE5, 0x73, 0x73);

/// Panel over artwork. Opaque enough to read against a bright background,
/// translucent enough that the art still shows through.
pub const PANEL: Color = Color::from_rgba(0.043, 0.055, 0.070, 0.88);

// ------------------------------------------------------------------ scales --

/// Spacing scale. Four values cover every gap on every screen; a fifth would
/// only invite eyeballing.
pub const XS: u16 = 4;
pub const SM: u16 = 8;
pub const MD: u16 = 16;
pub const LG: u16 = 24;
pub const XL: u16 = 32;

/// Type scale.
pub const DISPLAY: u16 = 34;
pub const TITLE: u16 = 22;
pub const HEADING: u16 = 16;
pub const BODY: u16 = 14;
pub const LABEL: u16 = 12;
pub const CAPTION: u16 = 11;

/// Product wordmark on the dashboard, sitting under the app title.
///
/// Large on purpose: the artwork behind it is the product's identity and the
/// wordmark is the label on it, so it is a headline rather than a footnote.
///
/// A manifest may override this per art set — a wide wordmark and a squarish
/// one drawn to the same height read as different sizes. [`logo_height`]
/// decides what an override is allowed to be.
pub const LOGO: f32 = 170.0;

/// Edge of a product tile in the header's product bar. Square, so this is
/// both dimensions; the art inside is drawn to fit.
pub const TILE: f32 = 38.0;

/// What a manifest's logo height hint is allowed to ask for.
///
/// The manifest is fetched over the network, so a typo or a bad publish must
/// not be able to push the wordmark off screen or shrink it to nothing. Below
/// the floor it stops being legible; above the ceiling it crowds the action
/// panel at [`crate::WINDOW_MIN`]'s height, which is where the room runs out.
const LOGO_RANGE: std::ops::RangeInclusive<f32> = 60.0..=440.0;

/// The height to draw a product's logo at: its hint, clamped, else [`LOGO`].
///
/// A hint that isn't a usable number at all — negative, zero, NaN — is treated
/// as absent rather than clamped, since there is no reading of it to honour.
pub fn logo_height(hint: Option<f32>) -> f32 {
    match hint {
        Some(height) if height.is_finite() && height > 0.0 => {
            height.clamp(*LOGO_RANGE.start(), *LOGO_RANGE.end())
        }
        _ => LOGO,
    }
}

/// Corner radius. One value for panels, one for controls.
pub const RADIUS: f32 = 10.0;
pub const RADIUS_SM: f32 = 6.0;

/// Face for versions, paths and ids.
pub const MONO: Font = Font::MONOSPACE;

// The scales are invariants, so they are checked when the crate compiles
// rather than when a test happens to run.
const _: () = assert!(XS < SM && SM < MD && MD < LG && LG < XL);
const _: () = assert!(CAPTION < LABEL && LABEL < BODY && BODY < HEADING);
const _: () = assert!(HEADING < TITLE && TITLE < DISPLAY);
const _: () = assert!(*LOGO_RANGE.start() <= LOGO && LOGO <= *LOGO_RANGE.end());

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
            success: PLAY,
            danger: ERROR,
        },
    )
}

// -------------------------------------------------------------- containers --

/// A panel: the default home for a group of controls.
pub fn card(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER, width: 1.0, radius: RADIUS.into() },
        ..container::Style::default()
    }
}

/// A panel that sits over artwork.
///
/// Deliberately **no drop shadow**. The software renderer's damage tracking
/// sizes a dirty region from the quad's bounds expanded by a single pixel,
/// with no account of a shadow's blur radius or offset, so the area under a
/// shadow never gets cleared and the shadow composites over itself on every
/// hover until it goes black (iced-rs/iced#2712, fixed upstream by #2715,
/// which landed after the 0.13 we build against).
///
/// A brighter hairline does the separating instead. Revisit when we move to
/// iced 0.14: the shadow can come back as-is.
pub fn floating(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: BORDER_OVER_ART,
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..container::Style::default()
    }
}

/// The header band, running edge to edge across the top of every screen.
///
/// The band is the point. On the dashboard the header sits directly on
/// product artwork, and a title over a bright sky is unreadable — so it gets
/// the same ground the action panel stands on, which also brackets the art
/// between two matching strips instead of letting it run under loose text.
pub fn ribbon(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        ..container::Style::default()
    }
}

/// The hairline closing the band off from the artwork below it.
///
/// Its own element because a [`Border`] applies to all four sides and only
/// the bottom edge wants a line here.
pub fn ribbon_edge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_OVER_ART)),
        ..container::Style::default()
    }
}

/// One row in a list. Quiet enough to repeat forty times.
pub fn row(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(RAISED)),
        border: Border { color: BORDER, width: 1.0, radius: RADIUS_SM.into() },
        ..container::Style::default()
    }
}

/// A pill carrying one word of status.
pub fn badge(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(Color { a: 0.16, ..color })),
        text_color: Some(color),
        border: Border { radius: 999.0.into(), ..Border::default() },
        ..container::Style::default()
    }
}

// ----------------------------------------------------------------- buttons --

/// Solid button in one of the state colours. The primary action, at most once
/// per screen.
pub fn filled(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let base = match status {
            button::Status::Hovered => lighten(color, 0.08),
            button::Status::Pressed => lighten(color, -0.06),
            button::Status::Disabled => Color { a: 0.35, ..color },
            button::Status::Active => color,
        };
        button::Style {
            background: Some(Background::Color(base)),
            text_color: on(color),
            border: Border { radius: RADIUS_SM.into(), ..Border::default() },
            ..button::Style::default()
        }
    }
}

/// Outlined button — secondary actions that still deserve a border.
pub fn outlined(_theme: &Theme, status: button::Status) -> button::Style {
    let (fill, border) = match status {
        button::Status::Hovered | button::Status::Pressed => {
            (Color { a: 0.10, ..TEXT }, Color { a: 0.24, ..TEXT })
        }
        button::Status::Disabled => (Color::TRANSPARENT, Color { a: 0.05, ..TEXT }),
        button::Status::Active => (Color::TRANSPARENT, BORDER),
    };
    button::Style {
        background: Some(Background::Color(fill)),
        text_color: match status {
            button::Status::Disabled => MUTED,
            _ => TEXT,
        },
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        ..button::Style::default()
    }
}

/// Borderless button — navigation and low-stakes actions.
pub fn ghost(_theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Hovered | button::Status::Pressed => 0.10,
        _ => 0.0,
    };
    button::Style {
        background: Some(Background::Color(Color { a: alpha, ..TEXT })),
        text_color: match status {
            button::Status::Disabled => MUTED,
            _ => TEXT,
        },
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// The selected tab or nav entry: filled quietly, never with the accent, so it
/// does not compete with the primary action.
pub fn selected(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: Some(Background::Color(Color { a: 0.14, ..ACCENT })),
        text_color: ACCENT,
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// The branch picker, and the list it drops down.
///
/// iced's default is a light control, which was tolerable when the picker sat
/// on the page and is not once it sits inside the header band. Both halves are
/// styled: the closed control and its overlay menu are separate types, and
/// styling only one leaves a pale list hanging off a dark button.
pub fn picker(_theme: &Theme, status: pick_list::Status) -> pick_list::Style {
    let hovered = matches!(status, pick_list::Status::Hovered | pick_list::Status::Opened);
    pick_list::Style {
        background: Background::Color(match hovered {
            true => RAISED,
            false => SURFACE,
        }),
        text_color: TEXT,
        placeholder_color: MUTED,
        handle_color: MUTED,
        border: Border {
            color: match hovered {
                true => BORDER_OVER_ART,
                false => BORDER,
            },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
    }
}

/// The open list behind [`picker`].
pub fn picker_menu(_theme: &Theme) -> overlay::menu::Style {
    overlay::menu::Style {
        background: Background::Color(SURFACE),
        text_color: TEXT,
        border: Border { color: BORDER, width: 1.0, radius: RADIUS_SM.into() },
        selected_background: Background::Color(Color { a: 0.18, ..ACCENT }),
        selected_text_color: TEXT,
    }
}

// ------------------------------------------------------------------ inputs --

pub fn field(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border = match status {
        text_input::Status::Focused => ACCENT,
        text_input::Status::Hovered => Color { a: 0.18, ..TEXT },
        _ => BORDER,
    };
    text_input::Style {
        background: Background::Color(RAISED),
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        icon: MUTED,
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.30, ..ACCENT },
    }
}

// ------------------------------------------------------------------ helpers --

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_on_a_state_colour_stays_readable() {
        // Light blue install needs dark text; green play needs white. Getting
        // this backwards makes the one button that matters unreadable.
        assert!(on(INSTALL).r < 0.5, "light backgrounds need dark text");
        assert_eq!(on(PLAY), Color::WHITE);
        assert!(on(UPDATE).r < 0.5, "orange is light enough for dark text");
    }

    #[test]
    fn a_missing_hint_is_the_default() {
        assert_eq!(logo_height(None), LOGO);
    }

    #[test]
    fn a_sane_hint_is_honoured() {
        assert_eq!(logo_height(Some(150.0)), 150.0);
    }

    #[test]
    fn a_wild_hint_is_clamped_rather_than_obeyed() {
        assert_eq!(logo_height(Some(5_000.0)), *LOGO_RANGE.end());
        assert_eq!(logo_height(Some(3.0)), *LOGO_RANGE.start());
    }

    #[test]
    fn a_hint_that_is_not_a_height_falls_back() {
        for nonsense in [0.0, -120.0, f32::NAN, f32::INFINITY] {
            assert_eq!(logo_height(Some(nonsense)), LOGO, "{nonsense}");
        }
    }
}

