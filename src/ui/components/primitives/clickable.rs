//! Cursor extension trait — sets the pointing-hand cursor on clickable
//! widgets so consumers don't each have to remember.

use eframe::egui::{self, Response};

pub trait Clickable {
    fn clickable(self) -> Self;
}

impl Clickable for Response {
    fn clickable(self) -> Self {
        if (self.sense.senses_click() || self.sense.senses_drag())
            && self.enabled()
            && self.contains_pointer()
        {
            self.ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        self
    }
}
