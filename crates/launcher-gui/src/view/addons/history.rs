//! What update passes did to this game's addons, newest first.

use crate::*;
use iced::widget::column;
use lightforge_core::addons::history::Trigger;
use lightforge_core::addons::manage::Change;

impl Lightforge {
    pub(crate) fn addons_history(&self) -> Element<'_, Message> {
        if self.addons.history.is_empty() {
            return empty(
                "No addon updates yet.",
                "Every update, automatic or by hand, is listed here with the versions it went between.",
            );
        }

        let runs = self.addons.history.iter().map(|run| {
            // "manual" already means auto-update off on My Addons.
            let trigger = match run.trigger {
                Trigger::Automatic => pill("automatic".to_owned(), theme::ACCENT),
                Trigger::Manual => pill("by hand".to_owned(), theme::MUTED),
            };
            let mut card = column![row![text(run.when()).size(theme::LABEL), trigger]
                .spacing(theme::SM)
                .align_y(Alignment::Center)]
            .spacing(theme::XS);

            for change in &run.updated {
                card = card.push(
                    row![
                        text(change.name.clone()).size(theme::BODY).width(Length::Fill),
                        mono(versions(change)),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                );
            }
            for problem in &run.problems {
                card = card.push(text(problem.clone()).size(theme::CAPTION).color(theme::WARN));
            }
            container(card).padding(theme::MD).style(theme::card).width(Length::Fill).into()
        });

        column(runs).spacing(theme::SM).into()
    }
}

/// `v1 → v2`. Not every author bumps `## Version`, and some leave it out.
fn versions(change: &Change) -> String {
    match (&change.from, &change.to) {
        (Some(from), Some(to)) if from != to => format!("{from} → {to}"),
        (_, Some(to)) => format!("{to}, new build"),
        (Some(from), None) => format!("{from} → new build"),
        (None, None) => "new build".to_owned(),
    }
}
