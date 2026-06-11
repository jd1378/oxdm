//! Headless self-screenshot harness. When `OXDM_SHOT=<path>[:<ms>]`
//! is set, a window subscribes to `window::frames()`, waits `ms`
//! milliseconds (default 1500; min 35 frames so fonts/SVGs settle),
//! captures itself to a PNG, and exits. Used by the visual-test
//! workflow (no WM/compositor needed — X11 capture of a borderless
//! software window is black; frames are unthrottled on Xvfb, so the
//! delay is wall-clock, not frame-count).

use std::time::Instant;

use iced::window;
use iced::{Subscription, Task};

pub struct Shot {
    path: String,
    delay_ms: u64,
    started: Instant,
    frames_seen: u32,
    fired: bool,
}

impl Shot {
    /// Reads `OXDM_SHOT`; `None` in normal runs.
    pub fn from_env() -> Option<Self> {
        let spec = std::env::var("OXDM_SHOT").ok()?;
        let (path, delay_ms) = match spec.rsplit_once(':') {
            Some((p, n)) if n.chars().all(|c| c.is_ascii_digit()) && !p.is_empty() => {
                (p.to_owned(), n.parse().unwrap_or(1500))
            }
            _ => (spec, 1500),
        };
        Some(Self {
            path,
            delay_ms,
            started: Instant::now(),
            frames_seen: 0,
            fired: false,
        })
    }

    /// Per-frame tick; returns the screenshot task once the delay has
    /// elapsed. Map the resulting `Screenshot` into the window's
    /// message and feed it to [`Shot::save_and_exit`].
    pub fn tick(&mut self) -> Option<Task<window::Screenshot>> {
        self.frames_seen += 1;
        if self.fired
            || self.frames_seen < 35
            || self.started.elapsed().as_millis() < self.delay_ms as u128
        {
            return None;
        }
        self.fired = true;
        Some(window::latest().and_then(window::screenshot))
    }

    /// Save the screenshot and quit.
    pub fn save_and_exit<M: Send + 'static>(&self, shot: window::Screenshot) -> Task<M> {
        match image::RgbaImage::from_raw(shot.size.width, shot.size.height, shot.rgba.to_vec()) {
            Some(img) => {
                if let Err(e) = img.save(&self.path) {
                    eprintln!("OXDM_SHOT: failed to save {}: {e}", self.path);
                } else {
                    println!("OXDM_SHOT: saved {}", self.path);
                }
            }
            None => eprintln!("OXDM_SHOT: bad screenshot buffer"),
        }
        iced::exit()
    }

    /// Raw frame subscription — map to the window's tick message with
    /// a NON-capturing closure (`.map(|_| Msg::ShotTick)`).
    pub fn frames() -> Subscription<std::time::Instant> {
        window::frames()
    }
}
