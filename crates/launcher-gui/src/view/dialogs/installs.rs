//! Installing and uninstalling: the install, downgrade, cancel and uninstall cards.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The install dialog's card.
    pub(crate) fn install_card<'a>(&'a self, dialog: &'a InstallDialog) -> Element<'a, Message> {
        let mut drives = column![].spacing(theme::SM);
        for (index, target) in dialog.targets.iter().enumerate() {
            drives = drives.push(install_target_row(index, target, index == dialog.chosen));
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
}

/// One drive the install can go on: its name and path, its free space, and
/// whether it would share a data store with what is already there.
fn install_target_row(index: usize, target: &InstallTarget, chosen: bool) -> Element<'_, Message> {
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
    tappable(
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
    )
}
