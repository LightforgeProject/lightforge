//! Moving one game to another library.

use crate::*;

impl Lightforge {
    pub(super) fn update_moving(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::MoveTargetPicked(id) => {
                if let Some(settings) = self.game_settings.as_mut() {
                    settings.move_to = Some(id);
                }
                Task::none()
            }
            Message::MoveGame => {
                let Some(settings) = self.game_settings.as_ref() else {
                    return Ok(Task::none());
                };
                let code = settings.code.clone();
                let to = settings.move_to.as_ref().and_then(|id| self.config.libraries.iter().find(|l| &l.id == id));
                let (Some(to), Some(from)) = (to.cloned(), self.layout.library(&code).cloned()) else {
                    return Ok(Task::none());
                };
                // Any writer: the move reads one store and writes another.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                let moving = Moving {
                    code: code.clone(),
                    from,
                    to: to.clone(),
                    progress: Arc::default(),
                    cancel: Cancel::new(),
                    releasing: false,
                };
                let task = tasks::move_copy(
                    self.http.clone(),
                    self.layout.clone(),
                    code.clone(),
                    to,
                    moving.cancel.clone(),
                    moving.progress.clone(),
                );
                self.moving = Some(moving);
                Task::perform(task, move |result| Message::GameCopied(code.clone(), result))
            }
            Message::CancelMove => {
                if let Some(moving) = self.moving.as_ref().filter(|m| !m.releasing) {
                    moving.cancel.cancel();
                }
                Task::none()
            }
            Message::GameCopied(code, result) => {
                let Some(moving) = self.moving.as_mut() else {
                    return Ok(Task::none());
                };
                if let Err(e) = result {
                    self.moving = None;
                    self.error = Some(e.into());
                    return Ok(self.pump());
                }
                // Recorded before the old copy goes: a crash after this leaves
                // the game in two places, never in none.
                moving.releasing = true;
                let (to, from) = (moving.to.id.clone(), moving.from.clone());
                if let Some(product) = self.config.product_mut(&code) {
                    product.profile.library = Some(to);
                }
                self.save_config();
                self.rebuild_layout();
                Task::perform(
                    tasks::move_release(self.http.clone(), self.layout.clone(), code.clone(), from),
                    move |result| Message::GameMoved(code.clone(), result),
                )
            }
            Message::GameMoved(code, result) => {
                let Some(moving) = self.moving.take() else {
                    return Ok(Task::none());
                };
                match result {
                    Ok(freed) => {
                        if code == self.selected.code {
                            self.status = format!("moved to {} — freed {} on {}", moving.to.label, size_of(freed), moving.from.label);
                        }
                    }
                    // The game is already playable from its new library.
                    Err(e) => {
                        self.error = Some(format!("moved to {}, but the old copy is still on {}: {e}", moving.to.label, moving.from.label).into());
                    }
                }
                Task::batch([self.measure_storage(), self.reload_if_showing(&code), self.pump()])
            }
            other => return Err(other),
        })
    }
}
