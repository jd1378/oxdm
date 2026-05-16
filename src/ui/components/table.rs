//! Main download table.
//!
//! Layout: a tab strip (All / Active / Finished) with right-aligned
//! aggregate stats, then a sticky column header, then the row list.
//! Each row paints custom (no `egui_extras::Table`) so we can match
//! the design's two-line "name + host", inline pill progress and
//! coloured status dot exactly.

use eframe::egui::{self, Align, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use super::primitives::TabBtn;
use crate::data::RemoveOpts;
use crate::domain::{Category, Job, JobId, Phase};
use crate::ui::AppShell;
use crate::ui::components::statusbar::matches_filter;
use crate::ui::theme::{self, radius, space};
use crate::ui::utils::format::{format_bytes, format_bytes_opt, format_speed};
use crate::ui::utils::icons;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Size,
    Status,
    Speed,
    Eta,
    Date,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortDir {
    Asc,
    Desc,
}

#[derive(Debug, Clone, Copy)]
pub struct TableSort {
    pub col: SortColumn,
    pub dir: SortDir,
}

impl Default for TableSort {
    fn default() -> Self {
        Self {
            col: SortColumn::Date,
            dir: SortDir::Desc,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextMenuState {
    pub job_id: JobId,
    pub pos: egui::Pos2,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    All,
    Active,
    Finished,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub enum Col {
    Name,
    Size,
    Status,
    Speed,
    Eta,
    Date,
}

impl Col {
    fn idx(self) -> usize {
        match self {
            Col::Name => 0,
            Col::Size => 1,
            Col::Status => 2,
            Col::Speed => 3,
            Col::Eta => 4,
            Col::Date => 5,
        }
    }
    fn label(self) -> &'static str {
        match self {
            Col::Name => "name",
            Col::Size => "size",
            Col::Status => "status",
            Col::Speed => "speed",
            Col::Eta => "time left",
            Col::Date => "date added",
        }
    }
    fn sort(self) -> Option<SortColumn> {
        match self {
            Col::Name => Some(SortColumn::Name),
            Col::Size => Some(SortColumn::Size),
            Col::Status => Some(SortColumn::Status),
            Col::Speed => Some(SortColumn::Speed),
            Col::Eta => Some(SortColumn::Eta),
            Col::Date => Some(SortColumn::Date),
        }
    }
    fn align(self) -> Align {
        Align::LEFT
    }
}

const ALL_COLS: [Col; 6] = [
    Col::Name,
    Col::Size,
    Col::Status,
    Col::Speed,
    Col::Eta,
    Col::Date,
];

const COL_MIN_W: f32 = 50.0;
const RESIZE_HANDLE_W: f32 = 6.0;
const COL_PAD: f32 = space::S1 as f32 * 2.0;

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ColumnsState {
    pub widths: [f32; 6],
    pub hidden: [bool; 6],
}

impl Default for ColumnsState {
    fn default() -> Self {
        let mut widths = [0.0; 6];
        widths[Col::Name.idx()] = 380.0;
        widths[Col::Size.idx()] = 92.0;
        widths[Col::Status.idx()] = 280.0;
        widths[Col::Speed.idx()] = 100.0;
        widths[Col::Eta.idx()] = 90.0;
        widths[Col::Date.idx()] = 130.0;
        Self {
            widths,
            hidden: [false; 6],
        }
    }
}

impl ColumnsState {
    pub fn is_visible(&self, c: Col) -> bool {
        !self.hidden[c.idx()]
    }
    pub fn width(&self, c: Col) -> f32 {
        self.widths[c.idx()]
    }
    pub fn set_width(&mut self, c: Col, w: f32) {
        self.widths[c.idx()] = w.max(COL_MIN_W);
    }
    pub fn toggle(&mut self, c: Col) {
        if c == Col::Name {
            return;
        } // name always visible
        self.hidden[c.idx()] = !self.hidden[c.idx()];
    }
    pub fn visible_cols(&self) -> Vec<Col> {
        ALL_COLS
            .iter()
            .copied()
            .filter(|c| self.is_visible(*c))
            .collect()
    }

    pub fn load() -> Self {
        crate::ui::ui_prefs::load().columns.unwrap_or_default()
    }

    pub fn save(&self) {
        crate::ui::ui_prefs::save_columns(self);
    }
}

pub fn ui(app: &mut AppShell, ui: &mut egui::Ui) {
    let t = theme::tokens(ui.ctx());
    let f = app.filter;
    let q = app.search.to_lowercase();
    let settings = app.snap.settings.clone();
    let mut visible: Vec<Job> = app
        .snap
        .jobs
        .iter()
        .filter(|j| matches_filter(j, f, &settings))
        .filter(|j| match app.tab {
            Tab::All => true,
            Tab::Active => !matches!(
                j.status.phase,
                Phase::Completed | Phase::Failed | Phase::Cancelled
            ),
            Tab::Finished => j.status.phase == Phase::Completed,
        })
        .filter(|j| {
            if q.is_empty() {
                return true;
            }
            let name = j.filename.as_deref().unwrap_or("");
            name.to_lowercase().contains(&q) || j.url.as_str().to_lowercase().contains(&q)
        })
        .cloned()
        .collect();
    sort_jobs(&mut visible, app.sort);

    if ui.input(|i| i.key_pressed(egui::Key::Delete)) {
        super::toolbar::trigger_delete(app);
    }

    // Tab strip + right-side aggregate (kept inset; table grid is full-bleed).
    let tab_rect = egui::Frame::NONE
        .inner_margin(egui::Margin::symmetric(space::S4, 0))
        .show(ui, |ui| tab_strip(app, ui, &t))
        .inner;
    // Divider sits flush with active tab underline (drawn at tab_bottom).
    let sep_y = tab_rect.bottom();
    let clip = ui.clip_rect();
    ui.painter().line_segment(
        [
            Pos2::new(clip.left(), sep_y),
            Pos2::new(clip.right(), sep_y),
        ],
        Stroke::new(1.0, t.border_subtle),
    );
    // Column header row.
    let layout = build_col_layout(&app.columns);
    ui.spacing_mut().item_spacing.y = 0.0;

    let visible_ids: Vec<JobId> = visible.iter().map(|j| j.id).collect();
    let ctx = ui.ctx().clone();
    let viewport_h = ui.available_height();
    let viewport_w = ui.available_width();

    egui::ScrollArea::both()
        .auto_shrink([false; 2])
        .id_salt("table-scroll")
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            paint_header(app, ui, &layout, &t);

            if visible.is_empty() {
                let body_h = (viewport_h - 22.0).max(0.0);
                let body_w = ui.min_rect().width().max(viewport_w);
                let (rect, _) = ui.allocate_exact_size(Vec2::new(body_w, body_h), Sense::hover());
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(rect)
                        .layout(egui::Layout::top_down(Align::Center)),
                );
                child.add_space(60.0);
                icons::show(&mut child, "download", 39.0, t.fg_3);
                child.add_space(space::S2 as f32);
                let searching = !q.is_empty();
                let label = if searching {
                    "No items found"
                } else {
                    match app.tab {
                        Tab::All => "No downloads yet",
                        Tab::Active => "Nothing in progress",
                        Tab::Finished => "No completed downloads",
                    }
                };
                child.label(
                    egui::RichText::new(label)
                        .font(theme::display(20.0))
                        .color(t.fg_2),
                );
                child.add_space(space::S1 as f32);
                let sub = if searching {
                    "Try a different search term."
                } else {
                    "Add a URL above to start."
                };
                child.label(egui::RichText::new(sub).color(t.fg_3));
                return;
            }

            for (i, job) in visible.iter().enumerate() {
                let id = job.id;
                let selected = app.selection.contains(&id);
                let resp = paint_row(ui, app, job, selected, &layout, i);

                if resp.clicked() {
                    let mods = ctx.input(|i| i.modifiers);
                    if mods.shift
                        && let Some(anchor) = app.last_clicked
                    {
                        let a = visible_ids.iter().position(|x| *x == anchor).unwrap_or(0);
                        let b = i;
                        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                        if !(mods.ctrl || mods.command) {
                            app.selection.clear();
                        }
                        for vid in &visible_ids[lo..=hi] {
                            app.selection.insert(*vid);
                        }
                    } else if mods.ctrl || mods.command {
                        if !app.selection.shift_remove(&id) {
                            app.selection.insert(id);
                        }
                        app.last_clicked = Some(id);
                    } else {
                        app.selection.clear();
                        app.selection.insert(id);
                        app.last_clicked = Some(id);
                    }
                }
                if resp.double_clicked() {
                    if job.status.phase == Phase::Completed {
                        if let Some(p) = job.status.final_path.clone() {
                            crate::ui::platform::open_path(&p);
                        }
                    } else {
                        crate::ui::windows::download::state::spawn(app, id);
                    }
                }
                // Build the row context menu manually so we can change
                // the close behaviour. egui's default for menus is
                // `CloseOnClick` — a press ANYWHERE inside the menu
                // dismisses it, including on a hover-only submenu
                // trigger like "Move To Queue". Switch to
                // `CloseOnClickOutside`; menu items still dismiss
                // explicitly via `ui.close()`.
                egui::Popup::context_menu(&resp)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| {
                        row_context_menu(app, ui, id);
                    });
            }

            // Click in the empty area below the last row deselects.
            let remaining_h = ui.available_height();
            if remaining_h > 0.0 {
                let body_w = ui.min_rect().width().max(viewport_w);
                let (_, empty_resp) =
                    ui.allocate_exact_size(Vec2::new(body_w, remaining_h), Sense::click());
                if empty_resp.clicked() {
                    app.selection.clear();
                    app.last_clicked = None;
                }
            }
        });
}

fn tab_strip(app: &mut AppShell, ui: &mut egui::Ui, _t: &theme::Tokens) -> egui::Rect {
    let counts = (
        // All visible (matches sidebar filter, not tab).
        app.snap
            .jobs
            .iter()
            .filter(|j| matches_filter(j, app.filter, &app.snap.settings))
            .count(),
        app.snap
            .jobs
            .iter()
            .filter(|j| {
                matches_filter(j, app.filter, &app.snap.settings)
                    && !matches!(
                        j.status.phase,
                        Phase::Completed | Phase::Failed | Phase::Cancelled
                    )
            })
            .count(),
        app.snap
            .jobs
            .iter()
            .filter(|j| {
                matches_filter(j, app.filter, &app.snap.settings)
                    && j.status.phase == Phase::Completed
            })
            .count(),
    );

    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for (which, label, n) in [
            (Tab::All, "All", counts.0),
            (Tab::Active, "Active", counts.1),
            (Tab::Finished, "Finished", counts.2),
        ] {
            if TabBtn::new(label)
                .count(n)
                .active(app.tab == which)
                .show(ui)
                .clicked()
            {
                app.tab = which;
                ui.request_repaint();
            }
        }
    })
    .response
    .rect
}

#[derive(Debug, Clone, Copy)]
struct ColSlot {
    col: Col,
    x: f32,
    w: f32,
}

fn build_col_layout(state: &ColumnsState) -> Vec<ColSlot> {
    let visible = state.visible_cols();
    let gap = space::S1 as f32;
    let mut out = Vec::with_capacity(visible.len());
    let mut x = 0.0;
    for (i, c) in visible.iter().enumerate() {
        let w = state.width(*c).max(COL_MIN_W);
        out.push(ColSlot { col: *c, x, w });
        x += w;
        if i + 1 < visible.len() {
            x += gap;
        }
    }
    out
}

fn layout_total_w(layout: &[ColSlot]) -> f32 {
    layout.iter().map(|s| s.w).sum::<f32>()
        + space::S1 as f32 * (layout.len().saturating_sub(1) as f32)
}

fn paint_header(app: &mut AppShell, ui: &mut Ui, layout: &[ColSlot], t: &theme::Tokens) {
    let header_h = 22.0;
    let avail_w = ui.available_width();
    let content_w = layout_total_w(layout);
    let total_w = content_w.max(avail_w);
    let extra = (avail_w - content_w).max(0.0);
    let last_idx = layout.len().saturating_sub(1);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(total_w, header_h), Sense::hover());
    for (idx, slot) in layout.iter().enumerate() {
        let cell_w = if idx == last_idx {
            slot.w + extra
        } else {
            slot.w
        };
        let cell_rect = Rect::from_min_size(
            Pos2::new(rect.left() + slot.x, rect.top()),
            Vec2::new(cell_w, header_h),
        );
        let inner_left = cell_rect.left() + COL_PAD;
        let inner_right = (cell_rect.right() - COL_PAD).max(inner_left);
        let inner_w = inner_right - inner_left;
        let cy = cell_rect.center().y;
        let align = slot.col.align();

        let resp = ui.interact(
            cell_rect,
            ui.id().with(("hdr", slot.col.idx())),
            Sense::click(),
        );
        // Sortable headers show the clickable cursor on hover.
        if slot.col.sort().is_some() && resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let active_sort = slot
            .col
            .sort()
            .map(|sc| app.sort.col == sc)
            .unwrap_or(false);
        let desc = app.sort.dir == SortDir::Desc;
        let color = if active_sort || resp.hovered() {
            t.fg_2
        } else {
            t.fg_3
        };
        let icon_size = 11.0;
        let chevron_gap = 4.0;
        let chevron_w = if active_sort {
            icon_size + chevron_gap
        } else {
            0.0
        };
        let text_max_w = (inner_w - chevron_w).max(0.0);
        let fmt = egui::TextFormat {
            font_id: theme::body_bold(11.0),
            color,
            extra_letter_spacing: 0.8,
            ..Default::default()
        };
        let galley = ellipsized_fmt(
            ui.painter(),
            &slot.col.label().to_uppercase(),
            fmt,
            text_max_w,
        );
        let painter = ui.painter().clone();
        let text_w = galley.size().x;
        let text_h = galley.size().y;
        let (text_x, icon_x) = match align {
            Align::RIGHT => {
                if active_sort {
                    let icon_x = inner_right - icon_size;
                    (icon_x - chevron_gap - text_w, icon_x)
                } else {
                    (inner_right - text_w, inner_right)
                }
            }
            _ => {
                let tx = inner_left;
                (tx, tx + text_w + chevron_gap)
            }
        };
        painter.galley(Pos2::new(text_x, cy - text_h / 2.0), galley, color);
        if active_sort {
            let name = if desc { "chevron-down" } else { "chevron-up" };
            let img = crate::ui::utils::icons::icon(ui.ctx(), name, icon_size, color);
            let irect = Rect::from_min_size(
                Pos2::new(icon_x, cy - icon_size / 2.0),
                Vec2::splat(icon_size),
            );
            img.paint_at(ui, irect);
        }
        if let Some(sc) = slot.col.sort()
            && resp.clicked()
        {
            if active_sort {
                app.sort.dir = if desc { SortDir::Asc } else { SortDir::Desc };
            } else {
                app.sort.col = sc;
                app.sort.dir = match sc {
                    SortColumn::Name => SortDir::Asc,
                    _ => SortDir::Desc,
                };
            }
        }
        resp.context_menu(|ui| header_context_menu(ui, &mut app.columns));

        // Resize handle on right edge (last col gets one too).
        let _ = idx;
        let sep_x = cell_rect.right() + space::S1 as f32 * 0.5;
        let handle_rect = Rect::from_min_size(
            Pos2::new(sep_x - RESIZE_HANDLE_W * 0.5, cell_rect.top()),
            Vec2::new(RESIZE_HANDLE_W, header_h),
        );
        let handle_id = ui.id().with(("hdr-resize", slot.col.idx()));
        let h_resp = ui.interact(handle_rect, handle_id, Sense::drag());
        let active = h_resp.hovered() || h_resp.dragged();
        if active {
            ui.set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        }
        let line_color = if active { t.fg_2 } else { t.border_subtle };
        let line_w = if active { 2.0 } else { 1.0 };
        ui.painter().line_segment(
            [
                Pos2::new(sep_x, cell_rect.top()),
                Pos2::new(sep_x, cell_rect.bottom()),
            ],
            Stroke::new(line_w, line_color),
        );
        if h_resp.dragged() {
            let dx = h_resp.drag_delta().x;
            let cur = app.columns.width(slot.col);
            app.columns.set_width(slot.col, cur + dx);
        }
        if h_resp.drag_stopped() {
            app.columns.save();
        }
    }

    // Bottom border under header. Inset by 0.5 px so the full 1px
    // stroke sits inside the header rect — otherwise the first row's
    // background fill covers the lower half of the line.
    let sep_y = rect.bottom() - 0.5;
    let clip = ui.clip_rect();
    ui.painter().line_segment(
        [
            Pos2::new(clip.left(), sep_y),
            Pos2::new(clip.right(), sep_y),
        ],
        Stroke::new(1.0, t.border_subtle),
    );
}

fn header_context_menu(ui: &mut Ui, state: &mut ColumnsState) {
    ui.set_min_width(180.0);
    ui.label(
        egui::RichText::new("Columns")
            .color(theme::tokens(ui.ctx()).fg_3)
            .small(),
    );
    ui.separator();
    for c in ALL_COLS.iter().copied() {
        let mut visible = state.is_visible(c);
        let enabled = c != Col::Name;
        let label = match c {
            Col::Name => "Name",
            Col::Size => "Size",
            Col::Status => "Status",
            Col::Speed => "Speed",
            Col::Eta => "Time left",
            Col::Date => "Date added",
        };
        let resp = ui.add_enabled(enabled, egui::Checkbox::new(&mut visible, label));
        if resp.changed() {
            state.toggle(c);
            state.save();
        }
    }
}

fn paint_row(
    ui: &mut Ui,
    app: &AppShell,
    job: &Job,
    selected: bool,
    layout: &[ColSlot],
    row_index: usize,
) -> Response {
    let t = theme::tokens(ui.ctx());

    let counters = app.cache.job_counters(job.id);
    let (downloaded, total, speed_bps, phase) = match counters {
        Some(c) => {
            let bps = if c.phase.is_running() {
                c.speed_bps
            } else {
                0.0
            };
            (c.downloaded, c.total, bps, c.phase)
        }
        None => (0u64, None, 0.0, job.status.phase),
    };
    let frac = match total {
        Some(t) if t > 0 => (downloaded as f32) / (t as f32),
        _ => 0.0,
    };
    let frac = frac.clamp(0.0, 1.0);

    let filename = job
        .filename
        .clone()
        .unwrap_or_else(|| job.url.path().rsplit('/').next().unwrap_or("").to_string());
    let host = job.url.host_str().unwrap_or("").to_string();
    let eta = match (total, speed_bps) {
        (Some(tt), s) if s > 0.0 && tt > downloaded => {
            let secs = ((tt - downloaded) as f64 / s) as u64;
            humantime::format_duration(std::time::Duration::from_secs(secs)).to_string()
        }
        _ => "—".into(),
    };

    // Content: name (~16) + 2 + host (~14) = ~32. Plus vertical padding.
    let row_h = 32.0 + space::S2 as f32 * 2.0;
    let total_w = layout_total_w(layout).max(ui.available_width());
    let row_id = ui.id().with(("oxdm-row", row_index));
    let (_, rect) = ui.allocate_space(Vec2::new(total_w, row_h));
    let resp = ui.interact(rect, row_id, Sense::click());
    let painter = ui.painter().clone();

    // Row background spans the full panel width (extends past the
    // central-panel inner margin so rows reach the sidebar/right edge).
    let clip = ui.clip_rect();
    // Reserve bottom 1px for the hairline so highlight tint doesn't bleed across it.
    let bg_rect = egui::Rect::from_min_max(
        Pos2::new(clip.left(), rect.top()),
        Pos2::new(clip.right(), rect.bottom() - 1.0),
    );
    let bg = if selected && resp.hovered() {
        // Matches design `tr.selected:hover` (clay-100 in light mode).
        soft_tint(t.action_primary, t.bg_surface, 0.32)
    } else if selected {
        // Matches design `tr.selected` (clay-50 in light mode).
        soft_tint(t.action_primary, t.bg_surface, 0.14)
    } else if resp.hovered() {
        t.bg_sunken
    } else {
        t.bg_page
    };
    painter.rect_filled(bg_rect, 0.0, bg);

    let cy = rect.center().y;

    for slot in layout {
        let cell_left = rect.left() + slot.x;
        let cell_right = cell_left + slot.w;
        let cell_x = cell_left + COL_PAD;
        let cell_w = (slot.w - 2.0 * COL_PAD).max(0.0);
        match slot.col {
            Col::Name => {
                let nfont = theme::body_bold(13.0);
                let hfont = theme::mono(10.0);
                let name_galley = ellipsized(&painter, &filename, nfont, t.fg_1, cell_w);
                let host_galley = ellipsized(&painter, &host, hfont, t.fg_3, cell_w);
                let total_h = name_galley.size().y + 2.0 + host_galley.size().y;
                let mut ny = cy - total_h / 2.0;
                let nh = name_galley.size().y;
                painter.galley(Pos2::new(cell_x, ny), name_galley, t.fg_1);
                ny += nh + 2.0;
                painter.galley(Pos2::new(cell_x, ny), host_galley, t.fg_3);
            }
            Col::Size => {
                let size_text = format_bytes_opt(total);
                paint_cell_text(
                    &painter,
                    &t,
                    cell_x,
                    cy,
                    cell_w,
                    &size_text,
                    theme::mono(12.0),
                    t.fg_2,
                    Align::LEFT,
                );
            }
            Col::Speed => {
                let speed_text = if phase.is_running() {
                    format_speed(speed_bps)
                } else {
                    "—".into()
                };
                paint_cell_text(
                    &painter,
                    &t,
                    cell_x,
                    cy,
                    cell_w,
                    &speed_text,
                    theme::mono(12.0),
                    t.fg_2,
                    Align::LEFT,
                );
            }
            Col::Eta => {
                paint_cell_text(
                    &painter,
                    &t,
                    cell_x,
                    cy,
                    cell_w,
                    &eta,
                    theme::mono(12.0),
                    t.fg_2,
                    Align::LEFT,
                );
            }
            Col::Status => {
                paint_status_cell(&painter, &t, cell_x, cy, cell_w, phase, frac, selected);
            }
            Col::Date => {
                let date_text = format_short_date(&job.created_at);
                paint_cell_text(
                    &painter,
                    &t,
                    cell_x,
                    cy,
                    cell_w,
                    &date_text,
                    theme::body(12.0),
                    t.fg_3,
                    Align::LEFT,
                );
            }
        }
        let _ = cell_right;
    }

    // Bottom hairline — sits in the 1px gap reserved below bg_rect, full panel width.
    painter.line_segment(
        [
            Pos2::new(clip.left(), rect.bottom() - 0.5),
            Pos2::new(clip.right(), rect.bottom() - 0.5),
        ],
        Stroke::new(1.0, t.border_subtle),
    );

    use super::primitives::Clickable;
    resp.clickable()
}

#[allow(clippy::too_many_arguments)]
fn paint_cell_text(
    painter: &egui::Painter,
    _t: &theme::Tokens,
    x: f32,
    cy: f32,
    w: f32,
    text: &str,
    font: egui::FontId,
    color: Color32,
    align: Align,
) {
    let galley = ellipsized(painter, text, font, color, w);
    let tx = match align {
        Align::LEFT => x,
        Align::Center => x + (w - galley.size().x) / 2.0,
        Align::RIGHT => x + w - galley.size().x,
    };
    painter.galley(Pos2::new(tx, cy - galley.size().y / 2.0), galley, color);
}

fn ellipsized(
    painter: &egui::Painter,
    text: &str,
    font: egui::FontId,
    color: Color32,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    ellipsized_fmt(
        painter,
        text,
        egui::TextFormat {
            font_id: font,
            color,
            ..Default::default()
        },
        max_width,
    )
}

fn ellipsized_fmt(
    painter: &egui::Painter,
    text: &str,
    fmt: egui::TextFormat,
    max_width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::single_section(text.to_owned(), fmt);
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    painter.layout_job(job)
}

fn soft_tint(accent: Color32, base: Color32, t: f32) -> Color32 {
    let lerp = |a: u8, b: u8| (a as f32 * (1.0 - t) + b as f32 * t) as u8;
    Color32::from_rgb(
        lerp(base.r(), accent.r()),
        lerp(base.g(), accent.g()),
        lerp(base.b(), accent.b()),
    )
}

#[allow(clippy::too_many_arguments)]
fn paint_status_cell(
    painter: &egui::Painter,
    t: &theme::Tokens,
    x: f32,
    cy: f32,
    w: f32,
    phase: Phase,
    frac: f32,
    selected: bool,
) {
    let (dot_color, mut label) = phase_visual(phase, t);
    let show_bar = match phase {
        Phase::Downloading
        | Phase::Evaluating
        | Phase::Assembling
        | Phase::ResolvingConflicts
        | Phase::Flushing
        | Phase::Verifying => true,
        Phase::Paused | Phase::Failed | Phase::Cancelled | Phase::Queued => frac > 0.0,
        Phase::Completed => false,
    };
    // `cancel_to_queued` preserves progress while flipping phase back to
    // Queued — surface that as "Cancelled" inside the bar so the row never
    // claims it's queued once bytes have landed.
    if matches!(phase, Phase::Queued) && frac > 0.0 {
        label = "Cancelled".to_owned();
    }
    if show_bar {
        let bar_h = 22.0;
        let bar_rect = Rect::from_min_size(
            Pos2::new(x, cy - bar_h / 2.0),
            Vec2::new(w.max(20.0), bar_h),
        );
        super::primitives::inline_progress(painter, bar_rect, t, frac, &label, selected);
    } else {
        painter.circle_filled(Pos2::new(x + 4.0, cy), 4.0, dot_color);
        let label_x = x + 14.0;
        let label_w = (w - 14.0).max(0.0);
        let g = ellipsized(painter, &label, theme::body_bold(12.0), dot_color, label_w);
        painter.galley(Pos2::new(label_x, cy - g.size().y / 2.0), g, dot_color);
    }
}

fn phase_visual(p: Phase, t: &theme::Tokens) -> (Color32, String) {
    match p {
        Phase::Downloading
        | Phase::Evaluating
        | Phase::Assembling
        | Phase::ResolvingConflicts
        | Phase::Flushing
        | Phase::Verifying => (t.action_primary, "Downloading".to_owned()),
        Phase::Queued => (t.status_info, "Queued".to_owned()),
        Phase::Paused => (t.fg_3, "Paused".to_owned()),
        Phase::Cancelled => (t.fg_3, "Cancelled".to_owned()),
        Phase::Completed => (t.status_success, "Complete".to_owned()),
        Phase::Failed => (t.status_danger, "Failed".to_owned()),
    }
}

fn format_short_date(dt: &chrono::DateTime<chrono::Utc>) -> String {
    let local = dt.with_timezone(&chrono::Local);
    let now = chrono::Local::now();
    let today = now.date_naive();
    let date = local.date_naive();
    let hm = local.format("%H:%M").to_string();
    if date == today {
        format!("Today, {hm}")
    } else if date == today.pred_opt().unwrap_or(today) {
        format!("Yesterday, {hm}")
    } else if date.year() == today.year() {
        local.format("%b %d").to_string()
    } else {
        local.format("%b %d, %Y").to_string()
    }
}

use chrono::Datelike;

fn row_context_menu(app: &mut AppShell, ui: &mut egui::Ui, id: JobId) {
    let s = app.client.clone();
    let entry = app.cache.job_entry_cached(id);
    let phase = entry
        .as_ref()
        .map(|e| e.counters.phase)
        .unwrap_or(Phase::Queued);
    let completed_final = entry.as_ref().and_then(|e| {
        (e.counters.phase == Phase::Completed)
            .then(|| e.job.status.final_path.clone())
            .flatten()
    });

    // Pre-measure every possible row so the right-aligned shortcut column
    // lines up across the menu and the menu only takes the width it
    // actually needs (+ a fixed `MENU_LABEL_TO_KBD_GAP`).
    let width = mu::measure_width(
        ui.ctx(),
        &[
            (Some("file"), "Open", Some("Ctrl+O")),
            (
                Some("folder"),
                crate::ui::platform::reveal_label(),
                Some("Ctrl+F"),
            ),
            (Some("play"), "Resume", Some("Ctrl+R")),
            (Some("pause"), "Pause", Some("Ctrl+P")),
            (Some("trash-2"), "Delete", Some("Delete")),
            (Some("refresh-cw"), "Restart Download", None),
            (Some("list"), "Move To Queue", None),
            (Some("layers"), "Move To Category", None),
            (Some("copy"), "Copy URL", None),
            (Some("info"), "Show Properties", None),
        ],
    );
    // Constrain BOTH min and max so each row's `ui.available_width()`
    // resolves to the measured menu width — not whatever the parent
    // popup defaulted to.
    ui.set_width(width);
    ui.spacing_mut().item_spacing.y = 0.0;

    let resumable = matches!(
        phase,
        Phase::Paused | Phase::Queued | Phase::Failed | Phase::Cancelled
    );
    let pausable = phase.is_running();
    let restartable = !matches!(phase, Phase::Queued);

    if mu::item(ui, Some("file"), "Open", Some("Ctrl+O"), true).clicked() {
        match &completed_final {
            Some(p) => crate::ui::platform::open_path(p),
            None => crate::ui::windows::download::state::spawn(app, id),
        }
        ui.close();
    }
    if mu::item(
        ui,
        Some("folder"),
        crate::ui::platform::reveal_label(),
        Some("Ctrl+F"),
        true,
    )
    .clicked()
    {
        match &completed_final {
            Some(p) => crate::ui::platform::reveal_in_folder(p),
            None => {
                if let Some(e) = entry.as_ref() {
                    crate::ui::platform::open_path(&e.job.save_dir);
                }
            }
        }
        ui.close();
    }
    if mu::item(ui, Some("play"), "Resume", Some("Ctrl+R"), resumable).clicked() {
        let s2 = s.clone();
        app.spawn(async move {
            let _ = s2.resume(id).await;
        });
        ui.close();
    }
    if mu::item(ui, Some("pause"), "Pause", Some("Ctrl+P"), pausable).clicked() {
        let s2 = s.clone();
        app.spawn(async move {
            let _ = s2.pause(id).await;
        });
        ui.close();
    }

    mu::separator(ui);

    if mu::item(ui, Some("trash-2"), "Delete", Some("Delete"), true).clicked() {
        if let Some(entry) = app.cache.job_entry_cached(id) {
            let phase = entry.counters.phase;
            let filename = entry
                .job
                .filename
                .clone()
                .unwrap_or_else(|| entry.job.url.to_string());
            let must_confirm = match phase {
                Phase::Completed => app.snap.settings.remove_confirm_completed,
                _ => app.snap.settings.remove_confirm_incomplete,
            };
            if must_confirm {
                app.remove = Some(crate::ui::dialogs::remove::RemoveRequest {
                    id,
                    filename,
                    phase,
                });
                app.remove_state = crate::ui::dialogs::remove::RemoveState::default();
            } else {
                let opts = match phase {
                    Phase::Completed => RemoveOpts {
                        purge_partial: false,
                        delete_final_file: false,
                    },
                    _ => RemoveOpts {
                        purge_partial: true,
                        delete_final_file: false,
                    },
                };
                let s2 = s.clone();
                app.spawn(async move {
                    let _ = s2.remove(id, opts).await;
                });
            }
        }
        ui.close();
    }
    if mu::item(
        ui,
        Some("refresh-cw"),
        "Restart Download",
        None,
        restartable,
    )
    .clicked()
    {
        let s2 = s.clone();
        app.spawn(async move {
            let _ = s2.restart_job(id).await;
        });
        ui.close();
    }

    let queues = app.snap.queues.clone();
    mu::submenu(ui, "list", "Move To Queue", |ui| {
        // No icons in this submenu → use the compact variant so labels
        // sit flush against the left pad instead of being indented by
        // a reserved-but-empty icon column.
        let items: Vec<(&str, Option<&str>)> =
            queues.iter().map(|q| (q.name.as_str(), None)).collect();
        ui.set_width(mu::measure_width_plain(ui.ctx(), &items));
        ui.spacing_mut().item_spacing.y = 0.0;
        for q in queues.iter() {
            let qid = q.id;
            let s2 = s.clone();
            if mu::item_plain(ui, &q.name, None, true).clicked() {
                app.spawn(async move {
                    let _ = s2.set_job_queue(id, qid).await;
                });
                ui.close();
            }
        }
    });
    let current_cat = entry.as_ref().map(|e| e.job.category);
    mu::submenu(ui, "layers", "Move To Category", |ui| {
        let labels: Vec<(&str, Option<&str>)> = Category::ALL_ASSIGNABLE
            .iter()
            .map(|c| (c.label(), None))
            .collect();
        ui.set_width(mu::measure_width_plain(ui.ctx(), &labels));
        ui.spacing_mut().item_spacing.y = 0.0;

        for cat in Category::ALL_ASSIGNABLE.iter().copied() {
            let is_current = current_cat == Some(cat);
            if mu::item_plain(ui, cat.label(), None, !is_current).clicked() {
                let s2 = s.clone();
                app.spawn(async move {
                    let _ = s2.set_job_category(id, cat).await;
                });
                ui.close();
            }
        }
    });

    if mu::item(ui, Some("copy"), "Copy URL", None, true).clicked() {
        if let Some(e) = entry.as_ref() {
            let url = e.job.url.to_string();
            ui.ctx()
                .output_mut(|o| o.commands.push(egui::OutputCommand::CopyText(url)));
        }
        ui.close();
    }

    mu::separator(ui);

    if mu::item(ui, Some("info"), "Show Properties", None, true).clicked() {
        crate::ui::ask_open_properties(app, id);
        ui.close();
    }
}

// Menu primitives live in `primitives::menu`. Aliases keep the local
// call sites short while the rest of the file is refactored.
use super::primitives::menu as mu;

fn sort_jobs(jobs: &mut [Job], sort: TableSort) {
    let dir = sort.dir;
    let cmp_fn: fn(&Job, &Job) -> std::cmp::Ordering = match sort.col {
        SortColumn::Name => |a, b| {
            a.filename
                .as_deref()
                .unwrap_or("")
                .cmp(b.filename.as_deref().unwrap_or(""))
        },
        SortColumn::Size => |a, b| a.status.total.cmp(&b.status.total),
        SortColumn::Status => |a, b| phase_order(a.status.phase).cmp(&phase_order(b.status.phase)),
        SortColumn::Speed => |a, b| {
            a.status
                .speed_bps
                .partial_cmp(&b.status.speed_bps)
                .unwrap_or(std::cmp::Ordering::Equal)
        },
        SortColumn::Eta => |a, b| a.status.eta_secs.cmp(&b.status.eta_secs),
        SortColumn::Date => |a, b| a.created_at.cmp(&b.created_at),
    };
    jobs.sort_by(|a, b| {
        let ord = cmp_fn(a, b);
        match dir {
            SortDir::Asc => ord,
            SortDir::Desc => ord.reverse(),
        }
    });
}

fn phase_order(p: Phase) -> u8 {
    match p {
        Phase::Downloading
        | Phase::Evaluating
        | Phase::Assembling
        | Phase::ResolvingConflicts
        | Phase::Flushing
        | Phase::Verifying => 0,
        Phase::Queued => 1,
        Phase::Paused | Phase::Cancelled => 2,
        Phase::Completed => 3,
        Phase::Failed => 4,
    }
}

pub fn phase_label(p: Phase) -> String {
    match p {
        Phase::Queued => "Queued",
        Phase::Evaluating => "Evaluating",
        Phase::ResolvingConflicts => "Resolving",
        Phase::Downloading => "Downloading",
        Phase::Assembling => "Assembling",
        Phase::Flushing => "Flushing",
        Phase::Verifying => "Verifying",
        Phase::Paused => "Paused",
        Phase::Completed => "Complete",
        Phase::Failed => "Failed",
        Phase::Cancelled => "Cancelled",
    }
    .into()
}

#[allow(dead_code)]
fn _u(_: f32) {
    let _ = (radius::SM as f32, format_bytes(0));
}
