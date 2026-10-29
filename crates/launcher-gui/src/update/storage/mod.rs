//! Drives, space, uninstalling, moving a game, and checking or repairing a store.

use crate::*;

mod caches;
mod drives;
mod moving;
mod repair;
mod uninstall;

impl Lightforge {
    pub(crate) fn update_storage(&mut self, message: Message) -> Result<Task<Message>, Message> {
        self.update_drives(message)
            .or_else(|m| self.update_uninstall(m))
            .or_else(|m| self.update_repair(m))
            .or_else(|m| self.update_caches(m))
            .or_else(|m| self.update_moving(m))
    }
}
