//! Handling one message: each area of the app answers for its own.
//!
//! A message is offered to the areas in turn, and the first one whose file
//! handles it answers. A new message goes in the area it belongs to, in that
//! file's `match`; one nobody handles trips the assertion below in a debug
//! build.

use crate::*;

mod products;
mod launch;
mod downloads;
mod addons;
mod game;
mod storage;
mod setup;
mod settings;
mod navigation;
mod report;
mod upgrade;
mod app;

/// What one area does with a message: a task when it is one of its own,
/// the message back when it is not.
type Area = fn(&mut Lightforge, Message) -> Result<Task<Message>, Message>;

impl Lightforge {
    pub(crate) fn handle(&mut self, message: Message) -> Task<Message> {
        const AREAS: [Area; 12] = [
            Lightforge::update_products,
            Lightforge::update_launch,
            Lightforge::update_downloads,
            Lightforge::update_addons,
            Lightforge::update_game,
            Lightforge::update_storage,
            Lightforge::update_setup,
            Lightforge::update_settings,
            Lightforge::update_navigation,
            Lightforge::update_report,
            Lightforge::update_upgrade,
            Lightforge::update_app,
        ];
        let mut message = message;
        for area in AREAS {
            match area(self, message) {
                Ok(task) => return task,
                Err(unclaimed) => message = unclaimed,
            }
        }
        debug_assert!(false, "no area of update/ handles {message:?}");
        Task::none()
    }
}

#[cfg(test)]
mod tests {
    /// Every area's source, for the check below.
    const AREAS: &[(&str, &str)] = &[
        ("products", include_str!("products.rs")),
        ("launch", include_str!("launch.rs")),
        ("downloads", include_str!("downloads.rs")),
        ("addons", include_str!("addons.rs")),
        ("game", include_str!("game.rs")),
        ("storage", include_str!("storage.rs")),
        ("setup", include_str!("setup.rs")),
        ("settings", include_str!("settings.rs")),
        ("navigation", include_str!("navigation.rs")),
        ("report", include_str!("report.rs")),
        ("upgrade", include_str!("upgrade.rs")),
        ("app", include_str!("app.rs")),
    ];

    /// The variant names of `Message`, read from its source.
    fn variants() -> Vec<String> {
        let source = include_str!("../message.rs");
        let body = source.split("enum Message {").nth(1).expect("Message is declared");
        body.lines()
            .take_while(|line| *line != "}")
            .filter(|line| line.starts_with("    ") && !line.starts_with("     "))
            .filter_map(|line| {
                let name: String = line[4..].chars().take_while(|c| c.is_alphanumeric()).collect();
                name.starts_with(|c: char| c.is_ascii_uppercase()).then_some(name)
            })
            .collect()
    }

    /// The router can only find a handler that exists: a message with no arm
    /// in any area would fall through to `Task::none()` in a release build.
    /// This is the compiler's old exhaustiveness check, kept as a test.
    #[test]
    fn every_message_is_handled_by_exactly_one_area() {
        let names = variants();
        assert!(names.len() > 100, "parsed only {} variants", names.len());
        for name in names {
            let arm = format!("            Message::{name}");
            let owners: Vec<&str> = AREAS
                .iter()
                .filter(|(_, source)| {
                    source.lines().any(|line| {
                        line.strip_prefix(arm.as_str())
                            .is_some_and(|rest| !rest.starts_with(|c: char| c.is_alphanumeric()))
                    })
                })
                .map(|(area, _)| *area)
                .collect();
            assert_eq!(owners.len(), 1, "Message::{name} is handled in {owners:?}");
        }
    }
}
