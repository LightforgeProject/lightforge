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
            // The window is the launcher's size but only the card is painted,
            // so the rest of it is a transparent rectangle that would still
            // swallow every click landing on whatever is behind it. On
            // Wayland this sets a real empty input region, not a hint.
            iced::window::oldest().and_then(iced::window::enable_mouse_passthrough),
            // Beside the artwork rather than before it: this is the one moment
            // a start is already waiting on the network, and nothing on screen
            // depends on the answer, so it never holds the splash open.
            Task::perform(
                tasks::check_for_update(self.http.clone(), self.config.check_for_updates),
                Message::UpdateChecked,
            ),
        ])
    }

    /// The splash's one decision, made when the selected game's cache-only
    /// pass lands: close, or wait for the network.
    ///
    /// The splash exists so nobody sees a grey launcher, so it closes early
    /// only when the cache already holds everything the first screen draws,
    /// decoded and in state by now. The revalidation `reload` started runs
    /// on behind the launcher and swaps in whatever changed, exactly as on a
    /// product switch. Anything missing, or a first run, whose wizard counts
    /// off the prefetch, waits for the full fetch as it always has.
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
            // Both ways out go through `finish_splash` and nothing else. An
            // earlier version did the work in two places, and the two copies
            // drifted: the one taken whenever the fetch outlasts the floor,
            // which is every start with anything to fetch, missed a step the
            // other had.
            _ => self.finish_splash(),
        }
    }

    /// Put the release notes up, if this start is the first one on this
    /// version.
    ///
    /// Four conditions, and each one is a case that should not see a popup:
    /// setup must be finished (a first run has the wizard to get through, and
    /// notes for a version nothing has been done with yet say nothing); the
    /// checkbox must not have been ticked; the notes must not already have
    /// been seen for this version; and the shipped file must actually describe
    /// this version, which a development build between releases does not.
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

    /// Turn the splash window into the launcher's own.
    ///
    /// Put the splash away and show the launcher.
    ///
    /// The one way out of the splash, reached whether the fetch outlasted
    /// [`SPLASH_FLOOR`] or the floor outlasted the fetch. There is deliberately
    /// no second copy of this: when there were two, they drifted.
    ///
    /// Nothing is resized or moved, the window has been the launcher's size
    /// and in the middle of the screen since it was mapped (see [`main`]).
    /// All that changes is that the frame goes on, the window starts taking
    /// clicks again, and `view` stops drawing the card.
    pub(crate) fn finish_splash(&mut self) -> Task<Message> {
        self.splash = None;
        trace::mark("splash done, showing the launcher");
        self.open_patch_notes();
        let console = self.console();
        let frame = iced::window::oldest().and_then(move |id| {
            // iced reads the frame's live state and inverts it, so given a
            // window mapped without one, this is "put it on". A console's
            // window goes fullscreen instead, where a frame has no place.
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
        // Nobody ever *navigates* to the opening screen, so `arrived` has
        // never run for it, and the news rail on it needs the headlines.
        // Through the same function the navigation uses rather than a second
        // call to `fetch_news`, so the dashboard has one definition of what
        // it wants on the way in.
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
        // The card, at [`SPLASH`]'s size. `theme::menu` rather than
        // `theme::floating`: floating is 12% transparent, which over a
        // transparent window means the desktop showing through the splash.
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

        // Centred on a window the size of the launcher, with **no background
        // of its own**: the window was created `transparent`, so everything
        // around the card is see-through and what anyone sees is a small
        // panel in the middle of their screen. Painting a backdrop here
        // instead would show a full-size empty launcher, which is the thing
        // the splash exists to avoid.
        container(card).center(Length::Fill).into()
    }
}
