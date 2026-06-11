//! Edge/corner resize handles + hairline window border for the
//! borderless windows: 6px bands on the four edges, 14px corner
//! squares, mapped to `window::drag_resize`.

use iced::widget::{canvas, container, mouse_area, stack};
use iced::window::Direction;
use iced::{Border, Element, Length, Point};

use crate::gui::chrome::WindowControl;
use crate::gui::theme::Tokens;

const EDGE: f32 = 6.0;
const CORNER: f32 = 14.0;
/// Visible diagonal hash + extra hit zone for the SE grip (egui parity).
const SE_GRIP: f32 = 22.0;

/// Diagonal hash painted in the bottom-right corner for resize
/// affordance: 4 hairlines, 1.2px, fg_4, 4px apart (matches the egui
/// `utils::resize` grip).
struct Grip {
    color: iced::Color,
}

impl<M> canvas::Program<M> for Grip {
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let (w, h) = (bounds.width, bounds.height);
        let pad = 4.0;
        // Start at i=1: the i=0 hash would be a zero-length segment
        // (off == pad), which tiny-skia's stroker rejects with a
        // "path stroking failed" warning every frame. egui drew the
        // same degenerate (invisible) line; skipping it is visually
        // identical.
        for i in 1..4 {
            let off = pad + (i as f32) * 4.0;
            let mut b = canvas::path::Builder::new();
            b.move_to(Point::new(w - off, h - pad));
            b.line_to(Point::new(w - pad, h - off));
            frame.stroke(
                &b.build(),
                canvas::Stroke::default()
                    .with_color(self.color)
                    .with_width(1.2),
            );
        }
        vec![frame.into_geometry()]
    }
}

fn grip<'a, M: Clone + 'a>(
    w: Length,
    h: Length,
    dir: Direction,
    interaction: iced::mouse::Interaction,
    on_control: &impl Fn(WindowControl) -> M,
) -> Element<'a, M> {
    mouse_area(container(iced::widget::Space::new()).width(w).height(h))
        .on_press(on_control(WindowControl::Resize(dir)))
        .interaction(interaction)
        .into()
}

/// Wrap a window's content with the resize-handle overlay and the
/// 1px hairline border. No-op handles when `resizable` is false.
pub fn resizable<'a, M: Clone + 'a>(
    t: &Tokens,
    content: Element<'a, M>,
    resizable: bool,
    on_control: impl Fn(WindowControl) -> M + 'a,
) -> Element<'a, M> {
    use iced::mouse::Interaction as I;
    let t = *t;
    let bordered = container(content)
        .width(Length::Fill)
        .height(Length::Fill)
        .style(move |_| container::Style {
            border: Border {
                color: t.border_subtle,
                width: 1.0,
                radius: 0.0.into(),
            },
            ..Default::default()
        });
    if !resizable {
        return bordered.into();
    }

    let col = |els: Vec<Element<'a, M>>| {
        let mut c = iced::widget::column![];
        for e in els {
            c = c.push(e);
        }
        c
    };
    let rw = |els: Vec<Element<'a, M>>| {
        let mut r = iced::widget::row![];
        for e in els {
            r = r.push(e);
        }
        r
    };

    let fixed = Length::Fixed(CORNER);
    let edge = Length::Fixed(EDGE);

    // Three rows: top strip (NW / N / NE), middle (W / fill / E),
    // bottom strip (SW / S / SE).
    let top = rw(vec![
        grip(
            fixed,
            edge,
            Direction::NorthWest,
            I::ResizingDiagonallyUp,
            &on_control,
        ),
        grip(
            Length::Fill,
            edge,
            Direction::North,
            I::ResizingVertically,
            &on_control,
        ),
        grip(
            fixed,
            edge,
            Direction::NorthEast,
            I::ResizingDiagonallyDown,
            &on_control,
        ),
    ])
    .height(edge);
    let middle = rw(vec![
        grip(
            edge,
            Length::Fill,
            Direction::West,
            I::ResizingHorizontally,
            &on_control,
        ),
        container(iced::widget::Space::new())
            .width(Length::Fill)
            .height(Length::Fill)
            .into(),
        grip(
            edge,
            Length::Fill,
            Direction::East,
            I::ResizingHorizontally,
            &on_control,
        ),
    ])
    .height(Length::Fill);
    let bottom = rw(vec![
        grip(
            fixed,
            edge,
            Direction::SouthWest,
            I::ResizingDiagonallyDown,
            &on_control,
        ),
        grip(
            Length::Fill,
            edge,
            Direction::South,
            I::ResizingVertically,
            &on_control,
        ),
        grip(
            fixed,
            edge,
            Direction::SouthEast,
            I::ResizingDiagonallyUp,
            &on_control,
        ),
    ])
    .height(edge);

    let overlay = col(vec![top.into(), middle.into(), bottom.into()])
        .width(Length::Fill)
        .height(Length::Fill);

    // Painted SE grip + a larger 22px grab zone on top of it.
    let grip_visual = container(
        canvas(Grip { color: t.fg_4 })
            .width(Length::Fixed(SE_GRIP))
            .height(Length::Fixed(SE_GRIP)),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::alignment::Horizontal::Right)
    .align_y(iced::alignment::Vertical::Bottom);

    let grip_zone = container(
        mouse_area(
            container(iced::widget::Space::new())
                .width(Length::Fixed(SE_GRIP))
                .height(Length::Fixed(SE_GRIP)),
        )
        .on_press(on_control(WindowControl::Resize(Direction::SouthEast)))
        .interaction(I::ResizingDiagonallyUp),
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .align_x(iced::alignment::Horizontal::Right)
    .align_y(iced::alignment::Vertical::Bottom);

    stack![bordered, grip_visual, overlay, grip_zone].into()
}
