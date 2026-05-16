pub mod about;
pub mod conflict;
pub mod host_settings;
pub mod properties;
pub mod queues;
pub mod remove;
pub mod settings;

use eframe::egui::{self, Color32};

/// Lerp `accent` toward `base` by factor `t`. `t=0` returns `accent`,
/// `t=1` returns `base`. Used by header/icon tiles for soft-tint backgrounds.
pub fn soft_tint(accent: Color32, base: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
    Color32::from_rgb(
        lerp(base.r(), accent.r()),
        lerp(base.g(), accent.g()),
        lerp(base.b(), accent.b()),
    )
}

/// Open an immediate child viewport. The body runs while the viewport
/// is alive; returns `true` when the user requested close so the
/// caller can clear its open flag.
///
/// `id` is salted with a per-id incarnation counter stored in egui
/// memory — every close bumps it so the next open lands on a fresh
/// `ViewportId`. egui 0.29 keeps stale viewport bookkeeping otherwise,
/// which made some dialogs refuse to reopen after closing.
pub fn child_viewport(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    size: (f32, f32),
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mem_id = egui::Id::new(("oxdm-viewport-incarnation", id));
    let seq: u64 = ctx.memory_mut(|m| *m.data.get_temp_mut_or_default::<u64>(mem_id));
    let vid = egui::ViewportId::from_hash_of((id, seq));
    let builder = crate::ui::utils::chrome::viewport_builder(title, size, Some((320.0, 200.0)));
    let mut closed = false;
    let mut body = Some(body);
    ctx.show_viewport_immediate(vid, builder, |ui, _class| {
        if let Some(b) = body.take() {
            b(ui);
        }
        let ctx = ui.ctx();
        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::resize::ChromeStyle {
                dark_border: true,
                resizable: true,
            },
        );
        if ctx.input(|i| i.viewport().close_requested()) {
            closed = true;
        }
        // Esc dismisses the dialog. Mirrors macOS/Windows convention and
        // the `11_confirm_dialog.md` spec ("Escape key … dismisses").
        // Filed only when this viewport has focus.
        if ctx.input(|i| i.focused && i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    if closed {
        ctx.memory_mut(|m| {
            let v = m.data.get_temp_mut_or_default::<u64>(mem_id);
            *v = v.wrapping_add(1);
        });
    }
    closed
}

/// Variant of [`child_viewport`] for compact, non-resizable dialogs
/// whose height should track their content. Width is fixed; height is
/// reset every frame to the root ui's measured `min_rect.height()`,
/// so the body must avoid `egui::Panel`s (which stretch to fill the
/// viewport) — stack `egui::Frame`s in the root ui instead.
pub fn child_viewport_fit(
    ctx: &egui::Context,
    id: &str,
    title: &str,
    width: f32,
    body: impl FnOnce(&mut egui::Ui),
) -> bool {
    let mem_id = egui::Id::new(("oxdm-viewport-incarnation", id));
    let seq: u64 = ctx.memory_mut(|m| *m.data.get_temp_mut_or_default::<u64>(mem_id));
    let vid = egui::ViewportId::from_hash_of((id, seq));
    let builder = crate::ui::utils::chrome::viewport_builder(title, (width, 320.0), None)
        .with_resizable(false);
    let mut closed = false;
    let mut body = Some(body);
    ctx.show_viewport_immediate(vid, builder, |ui, _class| {
        if let Some(b) = body.take() {
            b(ui);
        }
        let needed_h = ui.min_rect().height().ceil();
        let ctx = ui.ctx();
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
            width, needed_h,
        )));
        crate::ui::utils::resize::show_styled(
            ctx,
            crate::ui::utils::resize::ChromeStyle {
                dark_border: true,
                resizable: false,
            },
        );
        if ctx.input(|i| i.viewport().close_requested()) {
            closed = true;
        }
        if ctx.input(|i| i.focused && i.key_pressed(egui::Key::Escape)) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    });
    if closed {
        ctx.memory_mut(|m| {
            let v = m.data.get_temp_mut_or_default::<u64>(mem_id);
            *v = v.wrapping_add(1);
        });
    }
    closed
}
