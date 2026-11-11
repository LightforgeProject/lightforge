//! PLAY and its neighbours: launches, Battle.net hand-offs, pre-flight, the install dialog.

use crate::*;

impl Lightforge {
    pub(crate) fn update_launch(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_play(message)
            .or_else(|m| self.update_battle_net_launch(m))
            .or_else(|m| self.update_client(m))
            .or_else(|m| self.update_install_dialog(m))
    }

    /// Play and Stop, and watching the game in between.
    fn update_play(&mut self, message: Message) -> Result<Task<Message>, Message> {
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

            Message::GameStarted(result) => match result {
                Ok(tasks::Started::Up { game, generation }) => self.game_up(game, Some(generation)),
                Ok(tasks::Started::Over(note)) => self.update(Message::Launched(Ok(note))),
                Err(e) => self.update(Message::Launched(Err(e))),
            },

            Message::GameExited(code, result) => {
                self.playing.retain(|s| s.code != code);
                if self.stop_dialog.as_ref().is_some_and(|d| d.code == code) {
                    self.stop_dialog = None;
                }
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                self.after_play(code)
            }

            Message::AskStopGame => {
                let code = self.selected.code.clone();
                if !self.session(&code).is_some_and(|s| !s.stopping) {
                    return Ok(Task::none());
                }
                self.stop_dialog = Some(StopDialog { label: self.label_of(&code), code });
                Task::none()
            }

            Message::StopGameDismiss => {
                self.stop_dialog = None;
                Task::none()
            }

            Message::StopGameConfirm => {
                let Some(dialog) = self.stop_dialog.take() else {
                    return Ok(Task::none());
                };
                let Some(session) = self.playing.iter_mut().find(|s| s.code == dialog.code && !s.stopping) else {
                    return Ok(Task::none());
                };
                session.stopping = true;
                let game = session.game.clone();
                self.status = "stopping the game…".to_owned();
                let code = dialog.code;
                Task::perform(tasks::stop_game(self.layout.clone(), game), move |result| {
                    Message::GameStopped(code.clone(), result)
                })
            }

            Message::GameStopped(code, result) => {
                // The exit watcher's `GameExited` hands PLAY back; this only reports.
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => {
                        // Still up: STOP is offered again.
                        if let Some(session) = self.playing.iter_mut().find(|s| s.code == code) {
                            session.stopping = false;
                        }
                        self.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::Launched(result) => {
                self.busy = false;
                let launched = self.launching.take().unwrap_or_else(|| self.selected.code.clone());
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                self.after_play(launched)
            }
            other => return Err(other),
        })
    }

    /// Going through Battle.net: hand-offs, the client itself, and the pre-flight before it.
    fn update_battle_net_launch(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::HandoffStarted(result) => {
                match result {
                    // The paused queue lasts the session, as on PLAY.
                    Ok(tasks::Handoff::Playing(game)) => {
                        // Minimized only now, not when the hand-off started: the
                        // client's window and its sign-in are meant to be seen.
                        // The client stays up, so the prefix is not settled after.
                        // Battle.net started the game, so the launch options
                        // cannot go around it: they run beside it, `%command%`
                        // waiting for it. Read before `game_up` takes the code.
                        let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());
                        let line = self.config.product(&code).map(|p| p.profile.launch_options.clone()).unwrap_or_default();
                        let beside = Task::future(tasks::run_beside(line, code, game.clone())).discard();
                        Task::batch([self.game_up(game, None), self.minimize_if_asked(), beside])
                    }
                    // Closed while signing in, or quit in the middle of the launch.
                    Ok(tasks::Handoff::ClientClosed) => {
                        self.launch_failed("Battle.net closed before the game could start — press Play again")
                    }
                    // Waiting on a person (two-factor, an update); left running,
                    // and said where: a minimized client is in the tray.
                    Ok(tasks::Handoff::NeedsAttention { minimized: true }) => self.launch_failed(
                        "Battle.net needs attention — open it from the tray, finish there and press Play",
                    ),
                    Ok(tasks::Handoff::NeedsAttention { minimized: false }) => {
                        self.launch_failed("Battle.net needs attention — finish in its window and press Play there")
                    }
                    // Nothing went wrong that the user could act on: start it
                    // again and send the same launch. The button stays held.
                    Ok(tasks::Handoff::ClientUpdated) if self.config.battle_net_in_background => {
                        if self.client_update_retries >= 2 {
                            return Ok(self.launch_failed("Battle.net keeps restarting — open it to finish its update"));
                        }
                        self.client_update_retries += 1;
                        self.status = "Battle.net updated itself, starting it again…".to_owned();
                        let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());
                        self.launch_after_client = Some((Launch::BattleNet, code.clone()));
                        let warm = self.warm_client();
                        // Nothing to wait for: the hand-off starts the client itself.
                        match self.client_phase == ClientPhase::SigningIn {
                            true => warm,
                            false => {
                                self.launch_after_client = None;
                                self.preflight_for(Launch::BattleNet, code)
                            }
                        }
                    }
                    Ok(tasks::Handoff::ClientUpdated) => {
                        self.launch_failed("Battle.net updated itself — press Play again")
                    }
                    Ok(tasks::Handoff::NeedsUpdate) => {
                        let code = self.launching.clone().unwrap_or_else(|| self.selected.code.clone());
                        match (Kind::of(&code), self.config.battle_net_in_background) {
                            // Its store is ours: Agent must not patch it.
                            (Kind::Managed, _) => {
                                self.launch_failed("Battle.net wants to update this game — press Play to start it here")
                            }
                            (Kind::HandOff, false) => {
                                self.launch_failed("Battle.net has an update for this game — open Battle.net to install it")
                            }
                            // The lock is released: an update can take an hour,
                            // and another game can be played meanwhile.
                            (Kind::HandOff, true) => {
                                self.busy = false;
                                self.launching = None;
                                let update = self.start_client_update(code.clone());
                                let resumed = self.after_play(code);
                                // After `after_play`, whose reload says "checking…".
                                self.status = "updating in Battle.net…".to_owned();
                                Task::batch([update, resumed])
                            }
                        }
                    }
                    Ok(tasks::Handoff::Ignored) => {
                        self.launch_failed("Battle.net did not start the game — open Battle.net to see why")
                    }
                    // Signed in, told to launch, and nothing came up.
                    Ok(tasks::Handoff::NothingStarted) => {
                        self.launch_failed("Battle.net is open — no game started")
                    }
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
                let cleared = match result {
                    Ok(cleared) => cleared,
                    Err(e) => {
                        self.busy = false;
                        self.launching = None;
                        self.error = Some(e);
                        return Ok(self.pump());
                    }
                };
                // Paused runs still checkpoint; the client must not open the store yet.
                if self.launch_must_wait() {
                    self.launch_when_quiet = Some((what, cleared));
                    self.status = "pre-flight… waiting for the download to pause".to_owned();
                    return Ok(Task::none());
                }
                self.start_launch(what, cleared)
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
            other => return Err(other),
        })
    }

    /// The Battle.net client itself while it runs: seeing it, showing it,
    /// quitting it.
    fn update_client(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ClientWatch => self.check_client(),

            Message::ClientSeen { up, game } => {
                let vanished = self.client_up && !up;
                self.client_up = up;
                if !up {
                    self.client_menu_open = false;
                    // Quit before anyone signed it in.
                    if self.client_phase == ClientPhase::NeedsSignIn {
                        self.client_phase = ClientPhase::Settled;
                    }
                }
                if !up {
                    self.update_pending = None;
                }
                // `|`, not `||`: both are taken, or one answers again next poll.
                if !game && self.client_may_return() && (tasks::take_client_owed() | self.take_cli_owed()) {
                    return Ok(self.warm_client());
                }
                // Not during a launch, which reads the same log itself.
                if vanished && self.config.battle_net_in_background && !self.busy {
                    return Ok(Task::perform(
                        tasks::note_self_update(self.layout.clone(), self.config.prefix_user.clone()),
                        |()| Message::ClientWatch,
                    ));
                }
                match up {
                    true => self.check_update_pending(),
                    false => Task::none(),
                }
            }

            Message::ClientUpdatePending(code, pending) => {
                self.update_pending = pending.then_some(code);
                Task::none()
            }

            Message::StopWatchingClientUpdate => {
                if let Some(update) = &self.client_update {
                    update.stop.cancel();
                }
                Task::none()
            }

            Message::ClientUpdateDone(code, result) => {
                self.client_update = None;
                self.update_pending = None;
                match result {
                    // Asked for by pressing Play, so it starts, unless the user
                    // has since gone to another game or started something else.
                    Ok(tasks::ClientUpdated::Completed) if self.selected.code == code && !self.busy => {
                        self.preflight(Launch::BattleNet)
                    }
                    Ok(tasks::ClientUpdated::Completed) => {
                        self.status = "the update is done".to_owned();
                        Task::none()
                    }
                    Ok(tasks::ClientUpdated::StoppedWatching) => {
                        self.status = "Battle.net carries on updating".to_owned();
                        Task::none()
                    }
                    Err(e) => {
                        self.error = Some(e);
                        Task::none()
                    }
                }
            }

            // Overtaken by a newer warm-up, which answers for the client now.
            Message::BackgroundClient(generation, _) if generation != self.client_generation => Task::none(),

            Message::BackgroundClient(generation, result) => {
                self.warming = None;
                let waiting = self.launch_after_client.take();
                match result {
                    Ok(tasks::Warmed::NeedsSignIn { started }) => {
                        // The first answer: watch for someone signing in. The
                        // watch running out changes nothing.
                        let watch = match self.client_phase == ClientPhase::SigningIn {
                            true => Task::perform(
                                tasks::await_sign_in(self.layout.clone(), self.config.prefix_user.clone(), started),
                                move |result| Message::BackgroundClient(generation, result),
                            ),
                            false => Task::none(),
                        };
                        self.client_phase = ClientPhase::NeedsSignIn;
                        let released = match waiting {
                            Some(_) => self.launch_failed("Battle.net needs you to sign in — press its button"),
                            None => Task::none(),
                        };
                        Task::batch([watch, released, self.check_client()])
                    }
                    other => {
                        if let Err(e) = other {
                            tracing::warn!("Battle.net did not start in the background: {e}");
                        }
                        self.client_phase = ClientPhase::Settled;
                        let launch = waiting.map_or_else(Task::none, |(what, code)| self.preflight_for(what, code));
                        Task::batch([launch, self.check_client()])
                    }
                }
            }

            Message::ToggleClientMenu => {
                self.client_menu_open = !self.client_menu_open;
                Task::none()
            }

            Message::ShowBattleNet => {
                self.client_menu_open = false;
                // No pre-flight: it would close the client this is meant to
                // show. A second invocation hands it the request over IPC.
                Task::perform(
                    tasks::open_battle_net(
                        self.layout.clone(),
                        self.selected.code.clone(),
                        self.config.prefix_user.clone(),
                        None,
                    ),
                    Message::BattleNetShown,
                )
            }

            Message::BattleNetShown(result) => {
                if let Err(e) = result {
                    self.error = Some(e);
                }
                self.check_client()
            }

            Message::AskQuitBattleNet => {
                self.client_menu_open = false;
                Task::perform(tasks::running_games(self.layout.clone()), Message::QuitBattleNetChecked)
            }

            Message::QuitBattleNetChecked(games) => match games.is_empty() {
                true => self.quit_client(),
                false => {
                    self.quit_client_dialog = Some(QuitClientDialog { games });
                    Task::none()
                }
            },

            Message::QuitBattleNetDismiss => {
                self.quit_client_dialog = None;
                Task::none()
            }

            Message::QuitBattleNetConfirm => {
                self.quit_client_dialog = None;
                self.quit_client()
            }

            Message::BattleNetQuit(result) => {
                self.quitting_client = false;
                // Closed on purpose: no need to ask why it went.
                self.client_up = false;
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                self.check_client()
            }
            other => return Err(other),
        })
    }

    /// The install dialog, and the downgrade warning in front of a launch.
    fn update_install_dialog(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
                self.preflight_for(dialog.what, dialog.code)
            }
            other => return Err(other),
        })
    }
}
