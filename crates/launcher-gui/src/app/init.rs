//! Starting the app: what is read from disk, and what the first frame starts.

use crate::*;

impl Lightforge {
    /// The state the first frame draws: the config, the layout it resolves
    /// to, and everything built from those. Nothing is started here.
    pub(crate) fn load_state() -> Self {
        let dirs = AppDirs::discover().ok();
        // A missing file is a first run and gets defaults. A file that exists
        // but will not load is someone's settings with a problem in them: the
        // app stops on it rather than quietly running on defaults, which
        // point at the default library, and then saving those over it.
        let loaded = dirs.as_ref().map(|d| Config::load_or_default(&d.config_file()));
        let config_error = match (&loaded, dirs.as_ref()) {
            (Some(Err(e)), Some(dirs)) => {
                tracing::error!("{} will not load: {}", dirs.config_file().display(), chain(e));
                Some(broken_config(e, &dirs.config_file()))
            }
            _ => None,
        };
        let mut config = match loaded {
            Some(Ok(config)) => config,
            _ => Config::with_default_products(),
        };
        // A config written before multi-drive storage has no libraries in it.
        // Seeded here rather than lazily, because every path below this line
        // resolves through one. Deliberately *not* saved: starting the app is
        // not a reason to write to someone's config, and the seed does not
        // need writing to be stable; see `Config::ensure_libraries`. It goes
        // to disk with the first change the user makes.
        let _ = config.ensure_libraries();
        let config = config;
        // Honours `blizzard_root`, the same way the CLI does.
        let layout = config.layout().unwrap_or_else(|e| {
            tracing::error!("could not resolve the Blizzard root, using the current directory: {}", chain(&e));
            Layout::with_root(".")
        });
        tracing::info!(
            "config loaded: root {}, {} product(s), {} library(ies)",
            layout.root().display(),
            config.products.len(),
            layout.libraries().all().len(),
        );
        let bnet_icon = layout.battle_net_icon().and_then(|path| art::try_mark(&path));

        let choices: Vec<Choice> = config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        // The retired beta never opens the app: the live build of its line
        // does, or failing that the next game in the list.
        let first = choices.first().filter(|c| c.code != ProductCode::ClassicBeta).or_else(|| {
            choices
                .iter()
                .find(|c| c.code == ProductCode::Forever)
                .or_else(|| choices.iter().find(|c| c.code != ProductCode::ClassicBeta))
                .or_else(|| choices.first())
        });
        let selected = first.cloned().unwrap_or(Choice {
            code: ProductCode::Retail,
            label: "World of Warcraft".to_owned(),
        });

        // Without XDG directories there is no cache to theme into, and the
        // dashboard renders its own plain look rather than failing to start.
        let pipeline = dirs
            .as_ref()
            .map(|dirs| Pipeline::new(config.asset_origin(), Store::new(dirs.asset_cache_dir())));

        let limit_input = config.download_limit_mbps.map(|v| v.to_string()).unwrap_or_default();
        Self {
            layout,
            dirs,
            config,
            http: Http::new().ok(),
            pipeline,
            manifest: None,
            icons: HashMap::new(),
            last_branch: HashMap::new(),
            catalog: Catalog::builtin(),
            choices,
            selected: selected.clone(),
            local: Local::default(),
            remote: None,
            branding: None,
            queue: Vec::new(),
            manager_open: false,
            browse_open: false,
            strip_open: false,
            tile_menu: None,
            tile_hover: None,
            tile_drag: None,
            focus: None,
            focus_return: None,
            focus_resume: None,
            focus_pressed_at: None,
            focus_entering: 0,
            picker_open: None,
            focus_into: None,
            window_focused: true,
            nav_last: None,
            steamos: lightforge_core::runtime::OsRelease::read().is_steamos(),
            steam_shortcut: None,
            app_menu_open: false,
            visited: Vec::new(),
            forward: Vec::new(),
            hints: false,
            bnet_icon,
            gear_open: false,
            scan_rx: None,
            scan_sample: lightforge_core::tact::verify::Progress::default(),
            scanning: false,
            deep_cancel: None,
            owed_repair: None,
            scan_for: None,
            repairing: None,
            installed_versions: HashMap::new(),
            // From the start: a client left running from before, or the
            // first one after an install that predates Lightforge, writes
            // the registry with nobody having pressed anything here.
            registry_watch: Some(RegistryWatch::new()),
            known_codes: Vec::new(),
            resume_after_play: Vec::new(),
            saved_queue: transfers::Queue::default(),
            config_error,
            queue_frozen: false,
            winding_down: None,
            downloads: SharedDownloads::default(),
            busy: false,
            status: "checking…".to_owned(),
            error: None,
            closing: None,
            screen: Screen::Dashboard,
            setup: None,
            wizard: None,
            patch_notes: None,
            splash: None,
            ui_scale: 1.0,
            ui_scale_gen: 0,
            setup_rx: None,
            setup_sample: provision::Progress::default(),
            setup_running: false,
            limit_input,
            bnet_limit_input: String::new(),
            proton_notice: None,
            tested_proton_asked: false,
            proton_available: Vec::new(),
            steam_proton_asked: HashSet::new(),
            proton_installer: None,
            proton_sizes: HashMap::new(),
            storage: None,
            storage_measuring: false,
            cache_notice: None,
            location_input: String::new(),
            install_dialog: None,
            uninstall_dialog: None,
            cancel_dialog: None,
            update_notice: None,
            origin: lightforge_core::upgrade::Origin::detect(),
            exe: std::env::current_exe().ok(),
            upgrade: None,
            launcher_check: LauncherCheck::default(),
            delete_after_cancel: Vec::new(),
            discarding: None,
            launching: None,
            game_up: false,
            stopping: false,
            stop_dialog: None,
            version_dialog: None,
            modify_dialog: None,
            launch_when_quiet: None,
            report_dialog: None,
            system: None,
            history: std::collections::VecDeque::new(),
            last_report: None,
            downgrade_dialog: None,
            downgrade_ok: HashSet::new(),
            partitions: Vec::new(),
            drive_notice: None,
            unused: HashMap::new(),
            library_empty: false,
            drive_states: HashMap::new(),
            tools: HostTools::detect(),
            env_editor: iced::widget::text_editor::Content::new(),
            bnet_env_editor: iced::widget::text_editor::Content::new(),
            env_edits: 0,
            env_unsaved: false,
            addons: addonwork::Listing::default(),
            token_input: String::new(),
            addon_filter: String::new(),
            addon_remove_armed: None,
            addon_menu_open: None,
            version_picker: None,
            game_settings: None,
            addon_detail: None,
            families: lightforge_core::families::Families::default(),
            import_pinned: false,
            addon_auto_done: Vec::new(),
            addon_auto_running: false,
            addon_auto_note: None,
            launch_after_addons: None,
            addon_report_tab: AddonTab::Mine,
            search_seq: 0,
            export_token: String::new(),
            addon_rx: None,
            addon_sample: addonwork::Progress::default(),
            addons_busy: false,
            addon_report: None,
            addon_tab: AddonTab::Mine,
            settings_tab: SettingsTab::General,
            settings_jump: None,
            settings_jump_tries: 0,
            settings_anchor: None,
            settings_layout: SettingsLayout::default(),
            settings_viewport: None,
            search_input: String::new(),
            search: addonwork::SearchOutcome::default(),
            searching: false,
            updates: Vec::new(),
        }
    }

    /// What starts once the state is loaded: probes, restored downloads,
    /// the splash's artwork fetch, and the first run's wizard.
    pub(crate) fn startup_tasks(&mut self) -> Task<Message> {
        // Nothing starts on a config that did not load: no probes against a
        // library it may not describe, and above all no restored download.
        if self.config_error.is_some() {
            // The frame still goes on: the broken-config screen is prose
            // someone has to read, close and act on, and a window with no
            // way to close it is not the thing to hand them.
            return iced::window::oldest().and_then(iced::window::toggle_decorations);
        }
        self.load_tweak_inputs();
        // Before anything resolves a path: a drive that came back at a
        // different mount point is found again here, so a game on it reads as
        // installed rather than as missing. Cheap, one pass over
        // `/proc/self/mounts` and one marker file per absent library.
        self.refresh_drives();
        self.restore_queue();

        // What is provisioned decides which screen opens: a machine that has
        // never run the CLI cannot install or launch anything, and a dashboard
        // whose only working button is an error is worse than saying so.
        let probe = Task::perform(
            provision::status(self.layout.clone(), self.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        );
        let installed = Task::perform(
            tasks::installed_versions(self.layout.clone()),
            Message::InstalledVersions,
        );
        let codes = Task::perform(
            tasks::known_codes(self.layout.clone(), self.config.prefix_user.clone()),
            Message::KnownCodes,
        );
        let bnet_limit = Task::perform(
            tasks::battle_net_limit(self.layout.clone(), self.config.prefix_user.clone()),
            Message::BattleNetLimitRead,
        );
        // What was downloading when the app closed carries on, as it does in
        // Battle.net.
        let resume = self.pump();
        // Addons that update themselves do it now, so they are current by
        // the time anyone presses Play. Quiet, and nothing waits for it
        // except a launch; see `preflight`.
        let addons = Task::done(Message::AutoUpdateAddons);
        // Which tile each product belongs to, for the games Battle.net runs.
        let families = Task::perform(
            tasks::game_families(self.layout.clone(), self.config.prefix_user.clone(), self.config.region),
            |families| Message::FamiliesRead(Box::new(families)),
        );
        // The first run, exactly once: the flag is written when the wizard is
        // finished or dismissed, so a machine that has seen it never sees it
        // again by itself. Nothing is written here, starting the app is not a
        // reason to touch someone's config.
        if !self.config.setup_complete {
            self.wizard = Some(wizard::Wizard::default());
        }
        // Every tracked product's art into the cache before the launcher is
        // worth looking at, so the first visit to a panel is not two seconds
        // of empty card. The splash *is* this fetch, made visible, and the
        // wizard's first slide reads its count off the same run rather than
        // starting a second one.
        let artwork = self.open_splash();
        Task::batch([
            self.reload(self.selected.code.clone()),
            addons,
            families,
            probe,
            installed,
            codes,
            bnet_limit,
            resume,
            artwork,
            iced::system::information().map(|info| Message::SystemInfo(Box::new(info))),
        ])
    }
}
