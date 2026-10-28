//! Setup, the 32-bit runtime, Proton builds and pins, and where things install.

use crate::*;

impl Lightforge {
    pub(crate) fn update_setup(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_setup_run(message)
            .or_else(|m| self.update_setup_choices(m))
            .or_else(|m| self.update_proton_builds(m))
    }

    /// What setup found, and running it.
    fn update_setup_run(&mut self, message: Message) -> Result<Task<Message>, Message> {
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
                // Open on Settings' setup section if nothing can launch yet.
                if let Some(status) = &status {
                    // Not under the first run, which has its own setup step.
                    if !status.is_ready()
                        && self.screen == Screen::Dashboard
                        && !self.setup_running
                        && self.wizard.is_none()
                    {
                        self.screen = Screen::Settings;
                        self.settings_jump = Some(SettingsTab::Setup);
                        // Behind the splash, the jump waits for it.
                        if self.splash.is_none() {
                            opened = self.arrived();
                        }
                    }
                }
                // Setup is what installs Battle.net's mark.
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
            other => return Err(other),
        })
    }

    /// The Proton pins and the install location.
    fn update_setup_choices(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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

    /// Proton builds: checking for new ones, installing and removing them.
    fn update_proton_builds(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::CheckProton => {
                // With no build installed too: it fills the install picker.
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

            Message::OpenProtonInstaller => {
                self.close_panels();
                self.proton_installer = Some(ProtonCategory::GloriousEggroll);
                match self.proton_available.is_empty() {
                    true => Task::done(Message::CheckProton),
                    false => Task::none(),
                }
            }

            Message::CloseProtonInstaller => {
                self.proton_installer = None;
                Task::none()
            }

            Message::ProtonInstallerTab(tab) => {
                self.proton_installer = Some(tab);
                Task::none()
            }

            Message::InstallSteamProton(app_id) => {
                let Some(tool) = proton::STEAM_TOOLS.iter().find(|t| t.app_id == app_id) else {
                    return Ok(Task::none());
                };
                self.proton_notice = Some(match tasks::open_url(&tool.install_url()) {
                    Ok(()) => format!(
                        "Steam is asked to install {}. It appears in the list once Steam is done.",
                        tool.name
                    ),
                    Err(e) => format!("Could not reach Steam: {e}"),
                });
                self.steam_proton_asked.insert(app_id);
                Task::none()
            }

            Message::InstallProton(version) => {
                if self.setup_running {
                    return Ok(Task::none());
                }
                let (tx, rx) = tokio::sync::watch::channel(provision::Progress::default());
                self.setup_rx = Some(rx);
                self.setup_sample = provision::Progress::default();
                self.setup_running = true;
                self.error = None;
                // Stays on the Proton page. `install_build`, not `provision::run`,
                // which would go on to build the prefix and install Battle.net.
                Task::perform(provision::install_build(version, tx), Message::ProtonInstalled)
            }

            Message::ProtonInstalled(result) => {
                self.setup_running = false;
                self.setup_rx = None;
                match result {
                    Ok(()) => {
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
            other => return Err(other),
        })
    }
}
