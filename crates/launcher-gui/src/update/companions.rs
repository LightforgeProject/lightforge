//! The Game Tools page: installing, adding, starting and removing the
//! Windows programs that run beside the games.

use crate::*;
use lightforge_core::companion::Companion;

impl Lightforge {
    pub(crate) fn update_companions(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::CompanionGamePicked(code) => {
                self.companion_game = Some(code);
                Task::none()
            }

            Message::CompanionPick(pick) => {
                Task::perform(tasks::pick_program(), move |path| Message::CompanionPicked(pick, path))
            }

            Message::CompanionPicked(_, None) => Task::none(),

            Message::CompanionPicked(CompanionPick::Program, Some(exe)) => {
                // "None of these" while choosing ends that install too.
                let game = self.companion_install.take().map_or_else(|| self.companion_target(), |i| i.game);
                self.add_companion(&exe, game);
                Task::none()
            }

            Message::CompanionPicked(CompanionPick::Installer, Some(installer)) => {
                let game = self.companion_target();
                self.companion_install = Some(CompanionInstall { game: game.clone(), stage: CompanionStage::Starting });
                Task::perform(
                    tasks::start_installer(self.layout.clone(), game, installer),
                    Message::CompanionInstallerStarted,
                )
            }

            Message::CompanionInstallerStarted(result) => {
                match result {
                    Ok(before) => {
                        if let Some(install) = self.companion_install.as_mut() {
                            install.stage = CompanionStage::Running(before);
                        }
                    }
                    Err(e) => {
                        self.companion_install = None;
                        self.error = Some(e);
                    }
                }
                Task::none()
            }

            Message::CompanionInstallDone => {
                let Some(install) = self.companion_install.as_mut() else { return Ok(Task::none()) };
                let CompanionStage::Running(before) = std::mem::replace(&mut install.stage, CompanionStage::Looking)
                else {
                    return Ok(Task::none());
                };
                Task::perform(tasks::installed_programs(self.layout.clone(), before), Message::CompanionCandidates)
            }

            Message::CompanionCandidates(found) => {
                if let Some(install) = self.companion_install.as_mut() {
                    install.stage = CompanionStage::Choosing(found);
                }
                Task::none()
            }

            Message::CompanionChosen(exe) => {
                let Some(install) = self.companion_install.take() else { return Ok(Task::none()) };
                self.add_companion(&exe, install.game);
                Task::none()
            }

            Message::CompanionInstallCancel => {
                self.companion_install = None;
                Task::none()
            }

            Message::CompanionLaunch(at) => {
                let Some(companion) = self.config.companions.get(at).cloned() else { return Ok(Task::none()) };
                self.status = format!("starting {}…", companion.name);
                Task::perform(tasks::launch_companion(self.layout.clone(), companion), Message::CompanionStarted)
            }

            Message::CompanionStarted(result) => {
                match result {
                    Ok(note) => self.status = note,
                    Err(e) => self.error = Some(e),
                }
                Task::none()
            }

            Message::CompanionAutoStart(at, on) => {
                if let Some(companion) = self.config.companions.get_mut(at) {
                    companion.start_with_game = on;
                    self.save_config();
                }
                Task::none()
            }

            Message::CompanionRemove(at) => {
                if at < self.config.companions.len() {
                    let gone = self.config.companions.remove(at);
                    self.status = format!("{} removed from the list", gone.name);
                    self.save_config();
                }
                Task::none()
            }

            other => return Err(other),
        })
    }

    /// The game the page's next install goes with.
    pub(crate) fn companion_target(&self) -> ProductCode {
        self.companion_game.clone().unwrap_or_else(|| self.selected.code.clone())
    }

    fn add_companion(&mut self, exe: &Path, game: ProductCode) {
        let companion = Companion::new(&self.layout.prefix().join("drive_c"), exe, game);
        if self.config.companions.iter().any(|c| c.exe == companion.exe && c.game == companion.game) {
            self.status = format!("{} is already listed", companion.name);
            return;
        }
        self.status = format!("{} added", companion.name);
        self.config.companions.push(companion);
        self.save_config();
    }
}
