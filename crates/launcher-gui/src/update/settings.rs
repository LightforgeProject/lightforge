//! The Settings page itself: its index, scrolling and measuring.

use crate::*;

impl Lightforge {
    pub(crate) fn update_settings(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            // A jump: measured first, sections above may have grown.
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
                    // The jump's own scrolling; anything else is the reader's.
                    let ours = at(anchor.y) || anchor.prior.is_some_and(at);
                    if !ours {
                        if anchor.since.elapsed() < SETTINGS_JUMP_STALE {
                            return Ok(Task::none());
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
                        // Retried once the page is drawn, a bounded number of times.
                        self.settings_jump_tries += 1;
                        if self.settings_jump_tries > SETTINGS_JUMP_TRIES {
                            self.settings_jump = None;
                            return Ok(Task::none());
                        }
                        return Ok(Task::perform(tasks::after(Duration::from_millis(16)), |()| ())
                            .then(|()| measure_settings()));
                    };
                    self.settings_jump = None;
                    self.settings_anchor = Some(SettingsAnchor { tab, y, prior: None, since: Instant::now() });
                    return Ok(iced::widget::operation::scroll_to(
                        SETTINGS_PAGE,
                        scrollable::AbsoluteOffset { x: 0.0, y },
                    ));
                }
                // Held while the drives and Proton list fill in.
                if let Some(anchor) = &mut self.settings_anchor {
                    self.settings_tab = anchor.tab;
                    let Some(y) = self.settings_layout.offset(anchor.tab) else { return Ok(Task::none()) };
                    if (y - anchor.y).abs() < 1.0 {
                        return Ok(Task::none());
                    }
                    anchor.prior = Some(anchor.y);
                    anchor.y = y;
                    return Ok(iced::widget::operation::scroll_to(
                        SETTINGS_PAGE,
                        scrollable::AbsoluteOffset { x: 0.0, y },
                    ));
                }
                if let Some(viewport) = &self.settings_viewport {
                    if let Some(tab) = self.settings_layout.reading(viewport, self.settings_tab) {
                        self.settings_tab = tab;
                    }
                }
                Task::none()
            }
            Message::ShowSettingsTab(tab) => {
                self.settings_jump = Some(tab);
                self.update(Message::Show(Screen::Settings))
            }

            other => return Err(other),
        })
    }
}
