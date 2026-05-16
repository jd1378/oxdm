//! Left sidebar: collapsible CATEGORIES, QUEUES, TOOLS sections.
//! Selecting a row updates `app.filter`. Tools rows trigger dialogs.

use eframe::egui::{self, Color32, Rect, Response, Sense, Ui, Vec2};

use super::primitives::eyebrow;
use crate::domain::{Category, QueueId};
use crate::ui::AppShell;
use crate::ui::theme::{self, radius, space};
use crate::ui::utils::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SidebarFilter {
    All { category: Option<Category> },
    Finished { category: Option<Category> },
    Unfinished { category: Option<Category> },
    Queue(QueueId),
}

impl Default for SidebarFilter {
    fn default() -> Self {
        Self::All { category: None }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Categories,
    Queues,
    Tools,
    // legacy variants kept for back-compat with persisted state
    All,
    Finished,
    Unfinished,
}

#[derive(Clone, Copy)]
enum Leader {
    Icon(&'static str, Color32),
    Swatch(Color32),
}

pub fn ui(app: &mut AppShell, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.spacing_mut().item_spacing.y = 1.0;

        // ---- CATEGORIES -------------------------------------------
        let cats_open = is_open(app, Group::Categories);
        let head = section_head(ui, "categories", cats_open);
        if head.clicked() {
            toggle(app, Group::Categories);
        }

        if cats_open {
            // "All downloads" row.
            let total_jobs = app.snap.jobs.len();
            let active_all = matches!(app.filter, SidebarFilter::All { category: None });
            if row(
                ui,
                Leader::Icon("layers", t.fg_2),
                "All downloads",
                Some(total_jobs),
                active_all,
                false,
            )
            .clicked()
            {
                app.filter = SidebarFilter::All { category: None };
            }

            let cats = Category::ALL_VISIBLE;
            for cat in cats.iter().copied() {
                let count = count_in_category(app, cat);
                let active =
                    matches!(app.filter, SidebarFilter::All { category: Some(c) } if c == cat);
                let (icon_name, color) = category_visual(cat, &t);
                if row(
                    ui,
                    Leader::Icon(icon_name, color),
                    cat.label(),
                    Some(count),
                    active,
                    true,
                )
                .clicked()
                {
                    app.filter = SidebarFilter::All {
                        category: Some(cat),
                    };
                }
            }
        }

        // ---- QUEUES -----------------------------------------------
        let q_open = is_open(app, Group::Queues);
        let head = section_head(ui, "queues", q_open);
        if head.clicked() {
            toggle(app, Group::Queues);
        }

        if q_open {
            let main_id = app.cache.main_queue_id();
            let mut request_delete: Option<crate::domain::QueueId> = None;
            for q in app.snap.queues.clone().iter() {
                let active = matches!(app.filter, SidebarFilter::Queue(id) if id == q.id);
                let dot = queue_dot_color(q, &t);
                let resp = row(
                    ui,
                    Leader::Swatch(dot),
                    &q.name,
                    Some(q.job_ids.len()),
                    active,
                    true,
                );
                if resp.clicked() {
                    app.filter = SidebarFilter::Queue(q.id);
                }
                if q.id != main_id {
                    let qid = q.id;
                    resp.context_menu(|ui| {
                        if ui.button("Delete queue").clicked() {
                            request_delete = Some(qid);
                            ui.close();
                        }
                    });
                }
            }
            if let Some(qid) = request_delete {
                app.request_delete_queue(qid);
            }
        }

        // ---- TOOLS ------------------------------------------------
        let tools_open = is_open(app, Group::Tools);
        let head = section_head(ui, "tools", tools_open);
        if head.clicked() {
            toggle(app, Group::Tools);
        }

        if tools_open {
            if row(
                ui,
                Leader::Icon("calendar", t.fg_2),
                "Scheduler",
                None,
                false,
                false,
            )
            .clicked()
            {
                crate::ui::ask_open_queues(app);
            }
            if row(
                ui,
                Leader::Icon("settings", t.fg_2),
                "Settings",
                None,
                false,
                false,
            )
            .clicked()
            {
                crate::ui::ask_open_settings(app, None, false);
            }
            if row(
                ui,
                Leader::Icon("puzzle", t.fg_2),
                "Browser extension",
                None,
                false,
                false,
            )
            .clicked()
            {
                crate::ui::platform::open_url("https://github.com/jd1378/oxdm");
            }
            if row(
                ui,
                Leader::Icon("globe", t.fg_2),
                "Per host settings",
                None,
                false,
                false,
            )
            .clicked()
            {
                app.host_open = true;
            }
            if row(
                ui,
                Leader::Icon("info", t.fg_2),
                "About",
                None,
                false,
                false,
            )
            .clicked()
            {
                app.about_open = true;
            }
        }

        // suppress unused-import warning for `eyebrow`/`space`
        let _ = (eyebrow as fn(&mut Ui, &str), space::S1 as f32);
    });
}

/// Sections are open by default. We invert membership: presence in the
/// set means *collapsed*. This way fresh state shows everything.
fn is_open(app: &AppShell, g: Group) -> bool {
    !app.sidebar_expanded.contains(&g)
}

fn toggle(app: &mut AppShell, g: Group) {
    if app.sidebar_expanded.contains(&g) {
        app.sidebar_expanded.remove(&g);
    } else {
        app.sidebar_expanded.insert(g);
    }
}

fn section_head(ui: &mut Ui, label: &str, expanded: bool) -> Response {
    let t = theme::tokens(ui.ctx());
    let h = 28.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let painter = ui.painter().clone();

    let pad_l = 10.0;
    let chev_size = 14.0;
    let chev_rect = Rect::from_center_size(
        egui::pos2(rect.left() + pad_l + chev_size / 2.0, rect.center().y + 1.0),
        Vec2::splat(chev_size),
    );
    let chev_color = t.fg_3.gamma_multiply(0.85);
    let chev_name = if expanded {
        "chevron-down"
    } else {
        "chevron-right"
    };
    icons::icon(ui.ctx(), chev_name, chev_size, chev_color).paint_at(ui, chev_rect);

    let upper = label.to_uppercase();
    let mut job = egui::text::LayoutJob::default();
    job.append(
        &upper,
        0.0,
        egui::TextFormat {
            font_id: theme::body_bold(10.0),
            color: t.fg_3,
            extra_letter_spacing: 1.0,
            ..Default::default()
        },
    );
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let label_pos = egui::pos2(
        chev_rect.right() + 6.0,
        rect.center().y - galley.size().y / 2.0 + 1.0,
    );
    painter.galley(label_pos, galley, t.fg_3);

    use super::primitives::Clickable;
    resp.clickable()
}

fn row(
    ui: &mut Ui,
    leader: Leader,
    label: &str,
    count: Option<usize>,
    active: bool,
    indent: bool,
) -> Response {
    let t = theme::tokens(ui.ctx());
    let h = 26.0;
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h), Sense::click());
    let painter = ui.painter().clone();

    if active {
        painter.rect_filled(rect, radius::XS as f32, t.action_primary);
    } else if resp.hovered() {
        painter.rect_filled(rect, radius::XS as f32, t.bg_sunken);
    }

    let pad_l = if indent { 22.0 } else { 10.0 };
    let pad_r = 10.0;
    let label_color = if active {
        t.action_primary_fg
    } else if resp.hovered() {
        t.fg_1
    } else {
        t.fg_2
    };
    let count_fg = if active {
        // rgba(255,255,255,0.85)
        Color32::from_rgba_unmultiplied(255, 255, 255, 217)
    } else {
        t.fg_3
    };

    let mut cursor_x = rect.left() + pad_l;
    match leader {
        Leader::Icon(name, color) => {
            let sz = 17.0;
            let icon_color = if active { t.action_primary_fg } else { color };
            let r = Rect::from_min_size(
                egui::pos2(cursor_x, rect.center().y - sz / 2.0),
                Vec2::splat(sz),
            );
            icons::icon(ui.ctx(), name, sz, icon_color).paint_at(ui, r);
            cursor_x = r.right() + 8.0;
        }
        Leader::Swatch(color) => {
            let sz = 8.0;
            let swatch_color = if active { t.action_primary_fg } else { color };
            let r = Rect::from_min_size(
                egui::pos2(cursor_x + 3.0, rect.center().y - sz / 2.0),
                Vec2::splat(sz),
            );
            painter.rect_filled(r, 2.0, swatch_color);
            cursor_x = r.right() + 9.0;
        }
    }

    let label_galley = painter.layout_no_wrap(label.to_owned(), theme::body(12.0), label_color);
    let label_pos = egui::pos2(cursor_x, rect.center().y - label_galley.size().y / 2.0);
    painter.galley(label_pos, label_galley, label_color);

    if let Some(n) = count {
        let txt = painter.layout_no_wrap(n.to_string(), theme::mono(11.0), count_fg);
        let pos = egui::pos2(
            rect.right() - pad_r - txt.size().x,
            rect.center().y - txt.size().y / 2.0,
        );
        painter.galley(pos, txt, count_fg);
    }

    use super::primitives::Clickable;
    resp.clickable()
}

fn category_visual(cat: Category, t: &theme::Tokens) -> (&'static str, Color32) {
    let icon = match cat {
        Category::Compressed => "archive",
        Category::Programs => "package",
        Category::Videos => "film",
        Category::Music => "music",
        Category::Pictures => "image",
        Category::Documents => "file-text",
        Category::Other => "file",
    };
    (icon, t.fg_2)
}

/// Stable colour per queue, derived from the queue's name. Built-in
/// "Main" gets the brand action colour; everything else cycles through
/// the design palette.
pub fn queue_dot_color(q: &crate::domain::Queue, t: &theme::Tokens) -> Color32 {
    if let Some([r, g, b]) = q.color {
        return Color32::from_rgb(r, g, b);
    }
    if q.name.eq_ignore_ascii_case("Main") {
        return t.action_primary;
    }
    let palette = [
        t.cat_music,
        t.cat_programs,
        t.cat_pictures,
        t.cat_videos,
        t.cat_documents,
        t.cat_compressed,
        t.status_info,
        t.status_success,
    ];
    let mut h: u32 = 0;
    for b in q.name.bytes() {
        h = h.wrapping_mul(131).wrapping_add(b as u32);
    }
    palette[(h as usize) % palette.len()]
}

fn count_in_category(app: &AppShell, cat: Category) -> usize {
    app.snap.jobs.iter().filter(|j| j.category == cat).count()
}
