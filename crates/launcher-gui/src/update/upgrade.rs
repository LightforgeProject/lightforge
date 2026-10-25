//! A newer Lightforge: the notice, and updating to it; see
//! [`lightforge_core::upgrade`].

use crate::*;

/// How often to look at the binary while a terminal installs over it.
const WATCH: Duration = Duration::from_secs(2);

impl Lightforge {
    pub(crate) fn update_upgrade(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::UpdateChecked(found) => {
                let Some(available) = found else { return Ok(Task::none()) };
                // Once per release. Someone who has read "beta.3 is out" and
                // decided to stay on beta.2 should not be told again on every
                // start until they give in.
                if self.config.update_seen.as_deref() == Some(available.latest.as_str()) {
                    return Ok(Task::none());
                }
                self.update_notice = Some(available);
                Task::none()
            }
            Message::CheckLauncherNow => {
                if self.launcher_check == LauncherCheck::Asking || self.upgrading() {
                    return Ok(Task::none());
                }
                self.launcher_check = LauncherCheck::Asking;
                Task::perform(tasks::check_for_update_now(self.http.clone()), Message::LauncherChecked)
            }
            Message::LauncherChecked(answer) => {
                self.launcher_check = match answer {
                    // Shown even if this version was dismissed before: the
                    // button is the request, so "once per release" no longer applies.
                    Ok(Some(available)) => {
                        if self.update_notice.is_none() {
                            self.upgrade = None;
                            self.update_notice = Some(available);
                        }
                        LauncherCheck::Idle
                    }
                    Ok(None) => LauncherCheck::Newest,
                    Err(why) => LauncherCheck::Failed(why),
                };
                Task::none()
            }
            Message::DismissUpdate => {
                // Not halfway through replacing the files.
                if self.upgrading() {
                    return Ok(Task::none());
                }
                self.upgrade = None;
                if let Some(available) = self.update_notice.take() {
                    self.config.update_seen = Some(available.latest);
                    self.save_config();
                }
                Task::none()
            }
            Message::OpenReleases => {
                let url = self
                    .update_notice
                    .as_ref()
                    .map(|a| a.url.clone())
                    .unwrap_or_else(|| lightforge_core::update::RELEASES_PAGE.to_owned());
                if let Err(e) = open::that_detached(&url) {
                    self.error = Some(format!("{url}: {e}").into());
                }
                self.update(Message::DismissUpdate)
            }
            Message::UpgradeStart => {
                let Some(available) = &self.update_notice else { return Ok(Task::none()) };
                if self.upgrading() {
                    return Ok(Task::none());
                }
                let (tx, rx) = tokio::sync::watch::channel((0, None));
                self.upgrade = Some(Upgrade { stage: UpgradeStage::Fetching, rx: Some(rx), sample: (0, None) });
                Task::perform(
                    tasks::upgrade::run(self.origin.clone(), available.latest.clone(), tx),
                    Message::UpgradeReady,
                )
            }
            Message::UpgradeReady(result) => {
                let Some(upgrade) = self.upgrade.as_mut() else { return Ok(Task::none()) };
                upgrade.rx = None;
                upgrade.stage = match result {
                    Ok(tasks::Prepared::Installed) => UpgradeStage::Installed,
                    Ok(tasks::Prepared::Terminal { command }) => UpgradeStage::InTerminal {
                        command,
                        stamp: self.exe.as_deref().and_then(tasks::upgrade::stamp),
                    },
                    Ok(tasks::Prepared::NoTerminal { command }) => UpgradeStage::NoTerminal { command },
                    Err(problem) => UpgradeStage::Failed(problem),
                };
                Task::none()
            }
            Message::UpgradeWatch => {
                let Some(upgrade) = self.upgrade.as_mut() else { return Ok(Task::none()) };
                if let UpgradeStage::InTerminal { stamp: Some(was), .. } = &upgrade.stage {
                    let now = self.exe.as_deref().and_then(tasks::upgrade::stamp);
                    // Gone counts too: a package manager can be caught
                    // between removing the old file and writing the new.
                    if now.is_some_and(|now| now != *was) {
                        upgrade.stage = UpgradeStage::Installed;
                    }
                }
                Task::none()
            }
            Message::UpgradeRestart => {
                if !self.can_restart() {
                    return Ok(Task::none());
                }
                // Steam's Game Mode started this process and watches it: a
                // copy started from here would not be its game. Closing is
                // all that can be done; Steam starts the new one.
                if fit::under_gamescope() {
                    return Ok(iced::exit());
                }
                let started = self.exe.as_ref().map(tasks::upgrade::restart);
                match started {
                    Some(Ok(())) => iced::exit(),
                    Some(Err(e)) => {
                        self.error = Some(format!("could not start the new Lightforge: {e}").into());
                        Task::none()
                    }
                    None => iced::exit(),
                }
            }

            other => return Err(other),
        })
    }

    /// Whether the update is still replacing or fetching files.
    pub(crate) fn upgrading(&self) -> bool {
        self.upgrade.as_ref().is_some_and(|u| u.stage == UpgradeStage::Fetching)
    }

    /// Whether closing now would cut something off: a store being written
    /// winds down through the window's own close, which a restart skips.
    pub(crate) fn can_restart(&self) -> bool {
        self.store_writer().is_none() && self.running().is_none()
    }

    /// The watch on the binary, while a terminal installs over it.
    pub(crate) fn upgrade_watch(&self) -> Subscription<Message> {
        match self.upgrade.as_ref().map(|u| &u.stage) {
            Some(UpgradeStage::InTerminal { stamp: Some(_), .. }) => {
                iced::time::every(WATCH).map(|_| Message::UpgradeWatch)
            }
            _ => Subscription::none(),
        }
    }
}
