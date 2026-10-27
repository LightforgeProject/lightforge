//! Text fields: telling when one has the cursor, taking it away, and the
//! copy of it drawn above Steam's keyboard.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell};
use iced::{Border, Element, Event, Point, Rectangle, Size, Vector};

use crate::Message;

/// Takes the typing away from whatever field in a control has it, and says
/// whether one did.
pub(super) struct Leave(pub(super) bool);

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

type Field = iced::widget::text_input::State<<iced::Renderer as iced::advanced::text::Renderer>::Paragraph>;
/// A box of several lines, the bug report's.
pub(super) type Editor = iced::widget::text_editor::State<iced::advanced::text::highlighter::PlainText>;

/// Whether `tree` is a text field's, of one line or several.
pub(super) fn field(tree: &Tree) -> bool {
    tree.tag == tree::Tag::of::<Field>() || tree.tag == tree::Tag::of::<Editor>()
}

/// Whether `tree` is a text field with the cursor in it.
pub(super) fn typing(tree: &Tree) -> bool {
    if tree.tag == tree::Tag::of::<Field>() {
        return tree.state.downcast_ref::<Field>().is_focused();
    }
    tree.tag == tree::Tag::of::<Editor>() && tree.state.downcast_ref::<Editor>().is_focused()
}

/// Hand a made-up mouse event to `content`, as if the pointer were at `at`.
///
/// Everything `Widget::update` takes, which is what makes it this long.
#[allow(clippy::too_many_arguments)]
pub(super) fn pretend(
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

/// How far down the window Steam's keyboard starts, as a share of its
/// height. Measured 2026-10-17 at 1280x838: the keyboard took the bottom
/// 250 points, so from about 0.7; a field ending below half is taken as
/// covered, with room to spare for a taller one.
const KEYBOARD_TOP: f32 = 0.5;

/// A copy of the field being typed in, across the top of the window.
pub(super) struct TypingBar<'a, 'b> {
    pub(super) content: &'b Element<'a, Message>,
    pub(super) tree: &'b Tree,
    pub(super) field: Layout<'b>,
    /// Where the field is in the window.
    pub(super) screen: Rectangle,
    /// Whether the field is low enough for the keyboard to cover it. One
    /// higher up is already in sight, and a copy of it read as two fields.
    pub(super) shown: bool,
}

impl TypingBar<'_, '_> {
    /// Where the field is drawn inside `bar`: centred, and at least the
    /// field's own size, so a field as wide as a dialog keeps its width.
    fn spot(&self, bar: Rectangle) -> Point {
        let field = self.field.bounds();
        let x = (bar.x + (bar.width - field.width) / 2.0).max(bar.x + crate::theme::MD);
        Point::new(x, bar.y + crate::theme::SM)
    }
}

impl overlay::Overlay<Message, iced::Theme, iced::Renderer> for TypingBar<'_, '_> {
    fn layout(&mut self, _renderer: &iced::Renderer, bounds: Size) -> layout::Node {
        self.shown = self.screen.y + self.screen.height > bounds.height * KEYBOARD_TOP;
        if !self.shown {
            return layout::Node::new(Size::ZERO);
        }
        let height = self.field.bounds().height + crate::theme::SM * 2.0;
        layout::Node::new(Size::new(bounds.width, height))
    }

    fn draw(
        &self,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        _cursor: mouse::Cursor,
    ) {
        use iced::advanced::Renderer as _;

        if !self.shown {
            return;
        }
        let bar = layout.bounds();
        let field = self.field.bounds();
        let at = self.spot(bar);
        renderer.with_layer(bar, |renderer| {
            renderer.fill_quad(
                renderer::Quad {
                    bounds: bar,
                    border: Border {
                        color: crate::theme::BORDER,
                        width: 1.0,
                        radius: 0.0.into(),
                    },
                    ..renderer::Quad::default()
                },
                crate::theme::SURFACE,
            );
            renderer.with_translation(Vector::new(at.x - field.x, at.y - field.y), |renderer| {
                self.content.as_widget().draw(
                    self.tree,
                    renderer,
                    theme,
                    style,
                    self.field,
                    mouse::Cursor::Unavailable,
                    &field,
                );
            });
        });
    }

    fn update(
        &mut self,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
        _clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
    ) {
        // A picture of the field, not the field: a touch on it reaches
        // nothing, and certainly not the header it covers.
        if matches!(event, Event::Mouse(_) | Event::Touch(_)) && cursor.is_over(layout.bounds()) {
            shell.capture_event();
        }
    }

    fn mouse_interaction(
        &self,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        _renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        // `Idle` rather than `None`, which would leave what is under the bar
        // live; see the popover's note on the same thing.
        match cursor.is_over(layout.bounds()) {
            true => mouse::Interaction::Idle,
            false => mouse::Interaction::None,
        }
    }
}
