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
//! Without `HideOnClose` (no tray to go back to) that close quits the client
//! mid-launch (GlitchTip #43), so the window is unmapped from outside instead.
//! Measured on Hyprland 0.57, client 2.53.8, 2026-11-13: gone 21 ms after it
//! mapped, `WM_STATE` Withdrawn, the client stays up, and the next raise maps
//! the same window again.
//!
//! Wine maps each top-level as an X window (through XWayland under Wayland)
//! carrying the process's own pid in `_NET_WM_PID`, the pid pre-flight's scan
//! sees. That is what keeps a Battle.net in someone else's prefix untouched.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    AtomEnum, ChangeWindowAttributesAux, ClientMessageEvent, ConnectionExt, EventMask, MapState, Window,
};
use x11rb::rust_connection::RustConnection;
use x11rb::{CURRENT_TIME, NONE};

use super::LaunchStage;

/// How often the watch looks for news: X events, and the client's log.
const POLL: Duration = Duration::from_millis(20);

/// How often the window list is read with no event saying it changed, for a
/// window manager that announces nothing.
const RESCAN: Duration = Duration::from_millis(500);

/// The main window's title. The client's other top-levels, toasts and the
/// friends list, are left alone.
const MAIN_TITLE: &[u8] = b"Battle.net";

/// `WM_STATE`'s first field for a minimized window (ICCCM 4.1.3.1). A window
/// sent to the tray is withdrawn instead, and the window manager drops the
/// property or sets it to `WithdrawnState`. `_NET_WM_STATE_HIDDEN` is not read:
/// a manager may leave it on a withdrawn window, and a tray client would then
/// never be closed for a download.
const ICONIC_STATE: u32 = 3;

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
    wm_state: u32,
    wm_protocols: u32,
    wm_delete: u32,
    pids: Vec<u32>,
}

/// How one of the client's main windows sits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Showing {
    OnScreen,
    Minimized,
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
        let wm_state = atom(b"WM_STATE")?;
        let wm_protocols = atom(b"WM_PROTOCOLS")?;
        let wm_delete = atom(b"WM_DELETE_WINDOW")?;
        Ok(Self { conn, root, client_list, wm_pid, wm_state, wm_protocols, wm_delete, pids })
    }

    /// Whether someone has a main window open: on screen, or minimized.
    pub fn any_open(&self) -> Result<bool, WindowError> {
        Ok(!self.main_windows()?.is_empty())
    }

    /// Put away every main window that maps until `until`, while `stage` says
    /// the client has taken the launch: closed the way its X button does when
    /// that `to_tray`, unmapped otherwise. Not before:
    /// a window shown for a launch it has not taken may be a prompt, and one
    /// after an error is for the user. Done once the game runs, when the client
    /// puts the window away itself. A close is sent once per window, since the
    /// client hides it in answer; an unmapped window Wine shows again under
    /// the same id, so it is unmapped again. Returns how many times.
    pub fn keep_closed(
        &self,
        until: Instant,
        to_tray: bool,
        mut stage: impl FnMut() -> LaunchStage,
    ) -> Result<usize, WindowError> {
        // A new top-level, a frame mapped for one, or the manager's list changing.
        let watch = ChangeWindowAttributesAux::new().event_mask(EventMask::SUBSTRUCTURE_NOTIFY | EventMask::PROPERTY_CHANGE);
        self.conn.change_window_attributes(self.root, &watch)?.check()?;
        let mut asked = HashSet::new();
        let mut unmapped = 0;
        let mut changed = true;
        let mut scanned = Instant::now();
        while Instant::now() < until {
            while self.conn.poll_for_event()?.is_some() {
                changed = true;
            }
            match stage() {
                LaunchStage::Sent => {}
                LaunchStage::Taken if changed || scanned.elapsed() >= RESCAN => {
                    for (window, showing) in self.main_windows()? {
                        if showing != Showing::OnScreen {
                            continue;
                        }
                        match to_tray {
                            true if asked.insert(window) => self.close(window)?,
                            true => {}
                            false => {
                                self.conn.unmap_window(window)?;
                                self.conn.flush()?;
                                unmapped += 1;
                            }
                        }
                    }
                    changed = false;
                    scanned = Instant::now();
                }
                LaunchStage::Taken => {}
                LaunchStage::Failed | LaunchStage::Running => break,
            }
            std::thread::sleep(POLL);
        }
        Ok(asked.len() + unmapped)
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

    fn main_windows(&self) -> Result<Vec<(Window, Showing)>, WindowError> {
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

        let mut main = Vec::new();
        for window in ours {
            let title = self.conn.get_property(false, window, AtomEnum::WM_NAME, AtomEnum::ANY, 0, 64)?;
            let attributes = self.conn.get_window_attributes(window)?;
            let state = self.conn.get_property(false, window, self.wm_state, AtomEnum::ANY, 0, 1)?;
            let (Ok(title), Ok(attributes)) = (title.reply(), attributes.reply()) else { continue };
            if title.value != MAIN_TITLE {
                continue;
            }
            let wm_state = state.reply().ok().and_then(|reply| reply.value32().and_then(|mut v| v.next()));
            if let Some(showing) = showing(attributes.map_state, wm_state) {
                main.push((window, showing));
            }
        }
        Ok(main)
    }
}

/// Whether the display has a tray Wine can dock the client's icon in: an owner
/// of the XEmbed `_NET_SYSTEM_TRAY_S<screen>` selection, which is what Wine
/// looks for. KDE's xembedsniproxy is one; GNOME without its AppIndicator
/// extension and gamescope have none, and a client hidden there is unreachable.
pub fn tray_present() -> bool {
    let Ok((conn, screen)) = x11rb::connect(None) else {
        return false;
    };
    let name = format!("_NET_SYSTEM_TRAY_S{screen}");
    conn.intern_atom(false, name.as_bytes())
        .ok()
        .and_then(|cookie| cookie.reply().ok())
        .and_then(|atom| conn.get_selection_owner(atom.atom).ok()?.reply().ok())
        .is_some_and(|owner| owner.owner != NONE)
}

fn showing(map_state: MapState, wm_state: Option<u32>) -> Option<Showing> {
    match (map_state, wm_state) {
        (MapState::VIEWABLE, _) => Some(Showing::OnScreen),
        (_, Some(ICONIC_STATE)) => Some(Showing::Minimized),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minimized_window_counts_as_open() {
        assert_eq!(showing(MapState::UNMAPPED, Some(ICONIC_STATE)), Some(Showing::Minimized));
    }

    #[test]
    fn a_window_in_the_tray_does_not() {
        // Withdrawn: the property gone, or set to `WithdrawnState`.
        assert_eq!(showing(MapState::UNMAPPED, None), None);
        assert_eq!(showing(MapState::UNMAPPED, Some(0)), None);
    }

    #[test]
    fn a_mapped_window_is_on_screen_whatever_its_state_says() {
        assert_eq!(showing(MapState::VIEWABLE, Some(ICONIC_STATE)), Some(Showing::OnScreen));
    }
}
