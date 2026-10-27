//! The selection ring: keys, the gamepad, typing on a console.

use crate::*;

impl Lightforge {
    pub(crate) fn update_navigation(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_focus_moves(message)
            .or_else(|m| self.update_focus_settle(m))
    }

    /// An arrow, a press, or typing finishing: where the ring goes.
    fn update_focus_moves(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::Nav(from, input) => {
                let now = Instant::now();
                trace::pad(format_args!("nav {from:?} {input:?} focused={}", self.window_focused));
                if from == NavFrom::Pad && !self.window_focused {
                    return Ok(Task::none());
                }
                // Steam can deliver one press both ways: as keys from its
                // layout, and raw from a pad it did not name as translated.
                // Whichever arrives first keeps the floor while presses keep
                // coming, so neither a tap nor a held repeat counts twice.
                if let Some((last, at)) = self.nav_last {
                    if last != from && now.duration_since(at) < NAV_DOUBLED {
                        trace::pad("  dropped: doubled");
                        return Ok(Task::none());
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
                                return Ok(Task::none());
                            }
                            pad::Input::Press if on_field => {
                                self.focus = Some(field.clone());
                                self.steam_keyboard(&field);
                                return Ok(Task::none());
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

            Message::FocusStep(step) => {
                if self.keys_belong_elsewhere() {
                    return Ok(Task::none());
                }
                let spots = focus::live();
                // The first press only shows where the ring is.
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return Ok(match self.focus {
                        Some(_) => Task::none(),
                        // Nothing to select, only something to read.
                        None => self.scroll_page(step, None),
                    });
                };
                // The Settings index is a pane of its own, so Right from it
                // reached nothing. It reads as a way in, the same as A, and
                // Left off the edge of a section is the way back out, as B.
                if matches!(current, focus::Target::SettingsTab(_)) && step == focus::Step::Right {
                    return Ok(self.update(Message::FocusPress));
                }
                match focus::step(&spots, &current, step) {
                    Some(next) => {
                        Self::scroll_into_view(&next);
                        self.focus = Some(next);
                        Task::none()
                    }
                    None if step == focus::Step::Left
                        && self.screen == Screen::Settings
                        && focus::spot(&current).and_then(|spot| spot.region) == Some(SETTINGS_PAGE) =>
                    {
                        self.update(Message::FocusBack)
                    }
                    None => self.scroll_page(step, Some(&current)),
                }
            }

            Message::FocusPress => {
                if self.keys_belong_elsewhere() {
                    return Ok(Task::none());
                }
                let spots = focus::live();
                let current = self.focus.clone().filter(|t| spots.iter().any(|p| &p.target == t));
                let Some(current) = current else {
                    self.focus = self.focus_start(&spots);
                    return Ok(Task::none());
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
                    Some(focus::Press::Off) | None => return Ok(Task::none()),
                };
                // Whatever the press opened or closed, the ring follows once
                // it has been drawn.
                self.focus_entering = FOCUS_ENTER_FRAMES;
                Task::batch([pressed, focus_settle()])
            }
            other => return Err(other),
        })
    }

    /// Where the ring lands after the page has moved, Back, and hiding it.
    fn update_focus_settle(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::FocusSettle => {
                if self.focus_entering == 0 {
                    return Ok(Task::none());
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
                        return Ok(Task::none());
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
                    return Ok(Task::none());
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
                    return Ok(Task::none());
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
                    (None, false) => return Ok(Task::none()),
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
            other => return Err(other),
        })
    }
}
