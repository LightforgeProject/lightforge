//! The Settings page itself: its index, scrolling and measuring.

use crate::*;

impl Lightforge {
    pub(crate) fn update_settings(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
                        // Asked for in the same update that opened the page,
                        // so measured against the screen it replaced. Again
                        // once the page has been drawn, but not forever: a
                        // section that never appears is not worth a timer.
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
                // Held where the jump put it while the page fills in under
                // it: the drives and the Proton list arrive a moment after
                // the page opens, and every section below them moves down.
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
