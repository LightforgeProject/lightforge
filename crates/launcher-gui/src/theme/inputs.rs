//! Text fields and the multi-line editor.

use iced::widget::{text_editor, text_input};
use iced::{Background, Border, Color, Theme};

use super::*;

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

/// The multi-line twin of [`field`], for prose rather than a value.
///
/// Same surface, same border states, a report's description should look like
/// the field above it and simply be taller.
pub fn editor(_theme: &Theme, status: text_editor::Status) -> text_editor::Style {
    let border = match status {
        text_editor::Status::Focused { .. } => ACCENT,
        text_editor::Status::Hovered => Color { a: 0.18, ..TEXT },
        _ => BORDER,
    };
    text_editor::Style {
        background: Background::Color(RAISED),
        border: Border { color: border, width: 1.0, radius: RADIUS_SM.into() },
        placeholder: MUTED,
        value: TEXT,
        selection: Color { a: 0.30, ..ACCENT },
    }
}
