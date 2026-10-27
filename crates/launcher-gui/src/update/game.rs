//! One game's own settings: versions, backups, launch settings, textures.

use crate::*;

/// How long the env editor waits after the last keystroke before saving.
const ENV_SAVE_AFTER: std::time::Duration = std::time::Duration::from_millis(750);

impl Lightforge {
    pub(crate) fn update_game(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_game_version(message)
            .or_else(|m| self.update_game_settings(m))
            .or_else(|m| self.update_optional_content(m))
            .or_else(|m| self.update_backups(m))
    }

    /// Which build is installed, and modifying an install.
    fn update_game_version(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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

            Message::ShowInExplorer => {
                self.gear_open = false;
                let dir = self.layout.wow_install(&self.selected.code);
                if let Err(e) = tasks::open_path(dir) {
                    self.error = Some(e.into());
                }
                Task::none()
            }

            Message::AskGameVersion => {
                self.gear_open = false;
                self.version_dialog = Some(VersionDialog {
                    label: self.selected.label.clone(),
                    version: self.local.version.clone().unwrap_or_else(|| "unknown".to_owned()),
                    location: self.layout.wow_install(&self.selected.code).display().to_string(),
                });
                Task::none()
            }

            Message::CloseGameVersion => {
                self.version_dialog = None;
                Task::none()
            }

            Message::AskModifyInstall => {
                self.gear_open = false;
                let code = self.selected.code.clone();
                let current = self.config.locale_for(&code).to_owned();
                self.modify_dialog = Some(ModifyDialog {
                    code,
                    label: self.selected.label.clone(),
                    locale: current.clone(),
                    current,
                });
                Task::none()
            }

            Message::ModifyLocale(locale) => {
                if let Some(dialog) = self.modify_dialog.as_mut() {
                    dialog.locale = locale;
                }
                Task::none()
            }

            Message::ModifyCancel => {
                self.modify_dialog = None;
                Task::none()
            }

            Message::ModifyStart => {
                let Some(dialog) = self.modify_dialog.take() else {
                    return Ok(Task::none());
                };
                if dialog.locale == dialog.current {
                    return Ok(Task::none());
                }
                let Some(product) = self.config.product_mut(&dialog.code) else {
                    self.error = Some(format!("{} is not a managed game", dialog.label).into());
                    return Ok(Task::none());
                };
                product.profile.locale = Some(dialog.locale);
                self.save_config();
                // The download plans against the new language and fetches
                // only what the store lacks.
                self.enqueue_for(dialog.code)
            }
            other => return Err(other),
        })
    }

    /// The Game Settings card: opening it, its switches and its environment editor.
    fn update_game_settings(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::OpenGameSettings => {
                self.close_panels();
                self.game_settings = Some(GameSettings {
                    code: self.selected.code.clone(),
                    label: self.selected.label.clone(),
                    backups: None,
                    working: false,
                    confirm: None,
                    note: None,
                    textures_bytes: None,
                    textures: TexturesStep::Idle,
                });
                // The manifest says whether this build has textures behind
                // the option; retail's does not, and a switch for nothing is
                // not a choice.
                let code = self.selected.code.clone();
                Task::batch([
                    self.list_backups(),
                    Task::perform(
                        tasks::offerings(
                            self.http.clone(),
                            code.clone(),
                            self.config.region,
                            self.opted_optional(&code),
                            None,
                        ),
                        move |result| Message::OptionalSized(code.clone(), Box::new(result)),
                    ),
                ])
            }

            Message::CloseGameSettings => {
                self.game_settings = None;
                Task::none()
            }

            Message::FromGameSettings(message) => {
                self.game_settings = None;
                self.update(*message)
            }

            Message::Tweak(scope, which, on) => {
                self.edit_tweaks(scope, |tweaks| match which {
                    // Stored as an explicit refusal rather than a bare `false`:
                    // the default is on wherever the tool exists, so "off" has
                    // to be recorded to survive a restart.
                    Tweak::GamePerformance => tweaks.game_performance = Some(on),
                    Tweak::MangoHud => tweaks.mangohud = on,
                    Tweak::Wayland => tweaks.wayland = on,
                });
                Task::none()
            }

            Message::EnvEdit(scope, action) => {
                let editor = match scope {
                    Scope::Product => &mut self.env_editor,
                    Scope::BattleNet => &mut self.bnet_env_editor,
                };
                let edited = action.is_edit();
                editor.perform(action);
                if !edited {
                    return Ok(Task::none());
                }
                // In effect as it is typed, the lines that parse. A bad line
                // is shown under the editor and left out of the launch rather
                // than refusing the rest, so a half-typed name costs nothing.
                let (env, _) = lightforge_core::tweaks::parse_env(&editor.text());
                self.change_tweaks(scope, |tweaks| tweaks.env = env);
                // On disk once typing pauses: a save is a full rewrite with
                // two fsyncs, on this thread, and used to run per keystroke.
                self.env_edits += 1;
                self.env_unsaved = true;
                let edit = self.env_edits;
                Task::perform(async { tokio::time::sleep(ENV_SAVE_AFTER).await }, move |()| {
                    Message::EnvSettled(edit)
                })
            }

            Message::EnvSettled(edit) => {
                if edit == self.env_edits && self.env_unsaved {
                    self.save_config();
                }
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// Optional downloads such as the high-resolution textures: sizing, adding, removing.
    fn update_optional_content(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::OptionalSized(code, result) => {
                if let (Ok(offerings), Some(settings)) = (&*result, self.game_settings.as_mut()) {
                    if settings.code == code {
                        settings.textures_bytes = offerings
                            .optional_bytes
                            .iter()
                            .find(|(tag, _)| tag == HIGH_RES)
                            .map(|(_, bytes)| *bytes)
                            .filter(|bytes| *bytes > 0);
                    }
                }
                Task::none()
            }

            Message::OptionalContent(name, on) => {
                let code = self.selected.code.clone();
                self.set_optional(&code, &name, on);
                if on {
                    // The download is the point of switching it on. Out of the
                    // popup, onto the page where its bar is.
                    self.game_settings = None;
                    return Ok(self.enqueue_for(code));
                }
                let Some(settings) = self.game_settings.as_mut() else {
                    return Ok(Task::none());
                };
                settings.note = None;
                settings.textures = TexturesStep::Planning;
                let remaining = self.opted_optional(&code);
                Task::perform(
                    tasks::optional_plan(
                        self.http.clone(),
                        self.layout.clone(),
                        code,
                        name,
                        remaining,
                    ),
                    |result| Message::OptionalPlanned(result.map(Box::new)),
                )
            }

            Message::OptionalPlanned(result) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.textures = TexturesStep::Idle;
                    match result {
                        Ok(plan) if plan.entries.is_empty() => {
                            settings.note =
                                Some(Ok("Switched off. No texture files are installed.".to_owned()))
                        }
                        Ok(plan) => settings.textures = TexturesStep::Ask(plan),
                        Err(e) => {
                            settings.note = Some(Err(format!(
                                "Switched off, but the files could not be listed: {e}"
                            )))
                        }
                    }
                }
                Task::none()
            }

            Message::KeepOptional => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.textures = TexturesStep::Idle;
                    settings.note = Some(Ok(
                        "Switched off. The files stay on disk until you delete them.".to_owned(),
                    ));
                }
                Task::none()
            }

            Message::RemoveOptional => {
                let Some(code) = self.game_settings.as_ref().map(|s| s.code.clone()) else {
                    return Ok(Task::none());
                };
                let busy = self.downloading_into(&self.layout.wow_install(&code));
                let (layout, settings) = (self.layout.clone(), self.game_settings.as_mut().unwrap());
                let TexturesStep::Ask(plan) = std::mem::take(&mut settings.textures) else {
                    return Ok(Task::none());
                };
                if busy {
                    settings.textures = TexturesStep::Ask(plan);
                    settings.note =
                        Some(Err("A download is writing to this install; wait for it.".to_owned()));
                    return Ok(Task::none());
                }
                settings.note = None;
                settings.textures = TexturesStep::Removing;
                Task::perform(tasks::optional_remove(layout, code, *plan), Message::OptionalRemoved)
            }

            Message::OptionalRemoved(result) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.textures = TexturesStep::Idle;
                    settings.note = Some(match result {
                        Ok(freed) => Ok(format!("Deleted the textures, {} freed.", size_of(freed))),
                        Err(e) => Err(e.to_string()),
                    });
                }
                Task::none()
            }
            other => return Err(other),
        })
    }

    /// WTF backups: listing, taking, restoring, deleting.
    fn update_backups(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
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
                    return Ok(Task::none());
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
                    return Ok(Task::none());
                };
                let Some((archive, BackupAction::Delete)) = settings.confirm.take() else {
                    return Ok(Task::none());
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
                    return Ok(Task::none());
                };
                let Some((archive, BackupAction::Restore)) = settings.confirm.take() else {
                    return Ok(Task::none());
                };
                settings.working = true;
                Task::perform(
                    tasks::restore_backup(self.layout.clone(), settings.code.clone(), archive),
                    Message::BackupFinished,
                )
            }

            Message::BackupFinished(result) => {
                let Some(settings) = self.game_settings.as_mut() else {
                    return Ok(Task::none());
                };
                settings.working = false;
                settings.note = Some(result);
                // Either outcome can have changed the list: a restore adds
                // the backup of what it replaced.
                self.list_backups()
            }
            other => return Err(other),
        })
    }
}
