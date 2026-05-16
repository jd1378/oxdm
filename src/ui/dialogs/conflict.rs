//! Conflict-resolution dialog. Title-bar + warning hero card + a row
//! of resolution buttons styled with the design tokens.

use eframe::egui::{self, Align, Layout, RichText, Stroke};

use crate::data::ConflictKind;
use crate::ipc_local::protocol::{FileChangedRes, FinalFileRes, NotResumableRes, SameDownloadRes};
use crate::ui::AppShell;
use crate::ui::components::primitives::Btn;
use crate::ui::components::titlebar;
use crate::ui::theme::{self, space};

pub fn show(app: &mut AppShell, ctx: &egui::Context) {
    if app.snap.conflict_len == 0 {
        app.conflict_open = false;
        return;
    }
    let closed = super::child_viewport(
        ctx,
        "oxdm-conflict",
        "oxdm — Conflict",
        (560.0, 360.0),
        |ui| body(app, ui),
    );
    if closed {
        app.conflict_open = false;
    }
}

fn body(app: &mut AppShell, root_ui: &mut egui::Ui) {
    let ctx = &root_ui.ctx().clone();
    let Some((id, kind, token)) = app.snap.conflict_head else {
        app.conflict_open = false;
        return;
    };
    let t = theme::tokens(ctx);
    let title = title_for(kind);

    egui::Panel::top("conflict_titlebar")
        .frame(egui::Frame::NONE.fill(t.bg_titlebar))
        .show_separator_line(true)
        .show_inside(root_ui, |ui| {
            titlebar::show(ui, ctx, title);
        });

    egui::CentralPanel::default()
        .frame(
            egui::Frame::NONE
                .fill(t.bg_page)
                .inner_margin(egui::Margin::symmetric(space::S4, space::S4)),
        )
        .show_inside(root_ui, |ui| {
            ui.spacing_mut().item_spacing.y = space::S3 as f32;

            // Warning hero.
            egui::Frame::NONE
                .fill(t.status_warning_bg)
                .stroke(Stroke::new(t.border_width, t.border_subtle))
                .corner_radius(theme::surface::RADIUS)
                .inner_margin(space::S3 as f32)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        crate::ui::utils::icons::show(ui, "triangle-alert", 24.0, t.status_warning);
                        ui.add_space(space::S2 as f32);
                        ui.vertical(|ui| {
                            ui.label(
                                RichText::new(title)
                                    .font(theme::body_bold(14.0))
                                    .color(t.fg_1),
                            );
                            ui.label(
                                RichText::new(description_for(kind))
                                    .color(t.fg_2)
                                    .font(theme::body(12.0)),
                            );
                            ui.label(
                                RichText::new(format!("Job: {id}"))
                                    .color(t.fg_3)
                                    .font(theme::mono(11.0)),
                            );
                        });
                    });
                });

            // Resolution buttons.
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = space::S2 as f32;
                let s = app.client.clone();
                match kind {
                    ConflictKind::FileChanged => {
                        if Btn::new("Restart")
                            .primary()
                            .icon("rotate-cw")
                            .show(ui)
                            .clicked()
                        {
                            spawn_file_changed(app, &s, id, token, FileChangedRes::Restart);
                        }
                        if Btn::new("Abort")
                            .danger_filled()
                            .icon("x")
                            .show(ui)
                            .clicked()
                        {
                            spawn_file_changed(app, &s, id, token, FileChangedRes::Abort);
                        }
                    }
                    ConflictKind::NotResumable => {
                        if Btn::new("Restart")
                            .primary()
                            .icon("rotate-cw")
                            .show(ui)
                            .clicked()
                        {
                            spawn_not_resumable(app, &s, id, token, NotResumableRes::Restart);
                        }
                        if Btn::new("Abort")
                            .danger_filled()
                            .icon("x")
                            .show(ui)
                            .clicked()
                        {
                            spawn_not_resumable(app, &s, id, token, NotResumableRes::Abort);
                        }
                    }
                    ConflictKind::SameDownloadExists => {
                        if Btn::new("Resume").primary().icon("play").show(ui).clicked() {
                            spawn_same_download(app, &s, id, token, SameDownloadRes::Resume);
                        }
                        if Btn::new("Number suffix").icon("plus").show(ui).clicked() {
                            spawn_same_download(
                                app,
                                &s,
                                id,
                                token,
                                SameDownloadRes::AddNumberAndContinue,
                            );
                        }
                        if Btn::new("Abort")
                            .danger_filled()
                            .icon("x")
                            .show(ui)
                            .clicked()
                        {
                            spawn_same_download(app, &s, id, token, SameDownloadRes::Abort);
                        }
                    }
                    ConflictKind::FinalFileExists => {
                        if Btn::new("Replace")
                            .primary()
                            .icon("rotate-cw")
                            .show(ui)
                            .clicked()
                        {
                            spawn_final_file(app, &s, id, token, FinalFileRes::Replace);
                        }
                        if Btn::new("Number suffix").icon("plus").show(ui).clicked() {
                            spawn_final_file(
                                app,
                                &s,
                                id,
                                token,
                                FinalFileRes::AddNumberAndContinue,
                            );
                        }
                        if Btn::new("Abort")
                            .danger_filled()
                            .icon("x")
                            .show(ui)
                            .clicked()
                        {
                            spawn_final_file(app, &s, id, token, FinalFileRes::Abort);
                        }
                    }
                    ConflictKind::UrlBroken | ConflictKind::CredentialsInvalid => {
                        if Btn::new("OK").primary().icon("check").show(ui).clicked() {
                            let s2 = s.clone();
                            app.spawn(async move {
                                let _ = s2.pop_conflict().await;
                            });
                        }
                    }
                }
            });
        });
}

fn spawn_file_changed(
    app: &AppShell,
    s: &std::sync::Arc<crate::ipc_local::Client>,
    id: crate::domain::JobId,
    token: u64,
    res: FileChangedRes,
) {
    let s2 = s.clone();
    app.spawn(async move {
        let _ = s2.resolve_file_changed(id, token, res).await;
        let _ = s2.pop_conflict().await;
    });
}
fn spawn_not_resumable(
    app: &AppShell,
    s: &std::sync::Arc<crate::ipc_local::Client>,
    id: crate::domain::JobId,
    token: u64,
    res: NotResumableRes,
) {
    let s2 = s.clone();
    app.spawn(async move {
        let _ = s2.resolve_not_resumable(id, token, res).await;
        let _ = s2.pop_conflict().await;
    });
}
fn spawn_same_download(
    app: &AppShell,
    s: &std::sync::Arc<crate::ipc_local::Client>,
    id: crate::domain::JobId,
    token: u64,
    res: SameDownloadRes,
) {
    let s2 = s.clone();
    app.spawn(async move {
        let _ = s2.resolve_same_download(id, token, res).await;
        let _ = s2.pop_conflict().await;
    });
}
fn spawn_final_file(
    app: &AppShell,
    s: &std::sync::Arc<crate::ipc_local::Client>,
    id: crate::domain::JobId,
    token: u64,
    res: FinalFileRes,
) {
    let s2 = s.clone();
    app.spawn(async move {
        let _ = s2.resolve_final_file(id, token, res).await;
        let _ = s2.pop_conflict().await;
    });
}

fn title_for(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::FileChanged => "File changed on server",
        ConflictKind::NotResumable => "Server does not support resume",
        ConflictKind::UrlBroken => "URL no longer works",
        ConflictKind::CredentialsInvalid => "Authentication required",
        ConflictKind::SameDownloadExists => "Existing download found",
        ConflictKind::FinalFileExists => "File already exists",
    }
}

fn description_for(kind: ConflictKind) -> &'static str {
    match kind {
        ConflictKind::FileChanged => {
            "The remote file changed since the download started. Resuming would corrupt the file. Restart from scratch or abort?"
        }
        ConflictKind::NotResumable => {
            "The server cannot resume partial downloads. Restart or abort?"
        }
        ConflictKind::UrlBroken => "The URL no longer responds.",
        ConflictKind::CredentialsInvalid => "Credentials are missing or invalid.",
        ConflictKind::SameDownloadExists => {
            "An in-progress download with the same metadata is already in your download folder."
        }
        ConflictKind::FinalFileExists => "A file with this name already exists in the save folder.",
    }
}
