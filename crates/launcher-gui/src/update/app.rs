//! The app around the pages: window, splash, menus, history, release notes, general switches.

use crate::*;

impl Lightforge {
    pub(crate) fn update_app(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_window(message)
            .or_else(|m| self.update_preferences(m))
            .or_else(|m| self.update_pages(m))
    }

    /// The window: closing, sizing, focus, and the splash in front of it.
    fn update_window(&mut self, message: Message) -> Result<Task<Message>, Message> {
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
                // Other store writers finish too; `pump` closes the window after.
                match self.store_writer() {
                    Some(what) => {
                        self.closing = Some(id);
                        self.status = format!("finishing {what} safely…");
                        Task::none()
                    }
                    None => iced::window::close(id),
                }
            }

            Message::SplashDone(count) => {
                // The wizard counts off this same fetch.
                if let Some(wizard) = self.wizard.as_mut() {
                    wizard.prefetched = Some(count);
                }
                match self.splash.is_some() {
                    true => self.close_splash_after_floor(),
                    false => Task::none(),
                }
            }

            Message::FinishSplash => self.finish_splash(),

            Message::Exit => iced::exit(),

            // Sizes come in points at iced's scale then; across a scale change
            // they are stale (measured: 1080 px read as 1458). Tagged by
            // generation, a stale reading is asked again.
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

            // Compositors may refuse the focus (Wayland's focus-stealing
            // rules); the attention request still marks the taskbar entry.
            Message::Raise => iced::window::oldest().and_then(|id| {
                Task::batch([
                    iced::window::minimize(id, false),
                    iced::window::gain_focus(id),
                    iced::window::request_user_attention(id, Some(iced::window::UserAttention::Informational)),
                ])
            }),
            other => return Err(other),
        })
    }

    /// App-wide switches, the release notes, and Lightforge's own updates.
    fn update_preferences(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
            other => return Err(other),
        })
    }

    /// Moving between pages, links, and the menus and pickers that open over them.
    fn update_pages(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::Show(screen) => {
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

            Message::MenuClick => Task::none(),
            other => return Err(other),
        })
    }
}
