//! A panel pinned above the widget it belongs to, drawn outside the layout.
//!
//! Every attempt to place the cog menu with ordinary layout traded one problem
//! for another: inside the action panel's column it sized the card, and
//! floating on the spacer above it the card's own top edge and padding fell
//! between the menu and the button. Neither is a spacing mistake — a column
//! cannot put a child *over* its sibling, and a `Stack` clips to its own
//! bounds.
//!
//! An overlay is the mechanism iced already uses for exactly this: it is laid
//! out against the whole window rather than against a parent, which is how a
//! `pick_list`'s dropdown escapes the row it sits in. This wraps any widget so
//! a second widget can be pinned directly above it, over whatever is behind.
//!
//! The anchor's bounds are known when the overlay is laid out, so the position
//! is computed rather than configured — there is no offset to keep in step
//! with the panel's padding or the window's size.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell, Widget};
use iced::event::{self, Event};
use iced::{Element, Length, Point, Rectangle, Size, Vector};

/// Gap between the panel and the widget it is pinned to.
const GAP: f32 = 8.0;

/// Pin `panel` directly above `anchor`, without either one affecting the
/// other's layout.
pub fn popover<'a, Message, Theme, Renderer>(
    anchor: impl Into<Element<'a, Message, Theme, Renderer>>,
    panel: Option<Element<'a, Message, Theme, Renderer>>,
) -> Popover<'a, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    Popover { anchor: anchor.into(), panel }
}

pub struct Popover<'a, Message, Theme, Renderer> {
    anchor: Element<'a, Message, Theme, Renderer>,
    /// `None` when nothing is showing, which costs no overlay at all.
    panel: Option<Element<'a, Message, Theme, Renderer>>,
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

    fn layout(&self, tree: &mut Tree, renderer: &Renderer, limits: &layout::Limits) -> layout::Node {
        // Only the anchor is laid out here. That is the whole point: the panel
        // takes no room, so opening it moves nothing.
        self.anchor.as_widget().layout(&mut tree.children[0], renderer, limits)
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

    fn on_event(
        &mut self,
        tree: &mut Tree,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) -> event::Status {
        self.anchor.as_widget_mut().on_event(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        )
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
        &self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.anchor.as_widget().operate(&mut tree.children[0], layout, renderer, operation);
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, Theme, Renderer>> {
        let panel = self.panel.as_mut()?;
        let (anchor_tree, panel_tree) = tree.children.split_at_mut(1);

        // The anchor may have an overlay of its own — a pick_list inside it,
        // say. Dropping it here would be a silent bug, so it is kept and the
        // two are grouped.
        let beneath = self.anchor.as_widget_mut().overlay(
            &mut anchor_tree[0],
            layout,
            renderer,
            translation,
        );

        let pinned = overlay::Element::new(Box::new(Pinned {
            panel,
            tree: &mut panel_tree[0],
            anchor: layout.bounds() + translation,
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
}

impl<Message, Theme, Renderer> overlay::Overlay<Message, Theme, Renderer>
    for Pinned<'_, '_, Message, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
    fn layout(&mut self, renderer: &Renderer, bounds: Size) -> layout::Node {
        let node = self.panel.as_widget().layout(
            self.tree,
            renderer,
            &layout::Limits::new(Size::ZERO, bounds),
        );
        let size = node.size();

        // Above the anchor, right edges aligned — the anchor is the right-hand
        // end of a row, so the panel hangs back over the content rather than
        // off the window.
        let x = self.anchor.x + self.anchor.width - size.width;
        let y = self.anchor.y - size.height - GAP;

        // Never off the top or the left: if there is no room above, the panel
        // drops below the anchor instead of being cut in half.
        let y = match y < 0.0 {
            true => self.anchor.y + self.anchor.height + GAP,
            false => y,
        };
        node.move_to(Point::new(x.max(0.0), y))
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

    fn on_event(
        &mut self,
        event: Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) -> event::Status {
        let bounds = layout.bounds();
        self.panel.as_widget_mut().on_event(
            self.tree,
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            &bounds,
        )
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        self.panel
            .as_widget()
            .mouse_interaction(self.tree, layout, cursor, viewport, renderer)
    }

    fn operate(
        &mut self,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        self.panel.as_widget().operate(self.tree, layout, renderer, operation);
    }

    fn is_over(&self, layout: Layout<'_>, _renderer: &Renderer, cursor: Point) -> bool {
        layout.bounds().contains(cursor)
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
