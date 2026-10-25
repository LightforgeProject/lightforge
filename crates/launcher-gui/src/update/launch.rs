//! PLAY and its neighbours: launches, Battle.net hand-offs, pre-flight, the install dialog.

use crate::*;

impl Lightforge {
    pub(crate) fn update_launch(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
            Message::GameWatch => {
                if !(self.busy && self.launching.is_some()) {
                    return Ok(Task::none());
                }
                Task::perform(tasks::game_running(self.layout.clone()), Message::GameSeen)
            }
            Message::GameSeen(up) => {
                // A scan that was out when the session ended must not bring
                // the button back.
                self.game_up = up && self.busy && self.launching.is_some();
                Task::none()
            }
            Message::AskStopGame => {
                if !self.game_up || self.stopping {
                    return Ok(Task::none());
                }
                let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());
                let label = self
                    .choices
                    .iter()
                    .find(|c| c.code == code)
                    .map(|c| c.label.clone())
                    .unwrap_or_else(|| code.as_str().to_owned());
                self.stop_dialog = Some(StopDialog { label });
                Task::none()
            }
            Message::StopGameDismiss => {
                self.stop_dialog = None;
                Task::none()
            }
            Message::StopGameConfirm => {
                self.stop_dialog = None;
                if !self.game_up || self.stopping {
                    return Ok(Task::none());
                }
                self.stopping = true;
                self.status = "stopping the game…".to_owned();
                Task::perform(tasks::stop_game(self.layout.clone()), Message::GameStopped)
            }
            Message::GameStopped(result) => {
                // The launch's own watcher sees the process go and sends
                // `Launched`, which is what hands PLAY back; this only
                // reports how the stop went.
                self.stopping = false;
                self.game_up = false;
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }
            Message::Launched(result) => {
                self.game_up = false;
                self.stopping = false;
                self.stop_dialog = None;
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
            Message::PreflightDone(what, result) => {
                if let Err(e) = result {
                    self.busy = false;
                    self.launching = None;
                    self.error = Some(e);
                    return Ok(self.pump());
                }
                // Pre-flight trips the paused downloads' tokens and returns;
                // the runs themselves are still flushing what was in flight
                // and checkpointing their indices. The client opening the
                // store under them is the overlap this whole gate exists for.
                if self.launch_must_wait() {
                    self.launch_when_quiet = Some(what);
                    self.status = "pre-flight… waiting for the download to pause".to_owned();
                    return Ok(Task::none());
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
            Message::InstallCancel => {
                self.install_dialog = None;
                Task::none()
            }
            Message::InstallStart => self.start_install(),
            Message::DowngradeCancel => {
                self.downgrade_dialog = None;
                Task::none()
            }
            Message::DowngradeConfirm => {
                let Some(dialog) = self.downgrade_dialog.take() else {
                    return Ok(Task::none());
                };
                // Remembered before the launch, so the retry below walks
                // straight past the guard it just answered.
                self.downgrade_ok.insert(dialog.pick);
                self.preflight(dialog.what)
            }

            other => return Err(other),
        })
    }
}
