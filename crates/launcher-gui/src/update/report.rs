//! The bug report and feedback dialog.

use crate::*;

impl Lightforge {
    pub(crate) fn update_report(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_report_form(message)
            .or_else(|m| self.update_report_screenshot(m))
            .or_else(|m| self.update_report_send(m))
    }

    /// The debug info and the report form itself.
    fn update_report_form(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
                tracing::info!(
                    "renderer: {} over {}; {}, {} B RAM",
                    info.graphics_adapter,
                    info.graphics_backend,
                    info.cpu_brand,
                    info.memory_total,
                );
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

            Message::ReportContact(contact) => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Ok(Task::none());
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

            Message::ReportCancel => {
                self.report_dialog = None;
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// The screenshot attached to a report.
    fn update_report_screenshot(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
                let Some(path) = path else { return Ok(Task::none()) };
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

            Message::ReportShotCleared => {
                if let Some(dialog) = self.report_dialog.as_mut() {
                    dialog.shot = None;
                }
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// Getting the report out: copied, or sent.
    fn update_report_send(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ReportCopy => {
                let Some(dialog) = self.report_dialog.as_ref() else {
                    return Ok(Task::none());
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

            Message::ReportSend => {
                let Some(dialog) = self.report_dialog.as_mut() else {
                    return Ok(Task::none());
                };
                let note = dialog.note.text();
                // From a failed send too: that is the "Try again" button.
                let ready = matches!(dialog.stage, ReportStage::Writing | ReportStage::Failed);
                let Some(category) = dialog.category.filter(|_| ready && dialog.can_send()) else {
                    return Ok(Task::none());
                };
                // The same words twice in five minutes is the Send button
                // being pressed again, not a second problem.
                let repeat = self
                    .last_report
                    .as_ref()
                    .is_some_and(|(last, when)| last == &note && when.elapsed() < REPORT_COOLDOWN);
                if repeat {
                    dialog.stage = ReportStage::TooSoon;
                    return Ok(Task::none());
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
            other => return Err(other),
        })
    }
}
