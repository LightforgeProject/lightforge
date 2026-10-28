//! Moving between screens, the selection ring, and the console look.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Explanatory prose under a control, drawn only when the hints are on.
    ///
    /// `None`, not an empty widget, which would still cost its spacing.
    pub(crate) fn hint<'a>(&self, content: impl Into<String>) -> Option<iced::widget::Text<'a>> {
        self.hints.then(|| muted(content.into(), theme::CAPTION))
    }

    /// Go to a page, remembering where you were.
    ///
    /// The page you are on is not a history step.
    pub(crate) fn go_to(&mut self, screen: Screen) {
        if screen != self.screen {
            self.visited.push(self.screen);
            self.forward.clear();
            self.screen = screen;
        }
    }

    /// Work a page needs on the way in.
    ///
    /// From a tab, back and forward alike; `go_to` cannot return a `Task`.
    pub(crate) fn arrived(&mut self) -> Task<Message> {
        match self.screen {
            // An empty Browse asks what is there, however it was reached.
            Screen::Addons
                if self.addon_tab == AddonTab::Browse
                    && self.search_input.trim().is_empty()
                    && self.search.listings.is_empty() =>
            {
                self.popular_if_unasked()
            }
            Screen::Settings => self.settings_arrived(),
            _ => Task::none(),
        }
    }

    /// Settings opened: at the top, or at the section a link asked for.
    ///
    /// Fetches what the sections need: the Proton release list when there is
    /// none, and a drive scan, which also finds a library that moved.
    pub(crate) fn settings_arrived(&mut self) -> Task<Message> {
        // At the top unless a link said otherwise.
        self.settings_tab = self.settings_jump.unwrap_or(SettingsTab::ALL[0]);
        self.settings_jump = Some(self.settings_tab);
        self.settings_jump_tries = 0;
        self.settings_anchor = None;
        self.settings_viewport = None;
        self.refresh_drives();
        self.drive_notice = None;
        let proton = match self.proton_available.is_empty() {
            true => Task::done(Message::CheckProton),
            false => Task::none(),
        };
        Task::batch([proton, self.measure_storage(), self.check_unused(), measure_settings()])
    }

    /// Whether the controller and touch look is on: the answer setup asks
    /// for on SteamOS, or the switch in Settings › General anywhere.
    pub(crate) fn console(&self) -> bool {
        self.config.steamos_use == Some(SteamOsUse::Console)
    }

    /// Put this program in the Steam library of the account that signed in
    /// last, with its artwork; the outcome goes under the Settings button.
    ///
    /// Written at once; Steam reads it on its next start.
    pub(crate) fn add_to_steam(&mut self) {
        use lightforge_core::steam_shortcut;

        let outcome = (|| {
            let steam = lightforge_core::steam::SteamRoot::discover()
                .ok_or_else(|| chain(&steam_shortcut::ShortcutError::NoSteam))?;
            let exe = std::env::current_exe().map_err(|e| format!("where Lightforge is: {e}"))?;
            steam_shortcut::add(&steam, "Lightforge", &exe, &STEAM_ART).map_err(|e| chain(&e))
        })();
        self.steam_shortcut = Some(outcome.map(|added| match added.updated {
            true => "Lightforge's entry in Steam is up to date. Steam shows changes the next \
                     time it starts."
                .to_owned(),
            false => "Lightforge is in your Steam library from the next time Steam starts."
                .to_owned(),
        }));
    }

    /// Fullscreen on a console, a window anywhere else.
    ///
    /// 1280x820 does not fit a 1280x800 screen. The minimum size goes too.
    pub(crate) fn window_mode(&self) -> Task<Message> {
        let console = self.console();
        iced::window::oldest().and_then(move |id| match console {
            true => iced::window::set_min_size(id, None)
                .chain(iced::window::set_mode(id, iced::window::Mode::Fullscreen)),
            false => iced::window::set_mode(id, iced::window::Mode::Windowed)
                .chain(iced::window::set_min_size(id, Some(WINDOW_MIN))),
        })
    }

    /// Along the foot of the window on a console: what the two buttons do,
    /// the way Steam's own screens say it.
    pub(crate) fn button_hints(&self) -> Element<'_, Message> {
        let glyph = |letter: &'static str| {
            container(text(letter).size(theme::LABEL).font(theme::BOLD).color(theme::BG))
                .center_x(Length::Fixed(HINT_GLYPH))
                .center_y(Length::Fixed(HINT_GLYPH))
                .style(|_| container::Style {
                    background: Some(theme::TEXT.into()),
                    border: iced::Border { radius: (HINT_GLYPH / 2.0).into(), ..Default::default() },
                    ..container::Style::default()
                })
        };
        let hint = |letter, label: &'static str| {
            row![glyph(letter), text(label).size(theme::BODY)]
                .spacing(theme::SM)
                .align_y(Alignment::Center)
        };
        container(
            row![Space::new().width(Length::Fill), hint("A", "Select"), hint("B", "Back")]
                .spacing(theme::LG)
                .align_y(Alignment::Center),
        )
        .padding([theme::SM, theme::LG])
        .width(Length::Fill)
        .style(|_| container::Style {
            background: Some(theme::SURFACE.into()),
            ..container::Style::default()
        })
        .into()
    }

    /// `screen` with the button hints under it, on a console.
    pub(crate) fn with_hints<'a>(&'a self, screen: Element<'a, Message>) -> Element<'a, Message> {
        match self.console() {
            true => column![container(screen).height(Length::Fill), self.button_hints()].into(),
            false => screen,
        }
    }

    /// Whether the arrows, Enter and Escape are someone else's: the splash and
    /// the broken-config screen have nothing to press.
    pub(crate) fn keys_belong_elsewhere(&self) -> bool {
        self.config_error.is_some() || self.splash.is_some()
    }

    /// What closes the dialog on top, the way its Cancel or Close does.
    ///
    /// In the order the view stacks them, so Escape closes the one on top.
    pub(crate) fn dialog_dismissal(&self) -> Option<Message> {
        Some(match () {
            _ if self.stop_dialog.is_some() => Message::StopGameDismiss,
            _ if self.version_dialog.is_some() => Message::CloseGameVersion,
            _ if self.modify_dialog.is_some() => Message::ModifyCancel,
            _ if self.install_dialog.is_some() => Message::InstallCancel,
            _ if self.uninstall_dialog.is_some() => Message::UninstallCancel,
            _ if self.cancel_dialog.is_some() => Message::CancelDownloadDismiss,
            _ if self.downgrade_dialog.is_some() => Message::DowngradeCancel,
            _ if self.version_picker.is_some() => Message::CancelVersionPicker,
            _ if self.addon_detail.is_some() => Message::CloseAddonDetail,
            _ if self.report_dialog.is_some() => Message::ReportCancel,
            _ if self.game_settings.is_some() => Message::CloseGameSettings,
            _ if self.proton_installer.is_some() => Message::CloseProtonInstaller,
            _ if self.patch_notes.is_some() => Message::DismissPatchNotes,
            _ if self.update_notice.is_some() => Message::DismissUpdate,
            _ => return None,
        })
    }

    /// Scroll the page, for an arrow with no control in its direction.
    ///
    /// The region the ring is in, or failing that the largest on top: the
    /// page itself, or the text of the dialog over it.
    pub(crate) fn scroll_page(&self, step: focus::Step, from: Option<&focus::Target>) -> Task<Message> {
        let dy = match step {
            focus::Step::Up => -FOCUS_SCROLL,
            focus::Step::Down => FOCUS_SCROLL,
            focus::Step::Left | focus::Step::Right => return Task::none(),
        };
        let region = from.and_then(focus::spot).and_then(|spot| spot.region).or_else(focus::main_region);
        if let Some(region) = region {
            focus::scroll(region, dy);
        }
        Task::none()
    }

    /// Ask Steam for its on-screen keyboard for `field`, on a console that
    /// Steam started. Where the field is goes with it, the way SDL asks, so
    /// Steam can keep the keyboard off it.
    #[allow(clippy::cast_possible_truncation, reason = "screen coordinates")]
    pub(crate) fn steam_keyboard(&mut self, field: &focus::Target) {
        if !self.console() || !launched_by_steam() {
            return;
        }
        let url = match focus::spot(field) {
            Some(spot) => {
                let at = spot.screen();
                format!(
                    "{STEAM_KEYBOARD}?XPosition={}&YPosition={}&Width={}&Height={}&Mode={}",
                    at.x.round() as i32,
                    at.y.round() as i32,
                    at.width.round() as i32,
                    at.height.round() as i32,
                    // Steam's floating keyboard modes: 0 one line, 1 several.
                    u8::from(spot.lines),
                )
            }
            None => STEAM_KEYBOARD.to_owned(),
        };
        if let Err(e) = tasks::open_url(&url) {
            self.error = Some(e.into());
        }
    }

    /// Put Steam's keyboard away, typing being done.
    pub(crate) fn steam_keyboard_closed(&mut self) {
        if !self.console() || !launched_by_steam() {
            return;
        }
        if let Err(e) = tasks::open_url(STEAM_KEYBOARD_CLOSE) {
            self.error = Some(e.into());
        }
    }

    /// Where the ring appears when it is not on anything yet.
    ///
    /// Back on the control that opened the menu just closed, then PLAY, the
    /// thing a controller user came for, then wherever reading starts, which
    /// in a menu is its first entry.
    pub(crate) fn focus_start(&mut self, spots: &[focus::Place]) -> Option<focus::Target> {
        let drawn = |target: &focus::Target| spots.iter().any(|p| &p.target == target);
        // Kept while a dialog's button vanishes under it.
        if let Some(back) = self.focus_return.clone().filter(|t| drawn(t)) {
            self.focus_return = None;
            return Some(back);
        }
        if let Some(resume) = self.focus_resume.take().filter(|t| drawn(t)) {
            return Some(resume);
        }
        if drawn(&focus::Target::Primary) {
            return Some(focus::Target::Primary);
        }
        // In setup, the slide's own button, so Enter after Enter walks it.
        if let Some(forward) = self.wizard.as_ref().map(wizard::forward).filter(|t| drawn(t)) {
            return Some(forward);
        }
        focus::first(spots)
    }

    /// Scroll the region `target` is in so that it can be seen.
    ///
    /// Plus a row beyond: a list only draws the rows it shows, and the arrows
    /// need the next one drawn.
    pub(crate) fn scroll_into_view(target: &focus::Target) {
        let Some(spot) = focus::spot(target) else {
            return;
        };
        let Some(region) = spot.region else {
            return;
        };
        let (shown, at) = (spot.viewport, spot.bounds);
        let ahead = at.height + theme::MD * 2.0;
        let dy = match () {
            _ if at.y - ahead < shown.y => at.y - ahead - shown.y,
            _ if at.y + at.height + ahead > shown.y + shown.height => {
                at.y + at.height + ahead - (shown.y + shown.height)
            }
            _ => return,
        };
        focus::scroll(region, dy);
    }

    /// `content`, as a control the ring can land on, sending `press` when
    /// Enter is pressed on it. `None` is a disabled control.
    pub(crate) fn ring<'a>(
        &self,
        target: focus::Target,
        press: Option<Message>,
        content: impl Into<Element<'a, Message>>,
    ) -> Element<'a, Message> {
        let press = press.map_or(focus::Press::Off, focus::Press::Send);
        focus::ring(target, press, content)
    }

    /// Whether any dismissable panel is showing.
    ///
    /// One list for arming click-anywhere and for [`Message::Dismiss`].
    pub(crate) fn panel_open(&self) -> bool {
        self.picker_open.is_some()
            || self.gear_open
            || self.manager_open
            || self.browse_open
            || self.strip_open
            || self.app_menu_open
            || self.addon_menu_open.is_some()
            || self.tile_menu.is_some()
    }

    /// Close every one of them.
    ///
    /// On clicking the page, choosing a page or picking a game.
    pub(crate) fn close_panels(&mut self) {
        self.picker_open = None;
        self.gear_open = false;
        self.manager_open = false;
        self.browse_open = false;
        self.strip_open = false;
        self.app_menu_open = false;
        self.addon_menu_open = None;
        self.tile_menu = None;
    }
}
