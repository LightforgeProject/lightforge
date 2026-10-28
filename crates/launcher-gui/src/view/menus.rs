//! Popover menus and the picker that replaces a pick list.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// The menu under the wordmark: the app's own business, as opposed to the
    /// band's (which game) and the tabs' (which page).
    ///
    /// Settings lives here rather than in the ribbon because it is somewhere
    /// you go rarely and deliberately, unlike Play.
    pub(crate) fn app_menu(&self) -> Element<'_, Message> {
        // No destination yet means no press and a dimmed face: the entry says
        // what is coming without pretending it is here.
        let entry = |icon: image::Handle, label: &'static str, message: Option<Message>| {
            let live = message.is_some();
            let face = row![
                image(icon)
                    .width(Length::Fixed(MENU_GLYPH))
                    .height(Length::Fixed(MENU_GLYPH))
                    .opacity(match live {
                        true => 1.0_f32,
                        false => DIMMED,
                    }),
                match live {
                    true => text(label).size(theme::BODY),
                    false => muted(label.to_owned(), theme::BODY),
                },
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
            self.ring(
                focus::Target::Menu(label),
                message.clone(),
                button(face.width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press_maybe(message),
            )
        };
        let square = |icon: image::Handle, url: Option<&'static str>| {
            self.ring(
                focus::Target::Link(url.unwrap_or_default()),
                url.map(Message::OpenLink),
                link_square(icon, url, MENU_GLYPH),
            )
        };

        let rows = column![
            entry(gear_icon(), "Settings", Some(Message::Show(Screen::Settings))),
            entry(updates_icon(), "Lightforge Updates", Some(Message::Show(Screen::Updates))),
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
            entry(bug_icon(), "Report a Bug", Some(Message::OpenReport(ReportKind::Bug))),
            entry(
                feedback_icon(),
                "Send Feedback",
                Some(Message::OpenReport(ReportKind::Feedback)),
            ),
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
            entry(exit_icon(), "Exit", Some(Message::Exit)),
            // Where to find me, and where to say thanks. One row rather
            // than two entries with words: these are marks people recognise,
            // and a menu that ends in a line of labels reading "Ko-fi" and
            // "PayPal" would read as the point of the menu.
            row![
                square(discord_icon(), DISCORD_URL),
                square(x_icon(), X_URL),
                Space::new().width(Length::Fill),
                square(kofi_icon(), KOFI_URL),
                square(paypal_icon(), PAYPAL_URL),
            ]
            .spacing(theme::XS),
        ]
        .spacing(theme::XS)
        .width(Length::Fixed(240.0));

        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// One addon's own settings: what is true of this addon rather than what
    /// to do to it right now.
    ///
    /// A version pin and re-install belong here too, which is the reason for
    /// a menu rather than more buttons on an already full row.
    pub(crate) fn addon_row_menu(
        &self,
        managed: &addonwork::Managed,
        version: &str,
        name: &str,
    ) -> Element<'static, Message> {
        let (provider, id) = (managed.provider.clone(), managed.id.clone());
        let default_auto = self.config.addon_auto_update;
        let entry = |label: String, message: Message| {
            focus::ring(
                focus::Target::Key(format!("addon menu entry {label}")),
                focus::Press::Send(message.clone()),
                button(text(label).size(theme::BODY).width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press(message),
            )
        };

        // What this addon does today: its own choice where it has one, the
        // app's default otherwise.
        let auto = managed.auto_update.unwrap_or(default_auto);
        let mut rows = column![
            entry(
                match managed.ignored {
                    true => "Stop ignoring".to_owned(),
                    false => "Ignore this addon".to_owned(),
                },
                Message::AddonIgnore(provider.clone(), id.clone(), !managed.ignored),
            ),
            entry(
                match auto {
                    true => "Do not update automatically".to_owned(),
                    false => "Update automatically".to_owned(),
                },
                Message::AddonAutoUpdate(provider.clone(), id.clone(), Some(!auto)),
            ),
        ]
        .spacing(theme::XS)
        .width(Length::Fixed(240.0));

        // Only worth offering once the addon has an opinion to give up, and
        // worth naming the default so clearing it is not a leap of faith.
        if managed.auto_update.is_some() {
            rows = rows.push(entry(
                match default_auto {
                    true => "Follow the default (on)".to_owned(),
                    false => "Follow the default (off)".to_owned(),
                },
                Message::AddonAutoUpdate(provider.clone(), id.clone(), None),
            ));
        }

        // Choosing an older build on purpose, which is the other half of
        // pinning: "Pin to <version>" holds what is here, this goes and gets
        // something else. Only CurseForge publishes a build list, so this is
        // the only provider it is offered for.
        if managed.provider.eq_ignore_ascii_case("curse") {
            rows = rows.push(entry(
                "Details and changelog…".to_owned(),
                Message::ShowAddonDetail(provider.clone(), id.clone(), name.to_owned()),
            ));
            rows = rows.push(entry(
                "Choose a version…".to_owned(),
                Message::PickVersion(
                    provider.clone(),
                    id.clone(),
                    name.to_owned(),
                    managed.file_id.clone(),
                ),
            ));
        }

        // Pinning names the version it will hold, because "pin" alone does
        // not say *to what*: and after an update that is a different file.
        // An addon whose file the ledger never recorded has nothing to hold.
        match (&managed.pinned, &managed.file_id) {
            (Some(_), _) => {
                rows = rows.push(entry(
                    format!("Unpin (held at {version})"),
                    Message::AddonPin(provider, id, None),
                ));
            }
            (None, Some(file_id)) => {
                rows = rows.push(entry(
                    format!("Pin to {version}"),
                    Message::AddonPin(provider, id, Some(file_id.clone())),
                ));
            }
            (None, None) => {}
        }

        // Where the addon came from and who wrote it. Above the rule
        // because they leave Lightforge rather than change anything in it,
        // and the project page first: on CurseForge that page is where an
        // author is actually paid, so sending people to it is the point
        // rather than a courtesy.
        //
        // Each entry appears only when there is somewhere to go. A provider
        // that publishes no repository, and an addon installed before the
        // ledger kept these, both leave the menu one line shorter, which is
        // better than a line that opens nothing.
        let support = managed.support_url().map(str::to_owned);
        if managed.website_url.is_some() || support.is_some() {
            rows = rows.push(
                container(Space::new().height(Length::Fixed(1.0)))
                    .width(Length::Fill)
                    .style(theme::divider),
            );
        }
        if let Some(url) = managed.website_url.clone() {
            rows = rows.push(entry("Open project page".to_owned(), Message::OpenUrl(url)));
        }
        // Source before author profile, because that is where the money can
        // actually go: a repository usually carries a Sponsor button, where a
        // provider's author page is a list of their projects. Neither is a
        // donation link as such, no catalogue publishes one, so the label
        // promises help, not a payment form.
        if let Some(url) = support {
            rows = rows.push(entry("Support the author…".to_owned(), Message::OpenUrl(url)));
        }

        // An action, not a setting, so it sits under a rule of its own,
        // and busy means busy: a second fetch into the same directory while
        // one is running is the race the whole page guards against.
        rows = rows.push(
            container(Space::new().height(Length::Fixed(1.0)))
                .width(Length::Fill)
                .style(theme::divider),
        );
        let reinstall = (!self.addons_locked())
            .then(|| Message::ReinstallAddon(managed.provider.clone(), managed.id.clone()));
        rows = rows.push(self.ring(
            focus::Target::Key("addon menu entry Re-install".to_owned()),
            reinstall.clone(),
            button(text("Re-install".to_owned()).size(theme::BODY).width(Length::Fill))
                .padding(theme::tap([theme::XS, theme::SM]))
                .style(theme::ghost)
                .on_press_maybe(reinstall),
        ));

        // Why either is worth having, in the menu that offers them: the words
        // alone do not say that the addon stays installed and working.
        rows = rows.push(muted(
            match (managed.ignored, managed.pinned.is_some()) {
                (true, _) => "Ignored: left out of every pass, automatic or not. \
                              Updating it from its own row still works."
                    .to_owned(),
                (false, true) => "Pinned: held at this version until you unpin it, \
                                  and its row offers no update."
                    .to_owned(),
                (false, false) => "Ignoring leaves it out of Check for updates and \
                                   Update all; pinning holds it at one version."
                    .to_owned(),
            },
            theme::CAPTION,
        ));

        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// The games the band had no room for.
    pub(crate) fn overflow_menu(&self, hidden: &[ProductLine]) -> Element<'_, Message> {
        let mut rows = column![].spacing(theme::XS).width(Length::Fixed(220.0));
        for line in hidden {
            let mut entry = row![].spacing(theme::SM).align_y(Alignment::Center);
            if let Some(mark) = self.tile_art(line) {
                entry = entry.push(
                    image(mark.handle.clone())
                        .content_fit(ContentFit::Contain)
                        .width(Length::Fixed(theme::TILE * 0.6))
                        .height(Length::Fixed(theme::TILE * 0.6)),
                );
            }
            entry = entry.push(text(self.name_of(line)).size(theme::BODY));
            rows = rows.push(self.ring(
                focus::Target::Overflow(line.clone()),
                Some(Message::LineSelected(line.clone())),
                button(entry.width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .style(theme::ghost)
                    .on_press(Message::LineSelected(line.clone())),
            ));
        }
        focus::layer(container(rows).padding(theme::SM).style(theme::menu))
    }

    /// A dropdown the ring can open and move through.
    ///
    /// A button and a menu of our own rather than a `pick_list`, whose list
    /// keeps its open state to itself, so a keyboard can neither open it nor
    /// move through it. Looks the same, and a mouse uses it the same way.
    /// `key` names it, and has to be unique on the page.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn picker<'a, T: ToString + PartialEq + Clone>(
        &self,
        key: String,
        options: Vec<T>,
        current: Option<T>,
        placeholder: &str,
        on_pick: impl Fn(T) -> Message,
        width: Length,
        size: f32,
    ) -> Element<'a, Message> {
        let label = match &current {
            Some(current) => text(current.to_string()).size(size),
            None => text(placeholder.to_owned()).size(size).color(theme::MUTED),
        };
        let toggle = Message::TogglePicker(key.clone());
        let face = button(
            row![
                label.width(Length::Fill),
                image(chevron_icon())
                    .width(Length::Fixed(APP_MENU_CHEVRON))
                    .height(Length::Fixed(APP_MENU_CHEVRON)),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::tap([theme::XS + 2.0, theme::SM]))
        .width(width)
        .style(theme::picker_button)
        .on_press(toggle.clone());
        let face = focus::ring(focus::Target::Key(format!("picker {key}")), focus::Press::Send(toggle), face);

        let menu = (self.picker_open.as_ref() == Some(&key)).then(|| {
            // As wide as the button it opens from, where that has a width.
            let menu_width = match width {
                Length::Fixed(button) => (button - 2.0 * theme::XS).max(240.0),
                _ => 240.0,
            };
            let mut rows = column![].spacing(theme::XS).width(Length::Fixed(menu_width));
            for option in options {
                let chosen = Message::Picked(Box::new(on_pick(option.clone())));
                let name = option.to_string();
                let entry = button(text(name.clone()).size(size).width(Length::Fill))
                    .padding(theme::tap([theme::XS, theme::SM]))
                    .width(Length::Fill)
                    .style(match current.as_ref() == Some(&option) {
                        true => theme::selected,
                        false => theme::ghost,
                    })
                    .on_press(chosen.clone());
                rows = rows.push(focus::ring(
                    focus::Target::Key(format!("pick {key} {name}")),
                    focus::Press::Send(chosen),
                    entry,
                ));
            }
            focus::layer(
                mouse_area(container(rows).padding(theme::XS).style(theme::menu))
                    .on_press(Message::MenuClick),
            )
        });
        popover::popover(face, menu).align_left().into()
    }

    /// The cog menu: what you do *about* a game rather than to play it.
    pub(crate) fn gear_menu(&self) -> Element<'_, Message> {
        let item = |label: &'static str, message: Message, enabled: bool| {
            self.ring(
                focus::Target::Menu(label),
                enabled.then(|| message.clone()),
                button(text(label).size(theme::LABEL))
                    .style(theme::ghost)
                    .padding(theme::tap([theme::SM, theme::MD]))
                    .width(Length::Fill)
                    .on_press_maybe(enabled.then_some(message)),
            )
        };

        let installed = self.local.version.is_some();
        let can = self.can_maintain();
        // Anything in the queue for this product can be given up, running,
        // paused or still waiting its turn. Pause is on the button itself;
        // this is the one that ends it.
        let queued = self.queue.iter().any(|d| d.code == self.selected.code);
        // Battle.net's order and wording: the game's own entries, then the
        // two that change the install below a rule.
        let menu = container(
            column![
                item("Game Settings", Message::OpenGameSettings, installed),
                item("Show in Explorer", Message::ShowInExplorer, installed),
                item("Game Version", Message::AskGameVersion, installed),
                item("Scan and Repair", Message::ScanAndRepair, can.scan),
                item("Check for Updates", Message::CheckForUpdates, !self.busy),
            ]
            .extend(
                self.owed_repair
                    .is_some()
                    .then(|| item("Repair Damaged Files", Message::RepairDamaged, can.repair)))
            .extend(queued.then(|| {
                item(
                    "Cancel Download",
                    Message::AskCancelDownload(self.selected.code.clone()),
                    true,
                )
            }))
            .push(container(Space::new()).height(Length::Fixed(1.0)).width(Length::Fill).style(theme::divider))
            .push(item("Modify Install", Message::AskModifyInstall, can.scan))
            .push(item("Uninstall", Message::AskUninstall, can.uninstall))
            .push(item("Copy Debug Info", Message::CopyDebug, true))
            .spacing(theme::XS),
        )
        .padding(theme::XS)
        // The width of the split button beneath it, so the menu reads as
        // belonging to that control rather than floating near it.
        .width(Length::Fixed(240.0))
        .style(theme::floating);

        focus::layer(mouse_area(menu).on_press(Message::MenuClick))
    }
}
