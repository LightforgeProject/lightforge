//! Launching: pre-flight, the three ways to start a game, installs.

use crate::*;

impl Lightforge {
    /// Get out of the way, where the user asked for that.
    ///
    /// Nothing un-minimizes when the game exits.
    pub(crate) fn minimize_if_asked(&self) -> Task<Message> {
        match self.config.minimize_on_launch {
            true => iced::window::latest().and_then(|id| iced::window::minimize(id, true)),
            false => Task::none(),
        }
    }

    /// A product's running game, when it has one.
    pub(crate) fn session(&self, code: &ProductCode) -> Option<&Session> {
        self.playing.iter().find(|s| &s.code == code)
    }

    /// A launch of ours is under way or one of our games is up: the store
    /// and the prefix are in use.
    pub(crate) fn in_play(&self) -> bool {
        self.launching.is_some() || !self.playing.is_empty()
    }

    /// Nothing that needs the launcher to itself may start.
    pub(crate) fn locked(&self) -> bool {
        self.busy || !self.playing.is_empty()
    }

    /// The launched game is up: its tile turns to STOP, and the launcher is
    /// free for another game while this one runs.
    pub(crate) fn game_up(
        &mut self,
        game: lightforge_core::preflight::GameProcess,
        settle_after: Option<u64>,
    ) -> Task<Message> {
        let code = self.launching.take().unwrap_or_else(|| self.selected.code.clone());
        self.busy = false;
        self.status = format!("{} is running", game.name);
        self.playing.retain(|s| s.code != code);
        self.playing.push(Session { code: code.clone(), game: game.clone(), stopping: false });
        Task::perform(
            tasks::await_game_exit(self.layout.clone(), code.clone(), game, settle_after),
            move |result| Message::GameExited(code.clone(), result),
        )
    }

    /// Ask the process table whether the Battle.net client is up.
    pub(crate) fn check_client(&self) -> Task<Message> {
        Task::perform(tasks::client_running(self.layout.clone()), Message::ClientSeen)
    }

    /// Close Battle.net, unless a launch is under way: it may be the one
    /// starting the client.
    pub(crate) fn quit_client(&mut self) -> Task<Message> {
        if self.busy || self.quitting_client {
            return Task::none();
        }
        self.quitting_client = true;
        self.status = "closing Battle.net…".to_owned();
        Task::perform(
            tasks::quit_battle_net(self.layout.clone(), self.selected.code.clone()),
            Message::BattleNetQuit,
        )
    }

    /// What a finished launch or game owes: transfers paused for play go back
    /// in the queue once nothing of ours runs, and what the session may have
    /// changed is read again.
    pub(crate) fn after_play(&mut self, code: ProductCode) -> Task<Message> {
        if !self.in_play() {
            for paused in std::mem::take(&mut self.resume_after_play) {
                if let Some(at) = self.queue.iter().position(|d| d.code == paused) {
                    self.queue[at].stage = Stage::Queued;
                    self.queue[at].sample.stage = "queued".to_owned();
                }
            }
        }
        if self.downloads.pending().contains(&code) {
            if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                self.queue[at].stage = Stage::Queued;
            }
        }
        // The session may have filled the catalogue cache; 25 ms to re-read.
        Task::batch([
            self.pump(),
            self.check_client(),
            self.reload(self.selected.code.clone()),
            Task::perform(
                tasks::game_families(
                    self.layout.clone(),
                    self.config.prefix_user.clone(),
                    self.config.region,
                ),
                |families| Message::FamiliesRead(Box::new(families)),
            ),
            // And what it installed: only Agent's registry knows.
            Task::perform(
                tasks::installed_versions(self.layout.clone()),
                Message::InstalledVersions,
            ),
        ])
    }

    /// Clear the way for a launch, then let [`Message::PreflightDone`] start it.
    pub(crate) fn preflight(&mut self, what: Launch) -> Task<Message> {
        // An older Proton than the prefix's is allowed, but not silently.
        if let Some(dialog) = self.downgrade_ahead(what) {
            self.downgrade_dialog = Some(dialog);
            return Task::none();
        }
        // Here too: pre-flight relinks, and relinking a missing library
        // leaves an empty install.
        if !matches!(what, Launch::Client) {
            if let Some(drive) = self.offline_drive(&self.selected.code) {
                self.error = Some(format!("{drive} is not mounted").into());
                return Task::none();
            }
        }

        // Maintenance rewrites the store; downloads are paused by pre-flight.
        if let Some(what) = self.maintenance() {
            self.error = Some(format!("wait for {what} to finish first").into());
            return Task::none();
        }

        self.busy = true;
        self.error = None;
        self.status = "pre-flight…".to_owned();
        self.launching = Some(self.selected.code.clone());

        // Wait for an addon pass extracting into what the client reads.
        if self.addon_auto_running && !matches!(what, Launch::Client) {
            self.status = "finishing addon updates…".to_owned();
            self.launch_after_addons = Some(what);
            return Task::none();
        }

        // Pre-flight pauses this product's download; the rest only when asked.
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

        // The long wait starts here; say what it is.
        match what {
            Launch::Game => {
                self.status = "starting the game…".to_owned();
                Task::batch([
                    Task::perform(
                        tasks::start_game(self.layout.clone(), code.clone()),
                        Message::GameStarted,
                    ),
                    self.minimize_if_asked(),
                ])
            }
            Launch::BattleNet => {
                // Battle.net gives nothing back: the prefix is watched.
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
    /// `None` unless this would be an unagreed downgrade.
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
    /// A guess would open Warcraft III.
    pub(crate) fn family_of(&self, code: &ProductCode) -> Option<String> {
        lightforge_core::battlenet::resolve_game_uid(code, &self.families)
    }

    /// What Battle.net's install flow calls this product.
    ///
    /// The catalogue's install key (`hs_beta`), else the product code.
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

    /// What a hand-off game's button can be: without a family, only opening
    /// the client.
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
    /// Except a missing drive, decided here, which outranks every state.
    pub(crate) fn action(&self) -> ActionState {
        if let Some(drive) = self.offline_drive(&self.selected.code) {
            return ActionState::Offline { drive };
        }
        // Only this product's own transfer. Anything else is another game's
        // business, however loudly it is downloading. A pre-download leaves
        // PLAY alone: the installed build is untouched by it, and a launch
        // pauses it as it would any of this game's downloads.
        let transfer = self
            .download_for(&self.selected.code)
            .filter(|d| !d.pre_download)
            .map(Download::transfer);
        state::decide(&self.local, self.remote.as_ref(), transfer)
    }

    /// What, other than a download, is writing to a game store right now.
    ///
    /// Each is a writer of the shared store; core does not stop two at once.
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
        } else if self.moving.is_some() {
            Some("moving a game")
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

    /// The one transfer actually moving bytes, if any.
    pub(crate) fn running(&self) -> Option<&Download> {
        self.queue.iter().find(|d| matches!(d.stage, Stage::Running { .. }))
    }

    /// Open the install dialog and start pricing the build.
    pub(crate) fn open_install(&mut self) -> Task<Message> {
        let code = self.selected.code.clone();
        let targets = self.install_targets(&code);
        // Opens on the product's own drive, else the default.
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
        // Priced without opt-ins, against the existing store.
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
        // Re-checked, not trusted from when the dialog opened: the
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
    /// Also sets the library up and links it under its drive letters.
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

    /// Which of the store's maintenance actions would go ahead now, for the
    /// cog menu and Game Settings alike, so the two cannot disagree.
    ///
    /// Only when nothing writes the store and no launch is under way.
    pub(crate) fn can_maintain(&self) -> Maintain {
        let installed = self.local.version.is_some();
        let store_free = self.store_writer().is_none() && self.deep_cancel.is_none();
        Maintain {
            scan: installed && !self.locked() && store_free,
            deep_scan: installed && !self.locked() && !self.scanning && self.maintenance().is_none(),
            uninstall: installed && !self.locked() && store_free,
            repair: !self.locked() && store_free,
            move_game: installed && !self.locked() && store_free,
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
