//! The Addons screens: installed, browse, import, export and detail.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Whether any dismissable panel is showing.
    ///
    /// One list, used both to arm the click-anywhere dismissal and to clear
    /// everything in [`Message::Dismiss`]. Adding a panel and forgetting to
    /// name it here is what left the wordmark menu unable to be closed by
    /// clicking the page, so the two now read from the same place.
    /// Whether this provider/id pair is in the ledger for the open game.
    ///
    /// Asked by the detail card twice over, once to pick the opening tab,
    /// once to decide between Install and nothing, so it is one place.
    pub(crate) fn addon_is_installed(&self, provider: &str, id: &str) -> bool {
        self.addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .any(|m| m.provider == provider && m.id == id)
    }

    /// Clear everything on the addon screen that belongs to one product.
    ///
    /// A search is run against a product's flavor, so Retail's hits are the
    /// wrong answer for Classic and a filter typed over Retail's installed
    /// list hides Classic's. An armed Remove is worse than wrong: it is a
    /// confirmation the user gave about an addon they are no longer looking
    /// at. An in-flight search needs no cancelling, its reply is matched
    /// against the current query and dropped.
    pub(crate) fn reset_addon_screen(&mut self) {
        self.search_input.clear();
        self.search = addonwork::SearchOutcome::default();
        self.addon_filter.clear();
        self.addon_remove_armed = None;
        self.addon_menu_open = None;
        self.addon_auto_note = None;
        // Another product's addons, and their badges with them.
        self.updates.clear();
    }

    /// Addon management: what is installed, and what could be.
    pub(crate) fn addons_screen(&self) -> Element<'_, Message> {
        let tab = |label: &'static str, which: AddonTab| {
            let entry = button(text(label).size(theme::LABEL)).padding(theme::tap([theme::SM, theme::MD]));
            let entry = match self.addon_tab == which {
                true => entry.style(theme::selected),
                false => entry.style(theme::ghost).on_press(Message::AddonTab(which)),
            };
            // The open tab takes the ring too, so the row of tabs can be
            // walked without a gap where the current one is.
            focus::ring(
                focus::Target::Key(format!("addon tab {label}")),
                focus::Press::Send(Message::AddonTab(which)),
                entry,
            )
        };

        let mut body = column![row![
            tab("My Addons", AddonTab::Mine),
            tab("Browse Addons", AddonTab::Browse),
            tab("Import", AddonTab::Import),
            tab("Export", AddonTab::Export),
        ]
        .spacing(theme::XS)]
        .spacing(theme::MD);

        if self.addons_busy {
            let progress = &self.addon_sample;
            body = body.push(
                row![
                    text(progress.line.clone()).size(theme::LABEL),
                    mono(format!("{}/{}", progress.done, progress.total)),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            );
        }
        if let Some(e) = &self.error {
            body = body.push(problem(e));
        }

        body = body.push(match self.addon_tab {
            AddonTab::Mine => self.addons_mine(),
            AddonTab::Browse => self.addons_browse(),
            AddonTab::Import => self.addons_import(),
            AddonTab::Export => self.addons_export(),
        });

        // Parent folders only, so the count matches the rows you can see,
        // BigWigs is one addon here and eleven folders on disk.
        let installed = self.addons.rows.iter().filter(|r| r.child_of.is_none()).count();
        let aside = muted(
            match installed {
                1 => "1 addon installed".to_owned(),
                n => format!("{n} addons installed"),
            },
            theme::LABEL,
        );

        self.page("Addons", None, Some(aside.into()), body.into())
    }

    pub(crate) fn addons_mine(&self) -> Element<'_, Message> {
        let outdated = self.updates.iter().filter(|u| u.has_update()).count();

        let toolbar = container(
            row![
                // Sized, not filling: a filter box the width of the window
                // looks like the page's search when it only narrows the list
                // below it.
                clickable(
                    "addon filter",
                    text_input("Filter installed addons…", &self.addon_filter)
                        .on_input(Message::AddonFilterChanged)
                        .style(theme::field)
                        .size(theme::BODY)
                        .padding(theme::SM)
                        .width(Length::Fixed(260.0)),
                ),
                Space::new().width(Length::Fill),
                muted(
                    match (self.updates.is_empty(), outdated) {
                        (true, _) => String::new(),
                        (false, 0) => "everything is up to date".to_owned(),
                        (false, n) => format!("{n} have updates"),
                    },
                    theme::LABEL,
                ),
                // One shape for the three, so the toolbar reads as a set.
                // Colour still carries state: blue is the one to press, grey
                // is a thing you can do.
                action_button("Check for updates", theme::PAUSE, Message::CheckAddonUpdates)
                    .enabled(!self.addons_locked() && self.addons.managed > 0),
                action_button("Update all", theme::ACCENT, Message::UpdateAddons)
                    .enabled(!self.addons_locked() && self.addons.managed > 0),
                action_button("Open folder", theme::PAUSE, Message::OpenAddonsDir)
                    .enabled(true),
            ]
            .spacing(theme::SM)
            .align_y(Alignment::Center),
        )
        .padding(theme::MD)
        .style(theme::card);

        // Offered only when there is something to offer it for. A button
        // that does nothing on every press teaches people to ignore it, and
        // a library installed entirely through Lightforge has nothing to
        // recognise.
        let unmanaged = self
            .addons
            .rows
            .iter()
            .filter(|row| row.managed.is_none() && row.child_of.is_none())
            .count();

        let mut body = column![toolbar].spacing(theme::SM);

        if unmanaged > 0 {
            body = body.push(
                container(
                    row![
                        column![
                            text(match unmanaged {
                                1 => "1 addon was installed by hand".to_owned(),
                                n => format!("{n} addons were installed by hand"),
                            })
                            .size(theme::BODY),
                            muted(
                                "Lightforge can recognise them by their contents and \
                                 manage them from here — updates, channels and all."
                                    .to_owned(),
                                theme::CAPTION,
                            ),
                        ]
                        .spacing(theme::XS)
                        .width(Length::Fill),
                        action_button("Recognise them", theme::ACCENT, Message::AdoptAddons)
                            .enabled(!self.addons_locked()),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .padding(theme::MD)
                .style(theme::card),
            );
        }

        // A header, because four right-hand columns of bare dates are not
        // self-describing. Indented and spaced to match a row's own columns.
        if !self.addons.rows.is_empty() {
            body = body.push(
                container(
                    row![
                        muted("NAME".to_owned(), theme::CAPTION).width(Length::Fill),
                        column_head("LATEST"),
                        column_head("RELEASED"),
                        column_head("INSTALLED"),
                        // Standing in for the controls each row carries, so
                        // the three headings sit over their own columns
                        // rather than over the buttons.
                        Space::new().width(Length::Fixed(ADDON_ACTIONS)),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )
                .padding([0.0, theme::MD]),
            );
        }

        // What happened without anyone asking, said where the addons are.
        // Nothing interrupted the user to tell them, so this is the only
        // place it is said at all.
        if let Some(note) = &self.addon_auto_note {
            body = body.push(muted(note.clone(), theme::CAPTION));
        }

        body = body.extend(self.unavailable_rows(AddonTab::Mine));

        if self.addons.rows.is_empty() {
            body = body.push(empty(
                "No addons installed.",
                "Find one under Browse Addons, or bring your collection over under Import.",
            ));
        }

        // Ordered and filtered before drawing, rather than in the order the
        // folder scan happened to return. Outdated first. That is the reason
        // to open this page, then by name, so a list of forty is something
        // you can read down instead of search through.
        let needle = self.addon_filter.trim().to_lowercase();
        let mut shown: Vec<&_> = self
            .addons
            .rows
            .iter()
            // A package's other folders belong to the addon above them;
            // showing BigWigs' eleven folders as eleven addons would be a lie.
            .filter(|addon| addon.child_of.is_none())
            .filter(|addon| {
                needle.is_empty()
                    || addon.title.to_lowercase().contains(&needle)
                    || addon.folder.to_lowercase().contains(&needle)
            })
            .collect();
        shown.sort_by_cached_key(|addon| {
            let outdated = self.update_status(addon).is_some_and(|s| s.has_update());
            (!outdated, addon.title.to_lowercase())
        });

        if !needle.is_empty() && shown.is_empty() && !self.addons.rows.is_empty() {
            body = body.push(empty(
                &format!("No installed addon matches “{}”.", self.addon_filter.trim()),
                "Clear the filter to see everything again.",
            ));
        }

        for addon in shown {
            let extra = self
                .addons
                .rows
                .iter()
                .filter(|r| r.child_of.as_deref() == Some(addon.folder.as_str()))
                .count();
            let status = self.update_status(addon);

            let mut heading = row![text(addon.title.clone()).size(theme::BODY), mono(addon.version.clone())]
                .spacing(theme::SM)
                .align_y(Alignment::Center);
            match addon.managed.as_ref() {
                // Said once, in the place an update would be announced: this
                // row is why "Update all" leaves something behind.
                Some(m) if m.ignored => {
                    heading = heading.push(pill("ignored".to_owned(), theme::MUTED));
                }
                // The version it is held at is already beside the title, so
                // the pill says the state and not the number twice.
                Some(m) if m.pinned.is_some() => {
                    heading = heading.push(pill("pinned".to_owned(), theme::MUTED));
                }
                // Only worth a word when it differs from what everything else
                // on the page is doing; with the default on, that is the
                // addon somebody decided to keep still.
                Some(m) if !m.auto_update.unwrap_or(self.config.addon_auto_update) => {
                    heading = heading.push(pill("manual".to_owned(), theme::MUTED));
                }
                _ => {}
            }
            if let Some(status) = status.filter(|s| s.has_update()) {
                // What is on offer, named the way the provider names it, a
                // version people recognise, not the opaque file id the
                // ledger trades in. A channel switch says so instead, because
                // "→ 1.4.26" beside an alpha looks like going forwards.
                heading = heading.push(match status.is_channel_switch() {
                    true => pill(
                        format!(
                            "{} build",
                            status
                                .available_build
                                .map(|c| c.as_str())
                                .unwrap_or("different"),
                        ),
                        theme::ACCENT,
                    ),
                    false => pill(
                        format!(
                            "→ {}",
                            status
                                .available_name
                                .clone()
                                .or_else(|| status.available.clone())
                                .unwrap_or_default()
                        ),
                        theme::UPDATE,
                    ),
                });
            }

            let mut line = row![column![
                heading,
                muted(
                    match (&addon.managed, extra) {
                        (Some(m), 0) => format!("{} · managed", m.provider),
                        (Some(m), n) => format!("{} · managed · +{n} folders", m.provider),
                        (None, _) => "installed by hand".to_owned(),
                    },
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS)
            .width(Length::Fill)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

            // The three facts, in the columns the header names. A dash
            // rather than a blank for a fact we do not have: an empty cell
            // reads as a layout bug, where ", " reads as "not known", which
            // is what it is for an addon installed before this was recorded
            // or a provider that publishes no dates.
            let managed_row = addon.managed.as_ref();
            line = line.push(cell(
                status.and_then(|s| s.available_name.clone()).or_else(|| {
                    // Up to date: what is latest is what is installed, and
                    // saying so beats an empty column.
                    managed_row.map(|_| addon.version.clone())
                }),
            ));
            line = line.push(cell(
                status.and_then(|s| s.available_date.as_deref().map(short_day)),
            ));
            line = line.push(cell(
                managed_row.and_then(|m| m.installed_at.as_deref().map(short_day)),
            ));
            // Everything that acts on the row lives in a box of its own
            // width, right-aligned inside it. Without it the three columns
            // float: a row with an Update button pushes them left of a row
            // without one, and both sit left of the header that is supposed
            // to name them. The box is pushed for an addon installed by hand
            // too, which has no controls at all, otherwise its columns would
            // be the ones out of step.
            let mut actions = row![].spacing(theme::SM).align_y(Alignment::Center);
            if let Some(managed) = &addon.managed {
                // Which builds this addon accepts. Narrow and first, so the
                // buttons that act stay where the eye expects them whether or
                // not there is an update to show.
                // Sized like the buttons it sits beside: three controls on
                // one line that disagree about their height read as a
                // mistake rather than as a hierarchy. The width is fixed so
                // the buttons do not shift as the word changes between
                // Default, Stable, Beta and Alpha.
                actions = actions.push(self.picker(
                    format!("channel {} {}", managed.provider, managed.id),
                    RowChannel::ALL.to_vec(),
                    Some(RowChannel(managed.channel)),
                    "",
                    |choice| Message::AddonChannelFor(managed.provider.clone(), managed.id.clone(), choice),
                    Length::Fixed(120.0),
                    theme::LABEL,
                ));

                // One addon at a time. `install_one` fetches the newest file
                // and does not pin it, which is exactly what updating is, so
                // this is the same path "Update all" takes for every row.
                //
                // A pin is a statement about which version this addon is on,
                // so the row stops offering to move it, unlike Ignore, which
                // only means "not in bulk". Unpinning brings the button back.
                let pinned = managed.pinned.is_some();
                if let Some(status) = status.filter(|s| s.has_update() && !pinned) {
                    // "Update" would be a lie when the channel changed: going
                    // back to stable from an alpha installs an *older* build,
                    // which is right, but not what the word promises.
                    let label = match status.is_channel_switch() {
                        true => "Switch",
                        false => "Update",
                    };
                    actions = actions.push(
                        action_button(
                            label,
                            theme::UPDATE,
                            Message::InstallAddon(
                                managed.provider.clone(),
                                managed.id.clone(),
                            ),
                        )
                        .enabled(!self.addons_locked()),
                    );
                }

                // Two clicks, because removing deletes the addon's folders off
                // disk and there is no undo. The second button is the way out,
                // so the destructive one is never the only thing to press.
                let armed = self.addon_remove_armed.as_ref().is_some_and(|(p, id)| {
                    p == &managed.provider && id == &managed.id
                });
                // Remove and the Delete it turns into are one place for the
                // ring, which stays on it while the question is asked.
                let remove = format!("addon remove {} {}", managed.provider, managed.id);
                actions = match armed {
                    true => actions
                        .push(
                            action_button(
                                "Delete",
                                theme::ERROR,
                                Message::RemoveAddon(
                                    managed.provider.clone(),
                                    managed.id.clone(),
                                ),
                            )
                            .keyed(remove, !self.addons_locked()),
                        )
                        .push(
                            secondary("Keep", Message::CancelRemoveAddon).enabled(true),
                        ),
                    false => actions.push(
                        secondary(
                            "Remove",
                            Message::ArmRemoveAddon(
                                managed.provider.clone(),
                                managed.id.clone(),
                            ),
                        )
                        .keyed(remove, !self.addons_locked()),
                    ),
                };

                // Everything that is a setting rather than an action lives
                // behind this, so the row keeps its width as the settings
                // grow. Only one menu is open at a time, and the panel hangs
                // off the button itself rather than the row's corner.
                let key = (managed.provider.clone(), managed.id.clone());
                let open = self.addon_menu_open.as_ref() == Some(&key);
                let toggle = Message::AddonMenu(match open {
                    true => None,
                    false => Some(key.clone()),
                });
                actions = actions.push(popover::popover(
                    focus::ring(
                        focus::Target::Key(format!("addon menu {} {}", managed.provider, managed.id)),
                        focus::Press::Send(toggle.clone()),
                        button(
                            image(ellipsis_icon())
                                .width(Length::Fixed(MENU_GLYPH))
                                .height(Length::Fixed(MENU_GLYPH)),
                        )
                        .padding(theme::SM)
                        .style(theme::ghost)
                        .on_press(toggle),
                    ),
                    open.then(|| self.addon_row_menu(managed, &addon.version, &addon.title)),
                ));
            }
            line = line.push(
                container(actions)
                    .width(Length::Fixed(ADDON_ACTIONS))
                    .align_x(Alignment::End),
            );

            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }

        // Where all of this lands, kept quiet: "Open folder" in the toolbar is
        // the way there, and this is for the times you need the path itself.
        body = body.push(muted(self.addons.dir.display().to_string(), theme::CAPTION));
        body.into()
    }

    /// What the last update check said about one installed addon, if it is
    /// managed and the check covered it.
    pub(crate) fn update_status(
        &self,
        addon: &addonwork::Row,
    ) -> Option<&lightforge_core::addons::manage::UpdateStatus> {
        let managed = addon.managed.as_ref()?;
        self.updates
            .iter()
            .find(|u| u.id == managed.id && u.provider == managed.provider)
    }

    /// Bringing a collection over from WoWUp.
    ///
    /// Its own tab rather than a field above the list: it is something you do
    /// once, when you arrive, and it had the most prominent box on a page
    /// whose job is the addons you already have.
    pub(crate) fn addons_import(&self) -> Element<'_, Message> {
        let box_ = container(
            column![
                text("Import from WoWUp").size(theme::HEADING),
                muted(
                    "In WoWUp, export your addons and paste the token here. Everything it \
                     names is fetched from its provider and tracked by Lightforge from then \
                     on."
                        .to_owned(),
                    theme::LABEL,
                ),
                row![
                    clickable(
                        "import token",
                        text_input("Paste a WoWUp import token…", &self.token_input)
                            .on_input(Message::TokenChanged)
                            .on_submit(Message::ImportToken)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    action_button("Import", theme::ACCENT, Message::ImportToken)
                        .enabled(!self.addons_locked() && !self.token_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                // A WoWUp token names a file for every addon. Taking them is
                // reproducing someone's setup exactly, which is a pin and is
                // recorded as one; leaving them is taking their list of
                // addons and tracking each from there.
                clickable(
                    "import pinned",
                    tick(self.import_pinned)
                        .label("Install the exact versions this token names, and pin them")
                        .on_toggle(Message::ImportPinned)
                        .text_size(theme::tap_text(theme::BODY)),
                ),
            ]
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![box_].spacing(theme::SM);

        // What an import could not resolve belongs with the import that could
        // not resolve it, not scattered through the installed list.
        body = body.extend(self.unavailable_rows(AddonTab::Import));

        body.into()
    }

    /// Handing this collection to somewhere else.
    ///
    /// Beside Import rather than inside it: the two are opposite directions of
    /// the same idea, and a tab that does both is a tab you have to read before
    /// you can use.
    pub(crate) fn addons_export(&self) -> Element<'_, Message> {
        let managed = self.addons.managed;
        let box_ = container(
            column![
                row![
                    column![
                        text("Export").size(theme::HEADING),
                        muted(
                            match managed {
                                0 => "Nothing to export yet — only addons Lightforge \
                                      installed can be named in a token."
                                    .to_owned(),
                                1 => "A token naming the 1 addon Lightforge manages here, \
                                      to import on another machine or in WoWUp."
                                    .to_owned(),
                                n => format!(
                                    "A token naming the {n} addons Lightforge manages here, \
                                     to import on another machine or in WoWUp."
                                ),
                            },
                            theme::LABEL,
                        ),
                    ]
                    .spacing(theme::XS)
                    .width(Length::Fill),
                    action_button("Export", theme::ACCENT, Message::ExportAddons)
                        .enabled(!self.addons_locked() && managed > 0),
                ]
                .spacing(theme::MD)
                .align_y(Alignment::Center),
            ]
            .extend(match self.export_token.is_empty() {
                true => Vec::new(),
                false => vec![Element::from(
                    row![
                        // Editable so it can be selected and copied by hand;
                        // what is typed into it is never read back.
                        clickable(
                            "export token",
                            text_input("", &self.export_token)
                                .on_input(Message::ExportEdited)
                                .style(theme::field)
                                .size(theme::BODY)
                                .padding(theme::SM),
                        ),
                        secondary("Copy", Message::CopyExport).enabled(true),
                    ]
                    .spacing(theme::SM)
                    .align_y(Alignment::Center),
                )],
            })
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        column![box_].spacing(theme::SM).into()
    }

    /// What the last addon run could not install, shown on the tab it was
    /// started from: a refusal from Browse belongs beside the search that
    /// produced it, not on a page of addons it never became.
    pub(crate) fn unavailable_rows(&self, tab: AddonTab) -> Vec<Element<'_, Message>> {
        let Some(report) = self.addon_report.as_ref().filter(|_| self.addon_report_tab == tab) else {
            return Vec::new();
        };
        report
            .unavailable
            .iter()
            .map(|item| {
                let mut line = row![column![
                    text(item.name.clone()).size(theme::BODY),
                    muted(item.reason.clone(), theme::CAPTION),
                ]
                .spacing(theme::XS)]
                .spacing(theme::SM)
                .align_y(Alignment::Center);

                line = line.push(Space::new().width(Length::Fill));
                if let Some(repo) = &item.github {
                    line = line.push(
                        secondary(
                            "Get it from GitHub",
                            Message::InstallAddon("github".to_owned(), repo.clone()),
                        )
                        .enabled(!self.addons_locked()),
                    );
                }
                if let Some(url) = &item.website_url {
                    line = line.push(
                        secondary("Open page", Message::OpenUrl(url.clone())).enabled(true),
                    );
                }
                line = line.push(
                    secondary("Dismiss", Message::DismissUnavailable(item.name.clone())).enabled(true),
                );
                container(line).padding(theme::MD).style(theme::row).into()
            })
            .collect()
    }

    pub(crate) fn addons_browse(&self) -> Element<'_, Message> {
        let search = container(
            column![
                row![
                    clickable(
                        "addon search",
                        text_input("Search addons, or paste a project page URL…", &self.search_input)
                            .on_input(Message::SearchChanged)
                            .on_submit(Message::RunSearch)
                            .style(theme::field)
                            .size(theme::BODY)
                            .padding(theme::SM),
                    ),
                    action_button("Search", theme::ACCENT, Message::RunSearch)
                        .enabled(!self.searching && !self.search_input.trim().is_empty()),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
            ]
            .extend(self.hint(
                "WoWInterface, Tukui and GitHub (type owner/repo). CurseForge needs a \
                 catalog in Settings. A pasted CurseForge, GitHub, WoWInterface or Tukui \
                 page is looked up at that provider — it installs the same way as a \
                 search hit, so a dead link says so rather than failing oddly.",
            ).map(Element::from))
            .spacing(theme::SM),
        )
        .padding(theme::MD)
        .style(theme::card);

        let mut body = column![search].spacing(theme::SM);

        // A provider that could not answer is said out loud: an empty list
        // otherwise reads as "nothing matched".
        for problem in &self.search.problems {
            body = body.push(text(format!("⚠ {problem}")).size(theme::CAPTION).color(theme::UPDATE));
        }
        body = body.extend(self.unavailable_rows(AddonTab::Browse));

        if self.searching {
            body = body.push(muted("searching…".to_owned(), theme::BODY));
        } else if self.search.listings.is_empty() {
            body = body.push(match self.search.query.is_empty() {
                true => empty(
                    "Search for an addon.",
                    "The most-downloaded addons show here once the catalogue has been \
                     fetched. Results come from WoWInterface and Tukui; type owner/repo \
                     for GitHub, or paste an addon's page URL.",
                ),
                false => empty(
                    &format!("Nothing found for “{}”.", self.search.query),
                    "It may be a CurseForge addon, which needs a catalog configured.",
                ),
            });
        }

        // Said once, above the rows: a list nobody asked for has to explain
        // what it is, or it reads as the results of a search that went wrong.
        if self.search.query.is_empty() && !self.search.listings.is_empty() {
            body = body.push(muted("Most downloaded".to_owned(), theme::LABEL));
        }

        let installed_ids: Vec<(&str, &str)> = self
            .addons
            .rows
            .iter()
            .filter_map(|r| r.managed.as_ref())
            .map(|m| (m.provider.as_str(), m.id.as_str()))
            .collect();

        for listing in &self.search.listings {
            let provider = listing.provider.as_str().to_owned();
            // An opted-out project taken from its GitHub instead is installed
            // under that provider, and is just as installed.
            let already = installed_ids.iter().any(|(p, id)| {
                (*p == provider && *id == listing.id)
                    || listing.github.as_deref().is_some_and(|repo| {
                        p.eq_ignore_ascii_case("github") && id.eq_ignore_ascii_case(repo)
                    })
            });

            let mut info = column![
                row![
                    text(listing.name.clone()).size(theme::BODY),
                    pill(provider.clone(), theme::ACCENT),
                ]
                .spacing(theme::SM)
                .align_y(Alignment::Center),
                muted(
                    format!(
                        "{}{}",
                        listing.author.as_deref().unwrap_or("unknown author"),
                        match listing.downloads {
                            0 => String::new(),
                            n => format!(" · {} downloads", downloads_of(n)),
                        }
                    ),
                    theme::CAPTION,
                ),
            ]
            .spacing(theme::XS);
            if listing.opted_out {
                info = info.push(opted_out_note(match listing.github {
                    Some(_) => "the author has disabled third-party downloads, but publishes it on GitHub",
                    None => "the author has disabled third-party downloads",
                }));
            }
            let mut line = row![info].spacing(theme::SM).align_y(Alignment::Center);

            line = line.push(Space::new().width(Length::Fill));
            // "Details" where there is something to show and "Page" where
            // there is not: only CurseForge publishes a description and a
            // changelog, and the detail card offers the page anyway, so
            // carrying both buttons would just crowd the row.
            match provider.eq_ignore_ascii_case("curse") {
                true => {
                    let open = Message::ShowAddonDetail(
                        provider.clone(),
                        listing.id.clone(),
                        listing.name.clone(),
                    );
                    line = line.push(secondary("Details", open).enabled(true));
                }
                false => {
                    if let Some(url) = &listing.website_url {
                        line = line
                            .push(secondary("Page", Message::OpenUrl(url.clone())).enabled(true));
                    }
                }
            }
            line = line.push(match (already, listing.opted_out, &listing.github) {
                (true, ..) => tag("Installed".to_owned(), theme::OK),
                (false, true, Some(repo)) => action_button(
                    "Get it from GitHub",
                    theme::INSTALL,
                    Message::InstallAddon("github".to_owned(), repo.clone()),
                )
                .enabled(!self.addons_locked()),
                (false, true, None) => tag("Page only".to_owned(), theme::MUTED),
                (false, false, _) => action_button(
                    "Install",
                    theme::INSTALL,
                    Message::InstallAddon(provider.clone(), listing.id.clone()),
                )
                .enabled(!self.addons_locked()),
            });
            body = body.push(container(line).padding(theme::MD).style(theme::row));
        }
        body.into()
    }

    /// One addon's project page: what it is, and what changed in the build
    /// you would get.
    ///
    /// The two halves are tabs rather than one long scroll because they
    /// answer different questions and a description can be thousands of words,
    /// the changelog would never be found under it.
    pub(crate) fn detail_card<'a>(&'a self, detail: &'a AddonDetail) -> Element<'a, Message> {
        let title = detail
            .loaded
            .as_ref()
            .map(|d| d.project.name.clone())
            .unwrap_or_else(|| detail.name.clone());
        let mut head = row![text(title).size(theme::TITLE)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);

        if let Some(loaded) = &detail.loaded {
            if loaded.project.download_count > 0 {
                head = head.push(muted(
                    format!("{} downloads", downloads_of(loaded.project.download_count)),
                    theme::CAPTION,
                ));
            }
        }

        let mut body = column![head].spacing(theme::SM).width(Length::Fixed(620.0));

        // The author line sits under the name rather than beside it: addons
        // with four maintainers are common and the row would wrap badly.
        if let Some(loaded) = &detail.loaded {
            if !loaded.project.authors.is_empty() {
                body = body.push(muted(
                    format!("by {}", loaded.project.authors.join(", ")),
                    theme::CAPTION,
                ));
            }
            if let Some(summary) = &loaded.project.summary {
                body = body.push(text(summary.clone()).size(theme::BODY));
            }
        }

        match (&detail.loaded, &detail.problem) {
            (None, None) => {
                body = body.push(muted("asking the catalogue…".to_owned(), theme::BODY));
            }
            (_, Some(problem)) => {
                body = body.push(empty(
                    problem.as_str(),
                    "Only CurseForge publishes a description and a changelog.",
                ));
            }
            (Some(loaded), None) => {
                body = body.push(self.detail_tabs(detail, loaded));
                body = body.push(self.detail_body(detail, loaded));
            }
        }

        // Reading the changelog and then having to find the row again to act
        // on it is the whole reason this card is worth having, so the action
        // is here. Which action depends on whether it is already installed.
        let installed = self.addon_is_installed(&detail.provider, &detail.id);

        let mut footer = row![Space::new().width(Length::Fill)].spacing(theme::SM);
        if let Some(url) = detail.loaded.as_ref().and_then(|d| d.project.website_url.clone()) {
            footer = footer.push(secondary("Open the page", Message::OpenUrl(url)).enabled(true));
        }
        // The same offer the installed row's menu makes, in the panel someone
        // reads *before* installing, where a project's own repository is
        // often the thing they came to find.
        if let Some(url) = detail
            .loaded
            .as_ref()
            .and_then(|d| d.project.source_url.clone().or_else(|| d.project.author_url.clone()))
        {
            footer =
                footer.push(secondary("Support the author", Message::OpenUrl(url)).enabled(true));
        }
        let project = detail.loaded.as_ref().map(|d| &d.project);
        let opted_out = project.is_some_and(|p| p.opted_out);
        let github = project
            .filter(|p| p.opted_out)
            .and_then(|p| p.source_url.as_deref())
            .and_then(lightforge_core::addons::browse::github_repo_of);
        if opted_out {
            body = body.push(
                text(match github {
                    Some(_) => "The author has disabled third-party downloads from CurseForge. \
                                They publish it on GitHub too, which is where Lightforge can get it.",
                    None => "The author has disabled third-party downloads. It can only be \
                             installed from the project page.",
                })
                .size(theme::LABEL)
                .color(theme::UPDATE),
            );
        }
        if !installed {
            let install: Option<Element<'static, Message>> = match (opted_out, github) {
                (false, _) => Some(
                    action_button(
                        "Install",
                        theme::INSTALL,
                        Message::InstallAddon(detail.provider.clone(), detail.id.clone()),
                    )
                    .enabled(!self.addons_locked()),
                ),
                (true, Some(repo)) => Some(
                    action_button(
                        "Get it from GitHub",
                        theme::INSTALL,
                        Message::InstallAddon("github".to_owned(), repo),
                    )
                    .enabled(!self.addons_locked()),
                ),
                (true, None) => None,
            };
            if let Some(install) = install {
                footer = footer.push(install);
            }
        }
        footer = footer.push(secondary("Close", Message::CloseAddonDetail).enabled(true));
        body = body.push(footer);

        container(container(body).padding(theme::LG).style(theme::floating))
            .center(Length::Fill)
            .padding(theme::XL)
            .into()
    }

    /// The two tab buttons, each naming what it holds.
    ///
    /// A tab with nothing behind it is shown disabled rather than hidden:
    /// hiding it would move the other one, and "this addon has no changelog"
    /// is itself an answer worth being able to see.
    pub(crate) fn detail_tabs<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let tab = |label: String, which: DetailTab, available: bool| {
            let selected = detail.tab == which;
            let key = format!("detail tab {label}");
            let tab = button(text(label).size(theme::BODY))
                .padding([theme::XS, theme::MD])
                .style(match selected {
                    true => theme::selected,
                    false => theme::ghost,
                });
            tappable(key, (available && !selected).then_some(Message::AddonDetailTab(which)), tab)
        };

        let changelog_label = match &loaded.changelog_build {
            Some(build) => format!("What changed in {build}"),
            None => "What changed".to_owned(),
        };
        row![
            tab(
                changelog_label,
                DetailTab::Changelog,
                loaded.changelog.as_ref().is_some_and(|d| !d.is_empty()),
            ),
            tab(
                "About".to_owned(),
                DetailTab::About,
                loaded.description.as_ref().is_some_and(|d| !d.is_empty()),
            ),
        ]
        .spacing(theme::XS)
        .into()
    }

    /// Whichever document the open tab names, scrolled.
    pub(crate) fn detail_body<'a>(
        &'a self,
        detail: &'a AddonDetail,
        loaded: &'a addonwork::Detail,
    ) -> Element<'a, Message> {
        let (document, missing) = match detail.tab {
            DetailTab::Changelog => (
                loaded.changelog.as_ref(),
                "This build ships no changelog — plenty of authors never write one.",
            ),
            DetailTab::About => (
                loaded.description.as_ref(),
                "This addon has no description beyond its summary.",
            ),
        };

        let Some(document) = document.filter(|d| !d.is_empty()) else {
            return empty(missing, "The CurseForge page has whatever else there is.");
        };

        let mut content = column![].spacing(theme::SM);
        for block in &document.blocks {
            content = content.push(rich_block(block));
        }
        if document.truncated {
            content = content.push(muted(
                "This is longer than Lightforge shows — the rest is on the page.".to_owned(),
                theme::CAPTION,
            ));
        }

        focus::region(
            "addon detail",
            scrollable(container(content).padding(theme::MD))
                .height(Length::Fixed(320.0))
                .style(theme::scrollbar),
        )
    }
}
