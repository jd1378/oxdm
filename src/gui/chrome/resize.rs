//! Edge/corner resize handles + hairline window border for the
//! borderless windows: 6px bands on the four edges, 14px corner
//! squares, mapped to `window::drag_resize`.

use iced::widget::{container, mouse_area, stack};
use iced::window::Direction;
use iced::{Border, Element, Length};

use crate::gui::chrome::WindowControl;
use crate::gui::theme::Tokens;

const EDGE: f32 = 6.0;
const CORNER: f32 = 14.0;

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

    stack![bordered, overlay].into()
}
