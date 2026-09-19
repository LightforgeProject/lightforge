//! Colors and button styles.
//!
//! The action button's color *is* its state — light blue install, orange
//! update, green play — so the palette is part of the state machine rather
//! than decoration, and lives next to it.

use iced::widget::button;
use iced::{Background, Border, Color, Theme};

pub const INSTALL: Color = rgb(0x4F, 0xC3, 0xF7);
pub const UPDATE: Color = rgb(0xFF, 0x98, 0x00);
pub const PLAY: Color = rgb(0x4C, 0xAF, 0x50);
pub const PAUSE: Color = rgb(0x90, 0xA4, 0xAE);
pub const ERROR: Color = rgb(0xE5, 0x39, 0x35);
/// Blizzard blue, specified for the Battle.net hand-off button.
pub const BATTLE_NET: Color = rgb(0x00, 0xB4, 0xFF);
pub const SUBTEXT: Color = rgb(0x80, 0x80, 0x80);

pub const PANEL: Color = Color::from_rgba(0.06, 0.07, 0.09, 0.82);
pub const TEXT: Color = rgb(0xEC, 0xEF, 0xF1);

const fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::from_rgb(r as f32 / 255.0, g as f32 / 255.0, b as f32 / 255.0)
}

/// Solid button in one of the state colors.
pub fn filled(color: Color) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_theme, status| {
        let base = match status {
            button::Status::Hovered => lighten(color, 0.12),
            button::Status::Pressed => lighten(color, -0.08),
            button::Status::Disabled => Color { a: 0.4, ..color },
            button::Status::Active => color,
        };
        button::Style {
            background: Some(Background::Color(base)),
            text_color: on(color),
            border: Border { radius: 6.0.into(), ..Border::default() },
            ..button::Style::default()
        }
    }
}

/// Low-emphasis button for the utility row.
pub fn quiet(_theme: &Theme, status: button::Status) -> button::Style {
    let alpha = match status {
        button::Status::Hovered | button::Status::Pressed => 0.22,
        _ => 0.10,
    };
    button::Style {
        background: Some(Background::Color(Color { a: alpha, ..TEXT })),
        text_color: TEXT,
        border: Border { radius: 6.0.into(), ..Border::default() },
        ..button::Style::default()
    }
}

/// Black or white text, whichever the background can carry.
fn on(background: Color) -> Color {
    let luma = 0.299 * background.r + 0.587 * background.g + 0.114 * background.b;
    if luma > 0.6 {
        Color::from_rgb(0.05, 0.05, 0.06)
    } else {
        Color::WHITE
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
