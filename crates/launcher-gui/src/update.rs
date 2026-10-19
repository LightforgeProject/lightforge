//! Handling one message.

use crate::*;

impl Lightforge {
    pub(crate) fn handle(&mut self, message: Message) -> Task<Message> {
        match message {
            Message::LineSelected(line) => {
                // Picking from the overflow menu closes it: the tile is in the
                // band now, and the menu is answering a question already asked.
                self.close_panels();
                // The band is on News too, where choosing a game is a request
                // to go and look at it, leaving you on a page that says
                // nothing about the game you just picked would read as a dead
                // click.
                self.go_to(Screen::Dashboard);
                self.select_line(&line)
            }

            Message::BranchSelected(pick) => {
                // An unconfigured branch is offered, not hidden, picking it
                // is how it gets added, which beats hand-editing config.toml
                // to try a PTR for an afternoon.
                if !pick.configured {
                    let label = pick.label.clone();
                    self.config
                        .products
                        .push(Product::new(pick.code.clone(), label.clone()));
                    self.save_config();
                    self.choices.push(Choice { code: pick.code.clone(), label });
                }
                let Some(choice) = self.choices.iter().find(|c| c.code == pick.code).cloned() else {
                    return Task::none();
                };
                self.select(choice)
            }

            Message::Refreshed(code, refreshed) => {
                if code != self.selected.code {
                    return Task::none();
                }
                // A drive can go away between two refreshes, and this is the
                // moment the panel is being rebuilt anyway.
                self.check_drives();
                self.local = refreshed.local.clone();
                self.remote = refreshed.remote.clone();
                self.status = match &refreshed.offline {
                    // Rule 2 of the resolver: offline never blocks a launch, so
                    // it is a note, not an error.
                    Some(_) => "version service unreachable — offline".to_owned(),
                    None => refreshed
                        .remote
                        .as_ref()
                        .map(|r| format!("latest {}", r.versions_name))
                        .unwrap_or_default(),
                };
                Task::none()
            }

            Message::ResizeTraced(size) => {
                trace::mark(format_args!("resize {}x{}", size.width, size.height));
                Task::none()
            }

            Message::CachedTheme(code, themed) => {
                trace::mark(format_args!(
                    "cache-first theme for {}: {}, {} icon(s)",
                    code.as_str(),
                    match &themed.branding {
                        Some(_) => "hit",
                        None => "MISS — nothing usable on disk",
                    },
                    themed.icons.len(),
                ));
                // Strictly a gap-filler: the revalidated answer wins whenever
                // it happens to arrive first, so none of this overwrites it.
                if self.manifest.is_none() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                if self.icons.is_empty() {
                    self.icons = themed.icons;
                }
                let selected = code == self.selected.code;
                if selected && self.branding.is_none() {
                    self.branding = themed.branding;
                }
                match selected {
                    true => self.splash_decide(themed.warm),
                    false => Task::none(),
                }
            }

            Message::Themed(code, themed) => {
                trace::mark(format_args!(
                    "revalidated theme for {}: manifest {}, branding {}",
                    code.as_str(),
                    if themed.manifest.is_some() { "ok" } else { "failed" },
                    if themed.branding.is_some() { "ok" } else { "failed" },
                ));
                // The manifest describes every product, so it is kept even
                // when a switch has already moved on from the one it themed.
                if themed.manifest.is_some() {
                    self.manifest = themed.manifest;
                    self.refresh_catalog();
                }
                // Same reasoning as the branding swap below. A revalidation
                // that answered 304 produces the same files decoded again;
                // assigning them hands the renderer brand-new texture ids for
                // pixels already on screen, and the whole band blinks after
                // every switch.
                if !themed.icons.is_empty() && !tasks::same_marks(&self.icons, &themed.icons) {
                    self.icons = themed.icons;
                }
                if code == self.selected.code {
                    // Only when it is actually different art. A revalidation
                    // that answered 304 resolves to the same files, already
                    // decoded and on screen, but assigning it hands iced new
                    // image handles, which it has to upload again, and the
                    // page blinks a second or two after a switch for no
                    // reason. [`Branding`] carries each asset's path and
                    // whether its bytes moved this run, so comparing it
                    // answers exactly that question.
                    let same = self.branding.as_ref().map(|art| &art.branding)
                        == themed.branding.as_ref().map(|art| &art.branding);
                    trace::mark(match same {
                        true => "branding unchanged, keeping the decoded art",
                        false => "branding changed, swapping the art",
                    });
                    if !same {
                        self.branding = themed.branding;
                    }
                }
                Task::none()
            }

            Message::Primary => {
                // Pressed on a game only being looked at: it joins the band.
                self.track_selected();
                match self.action() {
                    // A first install asks first; an update or a retry already
                    // has its answers.
                    ActionState::Install => self.open_install(),
                    ActionState::Update { .. } | ActionState::Error { .. } => self.enqueue(),
                    ActionState::Downloading(_) => {
                        // Pause: trip the token. The loop stops between chunks and
                        // the store keeps everything already written.
                        self.pause(&self.selected.code.clone());
                        Task::none()
                    }
                    ActionState::Paused(_) => self.enqueue(),
                    // Leaving the queue is pausing: nothing has started, so there
                    // is nothing to stop, but the intent is the same.
                    ActionState::Queued => {
                        self.pause(&self.selected.code.clone());
                        Task::none()
                    }
                    ActionState::Play => self.preflight(Launch::Game),
                    ActionState::Unknown => Task::none(),
                    // Nothing to download and nothing to repair: look for the
                    // drive again, and reload the product if it came back.
                    ActionState::Offline { .. } => {
                        self.refresh_drives();
                        match self.offline_drive(&self.selected.code) {
                            Some(drive) => {
                                self.status = format!("{drive} is still not mounted");
                                Task::none()
                            }
                            None => {
                                self.error = None;
                                self.reload(self.selected.code.clone())
                            }
                        }
                    }
                }
            }

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

            Message::CloseRequested(id) => {
                // A deep scan only reads; there is nothing to wind down.
                if let Some(cancel) = &self.deep_cancel {
                    cancel.cancel();
                }
                // Closing mid-transfer must not orphan what was fetched: trip
                // the token, let the run checkpoint its store, then close.
                if let Some(code) = self.running().map(|d| d.code.clone()) {
                    // Saved as running a message ago; keep it that way.
                    self.queue_frozen = true;
                    self.pause(&code);
                }
                // Anything else writing to a store, a pause still winding
                // down, a repair, a compaction, is let finish too: `pump`
                // closes the window once the last of them reports back.
                match self.store_writer() {
                    Some(what) => {
                        self.closing = Some(id);
                        self.status = format!("finishing {what} safely…");
                        Task::none()
                    }
                    None => iced::window::close(id),
                }
            }

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
                    return self.pump();
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
                    return self.pump();
                }

                match result {
                    Ok(done) if done.complete => {
                        if let Some(at) = entry {
                            self.queue.remove(at);
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
                    Ok(_) => {
                        if let Some(at) = entry {
                            let d = &self.queue[at];
                            let reached = job_progress(d.job, &d.sample, 0);
                            self.queue[at].stage = Stage::Paused(reached);
                        }
                        note(self, "stopped — run again to continue".to_owned());
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

            Message::Launched(result) => {
                self.busy = false;
                let launched = self.launching.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                // A launch may have paused transfers, its own product through
                // pre-flight's guard, and the rest of the queue through the
                // setting. Both are owed a restart, and resuming means
                // planning afresh.
                for code in std::mem::take(&mut self.resume_after_play) {
                    if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                        self.queue[at].stage = Stage::Queued;
                        self.queue[at].sample.stage = "queued".to_owned();
                    }
                }
                let owed = self.downloads.pending().contains(&launched);
                if owed {
                    if let Some(at) = self.queue.iter().position(|d| d.code == launched) {
                        self.queue[at].stage = Stage::Queued;
                    }
                }
                // A Battle.net session may have filled a cache that was
                // empty when the app started, the first non-WoW install in
                // a prefix is exactly when its catalogue first appears. It is
                // 25 ms of reading, so it costs less than remembering not to.
                Task::batch([
                    self.pump(),
                    self.reload(self.selected.code.clone()),
                    Task::perform(
                        tasks::game_families(
                            self.layout.clone(),
                            self.config.prefix_user.clone(),
                            self.config.region,
                        ),
                        |families| Message::FamiliesRead(Box::new(families)),
                    ),
                    // And what it has installed: an install handed over to
                    // Battle.net finishes in Battle.net, so Agent's registry
                    // is the only thing that knows it happened.
                    Task::perform(
                        tasks::installed_versions(self.layout.clone()),
                        Message::InstalledVersions,
                    ),
                ])
            }

            Message::HandoffStarted(result) => {
                match result {
                    // Found it. `busy` stays set and the queue stays paused
                    // for the length of the session, exactly as a direct
                    // launch does, the whole point of watching.
                    Ok(tasks::Handoff::Playing(game)) => {
                        self.status = format!(
                            "{} is running — this unlocks when it exits",
                            game.name,
                        );
                        // Not when the hand-off started: the client's window
                        // and its sign-in are things the user is meant to see.
                        Task::batch([
                            Task::perform(
                                tasks::await_game_exit(self.layout.clone(), game),
                                Message::Launched,
                            ),
                            self.minimize_if_asked(),
                        ])
                    }
                    // The client is up and waiting on a person, two-factor,
                    // an outage, an update of its own. Said plainly, with the
                    // client left running so it can be finished there.
                    // Closed while we were waiting for it to sign in. Not a
                    // failure to report as one, just nothing to hand off to.
                    Ok(tasks::Handoff::ClientClosed) => self.update(Message::Launched(Ok(
                        "Battle.net was closed before the game could start".to_owned(),
                    ))),
                    Ok(tasks::Handoff::NeedsAttention { minimized }) => {
                        // Where it is matters as much as that it needs
                        // something: a client started minimized is in the
                        // tray, and "its window" is not on screen to look at.
                        self.update(Message::Launched(Ok(match minimized {
                            true => "Battle.net needs attention — open it from the tray, \
                                     finish there and press Play"
                                .to_owned(),
                            false => "Battle.net needs attention — finish in its window and \
                                      press Play there"
                                .to_owned(),
                        })))
                    }
                    // Signed in, told to launch, and nothing came up.
                    Ok(tasks::Handoff::NothingStarted) => self.update(Message::Launched(Ok(
                        "Battle.net is open — no game started".to_owned(),
                    ))),
                    Err(e) => self.update(Message::Launched(Err(e))),
                }
            }

            Message::BattleNet => {
                self.track_selected();
                self.preflight(Launch::BattleNet)
            }

            Message::InstallViaBattleNet => {
                self.track_selected();
                self.preflight(Launch::Install)
            }

            Message::OpenBattleNet => {
                self.track_selected();
                self.preflight(Launch::Client)
            }

            Message::ToggleManager => {
                self.manager_open = !self.manager_open;
                if self.manager_open {
                    self.browse_open = false;
                    self.strip_open = false;
                }
                Task::none()
            }

            Message::ToggleHints => {
                self.hints = !self.hints;
                Task::none()
            }

            Message::ToggleStrip => {
                self.strip_open = !self.strip_open;
                if self.strip_open {
                    self.browse_open = false;
                    self.manager_open = false;
                }
                Task::none()
            }

            Message::OpenProduct(code) => {
                self.close_panels();
                self.go_to(Screen::Dashboard);
                let choice = self.choices.iter().find(|c| c.code == code).cloned().unwrap_or_else(|| {
                    let label = self
                        .catalog
                        .get(&code)
                        .map(|e| e.label.clone())
                        .unwrap_or_else(|| code.as_str().to_owned());
                    Choice { code, label }
                });
                self.select(choice)
            }

            Message::TileMenu(line) => {
                let open = self.tile_menu.as_ref() != Some(&line);
                self.close_panels();
                self.tile_menu = open.then_some(line);
                Task::none()
            }

            Message::TileEntered(line) => {
                self.tile_hover = Some(line);
                Task::none()
            }

            Message::TileLeft(line) => {
                // Only if it is still the one recorded: entering the next tile
                // can arrive before leaving the last.
                if self.tile_hover.as_ref() == Some(&line) {
                    self.tile_hover = None;
                }
                Task::none()
            }

            Message::TileGrabbed(line) => {
                self.tile_drag = Some(TileDrag { line, origin: None, last: 0.0, moving: false });
                Task::none()
            }

            Message::TileDragged(x) => {
                let Some(drag) = &mut self.tile_drag else {
                    return Task::none();
                };
                let Some(origin) = drag.origin else {
                    drag.origin = Some(x);
                    drag.last = x;
                    return Task::none();
                };
                drag.moving |= (x - origin).abs() > DRAG_SLOP;
                let heading = x - drag.last;
                drag.last = x;
                if !drag.moving {
                    return Task::none();
                }

                // Moved over another tile: take its place, the way a browser
                // tab does. Only in the direction of travel, or two tiles of
                // different widths trade places back and forth under a
                // cursor that has come to rest on the seam.
                let held = drag.line.clone();
                let Some(over) = self.tile_hover.clone().filter(|line| line != &held) else {
                    return Task::none();
                };
                let lines = self.lines();
                let (Some(from), Some(to)) = (
                    lines.iter().position(|l| l == &held),
                    lines.iter().position(|l| l == &over),
                ) else {
                    return Task::none();
                };
                if (to > from && heading > 0.0) || (to < from && heading < 0.0) {
                    self.move_line(from, to);
                }
                Task::none()
            }

            Message::TileDropped => {
                let Some(drag) = self.tile_drag.take() else {
                    return Task::none();
                };
                match drag.moving {
                    // Saved once, at the end: the order changes on every
                    // tile crossed, and the file needs only the last one.
                    true => {
                        self.save_config();
                        Task::none()
                    }
                    // A click is a press and a release on the same tile.
                    false if self.tile_hover.as_ref() == Some(&drag.line)
                        && drag.line != self.catalog.line_of(&self.selected.code) =>
                    {
                        self.update(Message::LineSelected(drag.line))
                    }
                    false => Task::none(),
                }
            }

            Message::ToggleBrowse => {
                self.browse_open = !self.browse_open;
                self.strip_open = false;
                if !self.browse_open {
                    return Task::none();
                }
                self.manager_open = false;
                // Read on open, so the list says what is already on the
                // machine without anything running in the background.
                Task::perform(
                    tasks::installed_versions(self.layout.clone()),
                    Message::InstalledVersions,
                )
            }

            Message::InstalledVersions(found) => {
                self.installed_versions = found;
                Task::none()
            }

            Message::RegistryPoll => {
                let Some(watch) = &mut self.registry_watch else { return Task::none() };
                watch.polls += 1;
                let check_client = watch.polls % REGISTRY_CLIENT_EVERY == 0;
                Task::perform(tasks::registry_probe(self.layout.clone(), check_client), |(stamp, running)| {
                    Message::RegistryPolled(stamp, running)
                })
            }

            Message::RegistryPolled(stamp, running) => {
                let Some(watch) = &mut self.registry_watch else { return Task::none() };
                let changed = watch.stamp != stamp;
                watch.stamp = stamp;
                match running {
                    Some(true) => watch.seen_client = true,
                    Some(false) if watch.seen_client || watch.since.elapsed() > REGISTRY_GRACE => {
                        self.registry_watch = None;
                    }
                    _ => {}
                }
                match changed {
                    // The game families come from the client's cached
                    // catalogue, which the same session fills.
                    true => Task::batch([
                        Task::perform(
                            tasks::installed_versions(self.layout.clone()),
                            Message::InstalledVersions,
                        ),
                        Task::perform(
                            tasks::game_families(
                                self.layout.clone(),
                                self.config.prefix_user.clone(),
                                self.config.region,
                            ),
                            |families| Message::FamiliesRead(Box::new(families)),
                        ),
                    ]),
                    false => Task::none(),
                }
            }

            Message::KnownCodes(codes) => {
                self.known_codes = codes;
                self.refresh_catalog();
                Task::none()
            }

            Message::RemoveProduct(code) => {
                // A tile is a *line*, and its PTR and beta are branches behind
                // its version picker rather than games of their own, so
                // Remove takes the line with it. Dropping the head alone left
                // the tile on screen wearing whichever branch survived, which
                // is what someone removing the game has just said they do not
                // want.
                let line = self.catalog.line_of(&code);
                let going: Vec<ProductCode> = self
                    .choices
                    .iter()
                    .filter(|c| self.catalog.line_of(&c.code) == line)
                    .map(|c| c.code.clone())
                    .collect();

                // The header cannot be empty: with nothing tracked there is no
                // product to show and no way back to this panel.
                if going.len() >= self.choices.len() {
                    self.error =
                        Some("that is the only game left — add another first".into());
                    return Task::none();
                }

                for code in &going {
                    // A transfer for a product nobody is tracking has nowhere
                    // to report to, so it stops here rather than running on
                    // unseen.
                    self.pause(code);
                    self.queue.retain(|d| &d.code != code);
                    self.downloads.finished(code);
                }

                self.config.products.retain(|p| !going.contains(&p.code));
                self.save_config();
                self.choices.retain(|c| !going.contains(&c.code));
                self.tile_menu = None;
                self.last_branch.remove(&line);

                match going.contains(&self.selected.code) {
                    // Standing on one of the ones just removed: move to
                    // whatever the header now starts with.
                    true => {
                        let next = self.choices[0].clone();
                        self.select(next)
                    }
                    false => Task::none(),
                }
            }

            Message::MutePatchNotes(mute) => {
                if let Some(notes) = self.patch_notes.as_mut() {
                    notes.mute = mute;
                }
                Task::none()
            }

            Message::NewsOnPlay(show) => {
                self.config.news_on_play = show;
                self.save_config();
                Task::none()
            }

            Message::ShowReleaseNotes(show) => {
                self.config.patch_notes_muted = !show;
                self.save_config();
                Task::none()
            }

            Message::DismissPatchNotes => {
                if let Some(notes) = self.patch_notes.take() {
                    // Recorded on the way out, so a start that was killed
                    // before anyone read them shows them again.
                    self.config.patch_notes_seen = Some(notes.release.version.clone());
                    self.config.patch_notes_muted = notes.mute;
                    self.save_config();
                }
                Task::none()
            }

            Message::UpdateChecked(found) => {
                let Some(available) = found else { return Task::none() };
                // Once per release. Someone who has read "beta.3 is out" and
                // decided to stay on beta.2 should not be told again on every
                // start until they give in.
                if self.config.update_seen.as_deref() == Some(available.latest.as_str()) {
                    return Task::none();
                }
                self.update_notice = Some(available);
                Task::none()
            }

            Message::DismissUnavailable(name) => {
                if let Some(report) = &mut self.addon_report {
                    report.unavailable.retain(|item| item.name != name);
                }
                Task::none()
            }

            Message::DismissUpdate => {
                if let Some(available) = self.update_notice.take() {
                    self.config.update_seen = Some(available.latest);
                    self.save_config();
                }
                Task::none()
            }

            Message::OpenReleases => {
                let url = self
                    .update_notice
                    .as_ref()
                    .map(|a| a.url.clone())
                    .unwrap_or_else(|| lightforge_core::update::RELEASES_PAGE.to_owned());
                if let Err(e) = open::that_detached(&url) {
                    self.error = Some(format!("{url}: {e}").into());
                }
                self.update(Message::DismissUpdate)
            }

            Message::CheckLauncherUpdates(on) => {
                self.config.check_for_updates = on;
                self.save_config();
                Task::none()
            }

            Message::AddToSteam => {
                self.add_to_steam();
                Task::none()
            }

            Message::ControllerLook(on) => {
                self.config.steamos_use = Some(match on {
                    true => SteamOsUse::Console,
                    false => SteamOsUse::Desktop,
                });
                self.save_config();
                self.window_mode()
            }

            Message::SplashDone(count) => {
                // The wizard's own count comes off this run: it is the same
                // fetch, and starting a second one on the first slide would
                // revalidate everything the splash just revalidated.
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.prefetched = Some(count);
                }
                match self.splash.is_some() {
                    true => self.close_splash_after_floor(),
                    false => Task::none(),
                }
            }

            Message::FinishSplash => self.finish_splash(),

            Message::AddProduct(code) => {
                if self.choices.iter().any(|c| c.code == code) {
                    return Task::none();
                }
                let label = self
                    .catalog
                    .get(&code)
                    .map(|e| e.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());

                self.config.products.push(Product::new(code.clone(), label.clone()));
                self.save_config();
                self.choices.push(Choice { code, label });
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
                    return Task::none();
                }
                // Back to the end of the queue rather than jumping the line:
                // whatever is running was started first.
                if let Some(at) = self.queue.iter().position(|d| d.code == code) {
                    self.queue[at].stage = Stage::Queued;
                    self.queue[at].sample.stage = "queued".to_owned();
                }
                self.pump()
            }

            Message::CancelDownload(code) => {
                self.drop_transfer(&code);
                self.pump()
            }

            Message::AskCancelDownload(code) => {
                self.gear_open = false;
                if !self.queue.iter().any(|d| d.code == code) {
                    return Task::none();
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
                    return Task::none();
                };
                let code = dialog.code;
                // Finished while the dialog was open: there is no download
                // left to give up, and its files are now an install.
                if !self.queue.iter().any(|d| d.code == code) {
                    self.status = format!("{} had already finished; nothing was cancelled", dialog.label);
                    return Task::none();
                }
                self.drop_transfer(&code);
                self.status = match delete {
                    true => "cancelled — removing the files…".to_owned(),
                    false => "cancelled".to_owned(),
                };
                if !delete {
                    return self.pump();
                }
                // Removed once nothing is writing to the store any more: at
                // once for a queued entry, after the wind-down for a running
                // or just-paused one, after another product's run for a
                // store they share. See `pump`.
                self.delete_after_cancel.push(code);
                self.pump()
            }

            Message::PreflightDone(what, result) => {
                if let Err(e) = result {
                    self.busy = false;
                    self.launching = None;
                    self.error = Some(e);
                    return self.pump();
                }
                // Pre-flight trips the paused downloads' tokens and returns;
                // the runs themselves are still flushing what was in flight
                // and checkpointing their indices. The client opening the
                // store under them is the overlap this whole gate exists for.
                if self.launch_must_wait() {
                    self.launch_when_quiet = Some(what);
                    self.status = "pre-flight… waiting for the download to pause".to_owned();
                    return Task::none();
                }
                self.start_launch(what)
            }

            Message::ManageSubscription => {
                if let Err(e) = tasks::open_url(ACCOUNT_URL) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::OpenFiles => {
                let dir = self.layout.product_dir(&self.selected.code);
                let dir = if dir.is_dir() { dir } else { self.layout.games_root() };
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::Show(screen) => {
                // Asking for the page you are on is not a history step, or
                // back would walk through a page repeated however many times
                // its tab was clicked.
                self.go_to(screen);
                // Including when the page did not change: the menu entry was
                // still a click, and it has had its answer.
                self.close_panels();
                self.error = None;
                self.arrived()
            }

            Message::GoBack => {
                if let Some(screen) = self.visited.pop() {
                    self.forward.push(self.screen);
                    self.screen = screen;
                    self.error = None;
                }
                self.arrived()
            }

            Message::GoForward => {
                if let Some(screen) = self.forward.pop() {
                    self.visited.push(self.screen);
                    self.screen = screen;
                    self.error = None;
                }
                self.arrived()
            }

            Message::ToggleAppMenu => {
                self.app_menu_open = !self.app_menu_open;
                Task::none()
            }

            Message::OpenLink(url) => {
                if let Err(e) = tasks::open_url(url) {
                    self.error = Some(e.into());
                }
                self.close_panels();
                Task::none()
            }

            Message::OpenArticle(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::RefreshNews => self.fetch_news(true),

            Message::NewsLoaded(headlines) => {
                self.news_pending = false;
                self.news_asked = true;
                // A failed refresh keeps what is already on screen: headlines
                // from ten minutes ago are worth more than an empty page.
                if let Some(headlines) = headlines {
                    self.news = Some(*headlines);
                }
                self.fetch_news_images()
            }

            Message::NewsBackground(handle) => {
                self.news_bg = Some(handle);
                Task::none()
            }

            Message::NewsImages(images) => {
                // Only pictures the fetch found are in here, and only ones
                // not already held, so this adds, never replaces. Replacing
                // would hand the view a new handle for an identical picture
                // and re-upload every texture on the page.
                self.news_images.extend(images);
                Task::none()
            }

            Message::Exit => iced::exit(),

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
                self.setup = status;
                Task::batch([measure, opened])
            }

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

            Message::AddonChannelDefault(choice) => {
                self.config.addon_channel = choice.0;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // check or update, which is also when it could matter.
                Task::none()
            }

            Message::AddonChannelFor(provider, id, choice) => {
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_channel(&options, &provider, &id, choice.0) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonMenu(which) => {
                self.addon_menu_open = which;
                Task::none()
            }

            Message::AddonIgnore(provider, id, ignored) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_ignored(&options, &provider, &id, ignored) {
                    self.error = Some(Problem::of(&e));
                }
                // Re-list rather than patch the row in place: the ledger on
                // disk is the truth the rest of the page is drawn from.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdate(provider, id, choice) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_auto_update(&options, &provider, &id, choice) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::FamiliesRead(families) => {
                self.families = *families;
                Task::none()
            }

            Message::ImportPinned(on) => {
                self.import_pinned = on;
                Task::none()
            }

            Message::AdoptAddons => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addons_busy = true;
                self.error = None;
                self.addon_auto_note = Some("looking up what is installed…".to_owned());
                Task::perform(
                    addonwork::adopt(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                    ),
                    |result| Message::AdoptFinished(Box::new(result)),
                )
            }

            Message::AdoptFinished(result) => {
                self.addons_busy = false;
                match *result {
                    Ok(adopted) => {
                        self.addon_auto_note = Some(match adopted.len() {
                            0 => "nothing new recognised — every addon here is \
                                  either already managed or not on CurseForge"
                                .to_owned(),
                            1 => format!("{} is now managed", adopted[0].name),
                            n => format!("{n} addons are now managed"),
                        });
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }

            Message::PickVersion(provider, id, name, installed) => {
                self.addon_menu_open = None;
                self.version_picker = Some(VersionPicker {
                    provider: provider.clone(),
                    id: id.clone(),
                    name,
                    installed,
                    builds: None,
                    problem: None,
                });
                Task::perform(
                    addonwork::versions(
                        self.http.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                    ),
                    |result| Message::VersionsListed(Box::new(result)),
                )
            }

            Message::VersionsListed(result) => {
                // Dropped on the floor if the picker was closed while the
                // request was out: an answer to a question nobody is asking
                // any more.
                if let Some(picker) = self.version_picker.as_mut() {
                    match *result {
                        Ok(builds) => picker.builds = Some(builds),
                        Err(e) => {
                            picker.builds = Some(Vec::new());
                            picker.problem = Some(e);
                        }
                    }
                }
                Task::none()
            }

            Message::CancelVersionPicker => {
                self.version_picker = None;
                Task::none()
            }

            Message::OpenGameSettings => {
                self.close_panels();
                self.game_settings = Some(GameSettings {
                    code: self.selected.code.clone(),
                    label: self.selected.label.clone(),
                    backups: None,
                    working: false,
                    confirm: None,
                    note: None,
                });
                self.list_backups()
            }

            Message::CloseGameSettings => {
                self.game_settings = None;
                Task::none()
            }

            Message::FromGameSettings(message) => {
                self.game_settings = None;
                self.update(*message)
            }

            Message::BackupsListed(result) => {
                // Dropped if the popup closed while the listing was out.
                if let Some(settings) = self.game_settings.as_mut() {
                    match result {
                        Ok(backups) => settings.backups = Some(backups),
                        Err(e) => {
                            settings.backups = Some(Vec::new());
                            settings.note = Some(Err(e));
                        }
                    }
                }
                Task::none()
            }

            Message::BackupNow => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                settings.working = true;
                settings.confirm = None;
                Task::perform(
                    tasks::create_backup(self.layout.clone(), settings.code.clone()),
                    Message::BackupFinished,
                )
            }

            Message::AskRestore(archive) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.confirm = Some((archive, BackupAction::Restore));
                }
                Task::none()
            }

            Message::AskDeleteBackup(archive) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.confirm = Some((archive, BackupAction::Delete));
                }
                Task::none()
            }

            Message::ConfirmDeleteBackup => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                let Some((archive, BackupAction::Delete)) = settings.confirm.take() else {
                    return Task::none();
                };
                settings.working = true;
                Task::perform(
                    tasks::delete_backup(self.layout.clone(), settings.code.clone(), archive),
                    Message::BackupFinished,
                )
            }

            Message::CancelRestore => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.confirm = None;
                }
                Task::none()
            }

            Message::ConfirmRestore => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                let Some((archive, BackupAction::Restore)) = settings.confirm.take() else {
                    return Task::none();
                };
                settings.working = true;
                Task::perform(
                    tasks::restore_backup(self.layout.clone(), settings.code.clone(), archive),
                    Message::BackupFinished,
                )
            }

            Message::BackupFinished(result) => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Task::none();
                };
                settings.working = false;
                settings.note = Some(result);
                // Either outcome can have changed the list: a restore adds
                // the backup of what it replaced.
                self.list_backups()
            }

            Message::ShowAddonDetail(provider, id, name) => {
                self.addon_menu_open = None;
                // Changelog first for something already installed, About for
                // something being browsed: the question is different in each
                // case, and the tab that answers it should not need a click.
                let installed = self.addon_is_installed(&provider, &id);
                self.addon_detail = Some(AddonDetail {
                    provider: provider.clone(),
                    id: id.clone(),
                    name,
                    loaded: None,
                    problem: None,
                    tab: match installed {
                        true => DetailTab::Changelog,
                        false => DetailTab::About,
                    },
                });
                Task::perform(
                    addonwork::detail(self.http.clone(), self.selected.code.clone(), provider, id),
                    |result| Message::AddonDetailLoaded(Box::new(result)),
                )
            }

            Message::AddonDetailLoaded(result) => {
                // Dropped if the card was closed while the request was out,
                // the same as the version picker: an answer to a question
                // nobody is asking any more.
                if let Some(detail) = self.addon_detail.as_mut() {
                    match *result {
                        Ok(loaded) => detail.loaded = Some(loaded),
                        Err(e) => detail.problem = Some(e),
                    }
                }
                Task::none()
            }

            Message::AddonDetailTab(tab) => {
                if let Some(detail) = self.addon_detail.as_mut() {
                    detail.tab = tab;
                }
                Task::none()
            }

            Message::CloseAddonDetail => {
                self.addon_detail = None;
                Task::none()
            }

            Message::InstallVersion(provider, id, file_id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.version_picker = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::install_version(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        file_id,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonPin(provider, id, file_id) => {
                self.addon_menu_open = None;
                let Some(options) = addonwork::manage_options(
                    &self.layout,
                    &self.selected.code,
                    self.config.addon_channel,
                ) else {
                    return Task::none();
                };
                if let Err(e) = manage::set_pinned(&options, &provider, &id, file_id) {
                    self.error = Some(Problem::of(&e));
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AddonAutoUpdateDefault(on) => {
                self.config.addon_auto_update = on;
                self.save_config();
                // Nothing is fetched here: the new default applies at the next
                // pass, which is also the first time it could matter.
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::AutoUpdateAddons => {
                // Never against a running game, which writes SavedVariables
                // beside the addons and loads them, and never on top of work
                // the user started: both write the same directory. A game
                // that was already running when Lightforge started is not a
                // launch of ours, so the prefix itself is asked as well.
                if self.addon_auto_running
                    || self.addons_busy
                    || self.launching.is_some()
                    || self.addon_auto_done.contains(&self.selected.code)
                {
                    return Task::none();
                }
                self.addon_auto_done.push(self.selected.code.clone());
                self.addon_auto_running = true;
                let (tx, _rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                Task::perform(
                    addonwork::auto_update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        self.config.addon_auto_update,
                        tx,
                    ),
                    |result| Message::AutoUpdateFinished(result.map(|r| r.map(Box::new))),
                )
            }

            Message::AutoUpdateFinished(result) => {
                self.addon_auto_running = false;
                let mut tasks = Vec::new();

                match result {
                    // Nothing to do, so nothing to say.
                    Ok(None) => {}
                    Ok(Some(report)) => {
                        if !report.installed.is_empty() {
                            self.addon_auto_note = Some(match report.installed.len() {
                                1 => format!("{} updated automatically", report.installed[0]),
                                n => format!("{n} addons updated automatically"),
                            });
                            tasks.push(Task::perform(
                                addonwork::list(
                                    self.layout.clone(),
                                    self.selected.code.clone(),
                                ),
                                |listing| Message::AddonsListed(Box::new(listing)),
                            ));
                        }
                    }
                    // Said quietly and only here: a launcher that cannot
                    // reach CurseForge is not a launcher that cannot play.
                    Err(e) => self.addon_auto_note = Some(format!("addon check failed: {e}")),
                }

                // Whoever was waiting on this can go now.
                if let Some(what) = self.launch_after_addons.take() {
                    tasks.push(self.preflight(what));
                }
                Task::batch(tasks)
            }

            Message::PauseWhilePlaying(on) => {
                self.config.pause_downloads_while_playing = on;
                self.save_config();
                Task::none()
            }

            Message::MinimizeOnLaunch(on) => {
                self.config.minimize_on_launch = on;
                self.save_config();
                Task::none()
            }

            Message::BattleNetMinimized(on) => {
                self.config.battle_net_minimized = on;
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
                    return Task::none();
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
                    return Task::none();
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
                        return Task::perform(
                            provision::status(self.layout.clone(), self.config.clone()),
                            |status| Message::SetupStatus(status.map(Box::new)),
                        );
                    }
                    Err(e) => self.proton_notice = Some(e),
                }
                Task::none()
            }

            Message::ProtonSizes(sizes) => {
                self.proton_sizes = sizes.into_iter().collect();
                Task::none()
            }

            Message::AddonsListed(listing) => {
                // A slow listing for the product shown before a switch.
                if listing.dir != self.layout.addons_dir(&self.selected.code) {
                    return Task::none();
                }
                if let Some(problem) = &listing.problem {
                    self.error = Some(format!("the addon list cannot be read: {problem}").into());
                }
                self.addons = *listing;
                Task::none()
            }

            Message::TokenChanged(value) => {
                self.token_input = value;
                Task::none()
            }

            Message::ImportToken => {
                if self.addons_locked() {
                    return Task::none();
                }
                let token = self.token_input.trim().to_owned();
                if token.is_empty() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Import;
                self.error = None;

                Task::perform(
                    addonwork::import(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        token,
                        self.config.addon_channel,
                        self.import_pinned,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::UpdateAddons => {
                if self.addons_locked() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::update(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::AddonsFinished(result) => {
                self.addons_busy = false;
                self.addon_rx = None;
                match result {
                    Ok(report) => {
                        self.addon_report = Some(*report);
                        // What was installed is no longer an update waiting.
                        let installed = &self.addon_report.as_ref().expect("just set").installed;
                        self.updates.retain(|u| !installed.contains(&u.name));
                        // The list on screen is now stale in both directions.
                        Task::perform(
                            addonwork::list(self.layout.clone(), self.selected.code.clone()),
                            |listing| Message::AddonsListed(Box::new(listing)),
                        )
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            Message::ArmRemoveAddon(provider, id) => {
                self.addon_remove_armed = Some((provider, id));
                Task::none()
            }

            Message::ExportAddons => {
                match addonwork::export(&self.layout, &self.selected.code) {
                    Ok(token) => {
                        self.export_token = token;
                        self.status = "export ready".to_owned();
                    }
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }

            Message::CopyExport => iced::clipboard::write(self.export_token.clone()),

            Message::ExportEdited(text) => {
                self.export_token = text;
                Task::none()
            }

            Message::CancelRemoveAddon => {
                self.addon_remove_armed = None;
                Task::none()
            }

            Message::AddonFilterChanged(text) => {
                self.addon_filter = text;
                Task::none()
            }

            Message::OpenAddonsDir => {
                if let Err(e) = tasks::open_path(self.addons.dir.clone()) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::RemoveAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addon_remove_armed = None;
                Task::perform(
                    addonwork::remove(
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                    ),
                    Message::AddonRemoved,
                )
            }

            Message::AddonRemoved(result) => {
                match result {
                    Ok(folders) => self.status = format!("removed {}", folders.join(", ")),
                    Err(e) => self.error = Some(e.into()),
                }
                Task::perform(
                    addonwork::list(self.layout.clone(), self.selected.code.clone()),
                    |listing| Message::AddonsListed(Box::new(listing)),
                )
            }

            Message::OpenUrl(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::AddonTab(tab) => {
                self.addon_tab = tab;
                self.error = None;
                // Leaving the list disarms it: a Remove waiting to be
                // confirmed must not survive out of sight of the row it
                // belongs to.
                self.addon_remove_armed = None;
                // Browse opens on the most-downloaded addons instead of on an
                // empty list: "what is there?" has a useful answer before
                // anybody types, and the catalogue it comes out of is already
                // on disk.
                match tab == AddonTab::Browse && self.search_input.trim().is_empty() {
                    true => self.popular_if_unasked(),
                    false => Task::none(),
                }
            }

            // A jump, not a page: measured first, because the sections above
            // it may have changed height since the page was last measured,
            // a Proton table that filled in or a drive that appeared.
            Message::SettingsTab(tab) => {
                self.settings_tab = tab;
                self.settings_jump = Some(tab);
                self.settings_jump_tries = 0;
                self.settings_anchor = None;
                measure_settings()
            }

            Message::SettingsScrolled(viewport) => {
                self.settings_viewport = Some(viewport);
                if let Some(anchor) = &self.settings_anchor {
                    let max = (viewport.content_bounds().height - viewport.bounds().height).max(0.0);
                    let at = |y: f32| (viewport.absolute_offset().y - y.min(max)).abs() < 1.0;
                    // Where the jump put the page, or where it had put it
                    // before the page grew and it moved again: both are
                    // the jump's own doing. So is anything reported in
                    // the moment before it lands, which is the page saying
                    // where it was. Anywhere else is a wheel or the
                    // scrollbar, and the index follows the reader again.
                    let ours = at(anchor.y) || anchor.prior.is_some_and(at);
                    if !ours {
                        if anchor.since.elapsed() < SETTINGS_JUMP_STALE {
                            return Task::none();
                        }
                        self.settings_anchor = None;
                    }
                }
                measure_settings()
            }

            Message::SettingsMeasured(layout) => {
                self.settings_layout = layout;
                if let Some(tab) = self.settings_jump {
                    let Some(y) = self.settings_layout.offset(tab) else {
                        // Asked for in the same update that opened the page,
                        // so measured against the screen it replaced. Again
                        // once the page has been drawn, but not forever: a
                        // section that never appears is not worth a timer.
                        self.settings_jump_tries += 1;
                        if self.settings_jump_tries > SETTINGS_JUMP_TRIES {
                            self.settings_jump = None;
                            return Task::none();
                        }
                        return Task::perform(tasks::after(Duration::from_millis(16)), |()| ())
                            .then(|()| measure_settings());
                    };
                    self.settings_jump = None;
                    self.settings_anchor = Some(SettingsAnchor { tab, y, prior: None, since: Instant::now() });
                    return iced::widget::operation::scroll_to(
                        SETTINGS_PAGE,
                        scrollable::AbsoluteOffset { x: 0.0, y },
                    );
                }
                // Held where the jump put it while the page fills in under
                // it: the drives and the Proton list arrive a moment after
                // the page opens, and every section below them moves down.
                if let Some(anchor) = &mut self.settings_anchor {
                    self.settings_tab = anchor.tab;
                    let Some(y) = self.settings_layout.offset(anchor.tab) else { return Task::none() };
                    if (y - anchor.y).abs() < 1.0 {
                        return Task::none();
                    }
                    anchor.prior = Some(anchor.y);
                    anchor.y = y;
                    return iced::widget::operation::scroll_to(
                        SETTINGS_PAGE,
                        scrollable::AbsoluteOffset { x: 0.0, y },
                    );
                }
                if let Some(viewport) = &self.settings_viewport {
                    if let Some(tab) = self.settings_layout.reading(viewport, self.settings_tab) {
                        self.settings_tab = tab;
                    }
                }
                Task::none()
            }

            Message::StorageMeasured(report) => {
                self.storage_measuring = false;
                self.storage = Some(*report);
                Task::none()
            }

            Message::InstallOffered(code, result) => {
                if let Ok(offerings) = result.as_ref() {
                    self.remember_optional(&code, offerings.optional.clone());
                }
                if let Some(dialog) = self.install_dialog.as_mut().filter(|d| d.code == code) {
                    dialog.stage = match *result {
                        Ok(offerings) => DialogStage::Ready(Box::new(offerings)),
                        Err(e) => DialogStage::Failed(e),
                    };
                }
                Task::none()
            }

            Message::InstallHighRes(on) => {
                if let Some(dialog) = self.install_dialog.as_mut() {
                    dialog.high_res = on;
                }
                Task::none()
            }

            Message::InstallTarget(index) => {
                if let Some(dialog) = self.install_dialog.as_mut() {
                    if index < dialog.targets.len() {
                        dialog.chosen = index;
                    }
                }
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
                    return Task::none();
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
                    return Task::none();
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
                    return Task::none();
                };
                let install = self.layout.wow_install_in(library);
                if self.downloading_into(&install) {
                    return Task::none();
                }
                // Compaction moves and deletes archives for minutes; nothing
                // else may write the store meanwhile, and the client may not
                // have it open. `pump` and pre-flight both hold while it runs.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Task::none();
                }
                let total = match self.unused.get(&id) {
                    Some(UnusedData::Ready(plan)) => plan.moved,
                    _ => return Task::none(),
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

            Message::InstallCancel => {
                self.install_dialog = None;
                Task::none()
            }

            Message::InstallStart => self.start_install(),

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

            Message::DowngradeCancel => {
                self.downgrade_dialog = None;
                Task::none()
            }

            Message::DowngradeConfirm => {
                let Some(dialog) = self.downgrade_dialog.take() else {
                    return Task::none();
                };
                // Remembered before the launch, so the retry below walks
                // straight past the guard it just answered.
                self.downgrade_ok.insert(dialog.pick);
                self.preflight(dialog.what)
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
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Task::none();
                }
                let Some(dialog) = self.uninstall_dialog.as_mut() else {
                    return Task::none();
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
                    return Task::none();
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

            Message::SearchChanged(value) => {
                self.search_input = value;
                self.search_seq += 1;
                let seq = self.search_seq;

                // Typing is the search. The button stays. It is how you ask
                // again after a provider was down, and pressing Enter has to
                // do something, but nobody should have to reach for it.
                //
                // Two letters is where a search stops matching half the
                // catalogue, and the pause is long enough that a word typed at
                // speed goes out once rather than letter by letter.
                // Emptying the box is not "no search", it is the question
                // the tab opened with: back to the popular list.
                if self.search_input.trim().is_empty() {
                    return self.popular_if_unasked();
                }
                if self.search_input.trim().chars().count() < SEARCH_MIN {
                    return Task::none();
                }
                Task::perform(
                    async move {
                        tokio::time::sleep(SEARCH_DEBOUNCE).await;
                        seq
                    },
                    Message::SearchSettled,
                )
            }

            Message::SearchSettled(seq) => {
                // A later keystroke has already scheduled its own search.
                match seq == self.search_seq && !self.searching {
                    true => self.update(Message::RunSearch),
                    false => Task::none(),
                }
            }

            Message::RunSearch => {
                // An empty query is not a no-op: it asks for the most
                // downloaded addons, which is what the tab shows before the
                // first keystroke.
                let query = self.search_input.trim().to_owned();
                self.searching = true;
                Task::perform(
                    addonwork::search(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        query,
                    ),
                    |outcome| Message::SearchDone(Box::new(outcome)),
                )
            }

            Message::SearchDone(outcome) => {
                self.searching = false;
                // Ignore a reply for a query the user has already moved past.
                if outcome.query == self.search_input.trim() {
                    self.search = *outcome;
                }
                Task::none()
            }

            Message::InstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = self.addon_tab;
                self.error = None;

                Task::perform(
                    addonwork::install_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::ReinstallAddon(provider, id) => {
                if self.addons_locked() {
                    return Task::none();
                }
                self.addon_menu_open = None;
                let (tx, rx) = tokio::sync::watch::channel(addonwork::Progress::default());
                self.addon_rx = Some(rx);
                self.addons_busy = true;
                self.addon_report = None;
                self.addon_report_tab = AddonTab::Mine;
                self.error = None;

                Task::perform(
                    addonwork::reinstall_one(
                        self.http.clone(),
                        self.layout.clone(),
                        self.selected.code.clone(),
                        provider,
                        id,
                        self.config.addon_channel,
                        tx,
                    ),
                    |result| Message::AddonsFinished(result.map(Box::new)),
                )
            }

            Message::CheckAddonUpdates => {
                let code = self.selected.code.clone();
                Task::perform(
                    addonwork::check_updates(
                        self.http.clone(),
                        self.layout.clone(),
                        code.clone(),
                        self.config.addon_channel,
                    ),
                    move |result| Message::AddonUpdatesChecked(code.clone(), result),
                )
            }

            Message::AddonUpdatesChecked(code, result) => {
                if code != self.selected.code {
                    return Task::none();
                }
                match result {
                    Ok(statuses) => self.updates = statuses,
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }

            Message::CopyDebug => {
                let action = self.action();
                let info = DebugInfo::gather(
                    &self.layout,
                    &self.selected.code,
                    &format!("{action:?}"),
                    tasks::proton_for(&self.selected.code).as_ref(),
                );
                self.status = "debug info copied".to_owned();
                iced::clipboard::write(info.render())
            }

            Message::SystemInfo(info) => {
                self.system = Some(*info);
                Task::none()
            }

            Message::OpenReport(kind) => {
                self.close_panels();
                // Gathered here because this is where the four things
                // `DebugInfo::gather` needs are in scope, and gathered once:
                // the card shows exactly the text that gets sent, not a
                // second rendering of it.
                let shown: Vec<String> = match kind {
                    ReportKind::Bug => {
                        self.history.iter().map(|line| lightforge_core::debug::scrub(line)).collect()
                    }
                    ReportKind::Feedback => Vec::new(),
                };
                let (debug, tags) = match kind {
                    ReportKind::Bug => {
                        let action = self.action();
                        let mut info = DebugInfo::gather(
                            &self.layout,
                            &self.selected.code,
                            &format!("{action:?}"),
                            tasks::proton_for(&self.selected.code).as_ref(),
                        );
                        // What the renderer picked is the one thing core
                        // cannot know, so the GUI fills it in here.
                        if let Some(system) = &self.system {
                            info.graphics = Some(system.graphics_adapter.clone());
                            info.graphics_backend = Some(system.graphics_backend.clone());
                            info.cpu = Some(system.cpu_brand.clone());
                            info.memory = Some(system.memory_total);
                        }
                        let tags = report_tags(&info);
                        // The recent status lines go into the shown block as
                        // well as into the breadcrumbs, because the card
                        // promises that this is what gets sent, and they are
                        // scrubbed like everything else, since a failure
                        // message can carry a path.
                        let mut text = info.render();
                        if !shown.is_empty() {
                            text.push_str("\n--- recent activity ---\n");
                            for line in &shown {
                                text.push_str(line);
                                text.push('\n');
                            }
                        }
                        (text, tags)
                    }
                    ReportKind::Feedback => (String::new(), Vec::new()),
                };
                self.report_dialog = Some(ReportDialog {
                    kind,
                    note: iced::widget::text_editor::Content::new(),
                    category: None,
                    tags,
                    shot: None,
                    // Remembered between reports: someone who gave it once
                    // should not have to type it again to answer a follow-up.
                    contact: self.config.contact.clone().unwrap_or_default(),
                    base: debug.clone(),
                    debug,
                    history: shown,
                    stage: ReportStage::Writing,
                });
                Task::none()
            }

            Message::ReportNote(action) => {
                // `text_editor` has no "read only" of its own, so a report on
                // its way out is held here instead.
                if let Some(dialog) = self
                    .report_dialog
                    .as_mut()
                    .filter(|d| !matches!(d.stage, ReportStage::Sending | ReportStage::Sent))
                {
                    // Editing is how someone gets out of the cooldown: the
                    // text that was refused is no longer the text they have.
                    if dialog.stage == ReportStage::TooSoon && action.is_edit() {
                        dialog.stage = ReportStage::Writing;
                    }
                    dialog.note.perform(action);
                }
                Task::none()
            }

            Message::ReportShotPick => Task::perform(
                // The desktop's own dialog, asked for over the XDG portal.
                //
                // Spoken directly rather than through a file-dialog crate:
                // `rfd` drives the portal through `libdbus` on a thread of its
                // own, and inside this process that call never returns, the
                // dialog does not appear and the thread sits in a futex. The
                // same call from a bare test binary works, so it is this
                // process it dislikes, not the machine. `ashpd` is pure Rust
                // over the `zbus` connection iced already builds.
                async {
                    let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
                        .title("Choose a screenshot")
                        .modal(true)
                        .multiple(false)
                        .filter(
                            ashpd::desktop::file_chooser::FileFilter::new("Images")
                                .mimetype("image/png")
                                .mimetype("image/jpeg")
                                .mimetype("image/webp")
                                .mimetype("image/bmp"),
                        );

                    let files = request.send().await.ok()?.response().ok()?;
                    // Cancelling comes back as an empty list, which is not an
                    // error and means nothing happened. `to_file_path` rather
                    // than the URL's own `path`, which is still percent-encoded,
                    // and "Screenshot 2026-10-09.png" is the normal case.
                    files.uris().first().and_then(|uri| uri.to_file_path().ok())
                },
                Message::ReportShotPicked,
            ),

            Message::ReportShotPicked(path) => {
                // Cancelling is the common answer and means nothing happened.
                let Some(path) = path else { return Task::none() };
                Task::perform(
                    async move {
                        tokio::task::spawn_blocking(move || art::load_shot(&path))
                            .await
                            .unwrap_or_else(|e| Err(chain(&e)))
                            .map(Box::new)
                    },
                    Message::ReportShotLoaded,
                )
            }

            Message::ReportShotLoaded(result) => {
                match result {
                    Ok(shot) => {
                        if let Some(dialog) = self.report_dialog.as_mut() {
                            dialog.shot = Some(*shot);
                        }
                    }
                    Err(e) => self.status = format!("that file is not a screenshot: {e}"),
                }
                Task::none()
            }

            Message::ReportCopy => {
                let Some(dialog) = self.report_dialog.as_ref() else {
                    return Task::none();
                };
                // What they wrote first, then the diagnostics: pasted into a
                // chat window it reads as a report rather than a dump.
                let mut text = dialog.note.text();
                if !dialog.debug.is_empty() {
                    text.push('\n');
                    text.push_str(&dialog.debug);
                }
                self.status = "report copied".to_owned();
                iced::clipboard::write(text)
            }

            Message::ReportShotCleared => {
                if let Some(dialog) = self.report_dialog.as_mut() {
                    dialog.shot = None;
                }
                Task::none()
            }

            Message::ReportContact(contact) => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Task::none();
                };
                dialog.contact = contact;
                let contact = dialog.contact.trim().to_owned();
                self.config.contact = (!contact.is_empty()).then_some(contact);
                self.save_config();
                Task::none()
            }

            Message::ReportCategory(category) => {
                // Gathered before the dialog is borrowed: the attachment reads
                // the addon folder, the queue and the config off `self`.
                let attached = match self.report_dialog.as_ref().map(|d| d.kind) {
                    Some(ReportKind::Bug) => self.report_attachment(category),
                    _ => String::new(),
                };
                if let Some(dialog) = self
                    .report_dialog
                    .as_mut()
                    .filter(|d| !matches!(d.stage, ReportStage::Sending | ReportStage::Sent))
                {
                    dialog.category = Some(category);
                    if dialog.kind == ReportKind::Bug {
                        dialog.debug = format!("{}{attached}", dialog.base);
                    }
                }
                Task::none()
            }

            Message::ReportSend => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Task::none();
                };
                let note = dialog.note.text();
                // From a failed send too: that is the "Try again" button.
                let ready = matches!(dialog.stage, ReportStage::Writing | ReportStage::Failed);
                let Some(category) = dialog.category.filter(|_| ready && dialog.can_send()) else {
                    return Task::none();
                };
                // The same words twice in five minutes is the Send button
                // being pressed again, not a second problem.
                let repeat = self
                    .last_report
                    .as_ref()
                    .is_some_and(|(last, when)| last == &note && when.elapsed() < REPORT_COOLDOWN);
                if repeat {
                    dialog.stage = ReportStage::TooSoon;
                    return Task::none();
                }

                dialog.stage = ReportStage::Sending;
                let report = report::Report {
                    kind: match dialog.kind {
                        ReportKind::Bug => report::Kind::Bug(dialog.debug.clone()),
                        ReportKind::Feedback => report::Kind::Feedback,
                    },
                    category,
                    note: note.clone(),
                    contact: dialog.contact.clone(),
                    tags: dialog.tags.clone(),
                    shot: dialog.shot.as_ref().map(|shot| report::Shot {
                        name: shot.name.clone(),
                        bytes: shot.bytes.clone(),
                        media_type: shot.media_type,
                    }),
                    history: dialog.history.clone(),
                };
                Task::perform(report::send(report, self.http.clone()), Message::ReportSent)
            }

            Message::ReportSent(arrived) => {
                if let Some(dialog) = self.report_dialog.as_mut() {
                    dialog.stage = match arrived {
                        true => ReportStage::Sent,
                        false => ReportStage::Failed,
                    };
                    // Only a report that arrived counts against the cooldown:
                    // one that failed has to be sendable again at once.
                    if arrived {
                        self.last_report = Some((dialog.note.text(), Instant::now()));
                    }
                }
                self.status = match arrived {
                    true => "report sent".to_owned(),
                    false => "the report could not be sent".to_owned(),
                };
                Task::none()
            }

            Message::ReportCancel => {
                self.report_dialog = None;
                Task::none()
            }

            Message::ShowSettingsTab(tab) => {
                self.settings_jump = Some(tab);
                self.update(Message::Show(Screen::Settings))
            }

            Message::Tweak(scope, which, on) => {
                self.edit_tweaks(scope, |tweaks| match which {
                    // Stored as an explicit refusal rather than a bare `false`:
                    // the default is on wherever the tool exists, so "off" has
                    // to be recorded to survive a restart.
                    Tweak::GamePerformance => tweaks.game_performance = Some(on),
                    Tweak::GameMode => tweaks.gamemode = on,
                    Tweak::MangoHud => tweaks.mangohud = on,
                    Tweak::SkipAddonProfiling => tweaks.skip_addon_profiling = on,
                    Tweak::ReduceRaidEffects => tweaks.reduce_raid_effects = on,
                    Tweak::FasterZoneLoads => tweaks.faster_zone_loads = on,
                    Tweak::RawMouse => tweaks.raw_mouse = on,
                    Tweak::NoScreenEffects => tweaks.no_screen_effects = on,
                    Tweak::MaxDrawDistance => tweaks.max_draw_distance = on,
                    Tweak::NoClothPhysics => tweaks.no_cloth_physics = on,
                    Tweak::SoftwareCursor => tweaks.software_cursor = on,
                    Tweak::ClipCursor => tweaks.clip_cursor = on,
                    Tweak::NoMouseAcceleration => tweaks.no_mouse_acceleration = on,
                    Tweak::ShowCursorInRelativeMode => {
                        tweaks.show_cursor_in_relative_mode = on
                    }
                    Tweak::Controller => tweaks.controller = on,
                    Tweak::Wayland => tweaks.wayland = on,
                    Tweak::Hdr => tweaks.hdr = on,
                    Tweak::BigShaderCache => tweaks.big_shader_cache = on,
                });
                Task::none()
            }

            Message::TweakPreset(preset) => {
                self.edit_tweaks(Scope::Product, |tweaks| preset.apply(tweaks));
                Task::none()
            }

            Message::TweakCustom => {
                self.edit_tweaks(Scope::Product, Tweaks::clear_managed);
                Task::none()
            }

            Message::TweakEnvToggle(scope) => {
                match scope {
                    Scope::Product => self.env_open = !self.env_open,
                    Scope::BattleNet => self.bnet_env_open = !self.bnet_env_open,
                }
                Task::none()
            }

            Message::TweakApi(ApiChoice(api)) => {
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.graphics_api = api);
                Task::none()
            }

            Message::TweakFpsCap(value) => {
                // Digits only: a frame cap that is not a number is not a cap.
                // Clearing it is how you say uncapped.
                self.fps_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap = cap);
                Task::none()
            }

            Message::TweakFpsCapBackground(value) => {
                self.fps_bk_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_bk_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap_background = cap);
                Task::none()
            }

            Message::TweakWaylandMonitor(scope, value) => {
                // A connector name, so no spaces: `DP-1`, `HDMI-A-1`.
                let field = match scope {
                    Scope::Product => &mut self.monitor_input,
                    Scope::BattleNet => &mut self.bnet_monitor_input,
                };
                *field = value.chars().filter(|c| !c.is_whitespace()).collect();
                let name = (!field.is_empty()).then(|| field.clone());
                self.edit_tweaks(scope, |tweaks| tweaks.wayland_monitor = name);
                Task::none()
            }

            Message::TweakEnvKey(value) => {
                // No spaces or `=`: the two halves are separate fields here,
                // and a key holding either would not survive the environment.
                self.env_key_input =
                    value.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
                Task::none()
            }

            Message::TweakEnvValue(value) => {
                self.env_value_input = value;
                Task::none()
            }

            Message::TweakEnvAdd(scope) => {
                let key = std::mem::take(&mut self.env_key_input);
                let value = std::mem::take(&mut self.env_value_input);
                if !key.is_empty() {
                    self.edit_tweaks(scope, |tweaks| {
                        tweaks.env.insert(key, value);
                    });
                }
                Task::none()
            }

            Message::TweakEnvRemove(scope, key) => {
                self.edit_tweaks(scope, |tweaks| {
                    tweaks.env.remove(&key);
                });
                Task::none()
            }

            Message::OptionalContent(name, on) => {
                self.set_optional(&self.selected.code.clone(), &name, on);
                Task::none()
            }

            Message::TogglePicker(key) => {
                self.picker_open = match self.picker_open.as_ref() == Some(&key) {
                    true => None,
                    false => Some(key),
                };
                Task::none()
            }

            Message::Picked(message) => {
                self.picker_open = None;
                self.update(*message)
            }

            Message::ToggleGear => {
                self.gear_open = !self.gear_open;
                Task::none()
            }

            Message::Dismiss => {
                self.close_panels();
                Task::none()
            }

            Message::Nav(from, input) => {
                let now = Instant::now();
                trace::pad(format_args!("nav {from:?} {input:?} focused={}", self.window_focused));
                if from == NavFrom::Pad && !self.window_focused {
                    return Task::none();
                }
                // Steam can deliver one press both ways: as keys from its
                // layout, and raw from a pad it did not name as translated.
                // Whichever arrives first keeps the floor while presses keep
                // coming, so neither a tap nor a held repeat counts twice.
                if let Some((last, at)) = self.nav_last {
                    if last != from && now.duration_since(at) < NAV_DOUBLED {
                        trace::pad("  dropped: doubled");
                        return Task::none();
                    }
                }
                self.nav_last = Some((from, now));
                // A field with the cursor in it takes a pad's buttons the way
                // it takes the keys: arrows in a field are the field's, and
                // only Escape gets past it. So B leaves the field and stays
                // on it, A asks for the keyboard again, and an arrow leaves
                // and goes on from there.
                if from == NavFrom::Pad && !self.keys_belong_elsewhere() {
                    if let Some(field) = focus::typing_in() {
                        // The ring can be somewhere else by now, moved off
                        // the field while it kept the cursor: A is then for
                        // what the ring is on.
                        let on_field = self.focus.as_ref().is_none_or(|f| *f == field);
                        match input {
                            pad::Input::Back => {
                                self.focus = Some(field.clone());
                                focus::leave(field);
                                self.steam_keyboard_closed();
                                return Task::none();
                            }
                            pad::Input::Press if on_field => {
                                self.focus = Some(field.clone());
                                self.steam_keyboard(&field);
                                return Task::none();
                            }
                            pad::Input::Press => focus::leave(field),
                            pad::Input::Step(_) => {
                                if on_field {
                                    self.focus = Some(field.clone());
                                }
                                focus::leave(field);
                            }
                        }
                    }
                }
                self.update(match input {
                    pad::Input::Step(step) => Message::FocusStep(step),
                    pad::Input::Press => Message::FocusPress,
                    pad::Input::Back => Message::FocusBack,
                })
            }

            Message::TypingDone(field) => {
                self.focus = Some(field);
                self.steam_keyboard_closed();
                Task::none()
            }

            // Sizes arrive in points, converted with whatever scale iced had
            // applied by then, and a reading taken across a scale change
            // carries the old one (measured: 1080 px read back as 1458 and a
            // scale of 1.8). So each reading is tagged with the generation it
            // was asked under, a stale one is asked for again, and a fresh
            // one, a moment after the change has been applied, times
            // `ui_scale` is the pixels.
            Message::WindowResized => {
                let generation = self.ui_scale_gen;
                Task::perform(
                    async { tokio::time::sleep(std::time::Duration::from_millis(50)).await },
                    |()| (),
                )
                .then(|()| iced::window::oldest())
                .and_then(iced::window::size)
                .map(move |size| Message::WindowHeight(generation, size.height))
            }

            Message::WindowHeight(generation, _) if generation != self.ui_scale_gen => {
                Task::done(Message::WindowResized)
            }

            Message::WindowHeight(_, points) => {
                let scale = fit::scale_for(points * self.ui_scale);
                if scale != self.ui_scale {
                    crate::trace::mark(format!("gamescope: ui scale {} -> {scale}", self.ui_scale));
                    self.ui_scale = scale;
                    self.ui_scale_gen += 1;
                }
                Task::none()
            }

            Message::WindowFocused(focused) => {
                self.window_focused = focused;
                Task::none()
            }

            Message::FocusStep(step) => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                let spots = focus::live();
                // The first press only shows where the ring is.
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return match self.focus {
                        Some(_) => Task::none(),
                        // Nothing to select, only something to read.
                        None => self.scroll_page(step, None),
                    };
                };
                match focus::step(&spots, &current, step) {
                    Some(next) => {
                        Self::scroll_into_view(&next);
                        self.focus = Some(next);
                        Task::none()
                    }
                    None => self.scroll_page(step, Some(&current)),
                }
            }

            Message::FocusPress => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                let spots = focus::live();
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return Task::none();
                };
                // A Settings tab is a way in as much as a choice: the ring
                // goes on to the page it opens, and Escape brings it back.
                if let focus::Target::SettingsTab(_) = current {
                    self.focus_into = Some(SETTINGS_PAGE);
                }
                self.focus_pressed_at =
                    spots.iter().find(|p| p.target == current).map(|p| (p.bounds, self.screen));
                let pressed = match focus::spot(&current).map(|spot| spot.press) {
                    Some(focus::Press::Send(message)) => self.update(message),
                    Some(focus::Press::Click) => {
                        // A controller has no keys to type with. Steam has a
                        // keyboard of its own, and this is how to ask for it.
                        if focus::spot(&current).is_some_and(|spot| spot.field) {
                            self.steam_keyboard(&current);
                        }
                        focus::click(current);
                        Task::none()
                    }
                    Some(focus::Press::Off) | None => return Task::none(),
                };
                // Whatever the press opened or closed, the ring follows once
                // it has been drawn.
                self.focus_entering = FOCUS_ENTER_FRAMES;
                Task::batch([pressed, focus_settle()])
            }

            Message::FocusSettle => {
                if self.focus_entering == 0 {
                    return Task::none();
                }
                self.focus_entering -= 1;
                if let Some(region) = self.focus_into {
                    let mut places = focus::live_in(region);
                    // Into the section just jumped to, not whatever was at the
                    // top before the jump: it takes a frame or two to scroll
                    // into view, and these retries are what wait it out.
                    // Compared in layout coordinates, the ones the sections
                    // were measured in.
                    if region == SETTINGS_PAGE {
                        if let Some(section) = self.settings_layout.bounds(self.settings_tab) {
                            places.retain(|place| {
                                focus::spot(&place.target)
                                    .is_some_and(|spot| section.contains(spot.bounds.center()))
                            });
                        }
                    }
                    if let Some(first) = focus::first(&places) {
                        self.focus_into = None;
                        self.focus_entering = 0;
                        self.focus = Some(first);
                        return Task::none();
                    }
                }
                let spots = focus::live();
                let drawn = |target: &focus::Target| spots.iter().any(|p| &p.target == target);
                match self.focus.clone() {
                    // Covered by a menu or a dialog that has just opened, or
                    // gone with the one that just closed: on to what is on
                    // top now. Only a control still there under the cover is
                    // somewhere to come back to; one that is gone, Install
                    // inside a dialog once pressed, leaves the way back as
                    // it was.
                    Some(was) if !drawn(&was) && !spots.is_empty() => {
                        let covered = focus::spot(&was).is_some();
                        let at = self
                            .focus_pressed_at
                            .take()
                            .filter(|(_, screen)| *screen == self.screen)
                            .map(|(at, _)| at);
                        self.focus = match (covered, at) {
                            // Pressed and gone, on the same page, with nothing
                            // open that it leads back to: whatever is now
                            // nearest to where it was, which after Keep is the
                            // Remove beside it.
                            (false, Some(at)) if self.focus_return.as_ref().is_none_or(|t| !drawn(t)) => {
                                focus::nearest(&spots, at)
                            }
                            _ => self.focus_start(&spots),
                        };
                        if covered {
                            self.focus_return = Some(was);
                        }
                        self.focus_entering = 0;
                        Task::none()
                    }
                    Some(_) => focus_settle(),
                    None => Task::none(),
                }
            }

            Message::FocusBack => {
                if self.keys_belong_elsewhere() {
                    return Task::none();
                }
                // Out of a Settings page, back onto its tab in the list.
                let in_page = self.focus.as_ref().and_then(focus::spot).and_then(|spot| spot.region)
                    == Some(SETTINGS_PAGE);
                if self.screen == Screen::Settings
                    && in_page
                    && self.dialog_dismissal().is_none()
                    && !self.panel_open()
                {
                    self.focus = Some(focus::Target::SettingsTab(self.settings_tab));
                    return Task::none();
                }
                let closed = match (self.dialog_dismissal(), self.panel_open()) {
                    (Some(dismiss), _) => self.update(dismiss),
                    // Out of Settings from its list of sections: back to Play,
                    // wherever Settings was opened from. Its pages are left
                    // for the list the same way, above, so B twice from any
                    // switch is the dashboard.
                    (None, false) if self.screen == Screen::Settings && self.wizard.is_none() => {
                        self.update(Message::Show(Screen::Dashboard))
                    }
                    // Setup has no pages behind it: Escape is its Back, and
                    // never its way out.
                    (None, false) if self.wizard.is_some() => {
                        self.update(Message::Wizard(wizard::Msg::Back))
                    }
                    (None, true) => {
                        self.close_panels();
                        Task::none()
                    }
                    (None, false) if !self.visited.is_empty() => self.update(Message::GoBack),
                    (None, false) => return Task::none(),
                };
                self.focus_entering = FOCUS_ENTER_FRAMES;
                Task::batch([closed, focus_settle()])
            }

            Message::FocusHidden => {
                if let Some(was) = self.focus.take() {
                    self.focus_resume = Some(was);
                }
                self.focus_return = None;
                self.focus_entering = 0;
                Task::none()
            }

            Message::MenuClick => Task::none(),

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
                            return Task::none();
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

            Message::ScanAndRepair => {
                self.gear_open = false;
                // It drops damaged entries from the indices, so nothing else
                // may be writing the store, and the client may not have it
                // open.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Task::none();
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Task::none();
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
                    return Task::none();
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
                    return Task::none();
                }
                if let Some(what) = self.maintenance() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Task::none();
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
                    return Task::none();
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
                    return self.pump();
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
                    return Task::none();
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
        }
    }
}
