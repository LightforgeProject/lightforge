//! One game's own settings: versions, backups, Game Optimizations.

use crate::*;

impl Lightforge {
    pub(crate) fn update_game(&mut self, message: Message) -> Result<Task<Message>, Message> {
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
            Message::OpenGameSettings => {
                self.close_panels();
                self.game_settings = Some(GameSettings {
                    code: self.selected.code.clone(),
                    label: self.selected.label.clone(),
                    backups: None,
                    working: false,
                    confirm: None,
                    note: None,
                });
                self.list_backups()
            }
            Message::CloseGameSettings => {
                self.game_settings = None;
                Task::none()
            }
            Message::FromGameSettings(message) => {
                self.game_settings = None;
                self.update(*message)
            }
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
            Message::Tweak(scope, which, on) => {
                self.edit_tweaks(scope, |tweaks| match which {
                    // Stored as an explicit refusal rather than a bare `false`:
                    // the default is on wherever the tool exists, so "off" has
                    // to be recorded to survive a restart.
                    Tweak::GamePerformance => tweaks.game_performance = Some(on),
                    Tweak::GameMode => tweaks.gamemode = on,
                    Tweak::MangoHud => tweaks.mangohud = on,
                    Tweak::SkipAddonProfiling => tweaks.skip_addon_profiling = on,
                    Tweak::ReduceRaidEffects => tweaks.reduce_raid_effects = on,
                    Tweak::FasterZoneLoads => tweaks.faster_zone_loads = on,
                    Tweak::RawMouse => tweaks.raw_mouse = on,
                    Tweak::NoScreenEffects => tweaks.no_screen_effects = on,
                    Tweak::MaxDrawDistance => tweaks.max_draw_distance = on,
                    Tweak::NoClothPhysics => tweaks.no_cloth_physics = on,
                    Tweak::SoftwareCursor => tweaks.software_cursor = on,
                    Tweak::ClipCursor => tweaks.clip_cursor = on,
                    Tweak::NoMouseAcceleration => tweaks.no_mouse_acceleration = on,
                    Tweak::ShowCursorInRelativeMode => {
                        tweaks.show_cursor_in_relative_mode = on
                    }
                    Tweak::Controller => tweaks.controller = on,
                    Tweak::Wayland => tweaks.wayland = on,
                    Tweak::Hdr => tweaks.hdr = on,
                    Tweak::BigShaderCache => tweaks.big_shader_cache = on,
                });
                Task::none()
            }
            Message::TweakPreset(preset) => {
                self.edit_tweaks(Scope::Product, |tweaks| preset.apply(tweaks));
                Task::none()
            }
            Message::TweakCustom => {
                self.edit_tweaks(Scope::Product, Tweaks::clear_managed);
                Task::none()
            }
            Message::TweakEnvToggle(scope) => {
                match scope {
                    Scope::Product => self.env_open = !self.env_open,
                    Scope::BattleNet => self.bnet_env_open = !self.bnet_env_open,
                }
                Task::none()
            }
            Message::TweakApi(ApiChoice(api)) => {
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.graphics_api = api);
                Task::none()
            }
            Message::TweakFpsCap(value) => {
                // Digits only: a frame cap that is not a number is not a cap.
                // Clearing it is how you say uncapped.
                self.fps_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap = cap);
                Task::none()
            }
            Message::TweakFpsCapBackground(value) => {
                self.fps_bk_input = value.chars().filter(char::is_ascii_digit).collect();
                let cap = self.fps_bk_input.parse::<u32>().ok().filter(|n| *n > 0);
                self.edit_tweaks(Scope::Product, |tweaks| tweaks.fps_cap_background = cap);
                Task::none()
            }
            Message::TweakWaylandMonitor(scope, value) => {
                // A connector name, so no spaces: `DP-1`, `HDMI-A-1`.
                let field = match scope {
                    Scope::Product => &mut self.monitor_input,
                    Scope::BattleNet => &mut self.bnet_monitor_input,
                };
                *field = value.chars().filter(|c| !c.is_whitespace()).collect();
                let name = (!field.is_empty()).then(|| field.clone());
                self.edit_tweaks(scope, |tweaks| tweaks.wayland_monitor = name);
                Task::none()
            }
            Message::TweakEnvKey(value) => {
                // No spaces or `=`: the two halves are separate fields here,
                // and a key holding either would not survive the environment.
                self.env_key_input =
                    value.chars().filter(|c| !c.is_whitespace() && *c != '=').collect();
                Task::none()
            }
            Message::TweakEnvValue(value) => {
                self.env_value_input = value;
                Task::none()
            }
            Message::TweakEnvAdd(scope) => {
                let key = std::mem::take(&mut self.env_key_input);
                let value = std::mem::take(&mut self.env_value_input);
                if !key.is_empty() {
                    self.edit_tweaks(scope, |tweaks| {
                        tweaks.env.insert(key, value);
                    });
                }
                Task::none()
            }
            Message::TweakEnvRemove(scope, key) => {
                self.edit_tweaks(scope, |tweaks| {
                    tweaks.env.remove(&key);
                });
                Task::none()
            }

            other => return Err(other),
        })
    }
}
