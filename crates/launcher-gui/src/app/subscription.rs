//! What the app listens for between messages.

use crate::*;

impl Lightforge {
    /// The window closing, keys, gamepads and focus: listened to for the whole run.
    pub(crate) fn input_events() -> Subscription<Message> {
        let closing = iced::window::close_requests().map(Message::CloseRequested);

        // The hint chord, and the keys that move the selection ring. Only
        // keys no widget took, so arrows in a text field stay the field's,
        // and only these produce a message, so typing wakes nothing else.
        let hints = iced::keyboard::listen().filter_map(|event| {
            use iced::keyboard::key::Named;
            let iced::keyboard::Event::KeyPressed { ref key, modifiers, .. } = event else {
                return None;
            };
            if hints_chord(key, modifiers) {
                return Some(Message::ToggleHints);
            }
            let input = match key.as_ref() {
                iced::keyboard::Key::Named(Named::ArrowUp) => pad::Input::Step(focus::Step::Up),
                iced::keyboard::Key::Named(Named::ArrowDown) => pad::Input::Step(focus::Step::Down),
                iced::keyboard::Key::Named(Named::ArrowLeft) => pad::Input::Step(focus::Step::Left),
                iced::keyboard::Key::Named(Named::ArrowRight) => pad::Input::Step(focus::Step::Right),
                iced::keyboard::Key::Named(Named::Enter) => pad::Input::Press,
                iced::keyboard::Key::Named(Named::Escape) => pad::Input::Back,
                _ => return None,
            };
            Some(Message::Nav(NavFrom::Keys, input))
        });

        // Gamepads, for when Steam hands us a pad rather than keys. Read on a
        // thread of their own for the whole run; see [`pad`].
        let pads = Subscription::run(pad::inputs).map(|input| Message::Nav(NavFrom::Pad, input));
        let window_focus = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
            iced::Event::Window(iced::window::Event::Resized(_)) if fit::owned() => {
                Some(Message::WindowResized)
            }
            _ => None,
        });

        Subscription::batch([closing, hints, pads, window_focus])
    }

    /// The mouse, only while it means something: the ring is up, or a tile is held.
    pub(crate) fn pointer_events(&self) -> Subscription<Message> {
        // While the ring is up, the mouse moving takes it away, the way a
        // browser drops its focus outline once you reach for the mouse.
        let pointing = match self.focus.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { .. }) => {
                    Some(Message::FocusHidden)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        // A held tile follows the cursor, and only a held one: hover alone
        // comes from the tiles' own mouse areas, so moving across the window
        // wakes nothing.
        let dragging = match self.tile_drag.is_some() {
            true => iced::event::listen_with(|event, _, _| match event {
                iced::Event::Mouse(iced::mouse::Event::CursorMoved { position }) => {
                    Some(Message::TileDragged(position.x))
                }
                iced::Event::Mouse(iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left)) => {
                    Some(Message::TileDropped)
                }
                _ => None,
            }),
            false => Subscription::none(),
        };

        Subscription::batch([pointing, dragging])
    }

    /// Polls, each only while there is something for it to watch.
    pub(crate) fn watches(&self) -> Subscription<Message> {
        // Resize events are only interesting to a human reading a trace, and
        // subscribing to them makes every resize step rebuild the view, so
        // this stays out of a normal run entirely.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        let registry = match self.registry_watch.is_some() {
            true => iced::time::every(REGISTRY_POLL).map(|_| Message::RegistryPoll),
            false => Subscription::none(),
        };
        let upgrade = self.upgrade_watch();
        // Only during a launch of ours: asking the process table whether a
        // game is up is what turns PLAY into STOP.
        let game_watch = match self.busy && self.launching.is_some() {
            true => iced::time::every(Duration::from_secs(2)).map(|_| Message::GameWatch),
            false => Subscription::none(),
        };

        Subscription::batch([resizes, registry, upgrade, game_watch])
    }

    /// The clock the progress bars and the splash animate off.
    pub(crate) fn tick(&self) -> Subscription<Message> {
        // Only while something is moving: an idle dashboard should wake up for
        // nothing. The splash's bar sweeps off the clock, so it needs the tick
        // as much as a live transfer does.
        match self.splash.is_some()
            || self.running().is_some()
            || self.setup_running
            || self.addons_busy
            || self.scanning
            || self.upgrading()
            || self.uninstall_dialog.as_ref().is_some_and(|d| d.working)
            || self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. }))
        {
            true => iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            false => Subscription::none(),
        }
    }
}
