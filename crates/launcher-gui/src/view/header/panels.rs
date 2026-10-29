//! The panels that hang off the header: Add a game, and the download manager.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The catalog: everything that could be added to the header.
    ///
    /// Two halves: the WoW family, and Battle.net's games.
    pub(crate) fn browse(&self) -> Element<'_, Message> {
        use lightforge_core::product::CatalogEntry;

        let tracked = |code: &ProductCode| self.choices.iter().any(|c| &c.code == code);
        let mut rows = column![text("All Games").size(theme::HEADING)]
            .spacing(theme::SM)
            .width(Length::Fixed(400.0));

        let row_for = |entry: &CatalogEntry| -> Element<Message> {
            let mut facts = column![text(entry.label.clone()).size(theme::BODY)];
            // Installed or not; the version belongs to the game's panel.
            if self.installed_versions.contains_key(entry.code.as_str()) {
                facts = facts.push(muted("installed".to_owned(), theme::CAPTION));
            }
            // Remove takes the whole line, so it is held only when nothing
            // outside that line is left to fall back to.
            let elsewhere = |code: &ProductCode| {
                let line = self.catalog.line_of(code);
                self.choices.iter().any(|c| self.catalog.line_of(&c.code) != line)
            };
            let action: Element<Message> = match tracked(&entry.code) {
                true => secondary("Remove", Message::RemoveProduct(entry.code.clone()))
                    .keyed(format!("game {}", entry.code.as_str()), elsewhere(&entry.code)),
                false => secondary("Add", Message::AddProduct(entry.code.clone()))
                    .keyed(format!("game {}", entry.code.as_str()), true),
            };
            // The band's mark; the column is held without one, so labels align.
            let mark: Element<Message> = match self.icons.get(&entry.code) {
                Some(mark) => image(mark.handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BROWSE_MARK))
                    .height(Length::Fixed(BROWSE_MARK))
                    .into(),
                None => Space::new().width(Length::Fixed(BROWSE_MARK)).into(),
            };
            // The row opens the game's page; Add and Remove take their own clicks.
            let open = Message::OpenProduct(entry.code.clone());
            tappable(
                format!("open {}", entry.code.as_str()),
                Some(open.clone()),
                button(
                    row![mark, facts.width(Length::Fill), action]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                )
                .width(Length::Fill)
                .padding(theme::SM)
                .style(theme::row_button)
                .on_press(open),
            )
        };

        // Line heads only (Forever's head is its beta), tracked ones included.
        let managed: Vec<&CatalogEntry> =
            self.catalog.managed().filter(|e| e.code == e.line.head()).collect();
        if !managed.is_empty() {
            rows = rows.push(muted("World of Warcraft".to_owned(), theme::LABEL));
            rows = rows.extend(self.hint("Downloaded and launched by Lightforge.").map(Element::from));
            for entry in managed {
                rows = rows.push(row_for(entry));
            }
        }

        rows = rows.push(muted("Other Blizzard games".to_owned(), theme::LABEL));
        match self.config.battle_net_declined {
            // Nothing to hand them to. Listed anyway would be a row whose Add
            // button adds a tile that cannot do anything.
            true => {
                rows = rows.push(muted(
                    "Battle.net is not installed — add it from Settings › Setup.".to_owned(),
                    theme::CAPTION,
                ));
            }
            false => {
                rows = rows.extend(self.hint("Installed and started through Battle.net. Lightforge only opens them.").map(Element::from));
                for entry in self.catalog.hand_offs() {
                    rows = rows.push(row_for(entry));
                }
            }
        }

        // An embedded scrollbar, not over the row buttons.
        let panel = container(
            focus::region(
                "browse",
                scrollable(rows)
                    .direction(scrollable::Direction::Vertical(theme::scrollbar_shape()))
                    .style(theme::scrollbar),
            ),
        )
        .padding(theme::MD)
        .max_height(560.0)
        .style(theme::floating);
        focus::layer(panel)
    }

    /// The download manager: every transfer this session knows about.
    ///
    /// One row each, with the controls that row can offer.
    pub(crate) fn manager(&self) -> Element<'_, Message> {
        let mut rows = column![
            row![
                text("Downloads").size(theme::HEADING),
                Space::new().width(Length::Fill),
                muted(
                    match self.queue.len() {
                        1 => "1 in queue".to_owned(),
                        n => format!("{n} in queue"),
                    },
                    theme::CAPTION,
                ),
            ]
            .align_y(Alignment::Center),
        ]
        .spacing(theme::SM)
        .width(Length::Fixed(380.0));

        for entry in &self.queue {
            let mut item = column![text(entry.label.clone()).size(theme::BODY)].spacing(theme::XS);

            match &entry.stage {
                Stage::Queued => {
                    item = item.push(muted("waiting".to_owned(), theme::CAPTION));
                }
                Stage::Running { .. } => {
                    let progress = entry.progress();
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).girth(5));
                    item = item.push(
                        row![
                            mono(bytes_of(progress.downloaded_bytes, progress.total_bytes)),
                            dot(),
                            mono(rate_of(progress.bytes_per_second)),
                            dot(),
                            mono(eta_of(progress)),
                        ]
                        .spacing(theme::XS)
                        .align_y(Alignment::Center),
                    );
                }
                Stage::Paused(progress) => {
                    item = item.push(progress_bar(0.0..=1.0, progress.fraction()).girth(5));
                    item = item.push(muted("paused".to_owned(), theme::CAPTION));
                }
            }

            let control = match &entry.stage {
                Stage::Running { .. } => secondary("Pause", Message::PauseDownload(entry.code.clone())),
                Stage::Queued | Stage::Paused(_) => {
                    secondary("Resume", Message::ResumeDownload(entry.code.clone()))
                }
            }
            .enabled(true);

            rows = rows.push(
                container(
                    row![
                        item.width(Length::Fill),
                        column![
                            control,
                            secondary("Remove", Message::CancelDownload(entry.code.clone()))
                                .enabled(true),
                        ]
                        .spacing(theme::XS),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
                )
                .padding(theme::SM)
                .style(theme::row),
            );
        }

        container(rows)
            .padding(theme::MD)
            .style(theme::floating)
            .into()
    }
}
