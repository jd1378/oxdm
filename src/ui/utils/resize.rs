//! Edge / corner resize handles for borderless windows.
//!
//! Custom titlebars (`titlebar::use_custom() == true`) strip native
//! decorations, which removes the OS-provided invisible resize border.
//! This module restores it by overlaying invisible 5–6px hit zones on
//! all four sides plus 14px square grips on each corner. A small
//! diagonal grip is painted in the bottom-right for affordance.
//!
//! Call [`show`] once per `update()` from any window using a custom
//! titlebar. Maximised / fullscreen viewports skip the overlay so the
//! handles do not steal focus from edge buttons.
//!
//! On Windows, the `platform::windows` WNDPROC subclass turns the
//! same edge bands into proper non-client area, so DWM owns the
//! resize loop and the cursor. The egui hit zones are skipped there
//! to avoid double-handling, but the visual SE grip and the 1px
//! window border are still painted for affordance.

use eframe::egui::{
    self, CursorIcon, Id, LayerId, Order, PointerButton, Pos2, Rect, ResizeDirection, Sense,
    Stroke, Vec2, ViewportCommand,
};

use crate::ui::theme;

/// Visual style for the window chrome border.
#[derive(Clone, Copy)]
pub struct ChromeStyle {
    /// Use the near-black `border_strong` token instead of the
    /// default subtle hairline. Used for dialogs / secondary windows
    /// to lift them off the desktop without a real drop shadow.
    pub dark_border: bool,
    /// When false, skip the edge/corner resize hit zones and the SE
    /// diagonal grip. The border is still drawn so the viewport has a
    /// visible edge.
    pub resizable: bool,
}

impl Default for ChromeStyle {
    fn default() -> Self {
        Self {
            dark_border: false,
            resizable: true,
        }
    }
}

const EDGE: f32 = 6.0;
const CORNER: f32 = 14.0;
/// Visible diagonal hash + extra hit zone for the SE grip. Larger
/// than `CORNER` so the grip is comfortable to grab; the inner
/// 14×14 zone still uses the corner cursor / direction.
const SE_GRIP: f32 = 22.0;

pub fn show(ctx: &egui::Context) {
    show_styled(ctx, ChromeStyle::default());
}

/// True when the given pointer position falls inside any edge or
/// corner resize hit zone for the current viewport. Used by titlebar
/// to suppress its drag-start so resize handles win the click.
pub fn pos_in_resize_zone(ctx: &egui::Context, pos: Pos2) -> bool {
    let vp = ctx.input(|i| i.viewport().clone());
    if vp.maximized.unwrap_or(false) || vp.fullscreen.unwrap_or(false) {
        return false;
    }
    let s = ctx.content_rect();
    // Outer EDGE band on all four sides + larger CORNER squares.
    let near_left = pos.x <= s.left() + EDGE;
    let near_right = pos.x >= s.right() - EDGE;
    let near_top = pos.y <= s.top() + EDGE;
    let near_bottom = pos.y >= s.bottom() - EDGE;
    if near_left || near_right || near_top || near_bottom {
        return true;
    }
    let in_corner =
        |cx, cy| Rect::from_min_size(Pos2::new(cx, cy), Vec2::splat(CORNER)).contains(pos);
    in_corner(s.left(), s.top())
        || in_corner(s.right() - CORNER, s.top())
        || in_corner(s.left(), s.bottom() - CORNER)
        || in_corner(s.right() - CORNER, s.bottom())
        || Rect::from_min_size(
            Pos2::new(s.right() - SE_GRIP, s.bottom() - SE_GRIP),
            Vec2::splat(SE_GRIP),
        )
        .contains(pos)
}

pub fn show_styled(ctx: &egui::Context, style: ChromeStyle) {
    if !crate::ui::components::titlebar::use_custom() {
        return;
    }
    let vp = ctx.input(|i| i.viewport().clone());
    if vp.maximized.unwrap_or(false) || vp.fullscreen.unwrap_or(false) {
        return;
    }

    let s = ctx.content_rect();
    let handles: [(&str, Rect, ResizeDirection, CursorIcon); 8] = [
        (
            "rg-n",
            Rect::from_min_max(
                Pos2::new(s.left() + CORNER, s.top()),
                Pos2::new(s.right() - CORNER, s.top() + EDGE),
            ),
            ResizeDirection::North,
            CursorIcon::ResizeVertical,
        ),
        (
            "rg-s",
            Rect::from_min_max(
                Pos2::new(s.left() + CORNER, s.bottom() - EDGE),
                Pos2::new(s.right() - CORNER, s.bottom()),
            ),
            ResizeDirection::South,
            CursorIcon::ResizeVertical,
        ),
        (
            "rg-w",
            Rect::from_min_max(
                Pos2::new(s.left(), s.top() + CORNER),
                Pos2::new(s.left() + EDGE, s.bottom() - CORNER),
            ),
            ResizeDirection::West,
            CursorIcon::ResizeHorizontal,
        ),
        (
            "rg-e",
            Rect::from_min_max(
                Pos2::new(s.right() - EDGE, s.top() + CORNER),
                Pos2::new(s.right(), s.bottom() - CORNER),
            ),
            ResizeDirection::East,
            CursorIcon::ResizeHorizontal,
        ),
        (
            "rg-nw",
            Rect::from_min_size(Pos2::new(s.left(), s.top()), Vec2::splat(CORNER)),
            ResizeDirection::NorthWest,
            CursorIcon::ResizeNwSe,
        ),
        (
            "rg-ne",
            Rect::from_min_size(Pos2::new(s.right() - CORNER, s.top()), Vec2::splat(CORNER)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNeSw,
        ),
        (
            "rg-sw",
            Rect::from_min_size(
                Pos2::new(s.left(), s.bottom() - CORNER),
                Vec2::splat(CORNER),
            ),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeNeSw,
        ),
        (
            "rg-se",
            Rect::from_min_size(
                Pos2::new(s.right() - SE_GRIP, s.bottom() - SE_GRIP),
                Vec2::splat(SE_GRIP),
            ),
            ResizeDirection::SouthEast,
            CursorIcon::ResizeNwSe,
        ),
    ];

    // Pointer position regardless of focus; egui still receives
    // CursorMoved on unfocused windows so this stays up to date.
    let pointer_pos = ctx.input(|i| i.pointer.hover_pos());
    let primary_pressed = ctx.input(|i| i.pointer.button_pressed(PointerButton::Primary));

    // On Windows, WM_NCHITTEST in `platform::windows` already turns
    // these edges into native non-client area. Installing the egui
    // overlay on top would only catch clicks the OS chose not to
    // claim (e.g. if the subclass install failed), which is exactly
    // the fallback we want — but in the common case it's dead code.
    #[cfg(target_os = "windows")]
    let install_handles = false;
    #[cfg(not(target_os = "windows"))]
    let install_handles = true;

    if install_handles && style.resizable {
        for (id, rect, dir, cursor) in handles {
            let in_zone = pointer_pos.map(|p| rect.contains(p)).unwrap_or(false);

            // Set cursor before BeginResize: when window is unfocused egui
            // can skip widget-level hover updates between activation
            // clicks, so drive the cursor directly off the pointer
            // position and request a repaint to ensure the icon is
            // applied this frame.
            if in_zone {
                ctx.set_cursor_icon(cursor);
                ctx.request_repaint();
            }

            egui::Area::new(Id::new(id))
                .order(Order::Foreground)
                .movable(false)
                .interactable(true)
                .fixed_pos(rect.min)
                .show(ctx, |ui| {
                    let (_r, resp) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
                    if resp.hovered() || in_zone {
                        ui.set_cursor_icon(cursor);
                    }
                    // Trigger BeginResize on press, not drag_started:
                    // drag detection waits for movement past a threshold,
                    // and the activation click on an unfocused window can
                    // be consumed before that threshold is reached. The
                    // OS resize loop owns the drag from here regardless
                    // of focus.
                    let pressed_here = primary_pressed && in_zone;
                    if pressed_here || resp.drag_started_by(PointerButton::Primary) {
                        ui.ctx()
                            .send_viewport_cmd(ViewportCommand::BeginResize(dir));
                    }
                });
        }
    }

    // Visible diagonal grip in the SE corner. Skipped when the
    // viewport is not resizable.
    let t = theme::tokens(ctx);
    let painter = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("rg-paint")));
    let pad = 4.0;
    let stroke = Stroke::new(1.2, t.fg_4);
    for i in 0..4 {
        if !style.resizable {
            break;
        }
        let off = pad + (i as f32) * 4.0;
        painter.line_segment(
            [
                Pos2::new(s.right() - off, s.bottom() - pad),
                Pos2::new(s.right() - pad, s.bottom() - off),
            ],
            stroke,
        );
    }

    // Hairline outer window border so a borderless window has a visible
    // edge against the desktop. Drawn after the grips so it sits on
    // top. `dark_border` lifts dialog/secondary windows off the
    // desktop without needing a real drop shadow.
    let color = if matches!(t.theme, theme::ResolvedTheme::Dark) {
        egui::Color32::BLACK
    } else if style.dark_border {
        t.border_strong
    } else {
        t.border_subtle
    };
    let border = Stroke::new(t.border_width_hairline, color);
    painter.rect_stroke(s, 0.0, border, egui::StrokeKind::Inside);
}
