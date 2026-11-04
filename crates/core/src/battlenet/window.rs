//! The client's main window, over X11.
//!
//! The client raises its window for every command a second invocation hands
//! it, a hand-off included, and closes it again only once Agent reports the
//! game running: on screen 1.7 s warm, 3.3 s cold (client 2.53.6, measured
//! 2026-11-04). No flag stops it. Sending the `WM_DELETE_WINDOW` its X button
//! sends as soon as it maps cut that to about 20 ms, and with `HideOnClose`
//! the client puts itself back in the tray, so Wine's idea of the window
//! stays its own and a later plain start shows it again.
//!
//! Wine maps each top-level as an X window (through XWayland under Wayland)
//! carrying the process's own pid in `_NET_WM_PID`, the pid pre-flight's scan
//! sees. That is what keeps a Battle.net in someone else's prefix untouched.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ClientMessageEvent, ConnectionExt, EventMask, MapState, Window};
use x11rb::rust_connection::RustConnection;
use x11rb::CURRENT_TIME;

/// How often the window list is read while watching.
const POLL: Duration = Duration::from_millis(20);

/// The main window's title. The client's other top-levels, toasts and the
/// friends list, are left alone.
const MAIN_TITLE: &[u8] = b"Battle.net";

#[derive(Debug, thiserror::Error)]
pub enum WindowError {
    #[error("no X display to reach Battle.net's window on")]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error("lost the X display while watching Battle.net's window")]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error("an X request about Battle.net's window failed")]
    Reply(#[from] x11rb::errors::ReplyError),
}

/// The main windows of the client processes it was made for.
pub struct ClientWindows {
    conn: RustConnection,
    root: Window,
    client_list: u32,
    wm_pid: u32,
    wm_protocols: u32,
    wm_delete: u32,
    pids: Vec<u32>,
}

impl ClientWindows {
    /// Reach `$DISPLAY` for the windows of `pids`, the client's own
    /// processes.
    pub fn connect(pids: Vec<u32>) -> Result<Self, WindowError> {
        let (conn, screen) = x11rb::connect(None)?;
        let root = conn.setup().roots[screen].root;
        let atom = |name: &[u8]| -> Result<u32, WindowError> {
            Ok(conn.intern_atom(false, name)?.reply()?.atom)
        };
        let client_list = atom(b"_NET_CLIENT_LIST")?;
        let wm_pid = atom(b"_NET_WM_PID")?;
        let wm_protocols = atom(b"WM_PROTOCOLS")?;
        let wm_delete = atom(b"WM_DELETE_WINDOW")?;
        Ok(Self { conn, root, client_list, wm_pid, wm_protocols, wm_delete, pids })
    }

    /// Whether a main window is on screen.
    pub fn any_shown(&self) -> Result<bool, WindowError> {
        Ok(!self.shown()?.is_empty())
    }

    /// Close every main window that maps until `until`, the way its X button
    /// does. Each is asked once: Wine makes a new X window for every showing.
    /// Returns how many were closed.
    pub fn keep_closed(&self, until: Instant) -> Result<usize, WindowError> {
        let mut asked = HashSet::new();
        while Instant::now() < until {
            for window in self.shown()? {
                if asked.insert(window) {
                    self.close(window)?;
                }
            }
            std::thread::sleep(POLL);
        }
        Ok(asked.len())
    }

    fn close(&self, window: Window) -> Result<(), WindowError> {
        let event = ClientMessageEvent::new(
            32,
            window,
            self.wm_protocols,
            [self.wm_delete, CURRENT_TIME, 0, 0, 0],
        );
        self.conn.send_event(false, window, EventMask::NO_EVENT, event)?;
        self.conn.flush()?;
        Ok(())
    }

    fn shown(&self) -> Result<Vec<Window>, WindowError> {
        // The root's children and the window manager's own list: a manager
        // that reparents into frames (GNOME) only names the client in the
        // latter, one without EWMH only has the former.
        let mut windows = self.conn.query_tree(self.root)?.reply()?.children;
        let listed = self
            .conn
            .get_property(false, self.root, self.client_list, AtomEnum::WINDOW, 0, u32::MAX)?
            .reply()?;
        windows.extend(listed.value32().into_iter().flatten());
        windows.sort_unstable();
        windows.dedup();

        // One round trip for all of them, not one per window.
        let pids = windows
            .iter()
            .map(|&w| self.conn.get_property(false, w, self.wm_pid, AtomEnum::CARDINAL, 0, 1))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ours = Vec::new();
        for (window, cookie) in windows.into_iter().zip(pids) {
            // A window destroyed in between answers with an error, not a pid.
            let Ok(reply) = cookie.reply() else { continue };
            if reply.value32().and_then(|mut v| v.next()).is_some_and(|pid| self.pids.contains(&pid)) {
                ours.push(window);
            }
        }

        let mut shown = Vec::new();
        for window in ours {
            let title = self.conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 64)?;
            let attributes = self.conn.get_window_attributes(window)?;
            let (Ok(title), Ok(attributes)) = (title.reply(), attributes.reply()) else { continue };
            if attributes.map_state == MapState::VIEWABLE && title.value == MAIN_TITLE {
                shown.push(window);
            }
        }
        Ok(shown)
    }
}
