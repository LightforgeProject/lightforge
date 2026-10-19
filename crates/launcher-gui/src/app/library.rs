//! Drives, the layout they make, configuration and tweaks.

use crate::*;

impl Lightforge {
    /// The name of this product's drive, when it is one that has gone away.
    pub(crate) fn offline_drive(&self, code: &ProductCode) -> Option<String> {
        let library = self.config.library_of(code)?;
        match self.drive_states.get(&library.id) {
            Some(library::State::Missing) => Some(library.label.clone()),
            _ => None,
        }
    }

    /// Re-read every library's marker, and whether anything is installed yet.
    /// One small file per drive, plus two directory reads.
    pub(crate) fn check_drives(&mut self) {
        self.drive_states = self
            .config
            .libraries
            .iter()
            .map(|library| (library.id.clone(), library.state()))
            .collect();
        self.library_empty = storage::library_is_empty(&self.layout);
        // Seeded here rather than when Storage is first measured: the field
        // lives on Setup now, which is the page a first run opens on, before
        // anything has been measured.
        if self.location_input.is_empty() {
            self.location_input = self.layout.root().display().to_string();
        }
    }

    /// This product's performance settings.
    pub(crate) fn tweaks(&self) -> Tweaks {
        self.tweaks_of(Scope::Product)
    }

    /// One scope's performance settings.
    pub(crate) fn tweaks_of(&self, scope: Scope) -> Tweaks {
        match scope {
            Scope::Product => self
                .config
                .product(&self.selected.code)
                .map(|p| p.profile.tweaks.clone())
                .unwrap_or_default(),
            Scope::BattleNet => self.config.battle_net_tweaks.clone(),
        }
    }

    /// Change them and persist, which is what every switch on a panel does.
    pub(crate) fn edit_tweaks(&mut self, scope: Scope, edit: impl FnOnce(&mut Tweaks)) {
        match scope {
            Scope::Product => {
                if let Some(product) = self.config.product_mut(&self.selected.code) {
                    edit(&mut product.profile.tweaks);
                }
            }
            Scope::BattleNet => edit(&mut self.config.battle_net_tweaks),
        }
        self.save_config();
    }

    /// Refill the numeric fields from the selected product's settings.
    pub(crate) fn load_tweak_inputs(&mut self) {
        let tweaks = self.tweaks();
        self.fps_input = tweaks.fps_cap.map(|v| v.to_string()).unwrap_or_default();
        self.fps_bk_input =
            tweaks.fps_cap_background.map(|v| v.to_string()).unwrap_or_default();
        self.monitor_input = tweaks.wayland_monitor.clone().unwrap_or_default();
        self.bnet_monitor_input =
            self.config.battle_net_tweaks.wayland_monitor.clone().unwrap_or_default();
        self.env_key_input.clear();
        self.env_value_input.clear();
    }

    /// Rebuild the layout from the config, after a library is added, removed,
    /// made main, or assigned to a product.
    /// Re-read what is provisioned, off the UI thread.
    pub(crate) fn setup_status(&self) -> Task<Message> {
        Task::perform(
            provision::status(self.layout.clone(), self.config.clone()),
            |status| Message::SetupStatus(status.map(Box::new)),
        )
    }

    /// Bring the header's list back in line with the configured products.
    ///
    /// Adoption can add a product nobody chose, a found install of Classic Era
    /// is a game the user has, whether or not they would have thought to add
    /// the tile, so the two lists are reconciled rather than appended to.
    pub(crate) fn sync_choices(&mut self) {
        self.choices = self
            .config
            .products
            .iter()
            .map(|p| Choice { code: p.code.clone(), label: p.display_name.clone() })
            .collect();
        // A game opened from Add a game is shown without being in the band;
        // it stays shown until something else is picked.
        let previewing = self.catalog.get(&self.selected.code).is_some();
        if !previewing && !self.choices.iter().any(|c| c.code == self.selected.code) {
            if let Some(first) = self.choices.first().cloned() {
                self.selected = first;
            }
        }
    }

    /// Put the game on screen into the band, if it was only being looked at.
    ///
    /// Called by every action that starts something for it: a download or a
    /// game running for a product the band does not show would have nowhere
    /// to report to, and nowhere to be found again.
    pub(crate) fn track_selected(&mut self) {
        if self.choices.iter().any(|c| c.code == self.selected.code) {
            return;
        }
        let choice = self.selected.clone();
        if self.config.product(&choice.code).is_none() {
            self.config.products.push(Product::new(choice.code.clone(), choice.label.clone()));
            self.save_config();
        }
        self.choices.push(choice);
    }

    pub(crate) fn rebuild_layout(&mut self) {
        if let Ok(layout) = self.config.layout() {
            self.layout = layout;
        }
    }

    /// Re-read the mount table: which partitions could be added, and whether a
    /// library that was absent has come back somewhere else.
    ///
    /// The relocation is the half worth having. A drive that was mounted at
    /// `/run/media/user/games` and comes back at `/run/media/user/games1` is
    /// still the same volume, and its marker says so, so the recorded path is
    /// corrected rather than the library being declared lost.
    pub(crate) fn refresh_drives(&mut self) {
        let mut moved = false;
        for library in &mut self.config.libraries {
            if library.state() != library::State::Missing {
                continue;
            }
            if let Some(found) = library::relocate(library) {
                library.games = found;
                library.probe();
                moved = true;
            }
        }
        if moved {
            self.save_config();
            self.rebuild_layout();
        }
        self.partitions = library::partitions(&self.config.libraries);
        self.check_drives();
    }

    /// Add the partition at `index` as a library.
    pub(crate) fn add_drive(&mut self, index: usize) -> Result<(), String> {
        let Some(partition) = self.partitions.get(index).cloned() else {
            return Err("that drive is no longer there".to_owned());
        };
        let (games_letter, wow_letter) = library::allocate_letters(&self.config.libraries)
            .ok_or_else(|| library::LibraryError::NoLettersLeft.to_string())?;
        let mut library = library::Library::on_partition(
            &partition.point,
            partition.label.clone(),
            partition.uuid.clone(),
        );
        library.games_letter = games_letter;
        library.wow_letter = wow_letter;
        library.claim().map_err(|e| chain(&e))?;
        if self.config.libraries.iter().any(|l| l.id == library.id) {
            return Err(format!("{} is already a Lightforge drive", partition.label));
        }
        self.config.libraries.push(library);
        self.save_config();
        self.rebuild_layout();
        if lightforge_core::prefix::PrefixStatus::inspect(&self.layout).initialized {
            lightforge_core::prefix::link_libraries(&self.layout).map_err(|e| chain(&e))?;
        }
        Ok(())
    }

    /// Take a drive off Lightforge's list. Nothing on it is deleted, the
    /// games are still there, and re-adding the drive picks them back up by
    /// its marker.
    pub(crate) fn remove_drive(&mut self, id: &str) -> Result<(), String> {
        let Some(library) = self.config.libraries.iter().find(|l| l.id == id).cloned() else {
            return Ok(());
        };
        if library.is_default && self.config.libraries.len() > 1 {
            return Err("make another drive the default one first".to_owned());
        }
        if self.config.libraries.len() == 1 {
            return Err("this is the only drive Lightforge has".to_owned());
        }
        if storage::holds_wow(&self.layout, &library) {
            return Err(format!(
                "{} holds a World of Warcraft install — uninstall it first",
                library.label
            ));
        }
        // The letters go with it, and the prefix keeps the links until the
        // next provision tidies them: a dangling `dosdevices` entry is inert,
        // and removing one while the client is up is not worth the race.
        self.config.libraries.retain(|l| l.id != id);
        for product in &mut self.config.products {
            if product.profile.library.as_deref() == Some(id) {
                product.profile.library = None;
            }
        }
        self.save_config();
        self.rebuild_layout();
        Ok(())
    }

    /// Work out each library's unused game data, off the UI thread. A
    /// library being freed right now is left to finish.
    pub(crate) fn check_unused(&mut self) -> Task<Message> {
        let mut tasks = Vec::new();
        for library in &self.config.libraries {
            let install = self.layout.wow_install_in(library);
            if matches!(self.unused.get(&library.id), Some(UnusedData::Freeing { .. })) {
                continue;
            }
            if !install.join(".build.info").is_file() {
                self.unused.remove(&library.id);
                continue;
            }
            self.unused.insert(library.id.clone(), UnusedData::Checking);
            let id = library.id.clone();
            tasks.push(Task::perform(tasks::unused_plan(self.http.clone(), install), move |result| {
                Message::UnusedPlanned(id.clone(), result)
            }));
        }
        Task::batch(tasks)
    }

    /// Re-measure what Settings > Storage shows, off the UI thread.
    pub(crate) fn measure_storage(&mut self) -> Task<Message> {
        let Some(dirs) = self.dirs.clone() else {
            return Task::none();
        };
        self.storage_measuring = true;
        Task::perform(tasks::storage(self.layout.clone(), dirs), |report| {
            Message::StorageMeasured(Box::new(report))
        })
    }

    /// Point the library at the typed location.
    ///
    /// Only while nothing is installed in the current one: then it is a
    /// config change. With games or a prefix in it, it would be a data
    /// migration, re-pointing the prefix's game symlink, moving 100+ GB,
    /// and leaving the old library behind unannounced is not an answer.
    pub(crate) fn relocate(&mut self) -> Result<(), String> {
        let path = parse_path(&self.location_input)?;
        if path == self.layout.root() {
            return Ok(());
        }
        let downloading = self.queue.iter().any(|d| matches!(d.stage, Stage::Running { .. }));
        if self.setup_running || downloading {
            return Err("wait for setup and downloads to finish first".to_owned());
        }
        if !storage::library_is_empty(&self.layout) {
            return Err("the current library has games or a prefix in it; moving an \
                        installed library is not supported yet"
                .to_owned());
        }

        let default = Layout::default_for_user().ok();
        self.config.blizzard_root = match default.as_ref().map(Layout::root) {
            Some(root) if root == path => None,
            _ => Some(path.clone()),
        };
        // The first library moves with the root when it was under it, which
        // it is unless the user pointed `games_dir` somewhere else. Nothing is
        // installed (checked above), so this is a path change, not a move.
        let old = self.layout.games_root();
        if let Some(main) = self.config.libraries.iter_mut().find(|l| l.is_default) {
            if main.games == old {
                main.games = path.join("games");
                main.claimed = false;
                main.probe();
            }
        }
        self.save_config();
        self.rebuild_layout();
        self.check_drives();
        self.location_input = path.display().to_string();
        self.error = None;
        Ok(())
    }

    pub(crate) fn save_config(&mut self) {
        // Never over a file that did not load: it is the user's to fix.
        if self.config_error.is_some() {
            return;
        }
        let Some(dirs) = self.dirs.as_ref() else {
            self.error = Some("no config directory available".into());
            return;
        };
        if let Err(e) = self.config.save(&dirs.config_file()) {
            self.error = Some(Problem::of(&e));
        }
    }
}
