//! Custom window decorations.
//!
//! Behaviour by platform:
//! - **macOS**: native window decorations remain enabled (we do not
//!   strip them). The OS draws traffic lights on the left; we just
//!   render the centred title in the same bar by leaving room above
//!   our content.
//! - **Linux / Windows**: the viewport is launched borderless and we
//!   draw our own titlebar with right-aligned, larger window controls
//!   (minimise / maximise-restore / close) using Lucide icons.
//!
//! The titlebar is also a drag region: clicking-and-holding on empty
//! space starts a window drag, double-clicking toggles maximised.

use std::sync::Mutex;

use eframe::egui::{self, Color32, Pos2, Rect, Sense, Ui, Vec2, ViewportCommand, ViewportId};

/// Set by [`show`] right after dispatching `ViewportCommand::StartDrag`.
/// The OS drag-modal-loop swallows the matching mouse-up, so egui's
/// `pointer.primary_down` stays stuck until the next click. A
/// per-context `on_begin_pass` callback (installed by
/// [`install_drag_release_handler`]) drains this on the next frame and
/// resets pointer state so hover works.
///
/// Stores the [`ViewportId`] that initiated the drag so the reset only
/// fires for the affected viewport — immediate child viewports share a
/// single `Context` with the parent shell, so a global flag would clear
/// pointer state on unrelated viewports.
static DRAG_RELEASE_PENDING: Mutex<Option<ViewportId>> = Mutex::new(None);

fn mark_drag_release(viewport_id: ViewportId) {
    if let Ok(mut g) = DRAG_RELEASE_PENDING.lock() {
        *g = Some(viewport_id);
    }
}

fn take_drag_release_for(viewport_id: ViewportId) -> bool {
    let Ok(mut g) = DRAG_RELEASE_PENDING.lock() else {
        return false;
    };
    if g.as_ref() == Some(&viewport_id) {
        *g = None;
        return true;
    }
    false
}

/// Install a per-`Context` `on_begin_pass` plugin that, on the frame
/// after a `ViewportCommand::StartDrag`, replaces an immediate child
/// viewport's pointer state with `Default` to clear the stuck
/// `primary_down`.
///
/// The root viewport is handled separately via
/// [`drain_drag_release`] from `eframe::App::raw_input_hook` (synthetic
/// release events injected into `RawInput` before `begin_pass`). That
/// path is more surgical than blowing away `PointerState`, so we only
/// fall back to the heavier reset for non-root viewports — immediate
/// child viewports run inside the parent's `egui_ctx.run` and have no
/// equivalent hook.
///
/// Idempotent: a sentinel in `ctx.data_mut` ensures only one callback
/// is registered per `Context`.
pub fn install_drag_release_handler(ctx: &egui::Context) {
    let key = egui::Id::new("oxdm-drag-release-installed");
    let already = ctx.data_mut(|d| {
        let installed = d.get_temp::<bool>(key).unwrap_or(false);
        if !installed {
            d.insert_temp(key, true);
        }
        installed
    });
    if already {
        return;
    }
    ctx.on_begin_pass(
        "oxdm-drag-release",
        std::sync::Arc::new(|ui: &mut egui::Ui| {
            let ctx = ui.ctx();
            let vid = ctx.viewport_id();
            if vid == ViewportId::ROOT {
                return;
            }
            if take_drag_release_for(vid) {
                ctx.input_mut(|i| {
                    i.pointer = egui::PointerState::default();
                });
            }
        }),
    );
}

/// Inject synthetic primary-release + `PointerGone` events when the
/// current viewport started an OS window drag last frame. Call from
/// each shell's `eframe::App::raw_input_hook` — the root viewport
/// can't use the `on_begin_pass` plugin path because `RawInput` events
/// are consumed before the plugin fires.
pub fn drain_drag_release(viewport_id: ViewportId, raw_input: &mut egui::RawInput) {
    if !take_drag_release_for(viewport_id) {
        return;
    }
    let pos = raw_input
        .events
        .iter()
        .rev()
        .find_map(|e| match e {
            egui::Event::PointerMoved(p) | egui::Event::PointerButton { pos: p, .. } => Some(*p),
            _ => None,
        })
        .unwrap_or_default();
    raw_input.events.push(egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: raw_input.modifiers,
    });
    raw_input.events.push(egui::Event::PointerGone);
}
use crate::ui::theme::{self, radius, space};
use crate::ui::utils::icons;

pub const HEIGHT: f32 = 36.0;
pub const HEIGHT_MAC: f32 = 28.0;
/// Square side for window-control buttons. 24px leaves a 6px top/bottom
/// gap inside the 36px bar and matches the old non-square button height.
const BTN_SIDE: f32 = 24.0;

/// `true` when the running platform should use our custom-drawn
/// titlebar (Linux / Windows). On macOS the OS draws it.
pub const fn use_custom() -> bool {
    !cfg!(target_os = "macos")
}

#[derive(Clone, Copy)]
pub struct Opts {
    pub show_maximize: bool,
}

impl Default for Opts {
    fn default() -> Self {
        Self {
            show_maximize: true,
        }
    }
}

pub fn show(ui: &mut Ui, ctx: &egui::Context, title: &str) {
    show_with(ui, ctx, title, Opts::default());
}

pub fn show_with(ui: &mut Ui, ctx: &egui::Context, title: &str, opts: Opts) {
    install_drag_release_handler(ctx);
    let t = theme::tokens(ui.ctx());
    let h = if use_custom() { HEIGHT } else { HEIGHT_MAC };
    let bar_rect = Rect::from_min_size(ui.min_rect().min, Vec2::new(ui.available_width(), h));
    ui.painter().rect_filled(bar_rect, 0.0, t.bg_titlebar);
    // Publish the bar's bottom edge so modal overlays know exactly
    // where content starts and can position their backdrop without
    // clipping into the titlebar (which would block window drag).
    crate::ui::utils::modal::set_titlebar_bottom(ctx, bar_rect.bottom());

    // macOS: OS draws traffic lights at left → leave 76px breathing
    // room before the title. Linux/Windows: small inset.
    #[cfg(target_os = "macos")]
    let left_pad: f32 = 76.0;
    #[cfg(not(target_os = "macos"))]
    let left_pad: f32 = space::S4 as f32;
    // Width reserved for the 3 right-side window controls plus trailing
    // edge padding. macOS draws no controls on this side.
    #[cfg(target_os = "macos")]
    let controls_w: f32 = 0.0;
    #[cfg(not(target_os = "macos"))]
    let controls_w: f32 = BTN_SIDE * 3.0 + space::S2 as f32 * 2.0;
    // Extra gap between the (possibly truncated) title and the window
    // controls, so they don't visually touch in clamped layouts.
    let title_to_controls_gap: f32 = space::S2 as f32;
    let right_pad: f32 = controls_w + title_to_controls_gap;

    // Drag region excludes the window-controls strip so button clicks
    // and hovers aren't swallowed by the drag interaction.
    let mut drag_rect = bar_rect;
    drag_rect.max.x -= controls_w;
    let drag = ui.interact(
        drag_rect,
        ui.id().with("titlebar-drag"),
        Sense::click_and_drag(),
    );
    // Send StartDrag the moment the primary button is pressed on the
    // bar — egui's `drag_started` only fires after the drag threshold
    // is exceeded, which loses the first click on quick grab-and-move.
    let primary_pressed = ctx.input(|i| i.pointer.primary_pressed());
    let in_resize_zone = ctx
        .input(|i| i.pointer.hover_pos())
        .map(|p| crate::ui::utils::resize::pos_in_resize_zone(ctx, p))
        .unwrap_or(false);
    // Suppress StartDrag on the second press of a double-click —
    // otherwise the first press kicks the OS into a move loop and the
    // matching `Maximized` cmd applies at the dragged-to position,
    // producing a screen-sized window at the wrong offset.
    let now = ctx.input(|i| i.time);
    let last_press_id = ui.id().with("titlebar-last-press");
    let last_press: Option<f64> = ctx.memory(|m| m.data.get_temp(last_press_id));
    let double_click_window: f32 = 0.4;
    let is_second_press = primary_pressed
        && last_press
            .map(|t| (now - t) < double_click_window as f64)
            .unwrap_or(false);
    if primary_pressed {
        ctx.memory_mut(|m| m.data.insert_temp(last_press_id, now));
    }
    if drag.contains_pointer() && primary_pressed && !in_resize_zone && !is_second_press {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
        // OS drag-modal-loop eats the matching mouse-up on Win32/macOS.
        // Defer release injection to next frame's `raw_input_hook` so
        // it lands before `begin_pass` rebuilds pointer state from
        // raw events — pushing into `i.events` mid-frame would arrive
        // too late and leave `primary_down` stuck.
        mark_drag_release(ctx.viewport_id());
    }
    // Detect double-click off the press timestamps instead of
    // `drag.double_clicked()` — the first press sent StartDrag, and
    // the OS move loop eats releases, so egui's click tracker never
    // forms the (click, click) pair.
    if opts.show_maximize && drag.contains_pointer() && is_second_press {
        let maxed = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maxed));
    }
    if drag.hovered() {
        ctx.set_cursor_icon(egui::CursorIcon::Default);
    }

    // Title is centred within the bar, but clipped to the
    // window-controls-free region so it can't overlap them.
    // Truncate with an ellipsis when it would. Painted directly so it
    // is never selectable / interactive.
    let text_left = bar_rect.left() + left_pad;
    let text_right = bar_rect.right() - right_pad;
    let max_w = (text_right - text_left).max(0.0);
    let mut job = egui::text::LayoutJob::single_section(
        title.to_owned(),
        egui::TextFormat {
            font_id: theme::body_bold(13.0),
            color: t.fg_2,
            ..Default::default()
        },
    );
    job.wrap.max_width = max_w;
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    job.wrap.overflow_character = Some('…');
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let center_x = bar_rect.center().x;
    let mut x = center_x - galley.size().x / 2.0;
    if x < text_left {
        x = text_left;
    }
    if x + galley.size().x > text_right {
        x = text_right - galley.size().x;
    }
    let pos = egui::pos2(x, bar_rect.center().y - galley.size().y / 2.0);
    ui.painter().galley(pos, galley, t.fg_2);

    // Right-side window controls (Linux/Windows only). Painted at
    // explicit rects so vertical centering is exact regardless of the
    // surrounding layout's rounding behaviour.
    #[cfg(not(target_os = "macos"))]
    {
        let gap = space::S2 as f32;
        let btn_y = (bar_rect.center().y - BTN_SIDE / 2.0).round();
        let mut right_edge = bar_rect.right() - gap;
        let mut place = |role: &'static str,
                         icon: &'static str,
                         color: Color32,
                         danger: bool|
         -> egui::Response {
            let rect = Rect::from_min_size(
                Pos2::new(right_edge - BTN_SIDE, btn_y),
                Vec2::splat(BTN_SIDE),
            );
            right_edge -= BTN_SIDE + gap;
            window_btn_at(ui, rect, role, icon, color, danger)
        };
        if place("close", "x", t.status_danger, true).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Close);
        }
        if opts.show_maximize {
            let maxed = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            let max_icon = if maxed { "copy" } else { "square" };
            if place("maximize", max_icon, t.fg_2, false).clicked() {
                ctx.send_viewport_cmd(ViewportCommand::Maximized(!maxed));
            }
        }
        if place("minimize", "minus", t.fg_2, false).clicked() {
            ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
        }
    }
    #[cfg(target_os = "macos")]
    let _ = left_pad;

    ui.advance_cursor_after_rect(bar_rect);
}

#[allow(dead_code)]
fn window_btn_at(
    ui: &mut Ui,
    rect: Rect,
    role: &'static str,
    icon: &'static str,
    hover_fg: Color32,
    danger: bool,
) -> egui::Response {
    let t = theme::tokens(ui.ctx());
    let id = ui.id().with(("oxdm-window-btn", role));
    let resp = ui.interact(rect, id, Sense::click());
    let painter = ui.painter().clone();
    let hovered = resp.hovered();
    if hovered {
        let bg = if danger { hover_fg } else { t.bg_raised };
        painter.rect_filled(rect, radius::XS as f32, bg);
    }
    let icon_color = if hovered && danger {
        Color32::WHITE
    } else if hovered {
        t.fg_1
    } else {
        t.fg_2
    };
    let img = icons::icon(ui.ctx(), icon, 14.0, icon_color);
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(14.0));
    img.paint_at(ui, icon_rect);
    use crate::ui::components::primitives::Clickable;
    resp.clickable()
}
