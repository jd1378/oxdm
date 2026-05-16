//! Private helpers shared across widget modules.

use eframe::egui::{Color32, WidgetText};

pub(super) fn text_string(t: &WidgetText) -> String {
    match t {
        WidgetText::Text(s) => s.clone(),
        WidgetText::RichText(r) => r.text().to_owned(),
        WidgetText::LayoutJob(j) => j.text.clone(),
        WidgetText::Galley(g) => g.text().to_owned(),
    }
}

pub(super) fn darken(c: Color32, t: f32) -> Color32 {
    let lerp = |x: u8| ((x as f32) * (1.0 - t)).round() as u8;
    Color32::from_rgb(lerp(c.r()), lerp(c.g()), lerp(c.b()))
}

pub(super) fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let lerp = |x: u8, y: u8| ((x as f32) * (1.0 - t) + (y as f32) * t) as u8;
    Color32::from_rgb(lerp(a.r(), b.r()), lerp(a.g(), b.g()), lerp(a.b(), b.b()))
}
