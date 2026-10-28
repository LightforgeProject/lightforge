//! Which game and branch are on screen, and the catalog they are picked from.

use crate::*;

impl Lightforge {
    /// Rebuild the catalog from whatever manifest is in hand.
    pub(crate) fn refresh_catalog(&mut self) {
        let mut catalog = match &self.manifest {
            Some(manifest) => manifest.catalog(),
            None => Catalog::builtin(),
        };
        catalog.reconcile(&self.known_codes);
        self.catalog = catalog;
    }

    /// Every branch of the selected line, configured or not.
    ///
    /// `None` when the line has one branch.
    pub(crate) fn branch_choices(&self) -> Option<(Vec<BranchChoice>, BranchChoice)> {
        let line = self.catalog.line_of(&self.selected.code);
        let choices: Vec<BranchChoice> = self
            .catalog
            .branches(&line)
            .into_iter()
            // The retired beta is not offered. It is only listed while it is
            // the selected branch, so nobody is left on one the picker hides.
            .filter(|entry| {
                entry.code != ProductCode::ClassicBeta || entry.code == self.selected.code
            })
            .map(|entry| BranchChoice {
                configured: self.choices.iter().any(|c| c.code == entry.code),
                branch: entry.branch,
                name: self
                    .families
                    // The UI is English; the install locale is for the game's files.
                    .version_name(entry.code.as_str(), "default")
                    .map(str::to_owned),
                code: entry.code.clone(),
                label: entry.label.clone(),
            })
            .collect();

        if choices.len() < 2 {
            return None;
        }
        let current = choices.iter().find(|c| c.code == self.selected.code)?.clone();
        Some((choices, current))
    }

    /// Switch to `line`, keeping whichever of its branches was last in use.
    ///
    /// Falls back to the line's live build, then to whatever of it *is*
    /// configured: selecting a tile must always land somewhere.
    pub(crate) fn select_line(&mut self, line: &ProductLine) -> Task<Message> {
        let remembered = self.last_branch.get(line).cloned();
        let configured = |code: &ProductCode| self.choices.iter().find(|c| &c.code == code).cloned();

        let choice = remembered
            .as_ref()
            .and_then(configured)
            .or_else(|| configured(&line.head()))
            .or_else(|| {
                self.choices
                    .iter()
                    .find(|c| &self.catalog.line_of(&c.code) == line)
                    .cloned()
            });

        match choice {
            Some(choice) => self.select(choice),
            None => Task::none(),
        }
    }

    /// Switch to one product, remembering it as its line's current branch.
    pub(crate) fn select(&mut self, choice: Choice) -> Task<Message> {
        if choice.code == self.selected.code {
            return Task::none();
        }
        self.last_branch.insert(self.catalog.line_of(&choice.code), choice.code.clone());
        self.selected = choice.clone();
        self.local = Local::default();
        self.remote = None;
        self.branding = None;
        self.status = "checking…".to_owned();
        // These are per-product, so they follow the selection instead of
        // carrying Retail's frame cap over to Classic's panel.
        self.load_tweak_inputs();
        self.reset_addon_screen();
        Task::batch([
            self.reload(choice.code),
            Task::done(Message::AutoUpdateAddons),
            // The popular list is per flavor, so it follows the selection for
            // the same reason the installed list does.
            match self.addon_tab == AddonTab::Browse {
                true => self.popular_if_unasked(),
                false => Task::none(),
            },
        ])
    }

    /// Load the popular list, unless a search is already in flight.
    ///
    /// A typed search's reply would be dropped anyway.
    pub(crate) fn popular_if_unasked(&mut self) -> Task<Message> {
        match self.searching {
            true => Task::none(),
            false => Task::done(Message::RunSearch),
        }
    }
}
