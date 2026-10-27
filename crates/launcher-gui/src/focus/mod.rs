//! Moving through the launcher without a mouse.
//!
//! A controller reaches us as keys or as a gamepad, depending on the Steam
//! Input layout: the D-pad is the arrows, A is Enter and B is Escape either
//! way, the gamepad half read by [`crate::pad`]. iced has no focus for
//! buttons, so the launcher keeps its own: one selected control at a time,
//! drawn with a ring, moved by the arrows and pressed by Enter.
//!
//! Every control that can take the ring is wrapped in [`ring`], which writes
//! down where it was drawn and how to press it. The arrows then move to the
//! nearest control in that direction on the page as it was last drawn, the
//! way a television remote does, so up from the leftmost game is the logo
//! above it and not some fixed place.
//!
//! Three more wrappers shape where the ring may go:
//!
//! - [`root`] goes round the whole window and starts each frame's record
//!   afresh, so a control that is no longer drawn is no longer a place.
//! - [`layer`] goes round a dialog or a menu. Only the topmost layer drawn
//!   is reachable, the way only it is clickable.
//! - [`region`] goes round a scrolled list or page. The ring scrolls it to
//!   keep itself in view, and an arrow with nowhere to go scrolls it instead,
//!   which is how release notes are read with a D-pad.
//! - [`group`] goes round controls that belong together, a card or a list.
//!   Up and down stay inside it while there is anywhere to go, so the ring
//!   walks down a card rather than across into the one beside it. A region
//!   is a group too.
//!
//! The record is per thread: iced draws and updates on one, and each test
//! gets its own.

mod nav;
mod typing;
mod widgets;

pub use nav::{first, nearest, step};
pub use widgets::{group, layer, pane, region, ring, root};

use std::cell::RefCell;
use std::collections::HashMap;

use iced::Rectangle;

use lightforge_core::product::ProductLine;

use crate::{Message, Screen};


/// Something that can be selected with the keyboard.
///
/// Stable from one frame to the next, so the ring stays on a control while
/// the page is redrawn around it.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Target {
    /// The wordmark, which opens the app menu.
    AppMenu,
    Back,
    Forward,
    Tab(Screen),
    Shop,
    Downloads,
    Tile(ProductLine),
    /// The band's overflow button, and the games behind it.
    More,
    Overflow(ProductLine),
    AddGame,
    BattleNet,
    Addons,
    GameSettings,
    Subscription,
    Primary,
    Gear,
    LaunchViaBattleNet,
    /// A hand-off game's link to how Battle.net launches.
    HandOffSettings,
    /// An entry in the app menu or the gear menu, by its label.
    Menu(&'static str),
    /// One of the marks at the foot of the app menu, by its address.
    Link(&'static str),
    /// "Got it" on the release notes.
    NotesDismiss,
    /// The update notice's two answers.
    UpdateLater,
    UpdateOpen,
    /// A section in the list down the left of Settings.
    SettingsTab(crate::SettingsTab),
    /// Anything else, by a name unique on its page: a button by the message
    /// it sends, a field by its label.
    Key(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Up,
    Down,
    Left,
    Right,
}

/// What Enter does to a control.
#[derive(Debug, Clone)]
pub enum Press {
    /// Send this, as its click would.
    Send(Message),
    /// Nothing: the control is disabled, and the ring passes over it.
    Off,
    /// Click it, for a control whose click is more than one message: a
    /// checkbox, a text field that should take the typing.
    Click,
}

/// Where a control was drawn, and how to press it.
#[derive(Debug, Clone)]
pub struct Spot {
    pub bounds: Rectangle,
    /// Where it is in the window, which is what the arrows compare: inside
    /// a scrolled list `bounds` moves with the scrolling, so a row far down
    /// the list would otherwise sit below the buttons under the list.
    screen: Rectangle,
    /// What was visible of the region the control was drawn in. Inside a
    /// scrolled list both are in the list's own coordinates, which is what
    /// lets the list be scrolled to bring the control into view.
    pub viewport: Rectangle,
    pub press: Press,
    /// Whether it is a text field, which on a console wants a keyboard.
    pub field: bool,
    /// Whether that field takes several lines, the bug report's.
    pub lines: bool,
    /// The scrolled region it is in, if any.
    pub region: Option<&'static str>,
    /// The innermost group or region it is in, if any.
    group: Option<String>,
    /// The innermost pane it is in, if any.
    pane: Option<&'static str>,
    depth: u32,
    live: bool,
}

#[derive(Default)]
struct Record {
    spots: HashMap<Target, Spot>,
    /// Scrolled regions drawn this frame: their size, and their layer.
    regions: HashMap<&'static str, (f32, u32)>,
    /// The deepest layer drawn this frame.
    top: u32,
    /// While drawing: the layer, the regions and the groups being drawn into.
    depth: u32,
    within: Vec<&'static str>,
    /// What shows of each region being drawn into, in the window.
    shown: Vec<Rectangle>,
    panes: Vec<&'static str>,
    groups: Vec<String>,
    /// The control with the ring, as the app last said.
    current: Option<Target>,
    /// The control whose field has the cursor in it, as last drawn.
    typing: Option<Target>,
    /// Waiting to be done by the widget concerned, on the next frame.
    click: Option<Target>,
    leave: Option<Target>,
    scroll: Option<(&'static str, f32)>,
}

thread_local! {
    static RECORD: RefCell<Record> = RefCell::new(Record::default());
}

fn with<T>(f: impl FnOnce(&mut Record) -> T) -> T {
    RECORD.with(|record| f(&mut record.borrow_mut()))
}

/// Which control has the ring. Called by the view, before anything draws.
pub fn set_current(target: Option<Target>) {
    with(|r| r.current = target);
}

/// A control the ring can move to: what it is, where, and in which group.
#[derive(Debug, Clone)]
pub struct Place {
    pub target: Target,
    pub bounds: Rectangle,
    pub group: Option<String>,
    /// The [`pane`] it is in, if any, which the arrows never leave.
    pub pane: Option<&'static str>,
}

impl Place {
    fn of(target: &Target, spot: &Spot) -> Self {
        Self { target: target.clone(), bounds: spot.screen, group: spot.group.clone(), pane: spot.pane }
    }
}

/// Every control in the topmost layer that would do something if pressed.
pub fn live() -> Vec<Place> {
    with(|r| {
        r.spots
            .iter()
            .filter(|(_, spot)| spot.live && spot.depth == r.top)
            .map(|(target, spot)| Place::of(target, spot))
            .collect()
    })
}

/// The controls of [`live`] that are inside the scrolled region `region`.
pub fn live_in(region: &str) -> Vec<Place> {
    with(|r| {
        r.spots
            .iter()
            .filter(|(_, spot)| spot.live && spot.depth == r.top && spot.region == Some(region))
            .map(|(target, spot)| Place::of(target, spot))
            .collect()
    })
}

pub fn spot(target: &Target) -> Option<Spot> {
    with(|r| r.spots.get(target).cloned())
}

/// The largest scrolled region in the topmost layer: the page, or the
/// dialog's text.
pub fn main_region() -> Option<&'static str> {
    with(|r| {
        r.regions
            .iter()
            .filter(|(_, (_, depth))| *depth == r.top)
            .max_by(|a, b| a.1 .0.total_cmp(&b.1 .0))
            .map(|(id, _)| *id)
    })
}

/// Click `target` on the next frame.
pub fn click(target: Target) {
    with(|r| r.click = Some(target));
}

impl Spot {
    /// Where it is in the window.
    pub fn screen(&self) -> Rectangle {
        self.screen
    }
}

/// The control whose text field has the cursor in it, if one does.
pub fn typing_in() -> Option<Target> {
    with(|r| r.typing.clone())
}

/// Take the cursor out of `target`'s field on the next frame, the way
/// Escape in it does.
pub fn leave(target: Target) {
    with(|r| r.leave = Some(target));
}

/// Scroll `region` by `dy` points on the next frame, down for positive.
pub fn scroll(region: &'static str, dy: f32) {
    with(|r| r.scroll = Some((region, dy)));
}

/// Record `target` as drawn in layer `depth`, the way a frame would.
#[cfg(test)]
pub fn put(target: Target, bounds: Rectangle, press: Press, depth: u32) {
    with(|r| {
        let live = !matches!(press, Press::Off);
        r.top = r.top.max(depth);
        let spot = Spot {
            bounds,
            screen: bounds,
            viewport: bounds,
            press,
            field: false,
            lines: false,
            region: None,
            group: None,
            pane: None,
            depth,
            live,
        };
        r.spots.insert(target, spot);
    });
}

/// Record `target` as drawn inside the [`pane`] `id`.
#[cfg(test)]
pub fn put_in(id: &'static str, target: Target, bounds: Rectangle, press: Press) {
    put(target.clone(), bounds, press, 0);
    with(|r| {
        let spot = r.spots.get_mut(&target).expect("just put");
        spot.region = Some(id);
        spot.pane = Some(id);
    });
}

/// Forget the last frame, as the next one starting would.
#[cfg(test)]
pub fn reset() {
    with(|r| {
        r.spots.clear();
        r.top = 0;
    });
}
