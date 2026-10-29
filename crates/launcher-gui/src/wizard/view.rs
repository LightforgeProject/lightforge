//! The card around every step: title, dots, footer.

use super::steps::{done, found, games, install, place, welcome, welcome_answered};
use super::*;
// Named, not globbed: std's own `column!` would otherwise win the glob.
use iced::widget::column;

/// The Discord and X marks on the last slide, larger than in the logo menu.
const MARK: f32 = 32.0;

/// The whole wizard: one card, centred, with the step's own body in it.
pub fn view<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let body = match wizard.step {
        Step::Welcome => welcome(app),
        Step::Place => place(app, wizard),
        Step::Found => found(app, wizard),
        Step::Install => install(app, wizard),
        Step::Games => games(app),
        Step::Done => done(app, wizard),
    };

    let card = column![
        match wizard.scan_only {
            true => Element::from(Space::new()),
            false => dots(wizard.step),
        },
        text(match wizard.scan_only {
            true => "Look for games",
            false => wizard.step.title(),
        })
        .size(theme::TITLE),
        container(body).height(Length::Fill),
        footer(app, wizard),
    ]
    .spacing(theme::LG);

    container(
        container(card)
            .padding(theme::XL)
            .width(Length::Fixed(820.0))
            .height(Length::Fixed(660.0))
            .style(theme::floating),
    )
    .center(Length::Fill)
    .into()
}

/// How far along, as one dot per step.
fn dots(step: Step) -> Element<'static, Message> {
    let at = Step::ALL.iter().position(|s| *s == step).unwrap_or(0);
    let mut row = row![].spacing(theme::SM).align_y(Alignment::Center);
    for (index, _) in Step::ALL.iter().enumerate() {
        let colour = match index <= at {
            true => theme::ACCENT,
            false => theme::MUTED,
        };
        let size = match index == at {
            true => 8.0,
            false => 6.0,
        };
        row = row.push(
            container(Space::new().width(Length::Fixed(size)).height(Length::Fixed(size)))
                .style(theme::dot(colour)),
        );
    }
    row.into()
}

/// Back, and the step's own button.
///
/// No skip-all: every step can be passed, and A once lost the setup.
fn footer<'a>(app: &'a Lightforge, wizard: &'a Wizard) -> Element<'a, Message> {
    let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);

    if wizard.scan_only {
        let ticked = wizard.rows.iter().any(|row| row.take);
        return actions
            .push(secondary("Close", Message::Wizard(Msg::Back)).enabled(!app.busy))
            .push(Space::new().width(Length::Fill))
            .push(
                action_button("Add these", theme::INSTALL, Message::Wizard(Msg::Next))
                    .enabled(ticked && !wizard.scanning),
            )
            .into();
    }

    if wizard.step.back().is_some() && wizard.step != Step::Done {
        actions = actions.push(secondary("Back", Message::Wizard(Msg::Back)).enabled(!app.busy));
    }
    actions = actions.push(Space::new().width(Length::Fill));

    // What the step's own button says, and whether it can be pressed yet.
    let (label, ready): (&str, bool) = match wizard.step {
        Step::Welcome => ("Get started", welcome_answered(app)),
        Step::Place => ("Look for my games", true),
        Step::Found => (
            match wizard.rows.iter().any(|row| row.take) {
                true => "Add these and continue",
                false => "Continue",
            },
            !wizard.scanning,
        ),
        // Pressable while nothing has been provisioned: someone
        // who cannot install Battle.net today should still reach the end.
        Step::Install => ("Continue", !app.setup_running),
        Step::Games => ("Continue", true),
        Step::Done => ("Ready to play", true),
    };

    let message = match wizard.step {
        Step::Done => Msg::Finish,
        _ => Msg::Next,
    };
    actions = actions.push(
        action_button(label, theme::INSTALL, Message::Wizard(message)).enabled(ready),
    );

    actions.into()
}

/// One line about what adding an install did.
pub(super) fn note(note: &Note) -> Element<'_, Message> {
    match note.level {
        Level::Done => muted(note.text.clone(), theme::CAPTION).into(),
        // A game not added is what the user must act on.
        Level::Warning => text(note.text.clone()).size(theme::LABEL).color(theme::PAUSE).into(),
        Level::Failed => text(note.text.clone()).size(theme::BODY).color(theme::ERROR).into(),
    }
}

/// One of the last slide's marks, as a place for the ring.
pub(super) fn mark(icon: iced::widget::image::Handle, url: Option<&'static str>) -> Element<'static, Message> {
    let press = url.map_or(focus::Press::Off, |url| focus::Press::Send(Message::OpenLink(url)));
    focus::ring(focus::Target::Link(url.unwrap_or_default()), press, crate::link_square(icon, url, MARK))
}
