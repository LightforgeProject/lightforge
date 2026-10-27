//! The bar along the top: logo, tabs, the game band and its menus.

use crate::*;
use iced::widget::column;

mod band;
mod panels;
mod selection;

impl Lightforge {
    /// Title, product picker, and navigation.
    pub(crate) fn header(&self) -> Element<'_, Message> {
        // Title, then the pages, then everything that is not a page pushed to
        // the far end. The band is no longer in this row at all, it has one
        // of its own below, so the tiles answer "which game" where the tabs
        // answer "which page", instead of the two sharing a line.
        let mut bar = row![self.app_menu_button()]
            .align_y(Alignment::Center)
            .spacing(theme::SM);

        // Back and forward, between the wordmark and the tabs as in any
        // browser. Both are drawn always and lit only when there is somewhere
        // to go: a control that appears and disappears moves the tabs beside
        // it, and arrows that vanish are harder to find than arrows that are
        // simply dim.
        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.extend(self.history_arrows());

        // The pages in a row of their own rather than loose in `bar`, so
        // [`TAB_GAP`] sets the air between tabs and nothing else: the row above
        // separates groups, wordmark, arrows, pages, and those two distances
        // are not the same distance.
        bar = bar.push(Space::new().width(Length::Fixed(GROUP_GAP)));
        bar = bar.push(self.page_tabs());
        bar = bar.push(Space::new().width(Length::Fill));

        // Only while something is in flight. An idle launcher has no
        // downloads to manage, and a permanent button that opens an empty
        // panel is furniture.
        if !self.queue.is_empty() {
            bar = bar.push(self.downloads_button());
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
            rows = rows.push(self.band_card());
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

    /// The app menu's button, and the menu hanging off it.
    fn app_menu_button(&self) -> Element<'_, Message> {
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

        popover::popover(
            self.ring(focus::Target::AppMenu, Some(Message::ToggleAppMenu), wordmark),
            // No `mouse_area` guard here: a popover captures the pointer over
            // its own bounds, which is what that guard is for on the stacked
            // panels; see [`popover`].
            self.app_menu_open.then(|| self.app_menu()),
        )
        .align_left()
        .into()
    }

    /// Back and forward, lit only when there is somewhere to go.
    fn history_arrows(&self) -> [Element<'_, Message>; 2] {
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
        [
            self.ring(
                focus::Target::Back,
                (!self.visited.is_empty()).then_some(Message::GoBack),
                step(back_icon(), Message::GoBack, !self.visited.is_empty()),
            ),
            self.ring(
                focus::Target::Forward,
                (!self.forward.is_empty()).then_some(Message::GoForward),
                step(forward_icon(), Message::GoForward, !self.forward.is_empty()),
            ),
        ]
    }

    /// The page tabs, and the shop, which leaves the app.
    fn page_tabs(&self) -> Element<'_, Message> {
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
        .align_y(Alignment::Center)
        .into()
    }

    /// The downloads button, with how many are in the queue.
    fn downloads_button(&self) -> Element<'_, Message> {
        let count = self.queue.len();
        self.ring(
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
        )
    }

    /// The band of tiles, as a card of its own under the tabs.
    fn band_card(&self) -> Element<'_, Message> {
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
        .padding([0.0, theme::LG])
        .into()
    }
}
