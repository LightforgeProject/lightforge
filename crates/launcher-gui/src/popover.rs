//! A panel pinned above the widget it belongs to, drawn outside the layout.
//!
//! Every attempt to place the cog menu with ordinary layout traded one problem
//! for another: inside the action panel's column it sized the card, and
//! floating on the spacer above it the card's own top edge and padding fell
//! between the menu and the button. Neither is a spacing mistake, a column
//! cannot put a child *over* its sibling, and a `Stack` clips to its own
//! bounds.
//!
//! An overlay is the mechanism iced already uses for exactly this: it is laid
//! out against the whole window rather than against a parent, which is how a
//! `pick_list`'s dropdown escapes the row it sits in. This wraps any widget so
//! a second widget can be pinned directly above it, over whatever is behind.
//!
//! The anchor's bounds are known when the overlay is laid out, so the position
//! is computed rather than configured. There is no offset to keep in step
//! with the panel's padding or the window's size.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell, Widget};
use iced::Event;
use iced::{Element, Length, Point, Rectangle, Size, Vector};

/// Gap between the panel and the widget it is pinned to.
const GAP: f32 = 8.0;

/// Which of the panel's vertical edges lines up with the anchor's.
///
/// The anchor's own position decides this, not taste. A button at the end of a
/// row has nothing to its right, so its panel hangs back over the content
/// ([`Edge::Right`]); a button in the middle of one reads as the panel's
/// left-hand corner ([`Edge::Left`]). Either way the panel is clamped to the
/// window, so the choice only decides which way it grows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    Left,
    Right,
}

/// Pin `panel` directly above `anchor`, without either one affecting the
/// other's layout.
pub fn popover<'a, Message, Theme, Renderer>(
    anchor: impl Into<Element<'a, Message, Theme, Renderer>>,
    panel: Option<Element<'a, Message, Theme, Renderer>>,
) -> Popover<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    Popover { anchor: anchor.into(), panel, edge: Edge::Right }
}

pub struct Popover<'a, Message, Theme, Renderer> {
    anchor: Element<'a, Message, Theme, Renderer>,
    /// `None` when nothing is showing, which costs no overlay at all.
    panel: Option<Element<'a, Message, Theme, Renderer>>,
    edge: Edge,
}

impl<'a, Message, Theme, Renderer> Popover<'a, Message, Theme, Renderer> {
    /// Line the panel up by its left edge instead of its right.
    pub fn align_left(mut self) -> Self {
        self.edge = Edge::Left;
        self
    }
}

impl<Message, Theme, Renderer> Widget<Message, Theme, Renderer>
    for Popover<'_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn children(&self) -> Vec<Tree> {
        // The panel keeps a slot whether or not it is showing, so its state
        // survives being opened and closed.
        match &self.panel {
            Some(panel) => vec![Tree::new(&self.anchor), Tree::new(panel)],
            None => vec![Tree::new(&self.anchor), Tree::empty()],
        }
    }

    fn diff(&self, tree: &mut Tree) {
        match &self.panel {
            Some(panel) => tree.diff_children(&[&self.anchor, panel]),
            None => tree.diff_children(&[&self.anchor]),
        }
    }

    fn size(&self) -> Size<Length> {
        self.anchor.as_widget().size()
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        limits: &layout::Limits,
    ) -> layout::Node {
        // Only the anchor is laid out here. That is the whole point: the panel
        // takes no room, so opening it moves nothing.
        self.anchor.as_widget_mut().layout(&mut tree.children[0], renderer, limits)
    }

    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.anchor.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.anchor
            .as_widget()
            .mouse_interaction(&tree.children[0], layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.anchor.as_widget_mut().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let panel = self.panel.as_mut()?;
        let (anchor_tree, panel_tree) = tree.children.split_at_mut(1);

        // The anchor may have an overlay of its own, a pick_list inside it,
        // say. Dropping it here would be a silent bug, so it is kept and the
        // two are grouped.
        let beneath = self.anchor.as_widget_mut().overlay(
            &mut anchor_tree[0],
            layout,
            renderer,
            viewport,
            translation,
        );

        let pinned = overlay::Element::new(Box::new(Pinned {
            panel,
            tree: &mut panel_tree[0],
            anchor: layout.bounds() + translation,
            edge: self.edge,
        }));

        Some(match beneath {
            Some(beneath) => overlay::Group::with_children(vec![beneath, pinned]).overlay(),
            None => pinned,
        })
    }
}

/// The panel, positioned from the anchor's bounds at layout time.
struct Pinned<'a, 'b, Message, Theme, Renderer> {
    panel: &'b mut Element<'a, Message, Theme, Renderer>,
    tree: &'b mut Tree,
    anchor: Rectangle,
    edge: Edge,
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Pinned<'_, '_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.panel.as_widget_mut().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let size = node.size();

        // Above the anchor, lined up by whichever edge [`Edge`] names. The
        // anchor's bounds come from this layout pass, so a panel pinned to a
        // button that moves, the band's buttons shift as games are added and
        // removed, follows it without anything being recomputed.
        let x = match self.edge {
            Edge::Right => self.anchor.x + self.anchor.width - size.width,
            Edge::Left => self.anchor.x,
        };
        let y = self.anchor.y - size.height - GAP;

        // Never off the top: if there is no room above, the panel drops below
        // the anchor instead of being cut in half.
        let y = match y < 0.0 {
            true => self.anchor.y + self.anchor.height + GAP,
            false => y,
        };

        // Never off either side. A panel wider than the window keeps its left
        // edge, since a left edge off screen hides the side you read from.
        let x = x.min(bounds.width - size.width).max(0.0);

        node.move_to(Point::new(x, y))
    }

    fn draw(
        &self,
        renderer: &mut Renderer,
        theme: &Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
    ) {
        self.panel.as_widget().draw(
            self.tree,
            renderer,
            theme,
            style,
            layout,
            cursor,
            &layout.bounds(),
        );
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        let bounds = layout.bounds();
        self.panel.as_widget_mut().update(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &bounds,
        );

        // The panel owns its rectangle, including the gaps between its rows.
        // Without this a pointer event that no entry wanted, a click in the
        // spacing, a scroll, carried on to whatever the panel is covering,
        // so clicking between two menu entries pressed the game tile beneath.
        // The panel has already had the event by this point; capturing only
        // stops it travelling further.
        if matches!(event, Event::Mouse(_) | Event::Touch(_)) && cursor.is_over(bounds) {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        // 0.14 dropped the viewport argument here. The panel is laid out
        // against the whole window, so its own bounds are its viewport.
        let bounds = layout.bounds();
        let interaction =
            self.panel.as_widget().mouse_interaction(self.tree, layout, cursor, &bounds, renderer);

        // What this answers decides more than the cursor's shape: iced hands
        // the page beneath an unavailable cursor only while an overlay reports
        // *some* interaction (`iced_runtime`'s `user_interface`). Reporting
        // `None` anywhere over the panel would leave the widgets under it
        // live, hovering and highlighting through a menu that covers them.
        match (interaction, cursor.is_over(bounds)) {
            (mouse::Interaction::None, true) => mouse::Interaction::Idle,
            (interaction, _) => interaction,
        }
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.panel.as_widget_mut().operate(self.tree, layout, renderer, operation);
    }
}

impl<'a, Message, Theme, Renderer> From<Popover<'a, Message, Theme, Renderer>>
    for Element<'a, Message, Theme, Renderer>
where
    Message: 'a,
    Theme: 'a,
    Renderer: iced::advanced::Renderer + 'a,
{
    fn from(popover: Popover<'a, Message, Theme, Renderer>) -> Self {
        Element::new(popover)
    }
}
