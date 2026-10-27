//! The download queue, its speed limits, and the progress tick.

use crate::*;

impl Lightforge {
    pub(crate) fn update_downloads(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_transfer(message)
            .or_else(|m| self.update_transfer_ended(m))
            .or_else(|m| self.update_transfer_cancel(m))
            .or_else(|m| self.update_download_limits(m))
    }

    /// A transfer under way: its progress, pausing and resuming, and the update check that starts one.
    fn update_transfer(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::Tick => {
                if let Some(rx) = &self.setup_rx {
                    self.setup_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.addon_rx {
                    self.addon_sample = rx.borrow().clone();
                }
                if let Some(rx) = &self.scan_rx {
                    self.scan_sample = *rx.borrow();
                }
                if let Some(upgrade) = self.upgrade.as_mut() {
                    if let Some(rx) = &upgrade.rx {
                        upgrade.sample = *rx.borrow();
                    }
                }
                if let Some(entry) = self
                    .queue
                    .iter_mut()
                    .find(|d| matches!(d.stage, Stage::Running { .. }))
                {
                    if let Stage::Running { rx, .. } = &entry.stage {
                        entry.sample = rx.borrow().clone();
                    }
                    match (entry.job.total, entry.sample.remaining) {
                        (None, remaining) => entry.job.total = remaining,
                        // Restored from before a restart, and a new build has
                        // come out since: the saved size no longer describes
                        // this job, so it starts over from what is left.
                        (Some(total), Some(remaining)) if remaining > total => {
                            entry.job = Job { total: Some(remaining), reached: 0 };
                        }
                        _ => {}
                    }
                    // Rate is measured on what this run has stored; the
                    // baseline is a constant offset and would only mute the
                    // first sample.
                    entry.rate.sample(entry.sample.progress.bytes_stored);

                    // What this build offers is only knowable once its
                    // manifest has been read, so a run is the one chance to
                    // learn it. Cached per product so the settings panel has
                    // something real to list.
                    let (code, offered) =
                        (entry.code.clone(), entry.sample.optional.clone());
                    if !offered.is_empty() {
                        self.remember_optional(&code, offered);
                    }
                }
                Task::none()
            }

            Message::PauseDownload(code) => {
                self.pause(&code);
                Task::none()
            }

            Message::ResumeDownload(code) => {
                // Its game is running and has the store open: it goes back in
                // the queue when the game exits, as pre-flight arranged.
                if self.busy && self.launching.as_ref() == Some(&code) {
                    if !self.resume_after_play.contains(&code) {
                        self.resume_after_play.push(code);
                    }
                    self.status = "resumes when the game closes".to_owned();
                    return Ok(Task::none());
                }
                // Back to the end of the queue rather than jumping the line:
                // whatever is running was started first.
                if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                    self.queue[at].stage = Stage::Queued;
                    self.queue[at].sample.stage = "queued".to_owned();
                }
                self.pump()
            }

            Message::PauseWhilePlaying(on) => {
                self.config.pause_downloads_while_playing = on;
                self.save_config();
                Task::none()
            }

            Message::CheckForUpdates => {
                self.gear_open = false;
                self.busy = true;
                self.error = None;
                self.status = "checking for updates…".to_owned();
                let code = self.selected.code.clone();
                Task::perform(
                    tasks::offerings(
                        self.http.clone(),
                        code.clone(),
                        self.config.region,
                        self.opted_optional(&code),
                        None,
                    ),
                    move |result| Message::CheckedForUpdates(code.clone(), Box::new(result)),
                )
            }

            Message::CheckedForUpdates(code, result) => {
                self.busy = false;
                match *result {
                    Ok(offerings) => {
                        // The point of the round trip: the optional list is
                        // only knowable from the manifest, and this is the
                        // cheap way to learn it. Kept whichever product is
                        // showing by now; the rest only describes this one.
                        self.remember_optional(&code, offerings.optional.clone());
                        if code != self.selected.code {
                            return Ok(Task::none());
                        }
                        let current = self.local.version.as_deref() == Some(
                            offerings.remote.versions_name.as_str(),
                        );
                        self.remote = Some(offerings.remote.clone());
                        self.status = match current {
                            true => format!("up to date on {}", offerings.remote.versions_name),
                            false => {
                                format!("{} is available", offerings.remote.versions_name)
                            }
                        };
                    }
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// A transfer ending, however it ended.
    fn update_transfer_ended(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::Downloaded(code, result) => {
                // A run a launch paused stops as cancelled, and is still owed
                // its restart when the game exits: `finished` would forget
                // that, which is why `Launched` never found anything owed.
                let paused_by_launch = matches!(&result, Ok(done) if done.cancelled)
                    && self.downloads.pending().contains(&code);
                if !paused_by_launch {
                    self.downloads.finished(&code);
                }
                if self.winding_down.as_ref() == Some(&code) {
                    self.winding_down = None;
                }
                // The run this was waiting on has stopped, so the store is
                // closed and its files can go, which `pump` does before it
                // starts anything else. Before everything below, which is all
                // about a queue entry that no longer exists.
                if self.delete_after_cancel.contains(&code) {
                    return Ok(self.pump());
                }

                // Status and errors describe the selected product. A transfer
                // that finished while the user was looking elsewhere updates
                // the queue and says nothing.
                let showing = code == self.selected.code;
                let entry = self.queue.iter().position(|d| d.code == code);
                let note = |app: &mut Self, text: String| {
                    if showing {
                        app.status = text;
                    }
                };

                // Resumed while this run was still winding down: the entry is
                // already a fresh, queued one. Marking it paused here, as
                // this handler once did, swallowed the resume, and whether it
                // happened came down to how fast the old run stopped.
                let resumed = entry.is_some_and(|at| matches!(self.queue[at].stage, Stage::Queued));
                let complete = matches!(&result, Ok(done) if done.complete);
                if resumed && !complete {
                    if let (Err(e), true) = (&result, showing) {
                        self.error = Some(e.clone());
                    }
                    return Ok(self.pump());
                }

                match result {
                    Ok(done) if done.complete => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        // Only a game that was given its own language: one
                        // that follows the setup choice keeps whatever the
                        // client has already written.
                        if let Some(locale) =
                            self.config.product(&code).and_then(|p| p.profile.locale.clone())
                        {
                            let file = self.layout.product_dir(&code).join("WTF").join("Config.wtf");
                            if let Err(e) = lightforge_core::clientlocale::set(&file, &locale) {
                                self.error = Some(format!("{}: {e}", file.display()).into());
                            }
                        }
                        note(
                            self,
                            match done.version {
                                Some(v) => format!("installed {v}"),
                                None => format!("stored {} files", done.files),
                            },
                        );
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Ok(done) if done.cancelled => {
                        // Paused, so it keeps its place with what it reached.
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "paused".to_owned());
                        self.pump()
                    }
                    Ok(done) => {
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        // Said outright: "stopped" with no cause reads as a
                        // hang, and the same run again fails the same way.
                        let why = match done.reasons.is_empty() {
                            true => String::new(),
                            false => format!(": {}", done.reasons.join("; ")),
                        };
                        tracing::warn!("{} stopped without finishing{why}", code.as_str());
                        note(self, format!("stopped{why} — run again to retry"));
                        Task::batch([self.pump(), self.reload_if_showing(&code)])
                    }
                    Err(e) => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
                        }
                        if showing {
                            self.error = Some(e);
                        }
                        self.pump()
                    }
                }
            }
            other => return Err(other),
        })
    }

    /// Cancelling a transfer, and the cleanup after it.
    fn update_transfer_cancel(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::CancelDownload(code) => {
                self.drop_transfer(&code);
                self.pump()
            }

            Message::AskCancelDownload(code) => {
                self.gear_open = false;
                if !self.queue.iter().any(|d| d.code == code) {
                    return Ok(Task::none());
                }
                let label = self
                    .queue
                    .iter()
                    .find(|d| d.code == code)
                    .map(|d| d.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());
                // What is on disk for *this* product, which is the thing that
                // decides whether giving up may delete anything. Read from the
                // panel only when the panel is showing this product; otherwise
                // the disk is asked directly.
                let installed = match code == self.selected.code {
                    true => self.local.version.clone(),
                    false => state::Local::inspect(&self.layout, &code).version,
                };
                self.cancel_dialog = Some(CancelDialog { code, label, installed });
                Task::none()
            }

            Message::CancelDownloadDismiss => {
                self.cancel_dialog = None;
                Task::none()
            }

            Message::CancelCleaned(code, result) => {
                self.discarding = None;
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.short_name().to_owned());
                match result {
                    Ok(freed) => {
                        if code == self.selected.code {
                            self.status =
                                format!("cancelled {label} — freed {}", size_of(freed));
                        }
                    }
                    // The transfer is still cancelled; only the clean-up
                    // failed, and saying so is better than a status line
                    // claiming space came back.
                    Err(e) => {
                        self.error = Some(format!(
                            "{label} was cancelled, but its files could not be \
                             removed: {e}"
                        ).into());
                    }
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

            Message::CancelDownloadConfirm { delete } => {
                let Some(dialog) = self.cancel_dialog.take() else {
                    return Ok(Task::none());
                };
                let code = dialog.code;
                // Finished while the dialog was open: there is no download
                // left to give up, and its files are now an install.
                if !self.queue.iter().any(|d| d.code == code) {
                    self.status = format!("{} had already finished; nothing was cancelled", dialog.label);
                    return Ok(Task::none());
                }
                self.drop_transfer(&code);
                self.status = match delete {
                    true => "cancelled — removing the files…".to_owned(),
                    false => "cancelled".to_owned(),
                };
                if !delete {
                    return Ok(self.pump());
                }
                // Removed once nothing is writing to the store any more: at
                // once for a queued entry, after the wind-down for a running
                // or just-paused one, after another product's run for a
                // store they share. See `pump`.
                self.delete_after_cancel.push(code);
                self.pump()
            }
            other => return Err(other),
        })
    }

    /// Lightforge's own download limit, and Battle.net's.
    fn update_download_limits(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::DownloadLimitChanged(value) => {
                // Digits only, so the field cannot hold something that is not
                // a speed. Clearing it is how you say "unlimited".
                self.limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let trimmed = self.limit_input.trim();
                self.config.download_limit_mbps = match trimmed.parse::<u64>() {
                    Ok(0) | Err(_) => None,
                    Ok(mbps) => Some(mbps),
                };
                self.save_config();
                Task::none()
            }

            Message::BattleNetLimitRead(mbps) => {
                self.bnet_limit_input = mbps.map(|v| v.to_string()).unwrap_or_default();
                Task::none()
            }

            Message::BattleNetLimitChanged(value) => {
                self.bnet_limit_input = value.chars().filter(char::is_ascii_digit).collect();
                let mbps = self.bnet_limit_input.trim().parse::<u64>().ok().filter(|v| *v > 0);
                Task::perform(
                    tasks::set_battle_net_limit(
                        self.layout.clone(),
                        self.config.prefix_user.clone(),
                        mbps,
                    ),
                    Message::BattleNetLimitWritten,
                )
            }

            Message::BattleNetLimitWritten(result) => {
                if let Err(e) = result {
                    self.error = Some(e.into());
                }
                Task::none()
            }
            other => return Err(other),
        })
    }
}
