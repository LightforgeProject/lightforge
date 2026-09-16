//! Lightforge GUI.
//!
//! Milestone 1 is a compiling shell only — it proves the workspace wires up and
//! that `core`'s API is usable from iced. The real dashboard, theming, and the
//! action state machine land in Milestone 5.

use iced::widget::{column, text};
use iced::{Element, Task};
use lightforge_core::paths::Layout;

fn main() -> iced::Result {
    iced::application("Lightforge", Lightforge::update, Lightforge::view)
        .run_with(|| (Lightforge::new(), Task::none()))
}

#[derive(Debug, Clone)]
enum Message {}

struct Lightforge {
    /// Resolved layout, or the reason it couldn't be resolved.
    root: String,
}

impl Lightforge {
    fn new() -> Self {
        let root = match Layout::default_for_user() {
            Ok(layout) => layout.root().display().to_string(),
            Err(err) => format!("unavailable: {err}"),
        };
        Self { root }
    }

    fn update(&mut self, message: Message) {
        match message {}
    }

    fn view(&self) -> Element<'_, Message> {
        column![
            text("Lightforge").size(32),
            text(format!("Blizzard root: {}", self.root)).size(14),
            text("Milestone 1 — scaffolding only").size(12),
        ]
        .spacing(12)
        .padding(24)
        .into()
    }
}
