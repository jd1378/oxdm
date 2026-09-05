//! Remembering a window's size across launches.
//!
//! Resize events arrive on every pointer move of a drag, so the size is
//! persisted only once they go quiet, and only when the window is not
//! maximized: a saved maximized geometry would make every later launch
//! open filling the screen.

use iced::Task;

use crate::gui::ui_prefs::{self, WindowPrefs, WindowSlot};

/// Quiet period after the last resize event before the size is
/// persisted. Long enough to cover a drag, short enough to survive a
/// close right afterwards.
const SETTLE_MS: u64 = 400;

pub struct SizeMemo {
    slot: WindowSlot,
    /// Bumped on every resize event; a settle callback only persists if
    /// it still owns the latest generation. Throttling instead of
    /// debouncing dropped the *final* size of any drag shorter than the
    /// throttle window.
    generation: u64,
    size: (f32, f32),
}

impl SizeMemo {
    pub fn new(slot: WindowSlot) -> Self {
        Self {
            slot,
            generation: 0,
            size: (0.0, 0.0),
        }
    }

    /// The saved size for `slot`, no smaller than `min`, or `fallback`
    /// when nothing was saved.
    pub fn launch_size(slot: WindowSlot, fallback: iced::Size, min: iced::Size) -> iced::Size {
        ui_prefs::load_window(slot)
            .map(|w| iced::Size::new(w.width.max(min.width), w.height.max(min.height)))
            .unwrap_or(fallback)
    }

    /// A resize event landed. `settled` builds the message the settle
    /// timer sends back; hand it to [`Self::settled`].
    pub fn resized<M: Send + 'static>(&mut self, w: f32, h: f32, settled: fn(u64) -> M) -> Task<M> {
        self.size = (w, h);
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        Task::perform(
            async move {
                tokio::time::sleep(std::time::Duration::from_millis(SETTLE_MS)).await;
            },
            move |()| settled(generation),
        )
    }

    /// The settle timer for `generation` elapsed: ask the window whether
    /// it is maximized, then hand the answer to [`Self::save`] through
    /// `done`.
    pub fn settled<M: Send + 'static>(&self, generation: u64, done: fn(bool) -> M) -> Task<M> {
        if generation != self.generation {
            return Task::none(); // a later resize superseded this one
        }
        iced::window::latest()
            .and_then(iced::window::is_maximized)
            .map(done)
    }

    pub fn save(&self, maximized: bool) {
        if maximized || self.size.0 <= 0.0 {
            return;
        }
        ui_prefs::save_window(
            self.slot,
            WindowPrefs {
                width: self.size.0,
                height: self.size.1,
            },
        );
    }
}
