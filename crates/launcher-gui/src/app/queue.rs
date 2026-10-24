//! The download queue and the products it is working on.

use crate::*;

impl Lightforge {
    /// Re-read the disk, the version service, and the theme for one product.
    pub(crate) fn reload(&self, code: ProductCode) -> Task<Message> {
        Task::batch([
            Task::perform(
                tasks::refresh(
                    self.http.clone(),
                    self.layout.clone(),
                    code.clone(),
                    self.config.region,
                    self.kind_of(&code) == Kind::Managed,
                ),
                {
                    let code = code.clone();
                    move |r| Message::Refreshed(code.clone(), Box::new(r))
                },
            ),
            // Fires first and usually lands first: it reads the cache and
            // nothing else, so the window opens themed rather than snapping
            // to art once the revalidation below comes back.
            Task::perform(
                tasks::cached_theme(self.pipeline.clone(), self.product_codes(), code.clone()),
                {
                    let code = code.clone();
                    move |t| Message::CachedTheme(code.clone(), Box::new(t))
                },
            ),
            Task::perform(
                tasks::theme(
                    self.http.clone(),
                    self.pipeline.clone(),
                    self.manifest.clone(),
                    self.product_codes(),
                    code.clone(),
                ),
                {
                    let code = code.clone();
                    move |t| Message::Themed(code.clone(), Box::new(t))
                },
            ),
            Task::perform(addonwork::list(self.layout.clone(), code), |listing| {
                Message::AddonsListed(Box::new(listing))
            }),
        ])
    }

    /// A running scan's bar and caption, for under the status line.
    ///
    /// A scan otherwise says nothing for a minute or more, which reads as a
    /// hung launcher. A deep scan is measured in bytes: its files range from
    /// a few bytes to hundreds of megabytes, so a file count stalls and leaps.
    pub(crate) fn scan_progress<'a>(
        &self,
        left: iced::widget::Column<'a, Message>,
    ) -> iced::widget::Column<'a, Message> {
        let p = &self.scan_sample;
        if !self.scanning || p.total == 0 {
            return left;
        }
        let (done, what) = match p.bytes_total {
            0 => (
                p.checked as f32 / p.total as f32,
                format!("{} of {} files checked", p.checked, p.total),
            ),
            total => (
                p.bytes as f32 / total as f32,
                format!("{} checked", bytes_of(p.bytes, total)),
            ),
        };
        let caption = match p.damaged {
            0 => what,
            n => format!("{what} — {n} damaged so far"),
        };
        left.push(progress_bar(0.0..=1.0, done).girth(6)).push(muted(caption, theme::CAPTION))
    }

    /// Every configured product, in the order the bar draws them.
    /// Every product whose mark the UI may draw.
    ///
    /// Not just the configured ones: the browse panel lists the whole catalog
    /// and wants each row to wear the same mark its tile would, so a game you
    /// have not added yet is still recognisable at a glance. Marks are small,
    /// content-addressed and cached immutably, so the extra ones cost one
    /// fetch each, once, and nothing thereafter.
    pub(crate) fn product_codes(&self) -> Vec<ProductCode> {
        let mut codes: Vec<ProductCode> = self.choices.iter().map(|c| c.code.clone()).collect();
        for entry in self.catalog.entries() {
            if !codes.contains(&entry.code) {
                codes.push(entry.code.clone());
            }
        }
        codes
    }

    /// Progress against the whole product, not against this run.
    ///
    /// A resumed run plans only what is missing, so run-relative numbers read
    /// as "starting over" every time a download is paused and picked up again.
    /// Refresh only when the finished product is the one on screen.
    pub(crate) fn reload_if_showing(&mut self, code: &ProductCode) -> Task<Message> {
        match code == &self.selected.code {
            true => self.reload(code.clone()),
            false => Task::none(),
        }
    }

    pub(crate) fn download_for(&self, code: &ProductCode) -> Option<&Download> {
        self.queue.iter().find(|d| &d.code == code)
    }

    /// [`Self::maintenance`], or a download still writing.
    pub(crate) fn store_writer(&self) -> Option<&'static str> {
        match self.running().is_some() || self.winding_down.is_some() {
            true => Some("the download"),
            false => self.maintenance(),
        }
    }

    /// Whether addon work may start: not while the user's own runs, nor
    /// while the unattended pass is writing the same folder and ledger.
    pub(crate) fn addons_locked(&self) -> bool {
        self.addons_busy || self.addon_auto_running
    }

    /// Put the selected product in the queue, starting it if nothing is running.
    pub(crate) fn enqueue(&mut self) -> Task<Message> {
        self.enqueue_for(self.selected.code.clone())
    }

    /// Put `code` in the queue, starting it if nothing is running.
    pub(crate) fn enqueue_for(&mut self, code: ProductCode) -> Task<Message> {
        let label = self.label_of(&code);
        // The panel above should never offer this, but the download path is
        // the one that writes to disk, so it checks for itself.
        if self.kind_of(&code) == Kind::HandOff {
            self.error = Some(format!("{label} is installed through Battle.net").into());
            return Task::none();
        }
        // The panel holds this product when its drive is away, but the queue
        // is also restored from disk and resumed without anyone pressing
        // anything, so the check that actually protects the files is here.
        if let Some(drive) = self.offline_drive(&code) {
            self.error = Some(format!("{drive} is not mounted").into());
            return Task::none();
        }
        self.error = None;
        let showing = code == self.selected.code;

        // Already in the queue: a queued entry stays put, a running one is
        // left running, and a paused one is being resumed, as the same job,
        // so its bar continues. Taking a running entry out would orphan its
        // run, and the next one would start beside it on the same store.
        let mut job = Job::default();
        if let Some(existing) = self.queue.iter().position(|d| d.code == code) {
            if matches!(self.queue[existing].stage, Stage::Queued | Stage::Running { .. }) {
                return Task::none();
            }
            job = self.queue.remove(existing).resumed();
        }

        self.queue.push(Download {
            code,
            label,
            stage: Stage::Queued,
            sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
            rate: Rate::default(),
            job,
        });
        let started = self.pump();
        // Not started: waiting on another download, or on this one's own
        // paused run to finish winding down.
        let waiting = self.queue.iter().any(|d| {
            d.code == self.selected.code && matches!(d.stage, Stage::Queued)
        });
        if waiting && showing {
            self.status = "queued…".to_owned();
        }
        started
    }

    /// Do whatever was waiting for the store to be free: close the window,
    /// remove a cancelled download's files, or start the first queued
    /// transfer, in that order. Called whenever a writer finishes.
    ///
    /// One at a time on purpose: two downloads would split one connection and
    /// write into the same CASC store, making both slower and both ETAs
    /// fiction.
    pub(crate) fn pump(&mut self) -> Task<Message> {
        // A launch waiting for the downloads pre-flight paused to come to
        // rest goes first. Not on the whole line being free: with
        // pause-while-playing off, another product's download is meant to
        // keep running beside the game.
        if self.launch_when_quiet.is_some() && !self.launch_must_wait() {
            let what = self.launch_when_quiet.take().expect("checked above");
            return self.start_launch(what);
        }
        // A deep scan reads the store a transfer would write, and maintenance
        // rewrites it. Neither refuses the download, they hold it: the queue
        // picks up once they are done.
        if self.store_writer().is_some() || self.deep_cancel.is_some() {
            return Task::none();
        }
        if let Some(id) = self.closing.take() {
            return iced::window::close(id);
        }
        if !self.delete_after_cancel.is_empty() {
            let code = self.delete_after_cancel.remove(0);
            return self.remove_cancelled(code);
        }
        if self.launch_when_quiet.is_some() {
            return Task::none();
        }
        let Some(next) = self.queue.iter().position(|d| matches!(d.stage, Stage::Queued)) else {
            return Task::none();
        };

        let cancel = Cancel::new();
        let (tx, rx) = tokio::sync::watch::channel(TransferState::default());
        let code = self.queue[next].code.clone();

        self.downloads.register(code.clone(), cancel.clone());
        self.queue[next].stage = Stage::Running { rx, cancel: cancel.clone() };
        // Whatever the line said before, "paused", most often, is over.
        if code == self.selected.code {
            self.status = "downloading…".to_owned();
        }
        self.queue[next].rate = Rate::default();
        self.queue[next].sample =
            TransferState { stage: "preparing".to_owned(), ..TransferState::default() };

        Task::perform(
            tasks::download(
                self.layout.clone(),
                code.clone(),
                self.config.region,
                self.config.locale_for(&code).to_owned(),
                self.config.download_limit_bytes(),
                self.opted_optional(&code),
                cancel,
                tx,
            ),
            move |result| {
                let mapped = result.map(|summary| Complete {
                    complete: summary.complete,
                    cancelled: summary.outcome.cancelled,
                    files: summary.outcome.files_stored,
                    version: summary.version,
                });
                Message::Downloaded(code.clone(), mapped)
            },
        )
    }

    /// Stop one product's transfer, keeping what it reached.
    pub(crate) fn pause(&mut self, code: &ProductCode) {
        let Some(entry) = self.queue.iter_mut().find(|d| &d.code == code) else {
            return;
        };
        match &entry.stage {
            Stage::Running { cancel, .. } => {
                cancel.cancel();
                // Shown as paused immediately, the click is answered now,
                // not when the run finishes winding down.
                entry.stage = Stage::Paused(job_progress(entry.job, &entry.sample, 0));
                self.winding_down = Some(code.clone());
                self.status = "pausing…".to_owned();
            }
            // Never started, so there is nothing to wind down.
            Stage::Queued => entry.stage = Stage::Paused(state::Progress::default()),
            Stage::Paused(_) => {}
        }
    }

    /// Stop a transfer and take it out of the queue, keeping what it reached.
    ///
    /// The bytes stay in the store either way: this is the half of giving up
    /// that is always safe, and deleting them is a separate decision the
    /// dialog makes.
    pub(crate) fn drop_transfer(&mut self, code: &ProductCode) {
        self.pause(code);
        self.queue.retain(|d| &d.code != code);
        self.downloads.finished(code);
        if self.queue.is_empty() {
            self.manager_open = false;
        }
    }

    /// Delete what a given-up first install wrote.
    ///
    /// Not an uninstall: a WoW install holds one store for every flavour in
    /// it, so dropping the folder is only right when this was the only thing
    /// in there. Otherwise the entries this build wants and no remaining
    /// product wants are the ones to go, which is what
    /// [`tasks::discard_download`] works out.
    pub(crate) fn remove_cancelled(&mut self, code: ProductCode) -> Task<Message> {
        self.discarding = Some(code.clone());
        Task::perform(
            tasks::discard_download(
                self.http.clone(),
                self.layout.clone(),
                code.clone(),
                self.config.region,
            ),
            move |result| Message::CancelCleaned(code.clone(), result),
        )
    }

    /// Record what a build offers, if it has changed.
    ///
    /// Saved rather than held in memory: the list is the only thing standing
    /// between the settings panel and a hardcoded guess, and a guess is what
    /// put a 37 GiB texture pack in a plan nobody asked for.
    /// The optional downloads a product is opted into, limited to the ones
    /// Lightforge offers; see [`OFFERED_OPTIONAL`]. An opt-in to anything
    /// else, from before that list existed, is ignored rather than fetched
    /// behind a switch the user can no longer see.
    pub(crate) fn opted_optional(&self, code: &ProductCode) -> Vec<String> {
        offered_only(
            self.config.product(code).map(|p| p.profile.optional_content.as_slice()).unwrap_or(&[]),
        )
    }

    pub(crate) fn remember_optional(&mut self, code: &ProductCode, offered: Vec<String>) {
        let changed = self
            .config
            .product(code)
            .is_some_and(|p| p.profile.optional_available != offered);
        if !changed {
            return;
        }
        if let Some(product) = self.config.product_mut(code) {
            product.profile.optional_available = offered;
        }
        self.save_config();
    }

    /// Persist the config, surfacing a write failure rather than swallowing it.
    /// The queue as the next start should find it.
    pub(crate) fn queue_snapshot(&self) -> transfers::Queue {
        // Paused only because a game is running: owed a restart, and one the
        // next start should give it rather than leave it waiting.
        let owed = {
            let mut owed = self.downloads.pending();
            owed.extend(self.resume_after_play.iter().cloned());
            owed
        };
        let entries = self
            .queue
            .iter()
            .map(|d| {
                let (standing, reached) = match &d.stage {
                    _ if owed.contains(&d.code) => (Standing::Running, 0),
                    Stage::Running { .. } => (Standing::Running, 0),
                    Stage::Queued => (Standing::Queued, d.job.reached),
                    Stage::Paused(p) => (Standing::Paused, p.downloaded_bytes),
                };
                Saved { code: d.code.clone(), standing, job_bytes: d.job.total, reached }
            })
            .collect();
        transfers::Queue { entries }
    }

    /// Write the queue if it changed. Called after every message, which is
    /// what makes a crash or a kill keep it too, not only a clean close.
    pub(crate) fn persist_queue(&mut self) {
        if self.queue_frozen || self.config_error.is_some() {
            return;
        }
        let snapshot = self.queue_snapshot();
        if snapshot == self.saved_queue {
            return;
        }
        if let Some(dirs) = &self.dirs {
            if let Err(e) = snapshot.save(&dirs.queue_file()) {
                trace::mark(format_args!("saving the queue failed: {e}"));
                return;
            }
        }
        self.saved_queue = snapshot;
    }

    /// Put back the queue the last run left, for products still configured.
    pub(crate) fn restore_queue(&mut self) {
        let Some(dirs) = &self.dirs else {
            return;
        };
        let saved = match transfers::Queue::load(&dirs.queue_file()) {
            Ok(saved) => saved,
            Err(e) => {
                trace::mark(format_args!("ignoring the saved queue: {e}"));
                return;
            }
        };
        for entry in &saved.entries {
            let Some(choice) = self.choices.iter().find(|c| c.code == entry.code) else {
                continue;
            };
            let job = Job { total: entry.job_bytes, reached: entry.reached };
            let stage = match entry.standing {
                Standing::Running | Standing::Queued => Stage::Queued,
                Standing::Paused => Stage::Paused(state::Progress {
                    downloaded_bytes: entry.reached,
                    total_bytes: entry.job_bytes.unwrap_or(0),
                    bytes_per_second: 0,
                }),
            };
            self.queue.push(Download {
                code: entry.code.clone(),
                label: choice.label.clone(),
                stage,
                sample: TransferState { stage: "queued".to_owned(), ..TransferState::default() },
                rate: Rate::default(),
                job,
            });
        }
        self.saved_queue = self.queue_snapshot();
    }

    /// Opt one product in or out of one optional download.
    pub(crate) fn set_optional(&mut self, code: &ProductCode, name: &str, on: bool) {
        if let Some(product) = self.config.product_mut(code) {
            let list = &mut product.profile.optional_content;
            list.retain(|n| n != name);
            if on {
                list.push(name.to_owned());
                list.sort();
            }
        }
        self.save_config();
    }

    /// Whether a download, running or waiting, writes into `install`. What it
    /// has fetched so far belongs to no version yet, so freeing unused data
    /// would throw it away.
    pub(crate) fn downloading_into(&self, install: &Path) -> bool {
        self.queue.iter().any(|d| self.layout.wow_install(&d.code) == install)
    }
}
