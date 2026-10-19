//! Drives, space, uninstalling, and checking or repairing a store.

use crate::*;

impl Lightforge {
    pub(crate) fn update_storage(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::StorageMeasured(report) => {
                self.storage_measuring = false;
                self.storage = Some(*report);
                Task::none()
            }
            Message::RefreshDrives => {
                self.refresh_drives();
                self.measure_storage()
            }
            Message::AddDrive(index) => {
                match self.add_drive(index) {
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }
            Message::SetDefaultDrive(id) => {
                // The button is held for a drive that is away, but this also
                // decides where the *next* install goes and what Battle.net is
                // told, neither is worth pointing at a drive that is not here.
                if let Some(drive) = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == id)
                    .filter(|l| self.drive_states.get(&l.id) == Some(&library::State::Missing))
                    .map(|l| l.label.clone())
                {
                    self.error = Some(format!("{drive} is not mounted").into());
                    return Ok(Task::none());
                }
                for library in &mut self.config.libraries {
                    library.is_default = library.id == id;
                }
                self.save_config();
                self.rebuild_layout();
                // Battle.net's `DefaultInstallPath` follows the default drive,
                // and it is only written where nothing in the prefix is running,
                // so it is re-asserted by the next pre-flight, not here. The
                // client itself does not move: it lives in the prefix.
                self.status = "new installs will go to this drive".to_owned();
                self.measure_storage()
            }
            Message::RemoveDrive(id) => {
                match self.remove_drive(&id) {
                    // `refresh_drives` as well as the measurement: a drive
                    // that is no longer ours goes back to being a partition
                    // the page can offer, and that list is only rebuilt here.
                    // Without it the row stays gone until you leave Settings
                    // and come back.
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }
            Message::ClearDrive(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id).cloned()
                else {
                    return Ok(Task::none());
                };
                self.drive_notice = Some((id.clone(), "clearing…".to_owned()));
                let layout = self.layout.clone();
                Task::perform(
                    tasks::clear_library(layout, library),
                    move |result| Message::DriveCleared(id.clone(), result),
                )
            }
            Message::UnusedPlanned(id, result) => {
                if !matches!(self.unused.get(&id), Some(UnusedData::Freeing { .. })) {
                    let state = match result {
                        Ok(plan) => UnusedData::Ready(plan),
                        Err(e) => UnusedData::Said(e),
                    };
                    self.unused.insert(id, state);
                }
                Task::none()
            }
            Message::FreeUnused(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id) else {
                    return Ok(Task::none());
                };
                let install = self.layout.wow_install_in(library);
                if self.downloading_into(&install) {
                    return Ok(Task::none());
                }
                // Compaction moves and deletes archives for minutes; nothing
                // else may write the store meanwhile, and the client may not
                // have it open. `pump` and pre-flight both hold while it runs.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                let total = match self.unused.get(&id) {
                    Some(UnusedData::Ready(plan)) => plan.moved,
                    _ => return Ok(Task::none()),
                };
                let copied = Arc::<std::sync::atomic::AtomicU64>::default();
                self.unused.insert(id.clone(), UnusedData::Freeing { copied: copied.clone(), total });
                Task::perform(
                    tasks::free_unused(self.http.clone(), self.layout.clone(), install, copied),
                    move |result| Message::UnusedFreed(id.clone(), result),
                )
            }
            Message::UnusedFreed(id, result) => {
                let note = match result {
                    Ok(bytes) => format!("freed {}", size_of(bytes)),
                    Err(e) => e,
                };
                self.unused.insert(id, UnusedData::Said(note));
                Task::batch([self.measure_storage(), self.pump()])
            }
            Message::DriveCleared(id, result) => {
                self.drive_notice = Some(match result {
                    Ok(0) => (id, "nothing to clear".to_owned()),
                    Ok(bytes) => (id, format!("freed {}", size_of(bytes))),
                    Err(e) => (id, e),
                });
                self.measure_storage()
            }
            Message::OpenDrive(id) => {
                if let Some(library) = self.config.libraries.iter().find(|l| l.id == id) {
                    // A drive with nothing installed on it yet has no `games/`
                    // folder, so this opens the nearest thing that does exist,
                    // the place the folder would appear.
                    let path = library
                        .games()
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| library.games().to_path_buf());
                    if let Err(e) = tasks::open_path(path) {
                        self.error = Some(e.into());
                    }
                }
                Task::none()
            }
            Message::AskUninstall => {
                self.gear_open = false;
                let code = self.selected.code.clone();
                self.uninstall_dialog = Some(UninstallDialog {
                    code: code.clone(),
                    label: self.selected.label.clone(),
                    plan: None,
                    working: false,
                    copied: Arc::default(),
                });
                Task::perform(
                    tasks::uninstall_plan(self.http.clone(), self.layout.clone(), code.clone()),
                    move |plan| Message::UninstallPlanned(code.clone(), Box::new(plan)),
                )
            }
            Message::UninstallPlanned(code, plan) => {
                if let Some(dialog) = self.uninstall_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.plan = Some(*plan);
                }
                Task::none()
            }
            Message::UninstallCancel => {
                self.uninstall_dialog = None;
                Task::none()
            }
            Message::UninstallConfirm => {
                // Any writer, not only this product's own: the store is shared,
                // and a reclaim rewrites archives another flavour is writing.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                let Some(dialog) = self.uninstall_dialog.as_mut() else {
                    return Ok(Task::none());
                };
                dialog.working = true;
                let code = dialog.code.clone();
                let http = self.http.clone();
                Task::perform(
                    tasks::uninstall(http, self.layout.clone(), code.clone(), dialog.copied.clone()),
                    move |result| Message::Uninstalled(code.clone(), result),
                )
            }
            Message::Uninstalled(code, result) => {
                let label = self.uninstall_dialog.take().map(|d| d.label).unwrap_or_default();
                match result {
                    Ok(freed) => {
                        // A paused or queued download of it would only put
                        // the files straight back.
                        self.queue.retain(|d| d.code != code);
                        if code == self.selected.code {
                            self.status = format!("uninstalled {label} — freed {}", size_of(freed));
                        }
                        Task::batch([
                            Task::perform(
                                tasks::installed_versions(self.layout.clone()),
                                Message::InstalledVersions,
                            ),
                            self.reload_if_showing(&code),
                            self.pump(),
                        ])
                    }
                    Err(e) => {
                        self.error = Some(e);
                        self.pump()
                    }
                }
            }
            Message::ClearCache => {
                let Some(dirs) = self.dirs.clone() else {
                    return Ok(Task::none());
                };
                self.cache_notice = Some("clearing…".to_owned());
                Task::perform(tasks::clear_cache(dirs), Message::CacheCleared)
            }
            Message::CacheCleared(result) => {
                self.cache_notice = Some(match result {
                    Ok(freed) => format!("freed {}", size_of(freed)),
                    Err(e) => e,
                });
                self.measure_storage()
            }
            Message::ScanAndRepair => {
                self.gear_open = false;
                // It drops damaged entries from the indices, so nothing else
                // may be writing the store, and the client may not have it
                // open.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                self.error = None;
                self.scanning = true;
                self.scan_for = Some(self.selected.code.clone());
                self.busy = true;
                self.scan_sample = Default::default();
                self.status = "checking stored files against their headers…".to_owned();

                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(
                    tasks::scan_and_repair(self.layout.clone(), self.selected.code.clone(), tx),
                    |result| Message::Scanned(Box::new(result)),
                )
            }
            Message::DeepScan => {
                self.gear_open = false;
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                    self.status = "stopping the deep scan…".to_owned();
                    return Ok(Task::none());
                }
                // Every WoW flavour shares one store, so any transfer at all
                // is writing to what this would read.
                let transferring = self.winding_down.is_some()
                    || self
                        .queue
                        .iter()
                        .any(|d| matches!(d.stage, Stage::Running { .. } | Stage::Queued));
                if transferring {
                    self.error = Some(
                        "pause downloads first — they write to the files being checked".into(),
                    );
                    return Ok(Task::none());
                }
                if let Some(what) = self.maintenance() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                self.scan_for = Some(self.selected.code.clone());

                self.error = None;
                self.scanning = true;
                self.owed_repair = None;
                self.scan_sample = Default::default();
                self.status = "checking every stored byte — the game can stay open".to_owned();

                let cancel = Cancel::new();
                self.deep_cancel = Some(cancel.clone());
                let (tx, rx) = tokio::sync::watch::channel(Default::default());
                self.scan_rx = Some(rx);
                Task::perform(tasks::deep_scan(self.layout.clone(), self.selected.code.clone(), cancel, tx), |result| {
                    Message::DeepScanned(Box::new(result))
                })
            }
            Message::DeepScanned(result) => {
                self.scanning = false;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                self.scan_rx = None;
                let stopped = self.deep_cancel.take().is_some_and(|c| c.is_cancelled());
                match *result {
                    Err(_) if stopped => {
                        self.status = "deep scan stopped".to_owned();
                        self.pump()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        self.pump()
                    }
                    Ok(report) if report.is_healthy() => {
                        self.status = format!(
                            "{} files checked down to the byte — none damaged",
                            report.checked
                        );
                        self.pump()
                    }
                    // Repaired before the queue is released: a transfer
                    // landing first would move the store and void the
                    // findings.
                    Ok(report) => {
                        self.status = format!(
                            "{} damaged of {} — repairing…",
                            report.damaged.len(),
                            report.checked
                        );
                        self.owed_repair = Some((scanned, Box::new(report)));
                        self.update(Message::RepairDamaged)
                    }
                }
            }
            Message::RepairDamaged => {
                self.gear_open = false;
                let Some((code, report)) = self.owed_repair.clone() else {
                    return Ok(Task::none());
                };
                // Kept owed, and said so, rather than started: the game has
                // the store open, or something else is writing it.
                let held = match self.store_writer() {
                    Some(what) => Some(format!("wait for {what} to finish")),
                    None => self.launching.is_some().then(|| "close the game".to_owned()),
                };
                if let Some(held) = held {
                    self.status = format!(
                        "{} damaged files found — {held}, then choose Repair Damaged Files",
                        report.damaged.len()
                    );
                    return Ok(self.pump());
                }
                // A writer of its own rather than `busy`, which belongs to a
                // launch and a session: clearing it here once re-enabled Play
                // in the middle of one.
                self.repairing = Some(code.clone());
                Task::perform(tasks::repair(self.layout.clone(), code, *report), Message::Repaired)
            }
            Message::Repaired(result) => {
                let code = self.repairing.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(removed) => {
                        self.owed_repair = None;
                        if code == self.selected.code {
                            self.status =
                                format!("{removed} damaged files dropped — fetching them again");
                        }
                        self.enqueue_for(code)
                    }
                    Err(tasks::Unrepaired::InUse) => {
                        let damaged = self.owed_repair.as_ref().map_or(0, |(_, r)| r.damaged.len());
                        self.status = format!(
                            "{damaged} damaged files found — close the game and Battle.net, \
                             then choose Repair Damaged Files"
                        );
                        self.pump()
                    }
                    Err(tasks::Unrepaired::Failed(e)) => {
                        self.owed_repair = None;
                        self.error = Some(e.into());
                        self.pump()
                    }
                }
            }
            Message::Scanned(result) => {
                self.scanning = false;
                self.busy = false;
                self.scan_rx = None;
                let scanned = self.scan_for.take().unwrap_or_else(|| self.selected.code.clone());
                match *result {
                    Err(e) => {
                        self.error = Some(e.into());
                        self.pump()
                    }
                    Ok(scan) => {
                        // Either way the download path runs: repair works by
                        // removing damaged entries so the planner counts them
                        // missing, and the same plan also covers whatever was
                        // never fetched in the first place.
                        self.status = match scan.damaged {
                            0 => format!(
                                "{} files verified — none damaged; checking for anything missing",
                                scan.checked
                            ),
                            n => format!(
                                "{n} damaged of {} — {} dropped, fetching them again",
                                scan.checked, scan.removed
                            ),
                        };
                        self.enqueue_for(scanned)
                    }
                }
            }
            Message::ClearShaderCache => {
                let Some(dirs) = self.dirs.as_ref() else {
                    self.error = Some("no cache directory available".into());
                    return Ok(Task::none());
                };
                // Per product, so this cannot take another product's shaders
                // with it, the reason the cache is keyed that way at all.
                let dir = dirs.shader_cache_dir(&self.selected.code);
                match std::fs::remove_dir_all(&dir) {
                    Ok(()) => self.status = "shader cache cleared".to_owned(),
                    // Nothing there is the desired end state, not a failure.
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                        self.status = "no shader cache to clear".to_owned()
                    }
                    Err(e) => self.error = Some(format!("{}: {e}", dir.display()).into()),
                }
                Task::none()
            }
        

            other => return Err(other),
        })
    }
}
