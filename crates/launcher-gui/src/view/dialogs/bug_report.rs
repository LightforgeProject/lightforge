//! The bug report card, its screenshot row, and what gets attached.

use crate::*;
use iced::widget::column;

impl Lightforge {
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
