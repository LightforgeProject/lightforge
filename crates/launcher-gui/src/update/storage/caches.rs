//! Lightforge's own cache and the per-product shader caches.

use crate::*;

impl Lightforge {
    pub(super) fn update_caches(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::ClearCache => {
                let Some(dirs) = self.dirs.clone() else {
                    return Ok(Task::none());
                };
                self.cache_notice = Some("clearing…".to_owned());
                Task::perform(tasks::clear_cache(dirs), Message::CacheCleared)
            }
            Message::CacheCleared(result) => {
                self.cache_notice = Some(match result {
                    Ok(freed) => format!("freed {}", size_of(freed)),
                    Err(e) => e,
                });
                self.measure_storage()
            }
            Message::ClearShaderCache => {
                let Some(dirs) = self.dirs.as_ref() else {
                    self.error = Some("no cache directory available".into());
                    return Ok(Task::none());
                };
                // Per product, so this cannot take another product's shaders
                // with it, the reason the cache is keyed that way at all.
                let dir = dirs.shader_cache_dir(&self.selected.code);
                Task::perform(tasks::clear_shader_cache(dir), Message::ShaderCacheCleared)
            }
            Message::ShaderCacheCleared(result) => {
                match result {
                    Ok(true) => self.status = "shader cache cleared".to_owned(),
                    // Nothing there is the desired end state, not a failure.
                    Ok(false) => self.status = "no shader cache to clear".to_owned(),
                    Err(e) => self.error = Some(e.into()),
                }
                Task::none()
            }
            other => return Err(other),
        })
    }
}
