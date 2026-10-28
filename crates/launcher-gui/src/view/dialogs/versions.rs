//! Which build a game is on: the version picker, its info card, and modifying an install.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The cog's "Game Version": what is installed and where.
    pub(crate) fn version_card_info<'a>(&'a self, dialog: &'a VersionDialog) -> Element<'a, Message> {
        let body = column![
            field_row("Version".to_owned(), mono(dialog.version.clone()).into()),
            field_row("Install Location".to_owned(), mono(dialog.location.clone()).into()),
        ]
        .spacing(theme::SM);
        let actions = row![
            Space::new().width(Length::Fill),
            tappable(
                "close version",
                Some(Message::CloseGameVersion),
                button(text("Close").size(theme::LABEL).width(Length::Fill).center())
                    .width(Length::Fixed(170.0))
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .style(theme::outlined),
            ),
        ];
        container(
            column![section(dialog.label.clone(), body.into()), actions].spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The cog's "Modify Install": the language the game is installed in.
    pub(crate) fn modify_card<'a>(&'a self, dialog: &'a ModifyDialog) -> Element<'a, Message> {
        let locales: Vec<LocaleChoice> = lightforge_core::tact::install::LOCALES
            .iter()
            .map(|(code, _)| LocaleChoice(code.to_string()))
            .collect();
        let changed = dialog.locale != dialog.current;
        let body = column![
            muted(
                "Downloads the files for the chosen language, and the game starts in it. \
                 Files already stored are not fetched again."
                    .to_owned(),
                theme::LABEL,
            ),
            self.picker(
                "modify locale".to_owned(),
                locales,
                Some(LocaleChoice(
                    lightforge_core::tact::install::current_locale(&dialog.locale).to_owned(),
                )),
                "Language",
                |choice| Message::ModifyLocale(choice.0),
                Length::Fixed(300.0),
                theme::BODY,
            ),
        ]
        .spacing(theme::MD);
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("modify cancel", Some(Message::ModifyCancel), choice("Cancel").style(theme::outlined)),
            tappable(
                "modify start",
                changed.then_some(Message::ModifyStart),
                choice("Start Update").style(theme::filled(theme::UPDATE)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        container(
            column![section(format!("Modify {}", dialog.label), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The builds one addon offers, for choosing an older one on purpose.
    ///
    /// Choosing here always pins. Installing an older build without pinning
    /// would be undone by the next pass within the minute, which is not a
    /// choice so much as a delay.
    pub(crate) fn version_card<'a>(&'a self, picker: &'a VersionPicker) -> Element<'a, Message> {
        let mut body = column![
            text(format!("Choose a build of {}", picker.name)).size(theme::TITLE),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(560.0));

        body = body.extend(
            self.hint(
                "Installing a build from here pins the addon to it, so nothing moves it \
                 again until you unpin. Only CurseForge publishes a build list.",
            )
            .map(Element::from),
        );

        match picker.builds.as_deref() {
            None => body = body.push(muted("asking the catalogue…".to_owned(), theme::BODY)),
            Some([]) => {
                body = body.push(empty(
                    match &picker.problem {
                        Some(e) => e.as_str(),
                        None => "This provider publishes no build list.",
                    },
                    "Only CurseForge offers older builds; the others ship one.",
                ));
            }
            Some(builds) => {
                let mut list = column![].spacing(theme::XS);
                for build in builds {
                    let installed = picker.installed.as_deref() == Some(build.file_id.as_str());
                    let mut head = row![text(
                        build
                            .display_name
                            .clone()
                            .or_else(|| build.file_name.clone())
                            .unwrap_or_else(|| build.file_id.clone()),
                    )
                    .size(theme::BODY)]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center);

                    // The release type in the provider's own words, because
                    // that is what the author labelled the build with.
                    if let Some(kind) = &build.release_type {
                        if kind != "release" {
                            head = head.push(pill(kind.clone(), theme::UPDATE));
                        }
                    }
                    if installed {
                        head = head.push(pill("installed".to_owned(), theme::MUTED));
                    }

                    let mut line = row![column![
                        head,
                        muted(
                            match (&build.file_date, build.file_length) {
                                (Some(date), Some(size)) =>
                                    format!("{} · {:.1} MB", day_of(date), size as f64 / 1e6),
                                (Some(date), None) => day_of(date),
                                (None, Some(size)) => format!("{:.1} MB", size as f64 / 1e6),
                                (None, None) => String::new(),
                            },
                            theme::CAPTION,
                        ),
                    ]
                    .spacing(theme::XS)]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center);

                    line = line.push(Space::new().width(Length::Fill));
                    line = line.push(
                        secondary(
                            "Pin to this",
                            Message::InstallVersion(
                                picker.provider.clone(),
                                picker.id.clone(),
                                build.file_id.clone(),
                            ),
                        )
                        .enabled(!self.addons_locked()),
                    );
                    list = list.push(container(line).padding(theme::MD).style(theme::row));
                }
                body = body.push(focus::region(
                    "addon versions",
                    scrollable(list).height(Length::Fixed(340.0)).style(theme::scrollbar),
                ));
            }
        }

        body = body.push(
            row![
                Space::new().width(Length::Fill),
                secondary("Cancel", Message::CancelVersionPicker).enabled(true),
            ]
            .spacing(theme::SM),
        );

        container(container(body).padding(theme::LG).style(theme::floating))
            .center(Length::Fill)
            .padding(theme::XL)
            .into()
    }
}
