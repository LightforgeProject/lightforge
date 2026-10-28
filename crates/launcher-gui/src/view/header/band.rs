//! The band of game tiles: each tile, its menu and width, the strip that fits them, and Battle.net beside it.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The configured lines, in the order their first product appears.
    ///
    /// A line, not a product: PTR and beta sit behind a version picker.
    pub(crate) fn lines(&self) -> Vec<ProductLine> {
        let mut lines: Vec<ProductLine> = Vec::new();
        for choice in &self.choices {
            let line = self.catalog.line_of(&choice.code);
            if !lines.contains(&line) {
                lines.push(line);
            }
        }
        lines
    }

    /// Move the line at `from` in the band to `to`, its products with it.
    ///
    /// Reorders the configured products, stably.
    pub(crate) fn move_line(&mut self, from: usize, to: usize) {
        let mut order = self.lines();
        let line = order.remove(from);
        order.insert(to, line);
        let catalog = &self.catalog;
        let rank = |code: &ProductCode| {
            let line = catalog.line_of(code);
            order.iter().position(|l| l == &line).unwrap_or(usize::MAX)
        };
        self.config.products.sort_by_key(|p| rank(&p.code));
        self.choices.sort_by_key(|c| rank(&c.code));
    }

    /// What to call a line on screen.
    ///
    /// The user's name, then the catalog's, then the line's own.
    pub(crate) fn name_of(&self, line: &ProductLine) -> String {
        let head = line.head();
        self.choices
            .iter()
            .find(|c| c.code == head)
            .map(|c| c.label.clone())
            .or_else(|| self.catalog.get(&head).map(|e| e.label.clone()))
            .unwrap_or_else(|| line.label().to_owned())
    }

    /// The mark a line's tile wears, if the manifest gives it one.
    ///
    /// Hand-offs included. The head's mark, then any branch's (Forever's art
    /// stays on `wow_classic_beta`).
    pub(crate) fn tile_art(&self, line: &ProductLine) -> Option<&tasks::Mark> {
        line.branches().iter().find_map(|code| self.icons.get(code))
    }

    /// One tile.
    ///
    /// Without an icon, the label.
    pub(crate) fn tile(&self, line: &ProductLine) -> Element<'_, Message> {
        let selected = line == &self.catalog.line_of(&self.selected.code);
        let face: Element<Message> = match self.tile_art(line) {
            Some(mark) => image(mark.handle.clone())
                .content_fit(ContentFit::Contain)
                .width(Length::Fixed(theme::tile()))
                .height(Length::Fixed(theme::tile()))
                .into(),
            None => container(text(self.name_of(line)).size(theme::LABEL))
                .center_x(Length::Shrink)
                .center_y(Length::Fixed(theme::tile()))
                .into(),
        };

        // The button only for its look: its press is tracked by position, and
        // a drag moves tiles. The mouse area takes press, hover and release;
        // [`Message::TileDropped`] tells a click from a drag.
        let hovered = self.tile_hover.as_ref() == Some(line);
        let held = self.tile_drag.as_ref().is_some_and(|drag| &drag.line == line);
        let status = match hovered || held {
            true => button::Status::Hovered,
            false => button::Status::Active,
        };
        let tile = button(face).padding([theme::XS, theme::SM]);
        let tile = match selected {
            true => tile.style(theme::selected),
            false => tile.style(move |theme, _| theme::ghost(theme, status)),
        };
        let dragging = self.tile_drag.as_ref().is_some_and(|drag| drag.moving);
        let tile = self.ring(
            focus::Target::Tile(line.clone()),
            Some(Message::LineSelected(line.clone())),
            tile,
        );
        let tile = mouse_area(tile)
            .on_press(Message::TileGrabbed(line.clone()))
            .on_right_press(Message::TileMenu(line.clone()))
            .on_enter(Message::TileEntered(line.clone()))
            .on_exit(Message::TileLeft(line.clone()))
            .interaction(match dragging {
                true => iced::mouse::Interaction::Grabbing,
                false => iced::mouse::Interaction::Pointer,
            });

        // The menu hangs under the tile, where the tooltip would, so the
        // tooltip gives way while it is open instead of being drawn over it.
        if self.tile_menu.as_ref() == Some(line) {
            return popover::popover(tile, Some(self.tile_menu_panel(line))).align_left().below().into();
        }
        tooltip(
            tile,
            container(text(self.name_of(line)).size(theme::LABEL))
                .padding([theme::XS, theme::SM])
                .style(theme::floating),
            tooltip::Position::Bottom,
        )
        .into()
    }

    /// A tile's right-click menu.
    ///
    /// Remove only takes the tile out of the band; never the last one.
    pub(crate) fn tile_menu_panel(&self, line: &ProductLine) -> Element<'_, Message> {
        let elsewhere = self.lines().iter().any(|other| other != line);
        let face = row![
            image(close_icon())
                .width(Length::Fixed(MENU_GLYPH))
                .height(Length::Fixed(MENU_GLYPH))
                .opacity(if elsewhere { 1.0_f32 } else { 0.4_f32 }),
            text("Remove from list").size(theme::BODY).color_maybe((!elsewhere).then_some(theme::MUTED)),
        ]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
        let entry = button(face.width(Length::Fill))
            .padding(theme::tap([theme::XS, theme::SM]))
            .style(theme::ghost)
            .on_press_maybe(elsewhere.then(|| Message::RemoveProduct(line.head())));
        container(column![entry].width(Length::Fixed(200.0)))
            .padding(theme::SM)
            .style(theme::menu)
            .into()
    }

    /// How wide a tile will be.
    ///
    /// Exact for art; a label is estimated from its glyph count, leaning wide.
    pub(crate) fn tile_width(&self, line: &ProductLine) -> f32 {
        let padding = theme::SM * 2.0;
        match self.tile_art(line) {
            Some(_) => theme::tile() + padding,
            None => {
                let glyphs = self.name_of(line).chars().count() as f32;
                glyphs * theme::LABEL * 0.62 + padding
            }
        }
    }

    /// The game band: the tiles that fit, then Add a game, and whatever did
    /// not fit behind an overflow button.
    ///
    /// `room` is what the header leaves ([`Self::header`]).
    pub(crate) fn strip(&self, room: f32) -> Element<'_, Message> {
        let lines = self.lines();
        // The tile the ring is on stays in view in place of the selected one,
        // or the arrows would walk into the overflow and out of sight.
        let current = match &self.focus {
            Some(focus::Target::Tile(line)) => line.clone(),
            _ => self.catalog.line_of(&self.selected.code),
        };
        let selected = lines.iter().position(|line| line == &current).unwrap_or(0);
        let widths: Vec<f32> = lines.iter().map(|line| self.tile_width(line)).collect();

        // Add a game sits at the end of the band whatever happens, so its
        // width is not the band's to spend.
        let (shown, hidden) = divide_strip(&widths, selected, room - BAND_BUTTON);

        // Fills, then centres: `responsive` lays out at the top.
        let mut band =
            row![].height(Length::Fill).align_y(Alignment::Center).spacing(theme::SM);
        for id in shown {
            band = band.push(self.tile(&lines[id]));
        }

        if !hidden.is_empty() {
            let hidden: Vec<ProductLine> = hidden.into_iter().map(|id| lines[id].clone()).collect();
            let more = self.ring(
                focus::Target::More,
                Some(Message::ToggleStrip),
                band_button(
                    image(chevron_icon())
                        .width(Length::Fixed(BAND_GLYPH))
                        .height(Length::Fixed(BAND_GLYPH)),
                    Message::ToggleStrip,
                    self.strip_open,
                    format!("{} more game{}", hidden.len(), match hidden.len() {
                        1 => "",
                        _ => "s",
                    }),
                ),
            );
            // Under its button, not in the corner.
            band = band.push(popover::popover(
                more,
                self.strip_open.then(|| self.overflow_menu(&hidden)),
            ));
        }

        // Under the `+`, by its left edge.
        band = band.push(
            popover::popover(
                self.ring(
                    focus::Target::AddGame,
                    Some(Message::ToggleBrowse),
                    band_button(
                        image(plus_icon())
                            .width(Length::Fixed(BAND_GLYPH))
                            .height(Length::Fixed(BAND_GLYPH)),
                        Message::ToggleBrowse,
                        self.browse_open,
                        "All Games".to_owned(),
                    ),
                ),
                // The popover absorbs clicks inside the panel itself, so no
                // `mouse_area` guard is needed here; see [`popover`].
                self.browse_open.then(|| self.browse()),
            )
            .align_left(),
        );
        band.into()
    }

    /// Which screens wear the game band under the tabs.
    ///
    /// Only the dashboard.
    pub(crate) fn shows_band(&self) -> bool {
        matches!(self.screen, Screen::Dashboard)
    }

    /// The hand-off into Battle.net, at the right-hand end of the game card.
    ///
    /// The mark comes from the prefix ([`Layout::battle_net_icon`]); before
    /// Battle.net is installed, the word alone.
    pub(crate) fn battle_net_button(&self) -> Element<'_, Message> {
        let mut face = row![].spacing(theme::SM).align_y(Alignment::Center);
        if let Some(icon) = &self.bnet_icon {
            face = face.push(
                image(icon.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BNET_GLYPH))
                    .height(Length::Fixed(BNET_GLYPH)),
            );
        }
        // Same optical correction as the band's label, the word has no
        // descenders either, and it sits beside a mark that fills its box.
        let face = face.push(optically_centred(
            text("Battle.net").size(theme::LABEL).font(theme::BOLD),
        ));

        tooltip(
            button(face)
                .padding([theme::SM, theme::MD])
                .style(theme::ghost)
                .on_press_maybe((!self.busy).then_some(Message::OpenBattleNet)),
            container(
                text("Open Battle.net for other Blizzard titles".to_owned()).size(theme::LABEL),
            )
            .padding([theme::XS, theme::SM])
            .style(theme::floating),
            tooltip::Position::Bottom,
        )
        .into()
    }
}
