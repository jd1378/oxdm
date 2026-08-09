//! Put text on the clipboard and hold it there.
//!
//! Test scaffolding for the visual-test harness: X11 has no "set and
//! forget" clipboard — the owner process must stay alive to answer
//! paste requests — and the sandbox has no `xclip`. Reads the text
//! from argv (or stdin when given `-`), then sleeps.
//!
//!     DISPLAY=:99 cargo run --example clipset -- 'https://example/f.bin' &

use std::io::Read;

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| "-".into());
    // `--get` prints what oxdm's own reader sees, which is the question
    // when a paste appears to do nothing.
    if arg == "--get" {
        match oxdm::gui::clipboard::read_text() {
            Some(t) => println!("read_text: [{t}]"),
            None => println!("read_text: <nothing>"),
        }
        match oxdm::gui::clipboard::clipboard_first_link() {
            Some(t) => println!("first_link: [{t}]"),
            None => println!("first_link: <nothing>"),
        }
        let links = oxdm::gui::clipboard::clipboard_links();
        println!("links: {}", links.len());
        for l in links {
            println!("  {l}");
        }
        return;
    }
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
