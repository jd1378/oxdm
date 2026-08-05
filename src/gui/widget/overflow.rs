//! Lay a child out at its natural size, but claim less of it.
//!
//! CSS `overflow: visible` costs nothing: `.complete-burst` reserves an
//! 88px box while its rings scale to 1.5× and paint over whatever sits
//! next to them. iced has no equivalent — a widget occupies exactly what
//! it draws, so a canvas wide enough for the full pulse also pushes the
//! layout apart by the empty margin around it.
//!
//! This wrapper closes that gap: the child is laid out (and painted) at
//! the size it asks for, centered, while the parent is told a smaller
//! one. Decorative overflow — glows, pulses, bleeds — then costs no
//! space and can sit behind its neighbours.
//!
//! Nothing is clipped, so a child that overflows *up* paints over
//! earlier siblings and one that overflows *down* paints over later
//! ones; draw order is the parent's, unchanged. Only use it for paint
//! that is decorative: the overflowing part cannot be interacted with in
//! any way the parent knows about, since events are routed by the
//! reported bounds.

use iced::advanced::widget::{Operation, Tree, tree};
use iced::advanced::{Clipboard, Layout, Shell, Widget, layout, mouse, overlay, renderer};
use iced::{Element, Event, Length, Rectangle, Size, Vector};

/// Reserve `claimed` (square) for `content` while letting it lay out —
/// and paint — at whatever larger size it asks for, centered on the
/// reserved box.
pub fn overflowing<'a, M: 'a, Theme: 'a, Renderer: iced::advanced::Renderer + 'a>(
    content: impl Into<Element<'a, M, Theme, Renderer>>,
    claimed: f32,
) -> Element<'a, M, Theme, Renderer> {
    Element::new(Overflowing {
        content: content.into(),
        claimed,
    })
}

struct Overflowing<'a, M, Theme, Renderer> {
    content: Element<'a, M, Theme, Renderer>,
    claimed: f32,
}

impl<M, Theme, Renderer> Widget<M, Theme, Renderer> for Overflowing<'_, M, Theme, Renderer>
where
    Renderer: iced::advanced::Renderer,
{
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
        Size::new(Length::Fixed(self.claimed), Length::Fixed(self.claimed))
    }

    fn layout(
        &mut self,
        tree: &mut Tree,
        renderer: &Renderer,
        _limits: &layout::Limits,
    ) -> layout::Node {
        // Deliberately ignore the parent's limits for the child: the
        // point is to let it exceed what we report upward.
        let child = self
            .content
            .as_widget_mut()
            .layout(tree, renderer, &layout::Limits::NONE);
        let size = child.size();
        let claimed = Size::new(self.claimed, self.claimed);
        // Center the child's own (larger) box on the reserved one, so
        // the bleed is symmetric rather than hanging off one corner.
        let offset = Vector::new(
            (claimed.width - size.width) / 2.0,
            (claimed.height - size.height) / 2.0,
        );
        layout::Node::with_children(claimed, vec![child.translate(offset)])
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
        let Some(child) = layout.children().next() else {
            return;
        };
        // The child's bounds, not ours — a viewport cut to the reserved
        // box would clip exactly the overflow this widget exists for.
        let bounds = child.bounds();
        self.content.as_widget().draw(
            tree,
            renderer,
            theme,
            style,
            child,
            cursor,
            &bounds.union(viewport),
        );
    }

    fn operate(
        &mut self,
        tree: &mut Tree,
        layout: Layout<'_>,
        renderer: &Renderer,
        operation: &mut dyn Operation,
    ) {
        if let Some(child) = layout.children().next() {
            self.content
                .as_widget_mut()
                .operate(tree, child, renderer, operation);
        }
    }

    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, M>,
        viewport: &Rectangle,
    ) {
        if let Some(child) = layout.children().next() {
            self.content.as_widget_mut().update(
                tree, event, child, cursor, renderer, clipboard, shell, viewport,
            );
        }
    }

    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &Renderer,
    ) -> mouse::Interaction {
        layout
            .children()
            .next()
            .map(|child| {
                self.content
                    .as_widget()
                    .mouse_interaction(tree, child, cursor, viewport, renderer)
            })
            .unwrap_or_default()
    }

    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, M, Theme, Renderer>> {
        let child = layout.children().next()?;
        self.content
            .as_widget_mut()
            .overlay(tree, child, renderer, viewport, translation)
    }
}
