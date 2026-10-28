//! Addons: installing, updating, importing, searching and the detail card.

use crate::*;

impl Lightforge {
    pub(crate) fn update_addons(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_addon_choices(message)
            .or_else(|m| self.update_addon_sharing(m))
            .or_else(|m| self.update_addon_one(m))
            .or_else(|m| self.update_addon_updates(m))
            .or_else(|m| self.update_addon_list(m))
    }

    /// Per-addon choices: channel, ignore, auto-update, pin, and their defaults.
    fn update_addon_choices(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::DismissUnavailable(name) => {
                if let Some(report) = &mut self.addon_report {
                    report.unavailable.retain(|item| item.name != name);
                }
                Task::none()
            }

            Message::AddonChannelDefault(choice) => {
                self.config.addon_channel = choice.0;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // check or update, which is also when it could matter.
                Task::none()
            }

            Message::AddonChannelFor(provider, id, choice) => {
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Ok(Task::none());
                };
                if let Err(e) = manage::set_channel(&options, &provider, &id, choice.0) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonMenu(which) => {
                self.addon_menu_open = which;
                Task::none()
            }

            Message::AddonIgnore(provider, id, ignored) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Ok(Task::none());
                };
                if let Err(e) = manage::set_ignored(&options, &provider, &id, ignored) {
                    self.error = Some(Problem::of(&e));
                }
                // Re-list, not patch the row in place: the ledger on
                // disk is the truth the rest of the page is drawn from.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdate(provider, id, choice) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Ok(Task::none());
                };
                if let Err(e) = manage::set_auto_update(&options, &provider, &id, choice) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonPin(provider, id, file_id) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Ok(Task::none());
                };
                if let Err(e) = manage::set_pinned(&options, &provider, &id, file_id) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdateDefault(on) => {
                self.config.addon_auto_update = on;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // pass, which is also the first time it could matter.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }
            other => return Err(other),
        })
    }

    /// Taking in hand-installed addons, and importing or exporting a list.
    fn update_addon_sharing(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ImportPinned(on) => {
                self.import_pinned = on;
                Task::none()
            }

            Message::AdoptAddons => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                self.addons_busy = true;
                self.error = None;
                self.addon_auto_note = Some("looking up what is installed…".to_owned());
                Task::perform(
                    addonwork::adopt(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                    ),
                    |result| Message::AdoptFinished(Box::new(result)),
                )
            }

            Message::AdoptFinished(result) => {
                self.addons_busy = false;
                match *result {
                    Ok(adopted) => {
                        self.addon_auto_note = Some(match adopted.len() {
                            0 => "nothing new recognised — every addon here is \
                                  either already managed or not on CurseForge"
                                .to_owned(),
                            1 => format!("{} is now managed", adopted[0].name),
                            n => format!("{n} addons are now managed"),
                        });
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }

            Message::TokenChanged(value) => {
                self.token_input = value;
                Task::none()
            }

            Message::ImportToken => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                let token = self.token_input.trim().to_owned();
                if token.is_empty() {
                    return Ok(Task::none());
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Import;
                self.error = None;

                Task::perform(
                    addonwork::import(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        token,
                        self.config.addon_channel,
                        self.import_pinned,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::ExportAddons => {
                match addonwork::export(&self.layout, &self.selected.code) {
                    Ok(token) => {
                        self.export_token = token;
                        self.status = "export ready".to_owned();
                    }
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }

            Message::CopyExport => iced::clipboard::write(self.export_token.clone()),

            Message::ExportEdited(text) => {
                self.export_token = text;
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// One addon: its detail card, and installing, reinstalling or picking a version of it.
    fn update_addon_one(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ShowAddonDetail(provider, id, name) => {
                self.addon_menu_open = None;
                // Changelog first if installed, About when browsing.
                let installed = self.addon_is_installed(&provider, &id);
                self.addon_detail = Some(AddonDetail {
                    provider: provider.clone(),
                    id: id.clone(),
                    name,
                    loaded: None,
                    problem: None,
                    tab: match installed {
                        true => DetailTab::Changelog,
                        false => DetailTab::About,
                    },
                });
                Task::perform(
                    addonwork::detail(self.http.clone(), self.selected.code.clone(), provider, id),
                    |result| Message::AddonDetailLoaded(Box::new(result)),
                )
            }

            Message::AddonDetailLoaded(result) => {
                // Dropped if the card was closed meanwhile.
                if let Some(detail) = self.addon_detail.as_mut() {
                    match *result {
                        Ok(loaded) => detail.loaded = Some(loaded),
                        Err(e) => detail.problem = Some(e),
                    }
                }
                Task::none()
            }

            Message::AddonDetailTab(tab) => {
                if let Some(detail) = self.addon_detail.as_mut() {
                    detail.tab = tab;
                }
                Task::none()
            }

            Message::CloseAddonDetail => {
                self.addon_detail = None;
                Task::none()
            }

            Message::InstallVersion(provider, id, file_id) => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                self.version_picker = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::install_version(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        file_id,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::InstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = self.addon_tab;
                self.error = None;

                Task::perform(
                    addonwork::install_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::ReinstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                self.addon_menu_open = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::reinstall_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }
            other => return Err(other),
        })
    }

    /// Updating: checking, the bulk update, and the unattended pass before Play.
    fn update_addon_updates(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::AutoUpdateAddons => {
                // Not while a game runs (the prefix is asked too) or the user's
                // own addon work is: both write the same directory.
                if self.addon_auto_running
                    || self.addons_busy
                    || self.launching.is_some()
                    || self.addon_auto_done.contains(&self.selected.code)
                {
                    return Ok(Task::none());
                }
                self.addon_auto_done.push(self.selected.code.clone());
                self.addon_auto_running = true;
                let (tx, _rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                Task::perform(
                    addonwork::auto_update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        self.config.addon_auto_update,
                        tx,
                    ),
                    |result| Message::AutoUpdateFinished(result.map(|r| r.map(Box::new))),
                )
            }

            Message::AutoUpdateFinished(result) => {
                self.addon_auto_running = false;
                let mut tasks = Vec::new();

                match result {
                    // Nothing to do, so nothing to say.
                    Ok(None) => {}
                    Ok(Some(report)) => {
                        if !report.installed.is_empty() {
                            self.addon_auto_note = Some(match report.installed.len() {
                                1 => format!("{} updated automatically", report.installed[0]),
                                n => format!("{n} addons updated automatically"),
                            });
                            tasks.push(Task::perform(
                                addonwork::list(
                                    self.layout.clone(),
                                    self.selected.code.clone(),
                                ),
                                |listing| Message::AddonsListed(Box::new(listing)),
                            ));
                        }
                    }
                    // Said quietly and only here: a launcher that cannot
                    // reach CurseForge is not a launcher that cannot play.
                    Err(e) => self.addon_auto_note = Some(format!("addon check failed: {e}")),
                }

                // Whoever was waiting on this can go now.
                if let Some(what) = self.launch_after_addons.take() {
                    tasks.push(self.preflight(what));
                }
                Task::batch(tasks)
            }

            Message::AddonsListed(listing) => {
                // A slow listing for the product shown before a switch.
                if listing.dir != self.layout.addons_dir(&self.selected.code) {
                    return Ok(Task::none());
                }
                if let Some(problem) = &listing.problem {
                    self.error = Some(format!("the addon list cannot be read: {problem}").into());
                }
                self.addons = *listing;
                Task::none()
            }

            Message::UpdateAddons => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonsFinished(result) => {
                self.addons_busy = false;
                self.addon_rx = None;
                match result {
                    Ok(report) => {
                        self.addon_report = Some(*report);
                        // What was installed is no longer an update waiting.
                        let installed = &self.addon_report.as_ref().expect("just set").installed;
                        self.updates.retain(|u| !installed.contains(&u.name));
                        // The list on screen is now stale in both directions.
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::CheckAddonUpdates => {
                let code = self.selected.code.clone();
                Task::perform(
                    addonwork::check_updates(
                        self.http.clone(),
                        self.layout.clone(),
                        code.clone(),
                        self.config.addon_channel,
                    ),
                    move |result| Message::AddonUpdatesChecked(code.clone(), result),
                )
            }

            Message::AddonUpdatesChecked(code, result) => {
                if code != self.selected.code {
                    return Ok(Task::none());
                }
                match result {
                    Ok(statuses) => self.updates = statuses,
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// The installed list and the search: filtering, tabs, removing.
    fn update_addon_list(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ArmRemoveAddon(provider, id) => {
                self.addon_remove_armed = Some((provider, id));
                Task::none()
            }

            Message::CancelRemoveAddon => {
                self.addon_remove_armed = None;
                Task::none()
            }

            Message::AddonFilterChanged(text) => {
                self.addon_filter = text;
                Task::none()
            }

            Message::OpenAddonsDir => {
                // The layout, not the listing: the listing still names the
                // previous product's folder until its own scan comes back.
                if let Err(e) = tasks::open_path(self.layout.addons_dir(&self.selected.code)) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::RemoveAddon(provider, id) => {
                if self.addons_locked() {
                    return Ok(Task::none());
                }
                self.addon_remove_armed = None;
                Task::perform(
                    addonwork::remove(
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                    ),
                    Message::AddonRemoved,
                )
            }

            Message::AddonRemoved(result) => {
                match result {
                    Ok(folders) => self.status = format!("removed {}", folders.join(", ")),
                    Err(e) => self.error = Some(e.into()),
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonTab(tab) => {
                self.addon_tab = tab;
                self.error = None;
                // Leaving the list disarms Remove.
                self.addon_remove_armed = None;
                // Browse opens on the most-downloaded addons.
                match tab == AddonTab::Browse && self.search_input.trim().is_empty() {
                    true => self.popular_if_unasked(),
                    false => Task::none(),
                }
            }

            Message::SearchChanged(value) => {
                self.search_input = value;
                self.search_seq += 1;
                let seq = self.search_seq;

                // Typing searches, from two letters, after a pause. An empty
                // box goes back to the popular list.
                if self.search_input.trim().is_empty() {
                    return Ok(self.popular_if_unasked());
                }
                if self.search_input.trim().chars().count() < SEARCH_MIN {
                    return Ok(Task::none());
                }
                Task::perform(
                    async move {
                        tokio::time::sleep(SEARCH_DEBOUNCE).await;
                        seq
                    },
                    Message::SearchSettled,
                )
            }

            Message::SearchSettled(seq) => {
                // A later keystroke has already scheduled its own search.
                match seq == self.search_seq && !self.searching {
                    true => self.update(Message::RunSearch),
                    false => Task::none(),
                }
            }

            Message::RunSearch => {
                // An empty query asks for the most-downloaded addons.
                let query = self.search_input.trim().to_owned();
                self.searching = true;
                Task::perform(
                    addonwork::search(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        query,
                    ),
                    |outcome| Message::SearchDone(Box::new(outcome)),
                )
            }

            Message::SearchDone(outcome) => {
                self.searching = false;
                // Ignore a reply for a query the user has already moved past.
                if outcome.query == self.search_input.trim() {
                    self.search = *outcome;
                }
                Task::none()
            }
            other => return Err(other),
        })
    }
}
