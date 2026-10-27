//! The app around the pages: window, splash, menus, history, release notes, general switches.

use crate::*;

impl Lightforge {
    pub(crate) fn update_app(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ResizeTraced(size) => {
                trace::mark(format_args!("resize {}x{}", size.width, size.height));
                Task::none()
            }
            Message::CloseRequested(id) => {
                // An env edit still waiting for typing to pause.
                if self.env_unsaved {
                    self.save_config();
                }
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
            Message::ToggleHints => {
                self.hints = !self.hints;
                Task::none()
            }
            Message::MutePatchNotes(mute) => {
                if let Some(notes) = self.patch_notes.as_mut() {
                    notes.mute = mute;
                }
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
            Message::Exit => iced::exit(),
            Message::MinimizeOnLaunch(on) => {
                self.config.minimize_on_launch = on;
                self.save_config();
                Task::none()
            }
            Message::CloseBattleNetOnPlay(on) => {
                self.config.close_battle_net_on_play = on;
                self.save_config();
                Task::none()
            }
            Message::BattleNetMinimized(on) => {
                self.config.battle_net_minimized = on;
                self.save_config();
                Task::none()
            }
            Message::OpenUrl(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e.into());
                }
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
            Message::MenuClick => Task::none(),

            other => return Err(other),
        })
    }
}
