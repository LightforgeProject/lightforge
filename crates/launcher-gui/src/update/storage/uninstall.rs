//! Uninstalling one product's version.

use crate::*;

impl Lightforge {
    pub(super) fn update_uninstall(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
            Message::UninstallCancel => {
                self.uninstall_dialog = None;
                Task::none()
            }
            Message::UninstallConfirm => {
                // Any writer, not only this product's own: the store is shared,
                // and a reclaim rewrites archives another flavour is writing.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                let Some(dialog) = self.uninstall_dialog.as_mut() else {
                    return Ok(Task::none());
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
            other => return Err(other),
        })
    }
}
