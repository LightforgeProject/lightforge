//! Button styles. The filled ones carry the state colours: rule 2 in [`super`].

use iced::widget::button;
use iced::{border, Background, Border, Color, Theme};

use super::*;

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
/// Rounded on the outer edges only; the tab is a shade darker instead of a border.
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

/// Outlined button, secondary actions that still deserve a border.
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

/// Borderless button, navigation and low-stakes actions.
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

/// A whole card that is also a button.
///
/// [`card`] as a button: the whole card lights on hover. Translucent with
/// [`BORDER_OVER_ART`], like [`floating`].
pub fn card_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        // [`RAISED`] on hover, at PANEL's alpha.
        background: Some(Background::Color(match hovered {
            true => Color { a: PANEL.a, ..RAISED },
            false => Color { a: PANEL.a, ..SURFACE },
        })),
        text_color: TEXT,
        border: Border {
            color: match hovered {
                true => Color { a: 0.24, ..TEXT },
                false => BORDER_OVER_ART,
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

/// A dropdown's face, for a picker with our own menu (see `view::menus`).
pub fn picker_button(_theme: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered | button::Status::Pressed);
    button::Style {
        background: Some(Background::Color(match hovered {
            true => RAISED,
            false => SURFACE,
        })),
        text_color: TEXT,
        border: Border {
            color: match hovered {
                true => BORDER_OVER_ART,
                false => BORDER,
            },
            width: 1.0,
            radius: RADIUS_SM.into(),
        },
        ..button::Style::default()
    }
}
