//! Setup, the 32-bit runtime, Proton builds and pins, and where things install.

use crate::*;

impl Lightforge {
    pub(crate) fn update_setup(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::CopyRuntimeCommand(command) => iced::clipboard::write(command),
            Message::RecheckRuntime => Task::perform(
                provision::status(self.layout.clone(), self.config.clone()),
                |status| Message::SetupStatus(status.map(Box::new)),
            ),
            Message::SetupStatus(status) => {
                let status = status.map(|boxed| *boxed);
                let measure = match &status {
                    Some(status) => Task::perform(
                        provision::measure(
                            status.installed.iter().map(|b| b.path.clone()).collect(),
                        ),
                        Message::ProtonSizes,
                    ),
                    None => Task::none(),
                };
                let mut opened = Task::none();
                // Open on settings, at its setup section, when the machine
                // cannot launch anything yet, but never yank the user out of
                // a screen they chose.
                if let Some(status) = &status {
                    // Not while the first run is up: it has a setup step of its
                    // own, and moving the screen underneath it would be where
                    // the user lands the moment they dismiss it.
                    if !status.is_ready()
                        && self.screen == Screen::Dashboard
                        && !self.setup_running
                        && self.wizard.is_none()
                    {
                        self.screen = Screen::Settings;
                        self.settings_jump = Some(SettingsTab::Setup);
                        // Behind the splash on a first start, where nothing
                        // of the page is drawn to be jumped into yet. Taking
                        // the splash away arrives on the page, jump and all.
                        if self.splash.is_none() {
                            opened = self.arrived();
                        }
                    }
                }
                // Installing Battle.net is what puts its mark in the prefix,
                // and setup is the only thing that installs it, so this is the
                // one moment the answer can have changed.
                self.bnet_icon =
                    self.layout.battle_net_icon().and_then(|path| art::try_mark(&path));
                // Not under the first run or another install, and once a
                // session: offline it would fail again on every re-read.
                let fetch = match &status {
                    Some(status)
                        if !self.tested_proton_asked
                            && !self.setup_running
                            && self.wizard.is_none()
                            && proton::wants_tested(
                                &status.installed,
                                self.config.default_proton.version(),
                                proton::prefix_build(&self.layout.compat_data()).as_deref(),
                            ) =>
                    {
                        self.tested_proton_asked = true;
                        // Nobody reads this progress, and it stays out of
                        // `setup_running` so nothing in the UI locks for it.
                        let (tx, _) = tokio::sync::watch::channel(provision::Progress::default());
                        Task::perform(
                            provision::install_build(proton::TESTED.to_owned(), tx),
                            Message::TestedProtonFetched,
                        )
                    }
                    _ => Task::none(),
                };
                self.setup = status;
                Task::batch([measure, opened, fetch])
            }
            // Quiet either way: it was nobody's request. A failure leaves the
            // build they have, and the next start asks again.
            Message::TestedProtonFetched(result) => match result {
                Ok(()) => Task::perform(
                    provision::status(self.layout.clone(), self.config.clone()),
                    |status| Message::SetupStatus(status.map(Box::new)),
                ),
                Err(_) => Task::none(),
            },
            Message::Wizard(msg) => wizard::update(self, msg),
            Message::InstallBattleNet => {
                self.config.battle_net_declined = false;
                self.save_config();
                // Straight into the same run the button below starts: the two
                // steps before it are already done, so this is the one step.
                Task::done(Message::RunSetup)
            }
            Message::RunSetup => {
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;

                Task::perform(
                    provision::run(
                        self.layout.clone(),
                        self.config.clone(),
                        self.config.default_proton.version().map(str::to_owned),
                        tx,
                    ),
                    |result| Message::SetupFinished(result.map(Box::new)),
                )
            }
            Message::SetupFinished(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(status) => {
                        let ready = status.is_ready();
                        self.setup = Some(*status);
                        if ready && self.wizard.is_none() {
                            self.screen = Screen::Dashboard;
                        }
                        // Provisioning changes what the disk holds, so the
                        // action button has to be re-resolved.
                        self.reload(self.selected.code.clone())
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::PinBattleNetProton(choice) => {
                self.config.battle_net_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }
            Message::PinDefaultProton(choice) => {
                self.config.default_proton = ProtonPin::from(choice.0);
                self.save_config();
                Task::none()
            }
            Message::PinProductProton(code, choice) => {
                if let Some(product) = self.config.product_mut(&code) {
                    product.profile.proton_ge_version = choice.0;
                }
                self.save_config();
                Task::none()
            }
            Message::CheckProton => {
                // Runs with no build installed too: the release list is what
                // fills the install picker, and needing a build before you can
                // find one to install is the wrong way round.
                let installed = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.proton.as_ref())
                    .map(|p| p.version.clone());
                self.proton_notice = Some("checking…".to_owned());
                Task::perform(
                    provision::check_proton(self.http.clone(), installed),
                    Message::ProtonChecked,
                )
            }
            Message::ProtonChecked(result) => {
                match result {
                    Ok(releases) => {
                        self.proton_notice = Some(match &releases.update {
                            // Notify-only by policy: nothing updates itself.
                            Some(version) => format!("{version} is available"),
                            None => match self.setup.as_ref().and_then(|s| s.proton.as_ref()) {
                                Some(_) => "up to date".to_owned(),
                                None => "nothing installed yet".to_owned(),
                            },
                        });
                        self.proton_available = releases.available;
                    }
                    Err(e) => self.proton_notice = Some(format!("check failed: {e}")),
                }
                Task::none()
            }
            Message::ChooseProtonVersion(choice) => {
                self.proton_choice = choice.0;
                Task::none()
            }
            Message::InstallProton => {
                let Some(version) = self.proton_choice.clone() else {
                    return Ok(Task::none());
                };
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;
                // Stays on the Proton page, which is the point of the
                // progress now being drawn in the Builds card. It used to
                // switch to Settings > Setup because that was the only place
                // a bar existed, and pressing Install and landing somewhere
                // else reads as a misclick.
                //
                // `install_build` rather than `provision::run` with a pinned
                // version: that one goes on to the prefix and Battle.net
                // afterwards, so on a machine that is not set up yet, adding
                // a Proton build would quietly provision the whole thing.
                Task::perform(provision::install_build(version, tx), Message::ProtonInstalled)
            }
            Message::ProtonInstalled(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(()) => {
                        self.proton_choice = None;
                        self.proton_notice = Some("installed".to_owned());
                        // What is installed just changed, so re-probe rather
                        // than editing the list we happen to be holding.
                        Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }
            Message::RemoveProton(version) => {
                let Some(build) = self
                    .setup
                    .as_ref()
                    .and_then(|s| s.installed.iter().find(|b| b.version == version))
                    .cloned()
                else {
                    return Ok(Task::none());
                };
                self.proton_notice = Some(format!("removing {version}…"));
                Task::perform(
                    provision::remove_proton(self.config.clone(), build),
                    Message::ProtonRemoved,
                )
            }
            Message::ProtonRemoved(result) => {
                match result {
                    Ok(note) => {
                        self.proton_notice = Some(note);
                        // What is installed just changed, so re-probe rather
                        // than editing the list we happen to be holding.
                        return Ok(Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        ));
                    }
                    Err(e) => self.proton_notice = Some(e),
                }
                Task::none()
            }
            Message::ProtonSizes(sizes) => {
                self.proton_sizes = sizes.into_iter().collect();
                Task::none()
            }
            Message::OpenConfigFolder => {
                if let Some(dirs) = &self.dirs {
                    if let Err(e) = tasks::open_path(dirs.config_dir().to_path_buf()) {
                        self.status = e;
                    }
                }
                Task::none()
            }
            Message::OpenLibrary => {
                let root = self.layout.root().to_path_buf();
                let dir = match root.is_dir() {
                    true => root,
                    // Not created yet: show where it would go.
                    false => root
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or(root),
                };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e.into());
                }
                Task::none()
            }
            Message::LocationChanged(value) => {
                self.location_input = value;
                Task::none()
            }
            Message::UseLocation => {
                match self.relocate() {
                    Ok(()) => Task::batch([
                        Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        ),
                        self.measure_storage(),
                        self.reload(self.selected.code.clone()),
                    ]),
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }

            other => return Err(other),
        })
    }
}
