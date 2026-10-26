//! The modal cards: install, uninstall, versions, reports and the rest.

use crate::*;
use iced::widget::column;

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

    /// The install dialog's card.
    pub(crate) fn install_card<'a>(&'a self, dialog: &'a InstallDialog) -> Element<'a, Message> {
        let mut drives = column![].spacing(theme::SM);
        for (index, target) in dialog.targets.iter().enumerate() {
            let chosen = index == dialog.chosen;
            // Fixed, and wide enough for the longest thing that goes in it.
            // Left to shrink it was squeezed by the path beside it until
            // "1.37 TiB free" came out as three stacked words.
            let mut right = column![]
                .spacing(2)
                .width(Length::Fixed(110.0))
                .align_x(Alignment::End);
            right = match target.disk {
                Some(disk) => right.push(mono(format!("{} free", size_of(disk.free)))),
                None => right.push(muted("—".to_owned(), theme::LABEL)),
            };
            // Battle.net's own wording and colour for the same fact: a second
            // WoW version in a folder that already has one shares its data
            // store, so what is already downloaded is not fetched twice.
            if target.shares {
                right = right.push(text("Sharing data").size(theme::CAPTION).color(theme::OK));
            }

            let mut left = column![row![
                text(target.label.clone()).size(theme::BODY),
                Space::new().width(Length::Fixed(theme::SM)),
            ]
            .align_y(Alignment::Center)]
            .spacing(2);
            // `~/Games/...` rather than `/home/<name>/Games/...`: the home
            // prefix is the longest and least informative part of the path,
            // and this row is where the width is tightest.
            left = left.push(muted(home_relative(&target.install), theme::CAPTION));
            if let Some(reason) = target.state.reason(&target.label) {
                left = left.push(text(reason).size(theme::CAPTION).color(theme::ERROR));
            }

            let usable = target.state != library::State::Missing;
            drives = drives.push(tappable(
                format!("install target {index}"),
                usable.then_some(Message::InstallTarget(index)),
                button(
                    // The left column takes what the right one does not, so
                    // the path is the only thing that ever wraps.
                    row![left.width(Length::Fill), right]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding([theme::SM, theme::MD])
                .style(match chosen {
                    true => theme::selected,
                    false => theme::card_button,
                }),
            ));
        }
        let location: Element<Message> = column![drives]
            .extend(
                self.hint(
                    "World of Warcraft is installed in a folder of its own on the drive you \
                     pick. Every WoW version you later put on the same drive shares its game \
                     data; one on another drive is a separate install.",
                )
                .map(Element::from),
            )
            .spacing(theme::XS)
            .into();

        let mut body = column![field_row("Install to".to_owned(), location)].spacing(theme::MD);

        let mut ready = false;
        match &dialog.stage {
            DialogStage::Calculating => {
                body = body.push(muted("Calculating size…".to_owned(), theme::LABEL));
            }
            DialogStage::Failed(e) => {
                body = body.push(
                    text(format!("⚠ Could not read this build: {e}"))
                        .size(theme::LABEL)
                        .color(theme::ERROR),
                );
            }
            DialogStage::Ready(offerings) => {
                let extra = offerings
                    .optional_bytes
                    .iter()
                    .find(|(tag, _)| tag == HIGH_RES)
                    .map(|(_, bytes)| *bytes)
                    // Every build names the tag; only some have textures
                    // behind it. Retail's adds nothing, and a "+0 MiB"
                    // switch is not a choice.
                    .filter(|bytes| *bytes > 0);
                if let Some(extra) = extra {
                    body = body.push(field_row(
                        "Optional".to_owned(),
                        row![
                            clickable(
                                "high res",
                                tick(dialog.high_res)
                                    .label(optional_label(HIGH_RES))
                                    .on_toggle(Message::InstallHighRes)
                                    .text_size(theme::tap_text(theme::BODY)),
                            ),
                            muted(format!("+{}", size_of(extra)), theme::LABEL),
                        ]
                        .spacing(theme::SM)
                        .align_y(Alignment::Center)
                        .into(),
                    ));
                }

                let need = offerings.base_bytes
                    + if dialog.high_res { extra.unwrap_or(0) } else { 0 };
                // Exact: priced against the store, the same test the plan
                // applies, so shared files are already left out. Download
                // size, not disk size, the extracted install files (the
                // executables and the like) come on top, which is most of
                // why Battle.net's "required" reads a little higher.
                body = body.push(field_row(
                    "Download".to_owned(),
                    mono(format!("{} to download", size_of(need))).into(),
                ));
                match dialog.target().and_then(|t| t.disk) {
                    Some(disk) => {
                        body = body.push(field_row(
                            "Free space".to_owned(),
                            mono(size_of(disk.free)).into(),
                        ));
                        ready = disk.free > need;
                        if !ready {
                            body = body.push(
                                text("⚠ Not enough free space on that disk.")
                                    .size(theme::LABEL)
                                    .color(theme::ERROR),
                            );
                        }
                    }
                    None => ready = true,
                }
            }
        }

        // A pair, so one size: the same height and width, labels centred,
        // with only the fill telling the default from the way out.
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("cancel", Some(Message::InstallCancel), choice("Cancel").style(theme::outlined)),
            tappable(
                "start install",
                ready.then_some(Message::InstallStart),
                choice("Start install").style(theme::filled(theme::INSTALL)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![
                section(format!("Install {}", dialog.label), body.into()),
                actions,
            ]
            .spacing(theme::LG),
        )
        .width(Length::Fixed(640.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// The card asking whether to open the shared prefix with an older build.
    ///
    /// Two buttons and no third option on purpose. "Launch anyway" is the one
    /// the user came for, so it is not hidden behind an explanation; the
    /// explanation is there because the cost falls on games they are not
    /// looking at.
    pub(crate) fn downgrade_card<'a>(&'a self, dialog: &'a DowngradeDialog) -> Element<'a, Message> {
        let what = match dialog.what {
            Launch::Game => format!("Launching {}", dialog.label),
            Launch::BattleNet => format!("Starting {} through Battle.net", dialog.label),
            Launch::Client => "Opening Battle.net".to_owned(),
            Launch::Install => "Opening Battle.net's install flow".to_owned(),
        };

        let body = column![
            field_row("Would run".to_owned(), mono(dialog.pick.clone()).into()),
            field_row("Prefix built by".to_owned(), mono(dialog.built_by.clone()).into()),
            text(format!(
                "{what} under {} would open the Wine prefix with an older Proton than the \
                 {} that last upgraded it.",
                dialog.pick, dialog.built_by,
            ))
            .size(theme::LABEL)
            .color(theme::UPDATE),
            muted(
                "Every game here shares that one prefix, along with the Battle.net install \
                 and its login. Proton migrates a prefix forward and never back, so an older \
                 build can leave it in a state the newer one did not expect — which shows up \
                 as a game or the client failing to start, not as an error here. If this \
                 build is the one the game needs, go ahead; if it is not, pin it back under \
                 Settings › Proton."
                    .to_owned(),
                theme::LABEL,
            ),
        ]
        .spacing(theme::MD);

        let choice = |label: String| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable(
                "cancel",
                Some(Message::DowngradeCancel),
                choice("Cancel".to_owned()).style(theme::outlined),
            ),
            tappable(
                "launch anyway",
                Some(Message::DowngradeConfirm),
                choice("Launch anyway".to_owned()).style(theme::filled(theme::UPDATE)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section("Older Proton than this prefix".to_owned(), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

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

    /// The uninstall confirmation's card.
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

    /// The confirmation for giving a download up.
    ///
    /// Two shapes, because an update and a first install are not the same
    /// question. An update's files are never removed: the game on disk plays
    /// right now, and what has already come down is a head start on the next
    /// attempt rather than waste.
    ///
    /// One line of consequence under the title and two buttons, deliberately.
    /// The earlier version offered a third way out, cancel but keep the files,
    /// and stated the figure it would free; both made the reader do work to
    /// answer a question they had already decided.
    pub(crate) fn cancel_card<'a>(&'a self, dialog: &'a CancelDialog) -> Element<'a, Message> {
        let updating = dialog.installed.is_some();
        let (title, hint, keep, give_up) = match &dialog.installed {
            Some(version) => (
                format!("Cancel the update to {}?", dialog.label),
                format!(
                    "{} stays on {version}, and what has already downloaded is \
                     kept for the next update.",
                    dialog.label
                ),
                "Keep downloading",
                "Cancel update",
            ),
            None => (
                format!("Cancel installing {}?", dialog.label),
                "Cancelling the download will remove all downloaded files on \
                 your drive."
                    .to_owned(),
                "Keep installing",
                "Cancel install",
            ),
        };

        let body = column![muted(hint, theme::BODY)]
            .spacing(theme::SM)
            .width(Length::Fixed(520.0));

        // One shape for both, so neither reads as the safe one by being
        // bigger. Only the fill says which is which.
        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(170.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable("keep", Some(Message::CancelDownloadDismiss), choice(keep).style(theme::outlined)),
            tappable(
                "give up",
                Some(Message::CancelDownloadConfirm { delete: !updating }),
                choice(give_up).style(theme::filled(theme::ERROR)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(column![section(title, body.into()), actions].spacing(theme::LG))
            .width(Length::Fixed(520.0))
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

    pub(crate) fn uninstall_card<'a>(&'a self, dialog: &'a UninstallDialog) -> Element<'a, Message> {
        let mut body = column![].spacing(theme::MD);
        let mut ready = false;
        match &dialog.plan {
            None => {
                body = body.push(muted("Working out what to remove…".to_owned(), theme::LABEL));
            }
            Some(Err(e)) => {
                body = body.push(text(format!("⚠ {e}")).size(theme::LABEL).color(theme::ERROR));
            }
            Some(Ok(preview)) if preview.plan.is_empty() => {
                body = body.push(muted(
                    "Nothing to remove — it is not installed here.".to_owned(),
                    theme::LABEL,
                ));
            }
            Some(Ok(preview)) => {
                ready = true;
                let plan = &preview.plan;
                let reclaim = preview.reclaim.as_ref().and_then(|r| r.as_ref().ok());
                let deletes = column(
                    plan.flavor_dir
                        .iter()
                        .chain(plan.store.iter())
                        .map(|path| mono(path.display().to_string()).into()),
                )
                .spacing(theme::XS);
                body = body
                    .push(field_row("Deletes".to_owned(), deletes.into()))
                    .push(field_row(
                        "Frees".to_owned(),
                        mono(size_of(plan.bytes + reclaim.map_or(0, |r| r.freed))).into(),
                    ));

                let (note, color) = match plan.is_whole_install() {
                    true => (
                        "This is the last WoW version here, so the shared game data goes with \
                         it."
                            .to_owned(),
                        theme::ERROR,
                    ),
                    false => {
                        let others = plan
                            .remaining
                            .iter()
                            .map(|c| self.label_of(c))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let note = match &preview.reclaim {
                            // Only what nobody else uses goes; an archive
                            // it shares with them keeps its bytes as dead
                            // space until something rewrites it.
                            Some(Ok(r)) if r.stranded > 0 => format!(
                                "Game data it shares with {others} stays. About {} of its own \
                                 is scattered too thinly through their archives to be worth \
                                 rewriting and stays as unused space.",
                                size_of(r.stranded)
                            ),
                            Some(Ok(_)) => format!("Game data it shares with {others} stays."),
                            _ => format!(
                                "Could not check which game data only it uses, so the shared \
                                 store is left as it is for {others}."
                            ),
                        };
                        (note, theme::MUTED)
                    }
                };
                body = body.push(text(note).size(theme::LABEL).color(color));
                if let Some(r) = reclaim.filter(|r| r.moved > 0) {
                    let line = match dialog.working {
                        true => format!(
                            "Compacting: {} of {} copied.",
                            size_of(dialog.copied.load(std::sync::atomic::Ordering::Relaxed).min(r.moved)),
                            size_of(r.moved)
                        ),
                        false => format!(
                            "To free its space, {} of the game data they still use is copied into \
                             fresh archives first. That can take a few minutes.",
                            size_of(r.moved)
                        ),
                    };
                    body = body.push(muted(line, theme::LABEL));
                }
                body = body.push(muted(
                    "Its addons and settings (Interface, WTF) are deleted with it. Battle.net \
                     stops listing it too."
                        .to_owned(),
                    theme::LABEL,
                ));
            }
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        let actions = row![
            Space::new().width(Length::Fill),
            tappable(
                "cancel",
                (!dialog.working).then_some(Message::UninstallCancel),
                choice("Cancel").style(theme::outlined),
            ),
            tappable(
                "uninstall",
                (ready && !dialog.working).then_some(Message::UninstallConfirm),
                choice(if dialog.working { "Uninstalling…" } else { "Uninstall" })
                    .style(theme::filled(theme::ERROR)),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(
            column![section(format!("Uninstall {}", dialog.label), body.into()), actions]
                .spacing(theme::LG),
        )
        .width(Length::Fixed(560.0))
        .padding(theme::LG)
        .style(theme::dialog)
        .into()
    }

    /// What a bug report in this category carries beyond the base bundle.
    ///
    /// Only what answers that category's first question: the addon list for
    /// an addon problem, the queue and the drives for a download problem, the
    /// settings for a settings problem. Read when the category is picked, so
    /// the card shows it before anything is sent, and scrubbed like the rest.
    pub(crate) fn report_attachment(&self, category: report::Category) -> String {
        use std::fmt::Write as _;
        use report::Category;

        let code = &self.selected.code;
        let mut out = String::new();
        match category {
            Category::Addons => {
                let dir = self.layout.addons_dir(code);
                let _ = writeln!(out, "\n--- addons: {} ---", dir.display());
                let _ = writeln!(out, "{:<16}{}", "flavor", lightforge_core::addons::flavor_of(code));
                let installed = match lightforge_core::addons::scan(&dir) {
                    Ok(installed) => installed,
                    Err(e) => {
                        let _ = writeln!(out, "the folder could not be read: {}", chain(&e));
                        Vec::new()
                    }
                };
                // Read-only: the Addons page prunes the ledger as it lists, a
                // report must not change what it is reporting on.
                let ledger = AppDirs::discover()
                    .ok()
                    .map(|dirs| manage::ledger_path(dirs.data_dir(), &dir, code))
                    .map(|path| lightforge_core::addons::Ledger::load(&path));
                let ledger = match ledger {
                    Some(Ok(ledger)) => ledger,
                    Some(Err(e)) => {
                        let _ = writeln!(out, "the ledger could not be read: {}", chain(&e));
                        lightforge_core::addons::Ledger::default()
                    }
                    None => lightforge_core::addons::Ledger::default(),
                };
                let rows = addonwork::rows_from(&installed, &ledger);
                let managed = rows.iter().filter(|r| r.managed.is_some() && r.child_of.is_none()).count();
                let _ = writeln!(out, "{:<16}{} folders, {managed} managed", "installed", rows.len());
                for row in rows.iter().filter(|r| r.child_of.is_none()) {
                    let owner = match &row.managed {
                        Some(m) => {
                            let mut owner = format!("{}:{}", m.provider, m.id);
                            if let Some(file) = &m.file_id {
                                let _ = write!(owner, " file {file}");
                            }
                            if let Some(channel) = m.channel {
                                let _ = write!(owner, " {}", channel.as_str());
                            }
                            if m.pinned.is_some() {
                                owner.push_str(" pinned");
                            }
                            if m.ignored {
                                owner.push_str(" ignored");
                            }
                            owner
                        }
                        None => "not managed".to_owned(),
                    };
                    let children = rows
                        .iter()
                        .filter(|r| r.child_of.as_deref() == Some(row.folder.as_str()))
                        .count();
                    let children = match children {
                        0 => String::new(),
                        n => format!(" (+{n} folders)"),
                    };
                    let _ = writeln!(out, "{}{children}  {}  {owner}", row.folder, row.version);
                }
            }
            Category::Downloader => {
                let _ = writeln!(out, "\n--- downloads ---");
                if self.queue.is_empty() {
                    let _ = writeln!(out, "nothing queued");
                }
                for download in &self.queue {
                    let stage = match &download.stage {
                        Stage::Queued => "queued".to_owned(),
                        Stage::Running { .. } => "running".to_owned(),
                        Stage::Paused(p) => {
                            format!("paused at {} of {}", size_of(p.downloaded_bytes), size_of(p.total_bytes))
                        }
                    };
                    let planned = download.job.total.map_or("not planned yet".to_owned(), size_of);
                    let _ = writeln!(out, "{} ({})  {stage}  job {planned}", download.label, download.code.as_str());
                }
                let _ = writeln!(out, "\n--- libraries ---");
                for library in &self.config.libraries {
                    let disk = library.disk().map_or("size unavailable".to_owned(), |d| {
                        format!("{} free of {}", size_of(d.free), size_of(d.total))
                    });
                    let _ = writeln!(
                        out,
                        "{} {}  {}  {:?}  {disk}",
                        library.id,
                        library.label,
                        library.games().display(),
                        library.state()
                    );
                }
            }
            Category::Settings => {
                // The contact is in the report already, where it belongs;
                // nothing else in the config is personal.
                let mut config = self.config.clone();
                config.contact = None;
                let _ = writeln!(out, "\n--- config.toml ---");
                match config.to_toml() {
                    Ok(toml) => out.push_str(&toml),
                    Err(e) => {
                        let _ = writeln!(out, "could not be rendered: {}", chain(&e));
                    }
                }
            }
            // The base bundle is already the game's own: build, Proton, DXVK,
            // the newest client log. Battle.net's log names the account, so
            // it stays on the machine; the launcher's own state is in the
            // recent activity.
            Category::Game | Category::BattleNet | Category::Launcher | Category::General => {}
        }
        lightforge_core::debug::scrub(&out)
    }

    /// The bug report and the feedback dialog, which are one card.
    // (`report_tags` sits below the impl, since it needs nothing from `self`.)
    ///
    /// They differ in three things: the prompt, whether the diagnostic bundle
    /// is shown, and whether it is sent. Everything else, the stages, the
    /// outcome line, the two buttons, is the same, and splitting them in two
    /// would mean maintaining that twice.
    pub(crate) fn report_card<'a>(&'a self, dialog: &'a ReportDialog) -> Element<'a, Message> {
        let (title, prompt) = match dialog.kind {
            ReportKind::Bug => ("Report a Bug", "What were you doing when this occurred?"),
            ReportKind::Feedback => {
                ("Send Feedback", "What would you like to see improved or changed?")
            }
        };
        // Editable right up until it is on its way: a report that failed, or
        // one the cooldown refused, is one somebody is about to change.
        let writing = !matches!(dialog.stage, ReportStage::Sending | ReportStage::Sent);

        let mut body = column![
            self.picker(
                "report category".to_owned(),
                report::Category::ALL.to_vec(),
                dialog.category,
                "What is it about?",
                Message::ReportCategory,
                Length::Fill,
                theme::BODY,
            ),
            // Five lines: enough that a paragraph looks invited rather than
            // tolerated, and the card still fits a laptop screen.
            // Pressing a field puts the typing in it: Steam's own keyboard
            // (Steam and X on a Deck) then writes there. Escape leaves the
            // field, and a second one closes the card.
            clickable(
                "report note",
                text_editor(&dialog.note)
                    .placeholder(prompt)
                    .on_action(Message::ReportNote)
                    .height(Length::Fixed(theme::BODY * 1.4 * 5.0 + theme::SM * 2.0))
                    .style(theme::editor)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
            // Required on a bug, and said so in the box itself: a bug nobody
            // can be asked about usually cannot be reproduced. Feedback can
            // stay anonymous, it rarely needs a reply.
            clickable(
                "report contact",
                text_input(
                    match dialog.kind {
                        ReportKind::Bug => "Email or Discord name, so I can reach you (required)",
                        ReportKind::Feedback => {
                            "Email or Discord name, so I can reach you (optional)"
                        }
                    },
                    &dialog.contact,
                )
                    .on_input_maybe(writing.then_some(Message::ReportContact))
                    .style(theme::field)
                    .size(theme::BODY)
                    .padding(theme::SM),
            ),
        ]
        .spacing(theme::MD);

        body = match dialog.kind {
            // Shown in full, and shown before it is sent: the only way a
            // promise about what leaves the machine is checkable is if the
            // person can read the thing that leaves it.
            ReportKind::Bug => body
                .push(muted(
                    "This exact diagnostic data will be sent to my private server securely."
                        .to_owned(),
                    theme::LABEL,
                ))
                .push(
                    container(
                        scrollable(mono(dialog.debug.clone()))
                            .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                            .height(Length::Fixed(200.0))
                            .width(Length::Fill),
                    )
                    .padding(theme::SM)
                    .style(theme::row),
                )
                .push(self.shot_row(dialog)),
            ReportKind::Feedback => body.push(muted(
                "This will be sent directly to me securely.".to_owned(),
                theme::LABEL,
            )),
        };

        if let Some((note, color)) = match dialog.stage {
            ReportStage::Sent => Some(("Sent — thank you.".to_owned(), theme::OK)),
            ReportStage::Failed => Some((
                "It could not be sent. Try again, or copy it and send it on Discord \
                 instead — closing this loses it."
                    .to_owned(),
                theme::ERROR,
            )),
            ReportStage::TooSoon => Some((
                "That exact report just went out. Add what changed, or close this.".to_owned(),
                theme::PLAY,
            )),
            _ => None,
        } {
            body = body.push(text(note).size(theme::LABEL).color(color));
        }

        let choice = |label: &'static str| {
            button(text(label).size(theme::LABEL).width(Length::Fill).center())
                .width(Length::Fixed(150.0))
                .padding(theme::tap([theme::SM, theme::MD]))
        };
        // Once it has arrived there is nothing left to cancel, so the card
        // offers one button that says what it does.
        let actions = match dialog.stage {
            ReportStage::Sent => row![
                Space::new().width(Length::Fill),
                tappable(
                    "close",
                    Some(Message::ReportCancel),
                    choice("Close").style(theme::filled(theme::ACCENT)),
                ),
            ],
            _ => row![
                Space::new().width(Length::Fill),
                // Only once there is something to lose. The screenshot is not
                // in it, the clipboard carries text, which the Discord
                // route solves on its own, since an image is pasted there.
                match dialog.stage {
                    ReportStage::Failed => tappable(
                        "copy report",
                        Some(Message::ReportCopy),
                        choice("Copy report").style(theme::outlined),
                    ),
                    _ => Element::from(Space::new().width(Length::Shrink)),
                },
                tappable("cancel", Some(Message::ReportCancel), choice("Cancel").style(theme::outlined)),
                tappable(
                    "send",
                    // Nothing is sent without a description, a category, and
                    // for a bug a way back: each one missing is an issue
                    // nobody can act on.
                    (writing && dialog.stage != ReportStage::TooSoon && dialog.can_send())
                        .then_some(Message::ReportSend),
                    choice(match dialog.stage {
                        ReportStage::Sending => "Sending…",
                        ReportStage::Failed => "Try again",
                        ReportStage::TooSoon => "Just sent",
                        _ => match dialog.kind {
                            ReportKind::Bug => "Send Bug Report",
                            ReportKind::Feedback => "Send Feedback",
                        },
                    })
                    .style(theme::filled(theme::ACCENT)),
                ),
            ],
        }
        .spacing(theme::SM)
        .align_y(Alignment::Center);

        container(column![section(title, body.into()), actions].spacing(theme::LG))
            .width(Length::Fixed(560.0))
            .padding(theme::LG)
            .style(theme::dialog)
            .into()
    }

    /// The screenshot line of the bug report card.
    ///
    /// A button rather than a drop target: iced has no file dialog of its own,
    /// so this opens the desktop's through the XDG portal, which is also the
    /// only one that works when the launcher is sandboxed.
    pub(crate) fn shot_row<'a>(&self, dialog: &'a ReportDialog) -> Element<'a, Message> {
        let choose = |label: &'static str| {
            tappable(
                "screenshot",
                Some(Message::ReportShotPick),
                button(text(label).size(theme::LABEL))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::outlined),
            )
        };

        let Some(shot) = &dialog.shot else {
            return row![
                choose("Add a screenshot…"),
                muted("Optional, and it is sent with the report.".to_owned(), theme::LABEL),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center)
            .into();
        };

        let (width, height) = shot.size;
        row![
            container(
                image(shot.preview.clone())
                    .height(Length::Fixed(72.0))
                    .content_fit(ContentFit::Contain),
            )
            .padding(theme::XS)
            .style(theme::row),
            column![
                text(shot.name.clone()).size(theme::LABEL),
                muted(format!("{width}×{height}, {}", size_of(shot.bytes.len() as u64)), theme::CAPTION),
            ]
            .spacing(theme::XS)
            .width(Length::Fill),
            choose("Replace…"),
            tappable(
                "remove screenshot",
                Some(Message::ReportShotCleared),
                button(text("Remove").size(theme::LABEL))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::outlined),
            ),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center)
        .into()
    }
}
