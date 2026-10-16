//! Moving through the launcher without a mouse.
//!
//! A Steam Deck's controls reach us as keys: Steam Input turns the D-pad into
//! the arrow keys, A into Enter and B into Escape. iced has no focus for
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

use std::cell::RefCell;
use std::collections::HashMap;

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell, Widget};
use iced::{Border, Element, Event, Length, Point, Rectangle, Size, Vector};

use lightforge_core::product::ProductLine;

use crate::theme;
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
    Tweaks,
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
    /// What was visible of the region the control was drawn in. Inside a
    /// scrolled list both are in the list's own coordinates, which is what
    /// lets the list be scrolled to bring the control into view.
    pub viewport: Rectangle,
    pub press: Press,
    /// The scrolled region it is in, if any.
    pub region: Option<&'static str>,
    /// The innermost group or region it is in, if any.
    group: Option<String>,
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
    groups: Vec<String>,
    /// The control with the ring, as the app last said.
    current: Option<Target>,
    /// Waiting to be done by the widget concerned, on the next frame.
    click: Option<Target>,
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
}

impl Place {
    fn of(target: &Target, spot: &Spot) -> Self {
        Self { target: target.clone(), bounds: spot.bounds, group: spot.group.clone() }
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
        r.spots.insert(target, Spot { bounds, viewport: bounds, press, region: None, group: None, depth, live });
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

/// Where `step` goes from `current`, among `places`.
///
/// Up and down first stay inside the group the ring is in: down a card is
/// the next switch in that card, not the one in the card beside it that
/// happens to sit a little higher. Leaving a group, they go to what is
/// straight above or below. Outside any group, the header and the game band,
/// they go to the next row in that direction and then to the control in it
/// nearest sideways, so down from the header lands on the first row under it
/// wherever that row sits. Boxes that overlap sideways are no distance apart,
/// so up from any of the first few games is the wordmark that spans them.
///
/// Left and right stay on the line when there is anything on it, and only
/// then reach for the nearest control off it. Nothing in that direction
/// leaves the ring where it is.
pub fn step(places: &[Place], current: &Target, step: Step) -> Option<Target> {
    let here = places.iter().find(|p| &p.target == current)?;
    let from = here.bounds;

    // How far apart two spans are, zero when they overlap.
    let apart = |a0: f32, a1: f32, b0: f32, b1: f32| (b0 - a1).max(a0 - b1).max(0.0);
    // Two pixels of slack: neighbours drawn edge to edge, like PLAY and its
    // cog, share a border rather than leaving a gap.
    const SLACK: f32 = 2.0;
    // How far down a row may straggle and still be one row: a checkbox and
    // a button beside it do not share a top edge.
    const ROW: f32 = 16.0;

    // For each control in that direction: how far ahead, how far aside, and
    // how far apart the centres are, to break ties.
    struct Candidate<'t> {
        place: &'t Place,
        ahead: f32,
        aside: f32,
        centres: f32,
    }
    let candidates: Vec<Candidate> = places
        .iter()
        .filter(|p| &p.target != current)
        .filter_map(|p| {
            let to = p.bounds;
            let (ahead, aside, centres) = match step {
                Step::Down => (
                    to.y - (from.y + from.height),
                    apart(from.x, from.x + from.width, to.x, to.x + to.width),
                    (to.center_x() - from.center_x()).abs(),
                ),
                Step::Up => (
                    from.y - (to.y + to.height),
                    apart(from.x, from.x + from.width, to.x, to.x + to.width),
                    (to.center_x() - from.center_x()).abs(),
                ),
                Step::Right => (
                    to.x - (from.x + from.width),
                    apart(from.y, from.y + from.height, to.y, to.y + to.height),
                    (to.center_y() - from.center_y()).abs(),
                ),
                Step::Left => (
                    from.x - (to.x + to.width),
                    apart(from.y, from.y + from.height, to.y, to.y + to.height),
                    (to.center_y() - from.center_y()).abs(),
                ),
            };
            (ahead >= -SLACK).then_some(Candidate { place: p, ahead: ahead.max(0.0), aside, centres })
        })
        .collect();

    let pick = |pool: Vec<&Candidate>, key: &dyn Fn(&Candidate) -> f32| {
        pool.into_iter().min_by(|a, b| key(a).total_cmp(&key(b))).map(|c| c.place.target.clone())
    };
    // The next row in that direction, then the nearest sideways in it.
    let next_row = |pool: Vec<&Candidate>| {
        let nearest = pool.iter().map(|c| c.ahead).min_by(f32::total_cmp)?;
        let row = pool.into_iter().filter(|c| c.ahead <= nearest + ROW).collect();
        pick(row, &|c| c.aside + 0.01 * c.centres)
    };

    match step {
        Step::Up | Step::Down => {
            let Some(group) = &here.group else {
                return next_row(candidates.iter().collect());
            };
            let inside: Vec<_> = candidates.iter().filter(|c| c.place.group.as_ref() == Some(group)).collect();
            if !inside.is_empty() {
                return next_row(inside);
            }
            let straight: Vec<_> = candidates.iter().filter(|c| c.aside == 0.0).collect();
            match straight.is_empty() {
                false => next_row(straight),
                true => next_row(candidates.iter().collect()),
            }
        }
        Step::Left | Step::Right => {
            let line: Vec<_> = candidates.iter().filter(|c| c.aside == 0.0).collect();
            match line.is_empty() {
                false => pick(line, &|c| c.ahead + 0.01 * c.centres),
                true => pick(candidates.iter().collect(), &|c| c.ahead + 2.0 * c.aside + 0.01 * c.centres),
            }
        }
    }
}

/// The control nearest the top left, where reading starts.
pub fn first(places: &[Place]) -> Option<Target> {
    places
        .iter()
        .min_by(|a, b| a.bounds.y.total_cmp(&b.bounds.y).then(a.bounds.x.total_cmp(&b.bounds.x)))
        .map(|p| p.target.clone())
}

/// `content`, as a control the ring can land on.
pub fn ring<'a>(target: Target, press: Press, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Ring { content: content.into(), target, press })
}

/// `content`, as a dialog or menu over everything drawn before it.
pub fn layer<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Layer { content: content.into() })
}

/// `content`, a scrolled list the ring can scroll, named `id`.
pub fn region<'a>(id: &'static str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Region { content: content.into(), id })
}

/// `content`, controls that belong together, named `id`; see [`step`].
pub fn group<'a>(id: impl Into<String>, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Group { content: content.into(), id: id.into() })
}

/// The window's content, with the record started afresh for each frame.
pub fn root<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Root { content: content.into() })
}

struct Ring<'a> {
    content: Element<'a, Message>,
    target: Target,
    press: Press,
}

struct Layer<'a> {
    content: Element<'a, Message>,
}

struct Region<'a> {
    content: Element<'a, Message>,
    id: &'static str,
}

struct Group<'a> {
    content: Element<'a, Message>,
    id: String,
}

struct Root<'a> {
    content: Element<'a, Message>,
}

/// Takes the typing away from whatever field in a control has it, and says
/// whether one did.
struct Leave(bool);

impl Operation for Leave {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }

    fn focusable(
        &mut self,
        _id: Option<&iced::advanced::widget::Id>,
        _bounds: Rectangle,
        state: &mut dyn iced::advanced::widget::operation::Focusable,
    ) {
        if state.is_focused() {
            state.unfocus();
            self.0 = true;
        }
    }
}

/// How far outside a clicked control its ring is drawn, above and below
/// and to either side. Wider at the sides: a checkbox's box and the last
/// letter of its label sit right on its edges, and four points there still
/// read as touching, while the rows above and below are only so far away.
const RING_GAP_Y: f32 = 5.0;
const RING_GAP_X: f32 = 10.0;

/// Hand a made-up mouse event to `content`, as if the pointer were at `at`.
///
/// Everything `Widget::update` takes, which is what makes it this long.
#[allow(clippy::too_many_arguments)]
fn pretend(
    content: &mut Element<'_, Message>,
    tree: &mut Tree,
    event: iced::mouse::Event,
    at: Point,
    layout: Layout<'_>,
    renderer: &iced::Renderer,
    clipboard: &mut dyn Clipboard,
    shell: &mut Shell<'_, Message>,
    viewport: &Rectangle,
) {
    content.as_widget_mut().update(
        tree,
        &Event::Mouse(event),
        layout,
        mouse::Cursor::Available(at),
        renderer,
        clipboard,
        shell,
        viewport,
    );
}

/// Everything a wrapper hands straight to the one widget it wraps, except
/// `draw` and `update`, which are what each wrapper is for.
macro_rules! delegate {
    () => {
        fn tag(&self) -> tree::Tag {
            self.content.as_widget().tag()
        }

        fn state(&self) -> tree::State {
            self.content.as_widget().state()
        }

        fn children(&self) -> Vec<Tree> {
            self.content.as_widget().children()
        }

        fn diff(&self, tree: &mut Tree) {
            self.content.as_widget().diff(tree);
        }

        fn size(&self) -> Size<Length> {
            self.content.as_widget().size()
        }

        fn size_hint(&self) -> Size<Length> {
            self.content.as_widget().size_hint()
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &iced::Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            self.content.as_widget_mut().layout(tree, renderer, limits)
        }

        fn mouse_interaction(
            &self,
            tree: &Tree,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            viewport: &Rectangle,
            renderer: &iced::Renderer,
        ) -> mouse::Interaction {
            self.content.as_widget().mouse_interaction(tree, layout, cursor, viewport, renderer)
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout<'_>,
            renderer: &iced::Renderer,
            operation: &mut dyn Operation,
        ) {
            self.content.as_widget_mut().operate(tree, layout, renderer, operation);
        }

        fn overlay<'b>(
            &'b mut self,
            tree: &'b mut Tree,
            layout: Layout<'b>,
            renderer: &iced::Renderer,
            viewport: &Rectangle,
            translation: Vector,
        ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
            self.content.as_widget_mut().overlay(tree, layout, renderer, viewport, translation)
        }
    };
}

/// `update` for a wrapper with nothing of its own to do there.
macro_rules! pass_update {
    () => {
        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout<'_>,
            cursor: mouse::Cursor,
            renderer: &iced::Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            self.content
                .as_widget_mut()
                .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
        }
    };
}

impl Widget<Message, iced::Theme, iced::Renderer> for Ring<'_> {
    delegate!();

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        // Escape in a field that has the typing leaves the field and goes
        // no further. A text box lets it through once it lets go, by design,
        // and the same key then closed the dialog and the words with it.
        if let Event::Keyboard(iced::keyboard::Event::KeyPressed {
            key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
            ..
        }) = event
        {
            let mut leave = Leave(false);
            self.content.as_widget_mut().operate(tree, layout, renderer, &mut leave);
            if leave.0 {
                shell.capture_event();
                shell.request_redraw();
                return;
            }
        }

        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);

        // A click asked for from the keyboard, done here because only the
        // widget itself knows where the pointer would have to be: inside a
        // scrolled list, its bounds are in the list's own coordinates.
        let mine = with(|r| match r.click.as_ref() == Some(&self.target) {
            true => r.click.take().is_some(),
            false => false,
        });
        if mine {
            let at = layout.bounds().center();
            for event in [
                iced::mouse::Event::ButtonPressed(iced::mouse::Button::Left),
                iced::mouse::Event::ButtonReleased(iced::mouse::Button::Left),
            ] {
                pretend(&mut self.content, tree, event, at, layout, renderer, clipboard, shell, viewport);
            }
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        use iced::advanced::Renderer as _;

        let bounds = layout.bounds();
        // A control to be clicked is live when the pointer over it would
        // say so: a disabled button shows no hand.
        let live = match &self.press {
            Press::Send(_) => true,
            Press::Off => false,
            Press::Click => !matches!(
                self.content.as_widget().mouse_interaction(
                    tree,
                    layout,
                    mouse::Cursor::Available(bounds.center()),
                    viewport,
                    renderer,
                ),
                mouse::Interaction::None | mouse::Interaction::Idle
            ),
        };
        let on = with(|r| {
            let spot = Spot {
                bounds,
                viewport: *viewport,
                press: self.press.clone(),
                region: r.within.last().copied(),
                group: r.groups.last().cloned(),
                depth: r.depth,
                live,
            };
            r.spots.insert(self.target.clone(), spot);
            r.current.as_ref() == Some(&self.target)
        });

        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);

        // White rather than the accent: INSTALL is filled with nearly the
        // accent's blue, and a ring the colour of the button it is on is no
        // ring at all. A layer of its own, so it lands over the control's
        // picture and text and not under them.
        //
        // Drawn round a control to be clicked rather than on its edge: those
        // are checkboxes and fields, whose bounds end at the box or the
        // label's last letter, and a ring on that line touches both.
        if on {
            let (ring, radius) = match self.press {
                Press::Click => {
                    (bounds.expand([RING_GAP_Y, RING_GAP_X]), theme::RADIUS_SM + RING_GAP_Y)
                }
                Press::Send(_) | Press::Off => (bounds, theme::RADIUS_SM),
            };
            renderer.with_layer(ring, |renderer| {
                renderer.fill_quad(
                    renderer::Quad {
                        bounds: ring,
                        border: Border {
                            color: theme::TEXT,
                            width: 2.0,
                            radius: radius.into(),
                        },
                        ..renderer::Quad::default()
                    },
                    iced::Color::TRANSPARENT,
                );
            });
        }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Layer<'_> {
    delegate!();
    pass_update!();

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Counted whether or not anything in it can take the ring: a dialog
        // with nothing to press still covers the page.
        with(|r| {
            r.depth += 1;
            r.top = r.top.max(r.depth);
        });
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        with(|r| r.depth -= 1);
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Region<'_> {
    delegate!();

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content
            .as_widget_mut()
            .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);

        let dy = with(|r| match r.scroll {
            Some((id, dy)) if id == self.id => {
                r.scroll = None;
                Some(dy)
            }
            _ => None,
        });
        if let Some(dy) = dy {
            // A wheel turned over the middle of the list, which is the one
            // scroll every scrollable already understands. Positive is up.
            let wheel = iced::mouse::Event::WheelScrolled {
                delta: iced::mouse::ScrollDelta::Pixels { x: 0.0, y: -dy },
            };
            let at = layout.bounds().center();
            pretend(&mut self.content, tree, wheel, at, layout, renderer, clipboard, shell, viewport);
            shell.request_redraw();
        }
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        let bounds = layout.bounds();
        with(|r| {
            let depth = r.depth;
            r.regions.insert(self.id, (bounds.width * bounds.height, depth));
            r.within.push(self.id);
            r.groups.push(self.id.to_owned());
        });
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        with(|r| {
            r.within.pop();
            r.groups.pop();
        });
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Group<'_> {
    delegate!();
    pass_update!();

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        with(|r| r.groups.push(self.id.clone()));
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        with(|r| {
            r.groups.pop();
        });
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for Root<'_> {
    delegate!();
    pass_update!();

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        // Overlays, the menus, are drawn after this returns, so their
        // controls land in the fresh record too.
        with(|r| {
            r.spots.clear();
            r.regions.clear();
            r.top = 0;
            r.depth = 0;
            r.within.clear();
            r.groups.clear();
        });
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32, y: f32, width: f32, height: f32) -> Rectangle {
        Rectangle { x, y, width, height }
    }

    /// The dashboard's shape at Deck size, from a screenshot: the header,
    /// the band, the panel. Nothing on it is grouped.
    fn page() -> Vec<Place> {
        let places = vec![
            (Target::AppMenu, at(40.0, 10.0, 240.0, 30.0)),
            (Target::Tab(Screen::Dashboard), at(390.0, 10.0, 40.0, 30.0)),
            (Target::Tab(Screen::News), at(455.0, 10.0, 50.0, 30.0)),
            (Target::Tile(ProductLine::Retail), at(110.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::Classic), at(236.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::ClassicEra), at(300.0, 58.0, 56.0, 48.0)),
            (Target::Tile(ProductLine::Forever), at(385.0, 58.0, 56.0, 48.0)),
            (Target::BattleNet, at(1135.0, 66.0, 90.0, 32.0)),
            (Target::Key("picker version".to_owned()), at(107.0, 668.0, 56.0, 22.0)),
            (Target::Addons, at(62.0, 722.0, 118.0, 24.0)),
            (Target::Primary, at(983.0, 660.0, 185.0, 56.0)),
            (Target::Gear, at(1168.0, 660.0, 50.0, 56.0)),
            (Target::LaunchViaBattleNet, at(983.0, 722.0, 235.0, 30.0)),
        ];
        places.into_iter().map(|(target, bounds)| Place { target, bounds, group: None }).collect()
    }

    /// Two cards side by side, the right one's rows sitting a little lower,
    /// the way Game Optimizations draws Performance beside Looks.
    #[test]
    fn down_a_card_stays_in_the_card() {
        let place = |name: &str, x: f32, y: f32, group: &str| Place {
            target: Target::Key(name.to_owned()),
            bounds: at(x, y, 200.0, 20.0),
            group: Some(group.to_owned()),
        };
        let cards = vec![
            Place { target: Target::AppMenu, bounds: at(40.0, 10.0, 240.0, 30.0), group: None },
            place("custom", 880.0, 60.0, "presets"),
            place("cpu", 50.0, 120.0, "performance"),
            place("water", 50.0, 160.0, "performance"),
            place("clutter", 660.0, 120.0, "looks"),
            place("glow", 660.0, 140.0, "looks"),
            place("api", 660.0, 300.0, "advanced"),
        ];
        let key = |name: &str| Some(Target::Key(name.to_owned()));
        // Out of the header: the next row, the presets, however far right.
        assert_eq!(step(&cards, &Target::AppMenu, Step::Down), key("custom"));
        // Down the Looks card, then out of it straight down, not across.
        assert_eq!(step(&cards, &Target::Key("clutter".to_owned()), Step::Down), key("glow"));
        assert_eq!(step(&cards, &Target::Key("glow".to_owned()), Step::Down), key("api"));
    }

    #[test]
    fn up_from_the_first_game_is_the_logo_above_it() {
        let page = page();
        assert_eq!(step(&page, &Target::Tile(ProductLine::Retail), Step::Up), Some(Target::AppMenu));
        assert_eq!(step(&page, &Target::Tile(ProductLine::Classic), Step::Up), Some(Target::AppMenu));
        // Under a tab rather than the wordmark, the tab straight above wins.
        assert_eq!(
            step(&page, &Target::Tile(ProductLine::Forever), Step::Up),
            Some(Target::Tab(Screen::Dashboard))
        );
    }

    #[test]
    fn down_goes_to_what_is_below_not_to_a_fixed_place() {
        let page = page();
        // The next row down is the version picker's, not the links under it.
        assert_eq!(
            step(&page, &Target::Tile(ProductLine::Retail), Step::Down),
            Some(Target::Key("picker version".to_owned()))
        );
        assert_eq!(step(&page, &Target::Key("picker version".to_owned()), Step::Down), Some(Target::Addons));
        // Down from the header is the next row, however far to the side.
        assert_eq!(step(&page, &Target::AppMenu, Step::Down), Some(Target::Tile(ProductLine::Retail)));
        assert_eq!(step(&page, &Target::Tab(Screen::News), Step::Down), Some(Target::Tile(ProductLine::Forever)));
        assert_eq!(step(&page, &Target::BattleNet, Step::Down), Some(Target::Gear));
        assert_eq!(step(&page, &Target::Primary, Step::Down), Some(Target::LaunchViaBattleNet));
        assert_eq!(step(&page, &Target::Primary, Step::Right), Some(Target::Gear));
    }

    #[test]
    fn nothing_in_that_direction_stays_put() {
        let page = page();
        assert_eq!(step(&page, &Target::AppMenu, Step::Up), None);
        assert_eq!(step(&page, &Target::AppMenu, Step::Left), None);
        assert_eq!(first(&page), Some(Target::AppMenu));
    }
}
