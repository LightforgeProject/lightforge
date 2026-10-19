//! Launching: pre-flight, the three ways to start a game, installs.

use crate::*;

impl Lightforge {
    /// Clear the way for a launch, then let [`Message::PreflightDone`] start it.
    /// Get out of the way, where the user asked for that.
    ///
    /// The window id has to be asked for rather than kept: this is a
    /// single-window application, so there is exactly one, but iced hands it
    /// out at runtime and the only other place that needs it, the close
    /// request, is given it by the event.
    ///
    /// Nothing un-minimizes when the game exits. A launcher that raises itself
    /// over whatever the user moved on to is the behaviour this kind of
    /// setting is turned on to avoid; the window is in the taskbar.
    pub(crate) fn minimize_if_asked(&self) -> Task<Message> {
        match self.config.minimize_on_launch {
            true => iced::window::latest().and_then(|id| iced::window::minimize(id, true)),
            false => Task::none(),
        }
    }

    pub(crate) fn preflight(&mut self, what: Launch) -> Task<Message> {
        // Before anything is told to start: the prefix is shared by every
        // product and one Battle.net login, and Proton only ever migrates it
        // forward. Running an older build against it is allowed, somebody
        // pinned it on purpose, but not silently.
        if let Some(dialog) = self.downgrade_ahead(what) {
            self.downgrade_dialog = Some(dialog);
            return Task::none();
        }
        // Checked again here, not just on the button: pre-flight relinks the
        // prefix, and relinking a library that is not there is what would
        // leave an empty install behind for the client to offer a repair on.
        // A Battle.net hand-off is held too, the game it would start is on
        // the same drive.
        if !matches!(what, Launch::Client) {
            if let Some(drive) = self.offline_drive(&self.selected.code) {
                self.error = Some(format!("{drive} is not mounted").into());
                return Task::none();
            }
        }

        // Scan, repair, compaction and uninstall rewrite the store the client
        // is about to open. Downloads are not refused here: pre-flight pauses
        // them, and the launch waits for them to come to rest.
        if let Some(what) = self.maintenance() {
            self.error = Some(format!("wait for {what} to finish first").into());
            return Task::none();
        }

        self.busy = true;
        self.error = None;
        self.status = "pre-flight…".to_owned();
        self.launching = Some(self.selected.code.clone());

        // An unattended addon pass is extracting into the directory the
        // client is about to read. Hold the launch until it is done rather
        // than racing it. It is seconds, and the alternative is a half
        // written addon loaded at login. Opening the Battle.net client reads
        // nothing of ours, so it does not wait.
        if self.addon_auto_running && !matches!(what, Launch::Client) {
            self.status = "finishing addon updates…".to_owned();
            self.launch_after_addons = Some(what);
            return Task::none();
        }

        // Pre-flight pauses the launched product's own download by itself,
        // that one is a correctness guard, since the client and the
        // downloader would share one CASC store. The rest of the queue shares
        // that store too, so it is paused here when asked, and resumed when
        // the game exits.
        if self.config.pause_downloads_while_playing {
            let others: Vec<ProductCode> = self
                .queue
                .iter()
                .filter(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued))
                .map(|d| d.code.clone())
                .collect();
            for code in others {
                self.pause(&code);
                self.resume_after_play.push(code);
            }
        }
        Task::perform(
            tasks::preflight(
                self.layout.clone(),
                self.selected.code.clone(),
                self.downloads.clone(),
                // Cleared under whichever build is about to run in the prefix.
                Self::runs_for(what),
            ),
            move |result| Message::PreflightDone(what, result),
        )
    }

    /// Start what a finished pre-flight cleared the way for.
    pub(crate) fn start_launch(&mut self, what: Launch) -> Task<Message> {
        let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());
        if what != Launch::Game {
            // Kept if one is running: its timestamp is still the last read.
            self.registry_watch.get_or_insert_with(RegistryWatch::new).since = Instant::now();
        }

        // The long wait starts here, so say what it actually is. The
        // old code left "pre-flight…" on screen for the whole session
        // and the wind-down after it, which reads as a stuck launcher
        // rather than a running game.
        match what {
            Launch::Game => {
                // Play unlocks when the *game* exits, not when the
                // prefix is quiet, the wind-down behind it is watched
                // and reaped without holding the button. See
                // `tasks::run_game`.
                self.status = "running — Play unlocks when the game closes".to_owned();
                Task::batch([
                    Task::perform(
                        tasks::run_game(self.layout.clone(), code.clone()),
                        Message::Launched,
                    ),
                    self.minimize_if_asked(),
                ])
            }
            Launch::BattleNet => {
                // Two steps, because Battle.net gives nothing back:
                // the client is told to launch, then the prefix is
                // watched until the game shows up. Until it does,
                // this is still a launch in progress.
                self.status = "handing off to Battle.net…".to_owned();
                Task::perform(
                    tasks::launch_via_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                        self.family_of(&code),
                        self.config.battle_net_minimized,
                    ),
                    Message::HandoffStarted,
                )
            }
            Launch::Install => {
                self.status = "opening Battle.net…".to_owned();
                Task::perform(
                    tasks::install_via_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                        self.install_uid_of(&code),
                        self.label_of(&code),
                    ),
                    Message::Launched,
                )
            }
            Launch::Client => {
                self.status = "opening Battle.net…".to_owned();
                Task::perform(
                    tasks::open_battle_net(
                        self.layout.clone(),
                        code.clone(),
                        self.config.prefix_user.clone(),
                    ),
                    Message::Launched,
                )
            }
        }
    }

    /// The warning this launch owes the user, or `None` to go ahead.
    ///
    /// `None` covers every ordinary case: a prefix that does not exist yet, the
    /// build that made it, a newer one, a pair of tags with no ordering between
    /// them, and a build already agreed to this session.
    pub(crate) fn downgrade_ahead(&self, what: Launch) -> Option<DowngradeDialog> {
        use lightforge_core::proton::PrefixFit;

        let runs = Self::runs_for(what);
        let code = self.selected.code.clone();
        let (pick, fit) = tasks::prefix_fit(&self.layout, &code, runs);
        let (pick, built_by) = match (pick, fit) {
            (Some(pick), PrefixFit::Older { built_by }) => (pick, built_by),
            _ => return None,
        };
        if self.downgrade_ok.contains(&pick) {
            return None;
        }
        Some(DowngradeDialog {
            what,
            pick,
            built_by,
            label: self.label_of(&code),
        })
    }

    /// Which workload an intent runs as, the game itself, or the client
    /// acting for it.
    pub(crate) fn runs_for(what: Launch) -> tasks::Runs {
        match what {
            Launch::Game => tasks::Runs::Game,
            Launch::BattleNet | Launch::Client | Launch::Install => tasks::Runs::BattleNet,
        }
    }

    /// Which Battle.net tile a product belongs to, or `None` when neither
    /// the compiled-in families nor the client's catalogue name one.
    ///
    /// `None` is the honest answer rather than a guess: the client resolves
    /// an unknown family to the first one it knows, which is Warcraft III,
    /// so a wrong uid opens somebody else's game.
    pub(crate) fn family_of(&self, code: &ProductCode) -> Option<String> {
        lightforge_core::battlenet::resolve_game_uid(code, &self.families)
    }

    /// What Battle.net's install flow calls this product.
    ///
    /// The catalogue's install key where there is one, the product code
    /// otherwise. They are the same for most titles, and differ where the
    /// CDN and the client disagree (`hsb` against `hs_beta`).
    pub(crate) fn install_uid_of(&self, code: &ProductCode) -> String {
        self.families
            .install_uid(code.as_str())
            .unwrap_or_else(|| code.as_str())
            .to_owned()
    }

    /// Whether Lightforge installs this product or hands it to Battle.net.
    ///
    /// The catalog's answer where it has one, the code's shape otherwise, so a
    /// product added before the manifest was read is still classified.
    pub(crate) fn kind_of(&self, code: &ProductCode) -> Kind {
        self.catalog.get(code).map(|e| e.kind).unwrap_or_else(|| Kind::of(code))
    }

    /// The action panel for a game Battle.net installs and starts.
    ///
    ///
    /// Deliberately bare: no Install, no Update, no Play, not even greyed out.
    /// Offering those disabled would say Lightforge could manage this game and
    /// merely will not.
    /// What a hand-off game's button can honestly be, in order of how much
    /// is known. Without a game family the client cannot be told which tile
    /// to open, and a guess lands on somebody else's game, so that case
    /// promises nothing beyond opening the client.
    pub(crate) fn handoff_act(&self) -> HandoffAct {
        let family = self.family_of(&self.selected.code);
        let installed = self.installed_versions.contains_key(self.selected.code.as_str());
        match (family.is_some(), installed) {
            (false, _) => HandoffAct::OpenClient,
            (true, true) => HandoffAct::Play,
            (true, false) => HandoffAct::Install,
        }
    }

    /// The button state, which is core's decision and not the GUI's.
    ///
    /// The one thing decided here is the drive being away. That is a fact
    /// about the filesystem rather than about a build, and it outranks every
    /// other state: a product whose library is not mounted has nothing to
    /// launch, nothing worth downloading, and must not be repaired, the
    /// files are intact somewhere else.
    pub(crate) fn action(&self) -> ActionState {
        if let Some(drive) = self.offline_drive(&self.selected.code) {
            return ActionState::Offline { drive };
        }
        // Only this product's own transfer. Anything else is another game's
        // business, however loudly it is downloading.
        let transfer = self.download_for(&self.selected.code).map(Download::transfer);
        state::decide(&self.local, self.remote.as_ref(), transfer)
    }

    /// The one transfer actually moving bytes, if any.
    /// What, other than a download, is writing to a game store right now.
    ///
    /// Every WoW flavour in a library shares one CASC store, and nothing in
    /// core stops two of Lightforge's own writers from opening it at once,
    /// core only checks the prefix for processes. So the line is held here:
    /// each of these is a writer, and none starts while another runs.
    pub(crate) fn maintenance(&self) -> Option<&'static str> {
        if self.scanning {
            Some("the scan")
        } else if self.repairing.is_some() {
            Some("the repair")
        } else if self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. })) {
            Some("freeing unused game data")
        } else if self.uninstall_dialog.as_ref().is_some_and(|d| d.working) {
            Some("the uninstall")
        } else if self.discarding.is_some() {
            Some("removing the cancelled download")
        } else {
            None
        }
    }

    /// Whether a launch has to hold: a paused download is still winding
    /// down, or the launched product's own run has not stopped yet.
    pub(crate) fn launch_must_wait(&self) -> bool {
        self.winding_down.is_some()
            || self.queue.iter().any(|d| {
                Some(&d.code) == self.launching.as_ref() && matches!(d.stage, Stage::Running { .. })
            })
    }

    pub(crate) fn running(&self) -> Option<&Download> {
        self.queue.iter().find(|d| matches!(d.stage, Stage::Running { .. }))
    }

    /// Open the install dialog and start pricing the build.
    pub(crate) fn open_install(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        let targets = self.install_targets(&code);
        // Opens on the drive this product is already assigned to, else the
        // default one, not on the first row, which would silently re-target a
        // product whose files are elsewhere.
        let assigned = self
            .config
            .product(&code)
            .and_then(|p| p.profile.library.clone());
        let chosen = assigned
            .and_then(|id| targets.iter().position(|t| t.id == id))
            .unwrap_or(0);
        self.install_dialog = Some(InstallDialog {
            code: code.clone(),
            label: self.selected.label.clone(),
            stage: DialogStage::Calculating,
            high_res: self.opted_optional(&code).iter().any(|t| t == HIGH_RES),
            targets,
            chosen,
        });
        // Priced with no opt-ins, since the dialog adds each extra's own size,
        // and against the store already there, so files another WoW version
        // shares are not counted.
        Task::perform(
            tasks::offerings(
                self.http.clone(),
                code.clone(),
                self.config.region,
                Vec::new(),
                Some(self.layout.wow_install(&code)),
            ),
            move |result| Message::InstallOffered(code.clone(), Box::new(result)),
        )
    }

    /// Apply the dialog's answers and queue the install.
    pub(crate) fn start_install(&mut self) -> Task<Message> {
        let Some(dialog) = self.install_dialog.clone() else {
            return Task::none();
        };
        if dialog.code != self.selected.code {
            self.install_dialog = None;
            return Task::none();
        }
        // Re-checked rather than trusted from when the dialog opened: the
        // drive may have gone away while the user read the size.
        if let Some(target) = dialog.target() {
            if let Err(e) = self.target_install(&dialog.code, &target.id) {
                self.error = Some(e.into());
                return Task::none();
            }
        }
        if let DialogStage::Ready(offerings) = &dialog.stage {
            let offered = offerings.optional_bytes.iter().any(|(t, b)| t == HIGH_RES && *b > 0);
            if offered {
                self.set_optional(&dialog.code, HIGH_RES, dialog.high_res);
            }
        }
        self.install_dialog = None;
        self.enqueue()
    }

    /// Every drive this product could be installed to, main first.
    pub(crate) fn install_targets(&self, code: &ProductCode) -> Vec<InstallTarget> {
        let mut targets: Vec<InstallTarget> = self
            .config
            .libraries
            .iter()
            .map(|library| InstallTarget {
                id: library.id.clone(),
                label: library.label.clone(),
                install: self.layout.wow_install_in(library),
                disk: library.disk(),
                state: library.state(),
                shares: storage::holds_wow(&self.layout, library),
            })
            .collect();
        let main = self.config.libraries.iter().position(|l| l.is_default).unwrap_or(0);
        if main < targets.len() {
            let first = targets.remove(main);
            targets.insert(0, first);
        }
        let _ = code;
        targets
    }

    /// Assign a product to a library and make the prefix able to see it.
    ///
    /// Writing the assignment is the cheap half. The other half is that the
    /// library has to be set up, its folder and marker written, and linked
    /// into the prefix under its own drive letters, or the download would land
    /// somewhere Wine cannot address and the in-game shop would have no drive
    /// root to start on.
    pub(crate) fn target_install(&mut self, code: &ProductCode, id: &str) -> Result<(), String> {
        let Some(library) = self.config.libraries.iter_mut().find(|l| l.id == id) else {
            return Err("that drive is no longer configured".to_owned());
        };
        if library.state() == library::State::Missing {
            return Err(format!("{} is not mounted", library.label));
        }
        library.claim().map_err(|e| chain(&e))?;
        if let Some(product) = self.config.product_mut(code) {
            product.profile.library = Some(id.to_owned());
        }
        self.save_config();
        self.rebuild_layout();

        // Before setup there is no prefix to link into, and setup makes the
        // links itself; creating `drive_c` early would get in wineboot's way.
        if lightforge_core::prefix::PrefixStatus::inspect(&self.layout).initialized {
            lightforge_core::prefix::link_libraries(&self.layout).map_err(|e| chain(&e))?;
        }
        Ok(())
    }

    /// The cog menu: what you do *about* a game rather than to play it.
    ///
    /// Laid out above the action panel rather than stacked over the page,
    /// because iced 0.13 has no popover and a floating layer would need the
    /// panel's height as a magic number to sit clear of it. Taking its room
    /// from the spacer above the panel puts it in the right place by
    /// construction, at any window size.
    /// Which of the store's maintenance actions would go ahead now, for the
    /// cog menu and Game Settings alike, so the two cannot disagree.
    ///
    /// Offered only when the handler would go ahead: nothing else writing to
    /// the store, which every flavour shares, and no launch under way.
    pub(crate) fn can_maintain(&self) -> Maintain {
        let installed = self.local.version.is_some();
        let store_free = self.store_writer().is_none() && self.deep_cancel.is_none();
        Maintain {
            scan: installed && !self.busy && store_free,
            deep_scan: installed && !self.busy && !self.scanning && self.maintenance().is_none(),
            uninstall: installed && !self.busy && store_free,
            repair: !self.busy && store_free,
        }
    }

    /// A product's name as the picker shows it, or its code.
    pub(crate) fn label_of(&self, code: &ProductCode) -> String {
        self.choices
            .iter()
            .find(|c| &c.code == code)
            .map(|c| c.label.clone())
            .unwrap_or_else(|| code.as_str().to_owned())
    }
}
