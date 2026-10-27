//! The setup steps: runtime, Proton, prefix, Battle.net.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The 32-bit runtime, when it is missing.
    ///
    /// Battle.net's installer is the first 32-bit binary in setup, so without
    /// a 32-bit userspace everything looks fine until the last step dies.
    /// Said here, before anything downloads. The command is offered to copy,
    /// not run: a launcher running a package manager as root on a guess is a
    /// trust escalation, and the systems that most need help (SteamOS,
    /// rpm-ostree, NixOS) are the ones such a button could not serve.
    pub(crate) fn runtime_row() -> Element<'static, Message> {
        let remedy = lightforge_core::runtime::remedy();

        let mut lines = column![
            text("32-bit runtime").size(theme::BODY),
            mono("missing — Battle.net's installer cannot run without it".to_owned()),
        ]
        .spacing(theme::XS);

        if let Some(command) = &remedy.command {
            lines = lines.push(muted(
                format!("on {}, run:", remedy.distro),
                theme::CAPTION,
            ));
            lines = lines.push(mono(command.clone()));
        }
        if let Some(note) = &remedy.note {
            lines = lines.push(muted(note.clone(), theme::CAPTION));
        }
        if remedy.command.is_none() && remedy.note.is_none() {
            lines = lines.push(muted(
                lightforge_core::runtime::ADVICE.to_owned(),
                theme::CAPTION,
            ));
        }

        // Only offer a copy when there is something to copy. A dead button
        // beside an immutable distro's instructions would be worse than none.
        let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(command) = remedy.command.clone() {
            actions = actions.push(
                action_button(
                    "Copy command",
                    theme::ACCENT,
                    Message::CopyRuntimeCommand(command),
                )
                .enabled(true),
            );
        }
        actions = actions.push(secondary("Re-check", Message::RecheckRuntime).enabled(true));

        container(
            row![
                container(
                    container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                        .style(theme::dot(theme::ERROR)),
                )
                .padding(theme::SM)
                .style(theme::badge(theme::ERROR)),
                lines.width(Length::Fill),
                actions,
            ]
            .spacing(theme::MD)
            .align_y(Alignment::Center),
        )
        .width(Length::Fill)
        .padding(theme::MD)
        .style(theme::row)
        .into()
    }

    /// First-run provisioning: what is missing, and one button to fix it.
    /// Proton, the prefix and Battle.net: what is there, and the button that
    /// provisions whatever isn't.
    pub(crate) fn setup_section(&self) -> Element<'_, Message> {
        self.setup_steps(false)
    }

    /// The same list inside the first-run wizard, which owns two of the things
    /// this draws for itself: where to install, and whether Battle.net is
    /// wanted. Drawing them twice on one slide is how the wizard ended up with
    /// two "Install to" fields and two ways to say no to Battle.net.
    pub(crate) fn setup_section_in_wizard(&self) -> Element<'_, Message> {
        self.setup_steps(true)
    }

    pub(crate) fn setup_steps(&self, in_wizard: bool) -> Element<'_, Message> {
        let mut body = column![]
            .extend(self.hint(
                "Proton, the Wine prefix and Battle.net. Each step is skipped if it is \
                 already done.",
            ).map(Element::from))
            .spacing(theme::SM);

        // Where all three steps below install to. Only offered while nothing
        // is installed: after that, moving it is a data migration rather than
        // a setting; see `relocate`. Once the prefix is built, the Wine
        // prefix step below says where it is, so nothing is lost by hiding it.
        if self.library_empty && !in_wizard {
            body = body.extend(self.install_location());
        }

        let steps = match &self.setup {
            None => {
                body = body.push(muted("checking what is installed…".to_owned(), theme::BODY));
                Vec::new()
            }
            Some(status) => {
                // A precondition rather than a step: nothing below can be
                // provisioned without it, and unlike the three steps it is not
                // something Lightforge can do on the user's behalf. It leads
                // the list for the same reason it blocks setup.
                if !status.thirty_two_bit {
                    body = body.push(Self::runtime_row());
                }
                self.setup_rows(status, in_wizard)
            }
        };

        body = match in_wizard {
            true => body.extend(steps),
            false if steps.is_empty() => body,
            false => body.push(setting_group(steps)),
        };

        if self.setup_running {
            body = body.push(self.setup_log());
        }

        // Where the progress was, so a failed run says why. The Settings page
        // shows its errors above every section; the wizard covers that, and a
        // step that failed used to look like a button that did nothing.
        if in_wizard && !self.setup_running {
            if let Some(e) = &self.error {
                body = body.push(problem(e));
            }
        }

        body.push(self.setup_actions(in_wizard)).into()
    }

    /// The install-location field and what it means, while nothing is installed.
    fn install_location(&self) -> Vec<Element<'_, Message>> {
        let mut rows = vec![setting_group([
            setting(
                "Install location",
                "Battle.net and the Wine prefix go here. Games can go on any drive.",
                None,
                row![
                    clickable(
                        "install location",
                        text_input("~/Games/Blizzard", &self.location_input)
                            .on_input(Message::LocationChanged)
                            .on_submit(Message::UseLocation)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM)
                            .width(Length::Fixed(300.0)),
                    ),
                    secondary("Use", Message::UseLocation).enabled(true),
                    secondary("Open folder", Message::OpenLibrary).enabled(true),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ),
        ])];
        rows.extend(self.hint(
            "Battle.net and the Wine prefix are installed here, and stay here whatever \
             drives you add later. Games can go on any drive — see Storage.",
        ).map(Element::from));
        rows
    }

    /// One row per setup step, each saying whether it is done.
    fn setup_rows(&self, status: &SetupStatus, in_wizard: bool) -> Vec<Element<'_, Message>> {
        // On the Settings page the steps share one card like every other
        // group there; the wizard keeps a row apiece.
        let framed = |row: Element<'static, Message>| -> Element<'static, Message> {
            match in_wizard {
                true => container(row).width(Length::Fill).padding(theme::MD).style(theme::row).into(),
                false => container(row).width(Length::Fill).padding(theme::MD).into(),
            }
        };
        let mut steps: Vec<Element<'_, Message>> = Vec::new();
        let step = |done: bool, label: &'static str, detail: String| -> Element<'static, Message> {
            let color = match done {
                true => theme::OK,
                false => theme::MUTED,
            };
            framed(
                row![
                    container(
                        container(Space::new().width(Length::Fixed(8.0)).height(Length::Fixed(8.0)))
                            .style(theme::dot(color)),
                    )
                    .padding(theme::SM)
                    .style(theme::badge(color)),
                    column![text(label).size(theme::BODY), muted(detail, theme::LABEL)].spacing(theme::XS),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center)
                .into(),
            )
        };

        steps.push(step(
            status.proton.is_some(),
            "Proton-GE",
            match &status.proton {
                Some(build) => format!(
                    "{} — {}",
                    build.version,
                    lightforge_core::setup::describe_source(build)
                ),
                None => "not installed".to_owned(),
            },
        ));
        steps.push(step(
            status.prefix.is_ready(),
            "Wine prefix",
            match status.prefix.is_ready() {
                true => tilde(&status.prefix.prefix),
                false => "not built, or the game link is missing".to_owned(),
            },
        ));
        // Declined at setup is not the same as missing: the user was
        // told what it costs and said no, so the row offers it rather
        // than reporting it as unfinished.
        if !status.battle_net && !status.battle_net_wanted && !in_wizard {
            steps.push(
                framed(
                    row![
                        column![
                            text("Battle.net").size(theme::BODY),
                            mono("skipped — World of Warcraft only".to_owned()),
                        ]
                        .spacing(theme::XS)
                        .width(Length::Fill),
                        action_button(
                            "Install it now",
                            theme::ACCENT,
                            Message::InstallBattleNet,
                        )
                        .enabled(!self.setup_running),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center)
                    .into(),
                ),
            );
        } else {
            steps.push(step(
                status.battle_net,
                "Battle.net",
                match (status.battle_net, status.battle_net_wanted) {
                    (true, _) => tilde(&self.layout.battle_net_dir()),
                    (false, true) => "not installed in the prefix".to_owned(),
                    (false, false) => "skipped — World of Warcraft only".to_owned(),
                },
            ));
        }
        steps
    }

    /// What a running setup has done so far, and how far the current download is.
    fn setup_log(&self) -> Element<'_, Message> {
        let mut log = column![].spacing(theme::XS);
        for entry in &self.setup_sample.log {
            log = log.push(muted(entry.clone(), theme::CAPTION));
        }
        log = log.push(match self.setup_sample.fraction() {
            Some(fraction) => Element::from(progress_bar(0.0..=1.0, fraction).girth(6)),
            // A download with no content-length still has to look alive.
            None => Element::from(mono(format!(
                "{:.0} MB",
                self.setup_sample.done as f64 / 1e6
            ))),
        });
        container(log).padding(theme::MD).style(theme::card).into()
    }

    /// The button that runs setup, and on the Settings page the two that
    /// reopen the wizard.
    fn setup_actions(&self, in_wizard: bool) -> Element<'_, Message> {
        let ready = self.setup.as_ref().is_some_and(|s| s.is_ready());
        let label = match (self.setup_running, ready) {
            (true, _) => "Working…",
            (false, true) => "Re-run setup",
            (false, false) => "Set up Lightforge",
        };
        let start = action_button(
            label,
            match ready {
                true => theme::PAUSE,
                false => theme::INSTALL,
            },
            Message::RunSetup,
        )
        .width(220.0)
        .enabled(!self.setup_running);
        row![start]
            // The same steps, walked through with the questions a first run
            // asks, region, language, drives, and the installs that were
            // here first. Kept here because this is where someone who
            // dismissed it comes looking for it; inside the wizard it would
            // be a button that reopens the thing you are looking at.
            .extend((!in_wizard).then(|| {
                Element::from(
                    secondary("Run the setup wizard", Message::Wizard(wizard::Msg::Open))
                        .enabled(!self.setup_running),
                )
            }))
            // Games installed after setup, by Lutris or another Battle.net,
            // without walking through region and drives again.
            .extend((!in_wizard).then(|| {
                Element::from(
                    secondary("Look for games", Message::Wizard(wizard::Msg::Scan))
                        .enabled(!self.setup_running),
                )
            }))
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into()
    }
}
