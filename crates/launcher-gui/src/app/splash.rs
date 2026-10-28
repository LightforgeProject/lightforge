//! The first-start splash and the patch notes after it.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Open the splash and start the fetch it is waiting on.
    pub(crate) fn open_splash(&mut self) -> Task<Message> {
        let codes: Vec<ProductCode> = self.choices.iter().map(|c| c.code.clone()).collect();
        self.splash = Some(Splash { products: codes.len(), opened: Instant::now(), decided: false });
        // Nothing is fetched yet: the cache-only pass `reload` starts beside
        // this decides whether anything needs to be. See `splash_decide`.
        Task::batch([
            // The transparent rest of the window must not swallow clicks.
            iced::window::oldest().and_then(iced::window::enable_mouse_passthrough),
            // Alongside the art; it never holds the splash open.
            Task::perform(
                tasks::check_for_update(self.http.clone(), self.config.check_for_updates),
                Message::UpdateChecked,
            ),
        ])
    }

    /// The splash's one decision, made when the selected game's cache-only
    /// pass lands: close, or wait for the network.
    ///
    /// Early only when the cache holds all the first screen draws; the
    /// revalidation goes on behind. Otherwise, and on a first run, it waits.
    pub(crate) fn splash_decide(&mut self, warm: bool) -> Task<Message> {
        let Some(splash) = self.splash.as_mut().filter(|s| !s.decided) else {
            return Task::none();
        };
        splash.decided = true;
        if warm && self.wizard.is_none() {
            trace::mark("splash: cache is warm, not waiting on the network");
            return self.close_splash_after_floor();
        }
        Task::perform(
            tasks::prefetch_within(
                SPLASH_BUDGET,
                self.http.clone(),
                self.pipeline.clone(),
                self.choices.iter().map(|c| c.code.clone()).collect(),
            ),
            Message::SplashDone,
        )
    }

    /// Close the splash once it has been up for [`SPLASH_FLOOR`].
    pub(crate) fn close_splash_after_floor(&mut self) -> Task<Message> {
        let waited = self.splash.as_ref().map(|s| s.opened.elapsed()).unwrap_or_default();
        match SPLASH_FLOOR.checked_sub(waited) {
            Some(left) if !left.is_zero() => {
                Task::perform(tasks::after(left), |()| Message::FinishSplash)
            }
            // The only way out is `finish_splash`.
            _ => self.finish_splash(),
        }
    }

    /// Put the release notes up, if this start is the first one on this
    /// version.
    ///
    /// Only after setup, unless muted or already seen, and when the shipped
    /// notes describe this version.
    pub(crate) fn open_patch_notes(&mut self) {
        let version = env!("CARGO_PKG_VERSION");
        if !self.config.setup_complete
            || self.config.patch_notes_muted
            || self.config.patch_notes_seen.as_deref() == Some(version)
        {
            return;
        }
        if let Some(release) = lightforge_core::releases::notes_for(version) {
            self.patch_notes = Some(PatchNotes { release, mute: false });
        }
    }

    /// Put the splash away and show the launcher: the one way out. Nothing is
    /// resized (see [`main`]); the frame goes on and clicks come back.
    pub(crate) fn finish_splash(&mut self) -> Task<Message> {
        self.splash = None;
        trace::mark("splash done, showing the launcher");
        self.open_patch_notes();
        let console = self.console();
        let frame = iced::window::oldest().and_then(move |id| {
            // A toggle, from a window without one; a console goes fullscreen.
            let frame = match console {
                true => Task::none(),
                false => iced::window::toggle_decorations(id),
            };
            frame.chain(iced::window::disable_mouse_passthrough(id))
        });
        let frame = match console {
            true => frame.chain(self.window_mode()),
            false => frame,
        };
        // Nobody navigated to the opening screen, so `arrived` runs here.
        Task::batch([frame, self.arrived()])
    }

    /// The splash: the wordmark, what is being fetched, and a bar that says
    /// something is happening without pretending to know for how long.
    pub(crate) fn splash_screen(&self, splash: &Splash) -> Element<'_, Message> {
        const MARK: f32 = 40.0;
        const SPIN: f32 = 30.0;
        let note = match splash.products {
            // No tracked products at all is a machine that has never been set
            // up, which is a wizard away rather than an artwork fetch.
            0 => "Getting ready…".to_owned(),
            1 => "Fetching artwork for 1 game…".to_owned(),
            n => format!("Fetching artwork for {n} games…"),
        };
        // `theme::menu`: `floating` would show the desktop through it.
        let card = container(
            column![
                image(wordmark_image())
                    .width(Length::Fixed(MARK * WORDMARK_ASPECT))
                    .height(Length::Fixed(MARK)),
                muted(note, theme::CAPTION),
                canvas(Spinner).width(Length::Fixed(SPIN)).height(Length::Fixed(SPIN)),
            ]
            .spacing(theme::MD)
            .align_x(Alignment::Center),
        )
        .center(Length::Fixed(SPLASH.width))
        .height(Length::Fixed(SPLASH.height))
        .style(theme::menu);

        // **No background**: around the card the window is see-through.
        container(card).center(Length::Fill).into()
    }
}
