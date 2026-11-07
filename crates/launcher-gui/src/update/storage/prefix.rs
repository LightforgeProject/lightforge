//! Moving the Wine prefix to another folder or drive.

use crate::*;

impl Lightforge {
    pub(super) fn update_prefix_move(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::PrefixFolderTyped(value) => {
                self.prefix_folder = value;
                Task::none()
            }
            Message::BrowsePrefixFolder => Task::perform(tasks::pick_folder(), Message::PrefixFolderPicked),
            Message::PrefixFolderPicked(folder) => {
                if let Some(folder) = folder {
                    self.prefix_folder = folder.display().to_string();
                }
                Task::none()
            }
            Message::MovePrefix => {
                let chosen = match parse_path(&self.prefix_folder) {
                    Ok(chosen) => chosen,
                    Err(e) => {
                        self.error = Some(e.into());
                        return Ok(Task::none());
                    }
                };
                if self.prefix_moving {
                    return Ok(Task::none());
                }
                if self.locked() || self.setup_running {
                    self.error = Some("wait for setup or the game to finish first".into());
                    return Ok(Task::none());
                }
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                self.prefix_moving = true;
                // Nothing may start in a prefix that is on its way somewhere.
                self.busy = true;
                self.error = None;
                self.prefix_notice = Some("moving…".to_owned());
                Task::perform(tasks::move_prefix(self.layout.clone(), chosen), Message::PrefixPlaced)
            }
            Message::PrefixPlaced(Err(e)) => {
                self.prefix_moving = false;
                self.busy = false;
                self.prefix_notice = None;
                self.error = Some(e.into());
                Task::none()
            }
            Message::PrefixPlaced(Ok(moved)) => {
                let before = self.config.prefix_dir.clone();
                let layout = self.layout.clone();
                self.config.set_prefix_dir(&layout, moved.to.clone());
                // A config still naming the old place would find no prefix there.
                if !self.save_config() {
                    self.config.prefix_dir = before;
                    self.prefix_moving = false;
                    self.busy = false;
                    self.prefix_notice = None;
                    if let Err(e) = moved.undo() {
                        self.error = Some(format!("the prefix stayed at {}: {e}", moved.to.display()).into());
                    }
                    return Ok(Task::none());
                }
                self.rebuild_layout();
                Task::perform(tasks::release_prefix(moved), Message::PrefixReleased)
            }
            Message::PrefixReleased(result) => {
                self.prefix_moving = false;
                self.busy = false;
                self.prefix_folder.clear();
                self.prefix_notice = match result {
                    Ok(()) => Some(format!("moved to {}", tilde(&self.layout.compat_data()))),
                    Err(e) => Some(format!("moved, but the old copy could not be deleted: {e}")),
                };
                Task::batch([
                    Task::perform(
                        provision::status(self.layout.clone(), self.config.clone()),
                        |status| Message::SetupStatus(status.map(Box::new)),
                    ),
                    self.measure_storage(),
                ])
            }
            other => return Err(other),
        })
    }
}
