//! The wrappers that record where controls are drawn, and their `Widget` impls.

use iced::advanced::layout::{self, Layout};
use iced::advanced::widget::{tree, Operation, Tree};
use iced::advanced::{mouse, overlay, renderer, Clipboard, Shell, Widget};
use iced::{Border, Element, Event, Length, Size, Vector};

use super::typing::{field, pretend, typing, Editor, Leave, TypingBar};
use super::*;
use crate::theme;

/// Everything a wrapper hands straight to the one widget it wraps, except
/// `draw` and `update`, which are what each wrapper is for. `but overlay`
/// leaves out `overlay` too, for the ring, which adds the typing bar.
macro_rules! delegate {
    () => {
        delegate!(but overlay);

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
    (but overlay) => {
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
    Element::new(Region { content: content.into(), id, pane: false })
}

/// A [`region`] the arrows never leave: a page beside a list of pages, like
/// Settings, which is left by Escape, back to its entry in the list. Without
/// it, down from the last switch was the list's next entry, and left from
/// anything was the list.
pub fn pane<'a>(id: &'static str, content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(Region { content: content.into(), id, pane: true })
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
    pane: bool,
}

struct Group<'a> {
    content: Element<'a, Message>,
    id: String,
}

struct Root<'a> {
    content: Element<'a, Message>,
}

/// How far outside a checkbox or a radio its ring is drawn, above and below
/// and to either side. Wider at the sides: a checkbox's box and the last
/// letter of its label sit right on its edges, and four points there still
/// read as touching, while the rows above and below are only so far away.
const RING_GAP_Y: f32 = 5.0;
const RING_GAP_X: f32 = 10.0;

impl Widget<Message, iced::Theme, iced::Renderer> for Ring<'_> {
    delegate!(but overlay);

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        // Only while typing on a console, where Steam's keyboard covers the
        // bottom of the screen and a field down there would be typed into
        // blind. The field is drawn a second time, its own tree and all, so
        // the bar shows the words and the cursor exactly as the field has
        // them, whichever field it is.
        if !(crate::theme::console() && typing(tree)) {
            return self.content.as_widget_mut().overlay(tree, layout, renderer, viewport, translation);
        }
        let bar = overlay::Element::new(Box::new(TypingBar {
            content: &self.content,
            tree,
            field: layout,
            screen: layout.bounds() + translation,
            shown: false,
        }));
        Some(bar)
    }

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
        // and the same key then closed the dialog and the words with it. B
        // on a gamepad is no key, so it asks for the same through [`leave`].
        // Enter in a field, on a console, is done typing: the field is left,
        // with the ring on it, and the app closes Steam's keyboard. A field
        // that submits still does, first. Several lines take Enter here too,
        // since Steam's keyboard has no other key that means done.
        let enter = matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Enter),
                ..
            })
        );
        if enter && crate::theme::console() && typing(tree) {
            if tree.tag != tree::Tag::of::<Editor>() {
                self.content
                    .as_widget_mut()
                    .update(tree, event, layout, cursor, renderer, clipboard, shell, viewport);
            }
            self.content.as_widget_mut().operate(tree, layout, renderer, &mut Leave(false));
            shell.publish(Message::TypingDone(self.target.clone()));
            shell.capture_event();
            shell.request_redraw();
            return;
        }

        let asked = with(|r| match r.leave.as_ref() == Some(&self.target) {
            true => r.leave.take().is_some(),
            false => false,
        });
        let escape = matches!(
            event,
            Event::Keyboard(iced::keyboard::Event::KeyPressed {
                key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                ..
            })
        );
        if asked || escape {
            let mut leave = Leave(false);
            self.content.as_widget_mut().operate(tree, layout, renderer, &mut leave);
            if leave.0 {
                shell.request_redraw();
                if escape {
                    shell.capture_event();
                    return;
                }
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
            // A scrolled list hands its content the part of it that shows,
            // moved by however far it is scrolled; that move, taken back off.
            let screen = match r.shown.last() {
                Some(shown) => bounds - Vector::new(viewport.x - shown.x, viewport.y - shown.y),
                None => bounds,
            };
            let spot = Spot {
                bounds,
                screen,
                viewport: *viewport,
                press: self.press.clone(),
                field: field(tree),
                lines: tree.tag == tree::Tag::of::<Editor>(),
                region: r.within.last().copied(),
                group: r.groups.last().cloned(),
                pane: r.panes.last().copied(),
                depth: r.depth,
                live,
            };
            r.spots.insert(self.target.clone(), spot);
            if typing(tree) {
                r.typing = Some(self.target.clone());
            }
            r.current.as_ref() == Some(&self.target)
        });

        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);

        // White rather than the accent: INSTALL is filled with nearly the
        // accent's blue, and a ring the colour of the button it is on is no
        // ring at all. A layer of its own, so it lands over the control's
        // picture and text and not under them.
        //
        // Drawn round a checkbox or a radio rather than on its edge: their
        // bounds end at the box and the label's last letter, and a ring on
        // that line touches both. A field has a box of its own to sit on,
        // the way a dropdown does.
        // A field with the cursor in it already says so with its own
        // border, and a ring round that read as two selections at once.
        if on && !typing(tree) {
            let (ring, radius) = match self.press {
                Press::Click if !field(tree) => {
                    (bounds.expand([RING_GAP_Y, RING_GAP_X]), theme::RADIUS_SM + RING_GAP_Y)
                }
                Press::Click | Press::Send(_) | Press::Off => (bounds, theme::RADIUS_SM),
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
            if self.pane {
                r.panes.push(self.id);
            }
            r.shown.push(bounds.intersection(viewport).unwrap_or(bounds));
            r.groups.push(self.id.to_owned());
        });
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        with(|r| {
            r.within.pop();
            if self.pane {
                r.panes.pop();
            }
            r.shown.pop();
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
            r.typing = None;
            r.top = 0;
            r.depth = 0;
            r.within.clear();
            r.shown.clear();
            r.panes.clear();
            r.groups.clear();
        });
        self.content.as_widget().draw(tree, renderer, theme, style, layout, cursor, viewport);
        // The overlays come next, and a [`layer`] in one, a picker's menu,
        // counts from here: above the deepest layer the window drew. Counted
        // from zero instead, a menu opened inside a dialog tied with the
        // dialog, both stayed reachable, and the arrows walked on through the
        // dialog rather than into the list.
        with(|r| r.depth = r.top);
    }
}
