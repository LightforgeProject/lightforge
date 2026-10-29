//! The update notice: a newer Lightforge, and what this copy can do about it.

use crate::*;
use iced::widget::column;
use lightforge_core::upgrade::{Package, Route};

impl Lightforge {
    /// A newer Lightforge exists: a button, or commands to copy, by install route.
    pub(crate) fn update_card<'a>(
        &'a self,
        available: &'a lightforge_core::update::Available,
    ) -> Element<'a, Message> {
        let route = self.origin.route(&available.latest);
        let stage = self.upgrade.as_ref().map(|u| &u.stage);

        let mut body = column![
            text(format!("Lightforge {} is available.", available.latest)).size(theme::BODY),
            muted(format!("You are running {}.", available.current), theme::LABEL),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(520.0));

        let (lines, main): (Vec<Element<'a, Message>>, Option<(&'static str, Message)>) = match stage {
            None => self.offer(&route),
            Some(UpgradeStage::Fetching) => (vec![self.fetching()], None),
            Some(UpgradeStage::InTerminal { command, .. }) => (
                vec![
                    muted(
                        "A terminal is open with the update. Enter your password there; \
                         Lightforge notices when it is done."
                            .to_owned(),
                        theme::LABEL,
                    )
                    .into(),
                    mono(command.clone()).into(),
                ],
                None,
            ),
            Some(UpgradeStage::NoTerminal { command }) => (
                vec![
                    muted(
                        "No terminal could be opened here. Run this in one yourself:".to_owned(),
                        theme::LABEL,
                    )
                    .into(),
                    copyable(0, command.clone()),
                ],
                None,
            ),
            Some(UpgradeStage::Installed) => self.installed(&available.latest),
            Some(UpgradeStage::Failed(problem)) => (
                vec![text(problem.message.clone()).size(theme::LABEL).color(theme::ERROR).into()],
                Some(("Try again", Message::UpgradeStart)),
            ),
        };
        for line in lines {
            body = body.push(line);
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let busy = self.upgrading();
        let later = (!busy).then_some(Message::DismissUpdate);
        let mut actions = row![
            Space::new().width(Length::Fill),
            self.ring(
                focus::Target::UpdateLater,
                later.clone(),
                choice("Not now").style(theme::outlined).on_press_maybe(later),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        // The release page is the main answer only when there is nothing to
        // press; next to a button that updates, it is the way to read first.
        let release = choice("See the release").on_press_maybe((!busy).then_some(Message::OpenReleases));
        let release = match main {
            Some(_) => release.style(theme::outlined),
            None => release.style(theme::filled(theme::ACCENT)),
        };
        actions = actions.push(self.ring(
            focus::Target::UpdateOpen,
            (!busy).then_some(Message::OpenReleases),
            release,
        ));
        if let Some((label, message)) = main {
            actions = actions.push(tappable(
                "update",
                Some(message.clone()),
                choice(label).style(theme::filled(theme::ACCENT)),
            ));
        }

        container(column![section("Update available".to_owned(), body.into()), actions].spacing(theme::LG))
            .width(Length::Fixed(600.0))
            .padding(theme::LG)
            .style(theme::dialog)
            .into()
    }

    /// Before anything was pressed: what this copy's route will do.
    fn offer<'a>(&self, route: &Route) -> (Vec<Element<'a, Message>>, Option<(&'static str, Message)>) {
        let say = |line: &str| -> Element<'a, Message> { muted(line.to_owned(), theme::LABEL).into() };
        match route {
            Route::InPlace => (
                vec![say("Lightforge downloads it, checks it and replaces its own files, as the installer would.")],
                Some(("Update now", Message::UpgradeStart)),
            ),
            Route::Terminal(package) => {
                let mut lines = vec![say(
                    "Your package manager installs it: a terminal opens and asks for your password.",
                )];
                if *package == (Package::Pacman { package: "lightforge".to_owned() }) {
                    lines.push(say("This package is built from source, which takes a few minutes."));
                }
                (lines, Some(("Update", Message::UpgradeStart)))
            }
            Route::Instructions { commands, note } => {
                let mut lines = vec![say(note)];
                lines.extend(commands.iter().enumerate().map(|(i, c)| copyable(i, c.clone())));
                (lines, None)
            }
        }
    }

    #[allow(clippy::cast_possible_truncation, reason = "a percentage")]
    fn fetching<'a>(&self) -> Element<'a, Message> {
        let (done, total) = self.upgrade.as_ref().map_or((0, None), |u| u.sample);
        let fraction = total.filter(|t| *t > 0).map(|t| (done as f32 / t as f32).clamp(0.0, 1.0));
        let label = match fraction {
            Some(f) if f < 1.0 => format!("Downloading… {}%", (f * 100.0) as u32),
            Some(_) => "Checking and installing…".to_owned(),
            None => "Getting the update ready…".to_owned(),
        };
        column![
            muted(label, theme::LABEL),
            progress_bar(0.0..=1.0, fraction.unwrap_or(0.0)).girth(6),
        ]
        .spacing(theme::XS)
        .into()
    }

    /// The new version is on disk.
    fn installed<'a>(&self, version: &str) -> (Vec<Element<'a, Message>>, Option<(&'static str, Message)>) {
        let mut lines = vec![muted(
            match fit::under_gamescope() {
                true => format!("Lightforge {version} is installed. Close Lightforge and start it again from Steam to use it."),
                false => format!("Lightforge {version} is installed. Restart Lightforge to use it."),
            },
            theme::LABEL,
        )
        .into()];
        if !self.can_restart() {
            lines.push(muted("A restart waits until the download has stopped.".to_owned(), theme::CAPTION).into());
            return (lines, None);
        }
        let label = match fit::under_gamescope() {
            true => "Close Lightforge",
            false => "Restart now",
        };
        (lines, Some((label, Message::UpgradeRestart)))
    }
}

/// A command in mono, with a button that copies it.
fn copyable<'a>(index: usize, command: String) -> Element<'a, Message> {
    row![
        container(mono(command.clone())).width(Length::Fill),
        tappable(
            format!("copy command {index}"),
            Some(Message::CopyRuntimeCommand(command)),
            button(text("Copy").size(theme::LABEL)).style(theme::outlined).padding(theme::tap([theme::XS, theme::SM])),
        ),
    ]
    .spacing(theme::SM)
    .align_y(Alignment::Center)
    .into()
}
