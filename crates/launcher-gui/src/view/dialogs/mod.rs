//! The modal cards: install, uninstall, versions, reports and the rest.

use crate::*;
use iced::widget::column;

mod bug_report;
mod installs;
mod versions;

impl Lightforge {
    /// Ask for the open Game Settings popup's backups.
    pub(crate) fn list_backups(&self) -> Task<Message> {
        match &self.game_settings {
            Some(settings) => Task::perform(
                tasks::list_backups(self.layout.clone(), settings.code.clone()),
                Message::BackupsListed,
            ),
            None => Task::none(),
        }
    }

    /// All the app shows when `config.toml` did not load.
    pub(crate) fn broken_config_screen<'a>(&'a self, error: &'a str) -> Element<'a, Message> {
        let body = column![
            text("Lightforge can't read its settings").size(theme::TITLE),
            muted(
                "Nothing has been changed. Fix the file, or move it aside to start over \
                 with defaults, then restart Lightforge. A copy was saved next to it as \
                 config.toml.broken."
                    .to_owned(),
                theme::BODY,
            ),
            container(mono(error.to_owned()))
                .padding(theme::MD)
                .width(Length::Fill)
                .style(theme::row),
            row![
                secondary("Open config folder", Message::OpenConfigFolder).enabled(true),
            ],
        ]
        .spacing(theme::MD)
        .max_width(MESSAGE_WIDTH);

        container(body).center(Length::Fill).padding(theme::XL).style(theme::scrim).into()
    }

    /// What changed in the version that has just started.
    ///
    /// A dialog rather than a page, because it is the one thing in the app
    /// that opens itself: it has to be read and closed, and every other
    /// screen stays reachable afterwards from the navigation. The same notes
    /// live on the Lightforge Updates page for anyone who wants them again,
    /// including after the checkbox has been ticked.
    pub(crate) fn patch_notes_card<'a>(&'a self, notes: &'a PatchNotes) -> Element<'a, Message> {
        // Only the notes scroll; the heading and the button stay put, so a
        // long release can always be dismissed. This cap plus the rest comes
        // to about 630pt, inside the 820 minimum window. The scrollbar's
        // spacing (`theme::scrollbar_shape`) keeps it beside the text, not
        // over it.
        const NOTES_MAX: f32 = 480.0;

        let body = column![
            muted(format!("Version {}", notes.release.version), theme::LABEL),
            container(focus::region(
                "notes",
                scrollable(release_notes(&notes.release))
                    .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                    .style(theme::scrollbar),
            ))
            .max_height(NOTES_MAX),
        ]
        .spacing(theme::SM);

        let actions = row![
            self.ring(
                focus::Target::Key("mute notes".to_owned()),
                Some(Message::MutePatchNotes(!notes.mute)),
                tick(notes.mute)
                    .label("Never show this again")
                    .on_toggle(Message::MutePatchNotes)
                    .text_size(theme::tap_text(theme::BODY)),
            ),
            Space::new().width(Length::Fill),
            self.ring(
                focus::Target::NotesDismiss,
                Some(Message::DismissPatchNotes),
                button(text("Got it").size(theme::LABEL).width(Length::Fill).center())
                    .width(Length::Fixed(150.0))
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .style(theme::filled(theme::ACCENT))
                    .on_press(Message::DismissPatchNotes),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section("What's new in Lightforge".to_owned(), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The question before a running game is stopped.
    pub(crate) fn stop_card<'a>(&'a self, dialog: &'a StopDialog) -> Element<'a, Message> {
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("keep playing", Some(Message::StopGameDismiss), choice("Keep playing").style(theme::outlined)),
            tappable(
                "stop game",
                Some(Message::StopGameConfirm),
                choice("Stop game").style(theme::filled(theme::ERROR)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(column![text(format!("Stop {}?", dialog.label)).size(theme::HEADING + 2.0), actions].spacing(theme::LG))
            .width(Length::Fixed(520.0))
            .padding(theme::LG)
            .style(theme::dialog)
            .into()
    }
}
