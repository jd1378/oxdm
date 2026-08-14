//! Put text on the clipboard and hold it there.
//!
//! Test scaffolding for the visual-test harness: X11 has no "set and
//! forget" clipboard — the owner process must stay alive to answer
//! paste requests — and the sandbox has no `xclip`. Reads the text
//! from argv (or stdin when given `-`), then sleeps.
//!
//!     DISPLAY=:99 cargo run --example clipset -- 'https://example/f.bin' &
//!
//! Writes only. What oxdm *reads* is `iced::clipboard`, which needs a
//! window and a display-server connection of its own, so the way to
//! see what a paste finds is to paste in the app.

use std::io::Read;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "-".into());
    let text = if arg == "-" {
        let mut s = String::new();
        let _ = std::io::stdin().read_to_string(&mut s);
        s
    } else {
        arg
    };
    let secs: u64 = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(120);

    let mut cb = match arboard::Clipboard::new() {
        Ok(cb) => cb,
        Err(e) => {
            eprintln!("clipset: no clipboard: {e}");
            std::process::exit(1);
        }
    };
    if let Err(e) = cb.set_text(text) {
        eprintln!("clipset: set failed: {e}");
        std::process::exit(1);
    }
    println!("clipset: holding for {secs}s");
    std::thread::sleep(std::time::Duration::from_secs(secs));
}
