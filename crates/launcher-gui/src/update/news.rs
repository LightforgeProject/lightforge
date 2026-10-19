//! The News page and its pictures.

use crate::*;

impl Lightforge {
    pub(crate) fn update_news(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::OpenArticle(url) => {
                if let Err(e) = tasks::open_url(&url) {
                    self.error = Some(e.into());
                }
                Task::none()
            }
            Message::RefreshNews => self.fetch_news(true),
            Message::NewsLoaded(headlines) => {
                self.news_pending = false;
                self.news_asked = true;
                // A failed refresh keeps what is already on screen: headlines
                // from ten minutes ago are worth more than an empty page.
                if let Some(headlines) = headlines {
                    self.news = Some(*headlines);
                }
                self.fetch_news_images()
            }
            Message::NewsBackground(handle) => {
                self.news_bg = Some(handle);
                Task::none()
            }
            Message::NewsImages(images) => {
                // Only pictures the fetch found are in here, and only ones
                // not already held, so this adds, never replaces. Replacing
                // would hand the view a new handle for an identical picture
                // and re-upload every texture on the page.
                self.news_images.extend(images);
                Task::none()
            }

            other => return Err(other),
        })
    }
}
