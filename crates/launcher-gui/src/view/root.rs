//! The window under the first-run wizard: a screen, and what is over it.

use crate::*;

impl Lightforge {
    /// The wizard's card, on the backdrop the rest of the app is drawn on.
    pub(crate) fn wizard_layer<'a>(&'a self, open: &'a wizard::Wizard) -> Element<'a, Message> {
        let card = wizard::view(self, open);
        // An open dropdown closes on a click anywhere else, as it does
        // on every other page. Wrapped either way: see `screen_stack`.
        let card = mouse_area(card);
        let card = Element::from(match self.panel_open() {
            true => card.on_press(Message::Dismiss),
            false => card,
        });
        container(card).width(Length::Fill).height(Length::Fill).style(theme::backdrop).into()
    }

    /// The screen being shown, with the download manager hanging off its
    /// button, any dialog over both, and the click that closes a dropdown.
    pub(crate) fn screen_stack(&self) -> Element<'_, Message> {
        let screen = match self.screen {
            Screen::Dashboard => self.dashboard(),
            Screen::Updates => self.updates_screen(),
            Screen::Settings => self.settings(),
            Screen::Addons => self.addons_screen(),
        };

        // The manager hangs under its button on every screen, not just the
        // dashboard: a download keeps running while you are in Settings, and
        // that is exactly when you want to look at it. Its button is at the
        // end of the ribbon, so a stacked layer pinned to the top right lands
        // under it, padded clear of the ribbon it drops out of. (Add-a-game
        // used to share this corner; it now hangs off the `+` itself, see
        // [`Lightforge::strip`].)
        let overlay: Option<Element<Message>> =
            match self.manager_open && !self.queue.is_empty() {
                true => Some(self.manager()),
                false => None,
            };
        // Clicks inside a panel are absorbed so they never reach the dismissal
        // beneath it; without this, clicking the gap between two rows of the
        // download manager would close it.
        let overlay = overlay.map(|panel| focus::layer(mouse_area(panel).on_press(Message::MenuClick)));

        let screen = match overlay {
            Some(panel) => stack![
                screen,
                container(panel)
                    .width(Length::Fill)
                    .height(Length::Fill)
                    .align_x(Alignment::End)
                    .align_y(Alignment::Start)
                    .padding([theme::XL + theme::LG, theme::LG]),
            ]
            .into(),
            None => screen,
        };

        // Modal: over everything, including the panels above. The scrim takes
        // every press that the card itself does not, and a `Stack` stops
        // delivering an event once a layer captures it, so nothing beneath
        // can be clicked while the dialog is up.
        let screen = match self.dialog() {
            Some((card, on_scrim)) => stack![
                screen,
                focus::layer(
                    mouse_area(container(card).center(Length::Fill).style(theme::scrim))
                        .on_press(on_scrim),
                ),
            ]
            .into(),
            None => screen,
        };

        // Anything open closes on a click that no widget wanted. `mouse_area`
        // hands the event to its child first and only fires when the child
        // ignored it, so a button still does its own job rather than being
        // swallowed by the dismissal.
        //
        // Wrapped whether or not anything is open, and only the press comes
        // and goes. Adding the wrapper when a dropdown opened changed the
        // widget tree above the page, iced dropped the page's scroll state,
        // and Settings jumped back to the top.
        let screen = mouse_area(screen);
        let screen = Element::from(match self.panel_open() {
            true => screen.on_press(Message::Dismiss),
            false => screen,
        });

        // The lit corner goes behind everything, last, so every screen gets it
        // and nothing has to opt in. Where a page lays key art over the whole
        // window the art simply wins, which is the right order: the glow is
        // for the chrome, not for the picture.
        let screen = Element::from(
            container(screen)
                .width(Length::Fill)
                .height(Length::Fill)
                .style(theme::backdrop),
        );
        screen
    }

    /// The dialog on top, if one is open, and what a click on the scrim
    /// around it sends.
    fn dialog(&self) -> Option<(Element<'_, Message>, Message)> {
        let card = match (
            &self.install_dialog,
            &self.uninstall_dialog,
            &self.cancel_dialog,
            &self.downgrade_dialog,
            &self.version_picker,
            &self.addon_detail,
            &self.report_dialog,
            &self.game_settings,
            &self.patch_notes,
            &self.update_notice,
        ) {
            (Some(dialog), ..) => Some(self.install_card(dialog)),
            (_, Some(dialog), ..) => Some(self.uninstall_card(dialog)),
            (_, _, Some(dialog), ..) => Some(self.cancel_card(dialog)),
            (_, _, _, Some(dialog), ..) => Some(self.downgrade_card(dialog)),
            (_, _, _, _, Some(picker), ..) => Some(self.version_card(picker)),
            (_, _, _, _, _, Some(detail), ..) => Some(self.detail_card(detail)),
            (_, _, _, _, _, _, Some(dialog), ..) => Some(self.report_card(dialog)),
            // Closes on a click outside it, so a click inside that no control
            // wanted has to stop at the card rather than reach the scrim.
            (_, _, _, _, _, _, _, Some(settings), ..) => Some(
                mouse_area(self.game_settings_card(settings)).on_press(Message::MenuClick).into(),
            ),
            // Last, so a dialog someone opened themselves is never buried
            // under the one the app opened for them. The notes come before
            // the update notice: they are about the build that is running,
            // which is the one the user can do something with right now.
            (.., Some(notes), _) => Some(self.patch_notes_card(notes)),
            (.., Some(available)) => Some(self.update_card(available)),
            _ => None,
        };
        let card = card
            .or_else(|| self.stop_dialog.as_ref().map(|d| self.stop_card(d)))
            .or_else(|| self.version_dialog.as_ref().map(|d| self.version_card_info(d)))
            .or_else(|| self.modify_dialog.as_ref().map(|d| self.modify_card(d)))
            .or_else(|| self.proton_installer.map(|tab| self.proton_installer_card(tab)));
        let settings_on_top = self.game_settings.is_some()
            && self.install_dialog.is_none()
            && self.uninstall_dialog.is_none()
            && self.cancel_dialog.is_none()
            && self.downgrade_dialog.is_none()
            && self.version_picker.is_none()
            && self.addon_detail.is_none()
            && self.report_dialog.is_none();
        // A click no control in the dialog wanted closes a dropdown open in
        // it, as a click on a page does; otherwise it only stops at the dialog.
        let on_scrim = match (self.panel_open(), settings_on_top) {
            (true, _) => Message::Dismiss,
            // Game Settings is the one card a click outside closes: it holds
            // no half-finished input.
            (false, true) => Message::CloseGameSettings,
            (false, false) => Message::MenuClick,
        };
        card.map(|card| (card, on_scrim))
    }
}
