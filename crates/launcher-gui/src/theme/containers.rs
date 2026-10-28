//! Container styles: the backdrop, cards, rows, menus, dialogs, scrollbars, and the small marks.

use iced::widget::{button, container, scrollable};
use iced::{gradient, Background, Border, Color, Radians, Theme};

use super::*;

/// The window's own background: [`BG`] with a wash of [`ACCENT`] in the
/// bottom right, the one corner nothing else stands in. It fades out a third
/// of the way across. Linear because iced has no radial gradient, and a
/// diagonal fade reads the same at this strength.
pub fn backdrop(_theme: &Theme) -> container::Style {
    const LIT: Color = mix(ACCENT, BG, 0.09);
    container::Style {
        background: Some(Background::Gradient(
            // 315° clockwise from north: out of the bottom right corner,
            // towards the top left.
            gradient::Linear::new(Radians(std::f32::consts::FRAC_PI_4 * 7.0))
                .add_stop(0.0, LIT)
                .add_stop(0.32, BG)
                .into(),
        )),
        ..container::Style::default()
    }
}

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
/// [`LIFT`] separates it from the art; the [`BORDER_OVER_ART`] hairline draws
/// the edge a shadow leaves vague over bright art.
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

/// A dropdown's own surface, the app menu, the overflow list, an addon row's
/// menu.
///
/// Like [`floating`] but opaque: key art shows through a short entry at
/// [`PANEL`]'s alpha.
pub fn menu(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(SURFACE)),
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
/// Only without the game band. Its own element: a [`Border`] is all four sides.
pub fn ribbon_edge(_theme: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BORDER_OVER_ART)),
        ..container::Style::default()
    }
}

/// An on/off switch, for a setting that takes effect the moment it is
/// flipped. Accent when on, a quiet track when off, the knob always light.
pub fn switch(_theme: &Theme, status: iced::widget::toggler::Status) -> iced::widget::toggler::Style {
    use iced::widget::toggler::Status;

    let (on, hovered, disabled) = match status {
        Status::Active { is_toggled } => (is_toggled, false, false),
        Status::Hovered { is_toggled } => (is_toggled, true, false),
        Status::Disabled { is_toggled } => (is_toggled, false, true),
    };
    let track = match (on, hovered) {
        (true, false) => ACCENT,
        (true, true) => Color { a: 0.85, ..ACCENT },
        (false, false) => Color::from_rgba(1.0, 1.0, 1.0, 0.14),
        (false, true) => Color::from_rgba(1.0, 1.0, 1.0, 0.22),
    };
    let fade = |c: Color| match disabled {
        true => Color { a: c.a * 0.4, ..c },
        false => c,
    };
    iced::widget::toggler::Style {
        background: Background::Color(fade(track)),
        background_border_width: 0.0,
        background_border_color: Color::TRANSPARENT,
        foreground: Background::Color(fade(TEXT)),
        foreground_border_width: 0.0,
        foreground_border_color: Color::TRANSPARENT,
        text_color: None,
        border_radius: None,
        padding_ratio: 0.12,
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

/// [`row`] as a button, lit whole on hover (see [`card_button`]).
pub fn row_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    // A twentieth of the way to the text colour: the same small lift as
    // `ghost`'s hover wash, mixed in because a row is opaque.
    let lift = |c: f32, t: f32| c + (t - c) * 0.05;
    button::Style {
        background: Some(Background::Color(match hovered {
            true => Color {
                r: lift(RAISED.r, TEXT.r),
                g: lift(RAISED.g, TEXT.g),
                b: lift(RAISED.b, TEXT.b),
                a: RAISED.a,
            },
            false => RAISED,
        })),
        text_color: TEXT,
        border: Border {
            color: match hovered {
                true => Color { a: 0.24, ..TEXT },
                false => BORDER,
            },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
        ..button::Style::default()
    }
}

/// The shape of a scrollbar: narrow, and embedded rather than floating.
///
/// With [`SM`] spacing it takes its own space, not drawn over row buttons.
pub fn scrollbar_shape() -> scrollable::Scrollbar {
    scrollable::Scrollbar::new().width(6.0).scroller_width(6.0).spacing(SM)
}

/// A scrollbar: no rail, and a scroller that is a hairline made solid.
///
/// The light-theme rail is dropped; the scroller is [`BORDER`]'s white.
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

/// A state worn at a button's size: same height, same corners, no press.
///
/// Where a button would stand ("installed" beside Install); [`badge`] is for
/// inside text. Not a disabled button, which iced greys out.
pub fn tag(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_theme| container::Style {
        background: Some(Background::Color(Color { a: 0.18, ..color })),
        text_color: Some(color),
        border: Border {
            color: Color { a: 0.45, ..color },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
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
