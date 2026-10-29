//! What the app listens for between messages.

use crate::*;

impl Lightforge {
    /// The window closing, keys, gamepads, focus and second launches: listened
    /// to for the whole run.
    pub(crate) fn input_events() -> Subscription<Message> {
        let closing = iced::window::close_requests().map(Message::CloseRequested);

        // The hint chord and ring keys, only those no widget took.
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

        // Gamepads, for when Steam hands us a pad, not keys. Read on a
        // thread of their own for the whole run; see [`pad`].
        let pads = Subscription::run(pad::inputs).map(|input| Message::Nav(NavFrom::Pad, input));
        let raises = Subscription::run(crate::instance::raises).map(|()| Message::Raise);
        let window_focus = iced::event::listen_with(|event, _, _| match event {
            iced::Event::Window(iced::window::Event::Focused) => Some(Message::WindowFocused(true)),
            iced::Event::Window(iced::window::Event::Unfocused) => Some(Message::WindowFocused(false)),
            iced::Event::Window(iced::window::Event::Resized(_)) if fit::owned() => {
                Some(Message::WindowResized)
            }
            _ => None,
        });

        Subscription::batch([closing, hints, pads, raises, window_focus])
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

        // Only a held tile follows the cursor.
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
        // Only when tracing: it rebuilds the view on every resize step.
        let resizes = match trace::frames() {
            true => iced::window::resize_events().map(|(_, size)| Message::ResizeTraced(size)),
            false => Subscription::none(),
        };

        let registry = match self.registry_watch.is_some() {
            true => iced::time::every(REGISTRY_POLL).map(|_| Message::RegistryPoll),
            false => Subscription::none(),
        };
        let upgrade = self.upgrade_watch();
        // A scan is about 60 ms, cheap at this pace.
        let client = match self.config.battle_net_declined {
            true => Subscription::none(),
            false => iced::time::every(CLIENT_POLL).map(|_| Message::ClientWatch),
        };

        Subscription::batch([resizes, registry, upgrade, client])
    }

    /// The clock the progress bars and the splash animate off.
    pub(crate) fn tick(&self) -> Subscription<Message> {
        // Only while something moves, the splash's sweep included.
        match self.splash.is_some()
            || self.running().is_some()
            || self.setup_running
            || self.addons_busy
            || self.scanning
            || self.upgrading()
            || self.uninstall_dialog.as_ref().is_some_and(|d| d.working)
            || self.moving.is_some()
            || self.unused.values().any(|u| matches!(u, UnusedData::Freeing { .. }))
        {
            true => iced::time::every(Duration::from_millis(100)).map(|_| Message::Tick),
            false => Subscription::none(),
        }
    }
}
