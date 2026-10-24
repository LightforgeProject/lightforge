//! The bar along the top: logo, tabs, the game band and its menus.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Title, product picker, and navigation.
    pub(crate) fn header(&self) -> Element<'_, Message> {
        let nav = |label: &'static str, screen: Screen| {
            let entry = button(text(label).size(theme::tap_text(TAB_LABEL)).font(theme::BOLD))
                .padding(theme::tap([theme::SM, theme::SM]));
            // Addons is a game's own page, opened
            // from its panel, so Play stays lit on them, and stays clickable,
            // since it is the way back.
            let within = match screen {
                Screen::Dashboard => matches!(self.screen, Screen::Addons),
                _ => false,
            };
            match (self.screen == screen, within) {
                (true, _) => entry.style(theme::selected),
                (false, true) => entry.style(theme::selected).on_press(Message::Show(screen)),
                (false, false) => entry.style(theme::ghost).on_press(Message::Show(screen)),
            }
        };

        // A tab that leaves the app rather than changing the screen, so it
        // cannot go through `nav`: there is no `Screen` to compare against and
        // nothing behind it to light up. It wears the arrow for exactly that
        // reason. It is the only thing in this row that opens a browser, and
        // without the mark it reads as a page that never arrives.
        let outbound = |label: &'static str, url: &'static str| {
            button(
                row![
                    text(label).size(theme::tap_text(TAB_LABEL)).font(theme::BOLD),
                    image(outbound_icon())
                        .width(Length::Fixed(OUTBOUND_GLYPH))
                        .height(Length::Fixed(OUTBOUND_GLYPH)),
                ]
                .spacing(OUTBOUND_GAP)
                .align_y(Alignment::Center),
            )
            .padding(theme::tap([theme::SM, theme::SM]))
            .style(theme::ghost)
            .on_press(Message::OpenLink(url))
        };

        // The wordmark is the app menu's button, so it carries a chevron the
        // way every other menu in the app does.
        let wordmark = button(
            row![
                image(wordmark_image())
                    .width(Length::Fixed(WORDMARK_HEIGHT * WORDMARK_ASPECT))
                    .height(Length::Fixed(WORDMARK_HEIGHT)),
                image(chevron_icon())
                    .width(Length::Fixed(APP_MENU_CHEVRON))
                    .height(Length::Fixed(APP_MENU_CHEVRON)),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::tap([theme::XS, theme::SM]))
        .style(match self.app_menu_open {
            true => theme::selected,
            false => theme::ghost,
        })
        .on_press(Message::ToggleAppMenu);

        // Title, then the pages, then everything that is not a page pushed to
        // the far end. The band is no longer in this row at all, it has one
        // of its own below, so the tiles answer "which game" where the tabs
        // answer "which page", instead of the two sharing a line.
        let mut bar = row![popover::popover(
            self.ring(focus::Target::AppMenu, Some(Message::ToggleAppMenu), wordmark),
            // No `mouse_area` guard here: a popover captures the pointer over
            // its own bounds, which is what that guard is for on the stacked
            // panels; see [`popover`].
            self.app_menu_open.then(|| self.app_menu()),
        )
        .align_left()]
        .align_y(Alignment::Center)
        .spacing(theme::SM);

        // Back and forward, between the wordmark and the tabs as in any
        // browser. Both are drawn always and lit only when there is somewhere
        // to go: a control that appears and disappears moves the tabs beside
        // it, and arrows that vanish are harder to find than arrows that are
        // simply dim.
        let step = |icon: image::Handle, message: Message, enabled: bool| {
            button(
                image(icon)
                    .width(Length::Fixed(BAND_GLYPH * 0.85))
                    .height(Length::Fixed(BAND_GLYPH * 0.85))
                    .opacity(match enabled {
                        true => 1.0_f32,
                        false => DIMMED,
                    }),
            )
            .padding(theme::tap([theme::XS, theme::SM]))
            .style(theme::ghost)
            .on_press_maybe(enabled.then_some(message))
        };

        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.push(self.ring(
            focus::Target::Back,
            (!self.visited.is_empty()).then_some(Message::GoBack),
            step(back_icon(), Message::GoBack, !self.visited.is_empty()),
        ));
        bar = bar.push(self.ring(
            focus::Target::Forward,
            (!self.forward.is_empty()).then_some(Message::GoForward),
            step(forward_icon(), Message::GoForward, !self.forward.is_empty()),
        ));

        // The pages in a row of their own rather than loose in `bar`, so
        // [`TAB_GAP`] sets the air between tabs and nothing else: the row above
        // separates groups, wordmark, arrows, pages, and those two distances
        // are not the same distance.
        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.push(
            row![
                self.ring(
                    focus::Target::Tab(Screen::Dashboard),
                    Some(Message::Show(Screen::Dashboard)),
                    nav("Play", Screen::Dashboard),
                ),
                self.ring(
                    focus::Target::Shop,
                    Some(Message::OpenLink(SHOP_URL)),
                    outbound("Battle.net Shop", SHOP_URL),
                ),
            ]
            .spacing(TAB_GAP)
            .align_y(Alignment::Center),
        );
        bar = bar.push(Space::new().width(Length::Fill));

        // Only while something is in flight. An idle launcher has no
        // downloads to manage, and a permanent button that opens an empty
        // panel is furniture.
        if !self.queue.is_empty() {
            let count = self.queue.len();
            bar = bar.push(self.ring(
                focus::Target::Downloads,
                Some(Message::ToggleManager),
                // A drawn mark, like the back and forward arrows beside it; a
                // typed arrow sat on the text baseline at the font's weight
                // and read as a stray character.
                button(
                    row![
                        image(download_icon())
                            .width(Length::Fixed(BAND_GLYPH * 0.85))
                            .height(Length::Fixed(BAND_GLYPH * 0.85)),
                        text(count.to_string()).size(TAB_LABEL),
                    ]
                    .spacing(theme::XS)
                    .align_y(Alignment::Center),
                )
                .padding(theme::tap([theme::XS, theme::SM]))
                .style(match self.manager_open {
                    true => theme::selected,
                    false => theme::ghost,
                })
                .on_press(Message::ToggleManager),
            ));
            bar = bar.push(Space::new().width(Length::Fixed(theme::SM)));
        }

        // No backdrop behind the tabs. The wordmark, the arrows and the pages
        // are chrome, and chrome over artwork needs no bar of its own to be
        // legible, what made the old ribbon necessary was the band sharing
        // its line, and the band has moved out.
        let mut rows =
            column![container(bar).width(Length::Fill).padding([theme::SM, theme::LG])];

        // The band as a card of its own, inset from the window and detached
        // from the row above, the same `floating` treatment as the action
        // panel at the foot of the dashboard, so the two read as the same kind
        // of object: a surface laid over the art rather than a strip welded to
        // the window's edge. `responsive` hands `strip` the room it actually
        // has, which is what decides how many tiles fit before the overflow
        // takes the rest.
        if self.shows_band() {
            rows = rows.push(
                container(
                    container(
                        row![
                            // Labelled, the way Battle.net labels its own
                            // band: the tiles are marks without words, and one
                            // quiet word says what the row is before you know
                            // them.
                            // Centred on the letters, not on the line box;
                            // see [`optically_centred`].
                            container(optically_centred(
                                muted("GAMES".to_owned(), theme::LABEL).font(theme::BOLD),
                            ))
                            .height(Length::Fill)
                            .align_y(Alignment::Center),
                            responsive(|space| self.strip(space.width)),
                        ]
                        // Only where there is a client to open. Declining
                        // Battle.net during setup is a supported choice, and a
                        // button that opens something that is not installed is
                        // worse than no button.
                        .extend((!self.config.battle_net_declined).then(|| {
                            Element::from(
                                container(self.ring(
                                    focus::Target::BattleNet,
                                    (!self.busy).then_some(Message::OpenBattleNet),
                                    self.battle_net_button(),
                                ))
                                    .height(Length::Fill)
                                    .align_y(Alignment::Center),
                            )
                        }))
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    )
                    // Centred, not merely given the height: a container aligns
                    // its content to the top by default, so the row's slack
                    // all fell below the tiles and they sat against the card's
                    // upper edge. The row's own `align_y` cannot fix that, it
                    // centres the tiles within the row, which is exactly as
                    // tall as they are.
                    .align_y(Alignment::Center)
                    .height(Length::Fixed(strip_height()))
                    .padding([0.0, theme::MD])
                    .style(theme::floating),
                )
                .width(Length::Fill)
                .padding([0.0, theme::LG]),
            );
        }

        // The hairline only where the band card is not: the card is the
        // separation where it appears, and on a page without one a header with
        // no backdrop would otherwise run straight into the content.
        match self.shows_band() {
            true => rows.push(Space::new().height(Length::Fixed(theme::SM))).into(),
            false => rows
                .push(
                    container(Space::new().height(Length::Fixed(1.0)))
                        .width(Length::Fill)
                        .style(theme::ribbon_edge),
                )
                .into(),
        }
    }

    /// The configured lines, in the order their first product appears.
    ///
    /// A line, not a product: PTR and beta are builds of a game, not games
    /// beside it, so five World of Warcrafts collapse to three tiles and a
    /// version picker.
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
    /// The band's order is the configured products' order, so that is what
    /// changes; the choices follow so the band redraws without a reload. A
    /// stable sort keeps a line's PTR and beta where they were behind it.
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

    /// Rebuild the catalog from whatever manifest is in hand.
    pub(crate) fn refresh_catalog(&mut self) {
        let mut catalog = match &self.manifest {
            Some(manifest) => manifest.catalog(),
            None => Catalog::builtin(),
        };
        catalog.reconcile(&self.known_codes);
        self.catalog = catalog;
    }

    /// The catalog: everything that could be added to the header.
    ///
    /// Two halves, because they behave differently and saying so once here
    /// beats a surprise at the button. The WoW family Lightforge downloads;
    /// everything else is Battle.net's to install and start.
    pub(crate) fn browse(&self) -> Element<'_, Message> {
        use lightforge_core::product::CatalogEntry;

        let tracked = |code: &ProductCode| self.choices.iter().any(|c| &c.code == code);
        let mut rows = column![text("All Games").size(theme::HEADING)]
            .spacing(theme::SM)
            .width(Length::Fixed(400.0));

        let row_for = |entry: &CatalogEntry| -> Element<Message> {
            let mut facts = column![text(entry.label.clone()).size(theme::BODY)];
            // Whether it is installed, not which build: the version belongs to
            // the panel you launch from, and this list is for choosing which
            // games to keep in the band.
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
            // The mark the band would wear, so a row and its tile are plainly
            // the same game. The column is held even when there is no icon,
            // hand-offs have none, and art arrives one product at a time, so
            // the labels stay in line instead of stepping in and out.
            let mark: Element<Message> = match self.icons.get(&entry.code) {
                Some(mark) => image(mark.handle.clone())
                    .content_fit(ContentFit::Contain)
                    .width(Length::Fixed(BROWSE_MARK))
                    .height(Length::Fixed(BROWSE_MARK))
                    .into(),
                None => Space::new().width(Length::Fixed(BROWSE_MARK)).into(),
            };
            // The whole row opens the game's page, tracked or not; Add and
            // Remove inside it take their own clicks first and stay the only
            // things that change the band.
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

        // Only line heads: a line's other branches are reached from the
        // version picker, not added here as games of their own.
        // Tracked lines stay listed, or there would be no way to remove one.
        // By head, not by `Branch::Live`: Forever's head is its beta.
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

        // Embedded rather than floating: the default scrollbar is drawn over
        // the content, and the rows end in a button, so it landed on top of
        // Add and Remove. Given its own column it sits beside them instead.
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
    /// One row each, with the controls that row can actually offer, a queued
    /// transfer has no progress to show and a running one cannot be resumed.
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

    /// What to call a line on screen.
    ///
    /// The name the user gave it first, then the catalog's, and only then the
    /// line's own, which for a line Lightforge does not model is the bare
    /// product code, and `hs_beta` is not a thing to label a tile with.
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
    /// Hand-offs included: whether a tile shows art depends on whether art
    /// exists, not on who installs the game. The head's mark first, then any
    /// other branch's: Forever's head is `wowf`, whose art stays on
    /// `wow_classic_beta` (see `Manifest::head_theme`).
    pub(crate) fn tile_art(&self, line: &ProductLine) -> Option<&tasks::Mark> {
        line.branches().iter().find_map(|code| self.icons.get(code))
    }

    /// One tile.
    ///
    /// A line with no icon in the manifest falls back to its label, so the
    /// band is complete and usable before any art exists and fills in one line
    /// at a time as art is published.
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

        // The button only for its look. A pressable one keeps its pressed
        // state by position in the row, and a drag moves tiles between
        // positions, so a tile let go after a drag could fire a different
        // tile's press. The mouse area around it takes press, hover, and
        // release instead, and [`Message::TileDropped`] tells a click from a
        // drag. With no `on_press` the button always reports `Disabled`, so
        // the hover it would have shown is passed in from what is tracked.
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
        // tooltip gives way while it is open rather than being drawn over it.
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
    /// Remove takes the tile out of the band and nothing else: the game stays
    /// installed and can be added back from Add a game. Held when it is the
    /// last tile, for the reason [`Message::RemoveProduct`] refuses it.
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
    /// Exact for art: a fixed square in fixed padding. A line falling back to
    /// its name cannot be measured without a renderer, so its width is
    /// estimated from the glyph count at a width no glyph of this size
    /// exceeds. Erring wide costs a little unused band; erring narrow would
    /// cut a tile in half against the navigation, so the estimate leans wide
    /// on purpose.
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
    /// `room` is the width the header has left over once the title, the
    /// navigation and the buttons beside it have taken theirs, so the band
    /// never has to know the window's size; see [`Self::header`].
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

        // Fills the height it is given, rather than shrinking to the tiles.
        // `responsive` lays its content out at the top of the box, so a band
        // that is only as tall as its tiles leaves all of the card's slack
        // underneath them and they ride high in it. Filling first, then
        // centring, puts the slack on both sides.
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
            // The panel hangs under the button it belongs to rather than in
            // the corner like the download manager: which games are behind it
            // is the button's own answer, and the corner is a long way from
            // the band on a wide window.
            band = band.push(popover::popover(
                more,
                self.strip_open.then(|| self.overflow_menu(&hidden)),
            ));
        }

        // Under the `+` itself, not in the corner: the button moves along the
        // band as games are added and removed, and the panel is the button's
        // own answer. Pinned by its left edge, since the `+` has the rest of
        // the ribbon to its right to grow into.
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
    /// Only the dashboard. Settings and a game's own pages (Addons, Game
    /// Optimizations) are somewhere you went *into*: they are about one thing,
    /// and a band offering to switch games above them invites a click that
    /// would throw away what you are doing.
    pub(crate) fn shows_band(&self) -> bool {
        matches!(self.screen, Screen::Dashboard)
    }

    /// The hand-off into Battle.net, at the right-hand end of the game card.
    ///
    /// App-level, not per-product: everything outside the WoW family is a
    /// hand-off, so it belongs with the games rather than on a panel that
    /// describes one of them, and the card is where games live now.
    ///
    /// The mark comes out of the prefix rather than the binary: see
    /// [`Layout::battle_net_icon`]. Before Battle.net is installed there is
    /// none, and the button is the word alone.
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

    /// Every branch of the selected line, configured or not.
    ///
    /// `None` when the line has exactly one branch to offer, a picker with a
    /// single entry is furniture.
    pub(crate) fn branch_choices(&self) -> Option<(Vec<BranchChoice>, BranchChoice)> {
        let line = self.catalog.line_of(&self.selected.code);
        let choices: Vec<BranchChoice> = self
            .catalog
            .branches(&line)
            .into_iter()
            .map(|entry| BranchChoice {
                configured: self.choices.iter().any(|c| c.code == entry.code),
                branch: entry.branch,
                name: self
                    .families
                    // The UI is English; the install locale is for the game's files.
                    .version_name(entry.code.as_str(), "default")
                    .map(str::to_owned),
                code: entry.code.clone(),
                label: entry.label.clone(),
            })
            .collect();

        if choices.len() < 2 {
            return None;
        }
        let current = choices.iter().find(|c| c.code == self.selected.code)?.clone();
        Some((choices, current))
    }

    /// Switch to `line`, keeping whichever of its branches was last in use.
    ///
    /// Falls back to the line's live build, then to whatever of it *is*
    /// configured: selecting a tile must always land somewhere.
    pub(crate) fn select_line(&mut self, line: &ProductLine) -> Task<Message> {
        let remembered = self.last_branch.get(line).cloned();
        let configured = |code: &ProductCode| self.choices.iter().find(|c| &c.code == code).cloned();

        let choice = remembered
            .as_ref()
            .and_then(configured)
            .or_else(|| configured(&line.head()))
            .or_else(|| {
                self.choices
                    .iter()
                    .find(|c| &self.catalog.line_of(&c.code) == line)
                    .cloned()
            });

        match choice {
            Some(choice) => self.select(choice),
            None => Task::none(),
        }
    }

    /// Switch to one product, remembering it as its line's current branch.
    pub(crate) fn select(&mut self, choice: Choice) -> Task<Message> {
        if choice.code == self.selected.code {
            return Task::none();
        }
        self.last_branch.insert(self.catalog.line_of(&choice.code), choice.code.clone());
        self.selected = choice.clone();
        self.local = Local::default();
        self.remote = None;
        self.branding = None;
        self.status = "checking…".to_owned();
        // These are per-product, so they follow the selection rather than
        // carrying Retail's frame cap over to Classic's panel.
        self.load_tweak_inputs();
        self.reset_addon_screen();
        Task::batch([
            self.reload(choice.code),
            Task::done(Message::AutoUpdateAddons),
            // The popular list is per flavor, so it follows the selection for
            // the same reason the installed list does.
            match self.addon_tab == AddonTab::Browse {
                true => self.popular_if_unasked(),
                false => Task::none(),
            },
        ])
    }

    /// Load the popular list, unless a search is already in flight.
    ///
    /// Three things ask for it, opening Browse, clearing the box, switching
    /// product, and all three can happen while a typed search is still out.
    /// Its reply would be dropped by the query check anyway; not sending it is
    /// cheaper and keeps the spinner honest.
    pub(crate) fn popular_if_unasked(&mut self) -> Task<Message> {
        match self.searching {
            true => Task::none(),
            false => Task::done(Message::RunSearch),
        }
    }
}
