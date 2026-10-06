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

use iced::widget::{button, container, overlay, pick_list, scrollable, text_input};
use iced::{border, Background, Border, Color, Font, Shadow, Theme, Vector};

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
/// surface disappears. Draws the edge that [`LIFT`]'s shadow cannot — see
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
/// Height of a primary control, so the halves of a split button match exactly.
///
/// Pinned rather than left to content: the action half is sized by a line of
/// HEADING text and the menu half by an 18px icon, and letting each shrink to
/// its own content leaves the tab visibly short of the button it belongs to.
pub const CONTROL_HEIGHT: f32 = 56.0;

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
///
/// Sized for a row of its own under the tabs rather than a seat in them: when
/// the band shared the ribbon it had to stay near the height of a button, and
/// it no longer does. Not as large as that freedom allows, though — the band
/// is a card over the artwork now, and a card is sized by what it holds.
pub const TILE: f32 = 40.0;

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

/// The one drop shadow, for a surface that floats above the page.
///
/// A single value rather than a scale, because depth here is binary: a panel
/// is either part of the page or laid over it. Two shadows would imply a
/// hierarchy among floating things that the four screens do not have.
///
/// Tuned for a near-black page — pure black at a low alpha, since a grey
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
            // 0.14 added a warning slot. Ours is the update colour: an update
            // waiting is the only thing in Lightforge that warns.
            warning: UPDATE,
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
/// Carries [`LIFT`]. Through 0.13 it could not: the software renderer sized a
/// dirty region from the quad's bounds expanded by a single pixel, with no
/// account of a shadow's blur radius or offset, so the area under a shadow
/// never got cleared and the shadow composited over itself on every hover
/// until it went black (iced-rs/iced#2712). #2715 fixed it, and 0.14 has it.
///
/// The brighter hairline stays alongside it. The two do different jobs — the
/// hairline draws the edge, the shadow separates the panel from what is
/// behind it — and over bright artwork a shadow alone leaves the top edge
/// indistinct, which is the case [`BORDER_OVER_ART`] exists for.
pub fn floating(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: BORDER_OVER_ART,
            width: 1.0,
            radius: RADIUS.into(),
        },
        shadow: LIFT,
        ..container::Style::default()
    }
}

/// The dimmed window behind a dialog: the page stays visible, but plainly
/// not the thing to interact with.
pub fn scrim(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(Color { a: 0.72, ..BG })),
        ..container::Style::default()
    }
}

/// A dialog's own card: opaque, unlike [`floating`], since nothing behind it
/// should show through the text.
pub fn dialog(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
        border: Border { color: BORDER_OVER_ART, width: 1.0, radius: RADIUS.into() },
        ..container::Style::default()
    }
}

/// The hairline closing the header off from the content below it.
///
/// Only on the screens without the game band: on the dashboard the band's own
/// card does the separating. Its own element because a [`Border`] applies to
/// all four sides and only the bottom edge wants a line here.
///
/// (There was a `ribbon` here — an edge-to-edge panel behind the whole header.
/// It went when the band moved out into a card of its own: the bar existed to
/// give the band and the title a readable ground, and the card now carries its
/// own. Worth remembering why it was there, though — the header sits directly
/// on product artwork, and loose text over a bright sky is the thing to watch.)
pub fn ribbon_edge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_OVER_ART)),
        ..container::Style::default()
    }
}

/// A hairline between sections, and between Settings' list and its page.
pub fn divider(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER)),
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

/// The shape of a scrollbar: narrow, and embedded rather than floating.
///
/// iced's default is 10pt wide and drawn *over* the content, which on a list
/// whose rows end in a button puts the scroller on top of the button. Giving
/// it [`SM`] of spacing makes it take layout space of its own, so it sits
/// beside the rows and the list below is simply that much narrower.
pub fn scrollbar_shape() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(6.0).scroller_width(6.0).spacing(SM)
}

/// A scrollbar: no rail, and a scroller that is a hairline made solid.
///
/// Unstyled, iced draws a light grey rail that belongs to a light theme. The
/// rail is dropped entirely — the gutter is already empty space — and the
/// scroller is [`BORDER`]'s white at an alpha that reads over both a panel and
/// artwork, brightening under the pointer the way the buttons do.
pub fn scrollbar(_theme: &Theme, status: scrollable::Status) -> scrollable::Style {
    let hovered = matches!(
        status,
        scrollable::Status::Hovered { is_vertical_scrollbar_hovered: true, .. }
            | scrollable::Status::Dragged { is_vertical_scrollbar_dragged: true, .. }
    );
    let rail = scrollable::Rail {
        background: None,
        border: Border::default(),
        scroller: scrollable::Scroller {
            background: Background::Color(Color {
                a: match hovered {
                    true => 0.34,
                    false => 0.18,
                },
                ..TEXT
            }),
            border: Border { radius: 999.0.into(), ..Border::default() },
        },
    };
    scrollable::Style {
        container: container::Style::default(),
        vertical_rail: rail,
        horizontal_rail: rail,
        gap: None,
        ..scrollable::default(_theme, status)
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

/// A solid round mark, for a status a glyph can't carry: the UI font has no
/// check mark, and renders U+2713 as a tofu box.
pub fn dot(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(color)),
        border: Border { radius: 999.0.into(), ..Border::default() },
        ..container::Style::default()
    }
}

// ----------------------------------------------------------------- buttons --

/// Solid button in one of the state colours. The primary action, at most once
/// per screen.
pub fn filled(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| button::Style {
        border: Border { radius: RADIUS_SM.into(), ..Border::default() },
        ..filled_base(color, status)
    }
}

/// The two halves of a split button: a wide primary action and a narrow menu
/// tab welded to its right, the way Battle.net's Play control is built.
///
/// Rounded on their outer edges only, so the pair reads as one control rather
/// than as two buttons that happen to touch. The tab is shaded a little darker
/// than the action so the divide is visible without a border between them —
/// a border would read as a seam, which is exactly what a split button is
/// trying not to be.
pub fn filled_left(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| button::Style {
        border: Border {
            radius: border::radius(RADIUS_SM).top_right(0).bottom_right(0),
            ..Border::default()
        },
        ..filled_base(color, status)
    }
}

pub fn filled_right(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let tab = lighten(color, -0.10);
        button::Style {
            border: Border {
                radius: border::radius(RADIUS_SM).top_left(0).bottom_left(0),
                ..Border::default()
            },
            ..filled_base(tab, status)
        }
    }
}

/// Fill and text for a solid button, without committing to a corner radius.
fn filled_base(color: Color, status: button::Status) -> button::Style {
    let base = match status {
        button::Status::Hovered => lighten(color, 0.08),
        button::Status::Pressed => lighten(color, -0.06),
        button::Status::Disabled => Color { a: 0.35, ..color },
        button::Status::Active => color,
    };
    button::Style {
        background: Some(Background::Color(base)),
        text_color: on(color),
        ..button::Style::default()
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

/// A whole card that is also a link — the News page's headlines.
///
/// [`card`] as a button rather than a container: the card *is* the click
/// target, so the hover has to happen on the card itself. A row-sized hover
/// highlight inside a static card would say the row is pressable and leave
/// the rest of the card looking like it is not.
pub fn card_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        // [`RAISED`] on hover — the existing "a panel on a panel" step, so a
        // hovered card lifts by the same amount as every other surface in the
        // app rather than by a brightness invented here.
        background: Some(Background::Color(match hovered {
            true => RAISED,
            false => SURFACE,
        })),
        text_color: TEXT,
        border: Border {
            color: match hovered {
                true => Color { a: 0.24, ..TEXT },
                false => BORDER,
            },
            width: 1.0,
            radius: RADIUS.into(),
        },
        ..button::Style::default()
    }
}

/// The selected tab or nav entry: filled quietly, never with the accent, so it
/// does not compete with the primary action.
pub fn selected(_theme: &Theme, status: button::Status) -> button::Style {
    // Only a lit tab that is also a way back (Play, from a game's own pages)
    // can be hovered; the current tab has no action and reports `Disabled`.
    let a = match status {
        button::Status::Hovered | button::Status::Pressed => 0.22,
        _ => 0.14,
    };
    button::Style {
        background: Some(Background::Color(Color { a, ..ACCENT })),
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
    let hovered =
        matches!(status, pick_list::Status::Hovered | pick_list::Status::Opened { .. });
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
        // 0.14 fixed the software renderer's shadow damage (iced-rs/iced#2715),
        // so a shadow here is safe again — but the menu never had one, and
        // adding one is a design decision rather than part of this port.
        shadow: iced::Shadow::default(),
    }
}

// ------------------------------------------------------------------ inputs --

pub fn field(_theme: &Theme, status: text_input::Status) -> text_input::Style {
    let border = match status {
        text_input::Status::Focused { .. } => ACCENT,
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

