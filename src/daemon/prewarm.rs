//! Warm the page cache for the first window.
//!
//! Every window is a fresh process, and the first one after a boot
//! pages its share of the executable in from disk: the daemon runs from
//! the same file, but only its own pages, and the windows' code is
//! elsewhere in it and untouched until one starts.
//!
//! Done once, when the daemon starts, on a thread nothing waits for. A
//! failure costs nothing but the warm-up.

use std::io::Read;

pub fn spawn() {
    let _ = std::thread::Builder::new()
        .name("oxdm-prewarm".into())
        .spawn(|| {
            // The daemon has its own startup to get through first, and
            // this can wait a moment without missing the first click.
            std::thread::sleep(std::time::Duration::from_secs(2));
            touch_executable();
        });
}

fn touch_executable() {
    let Ok(path) = crate::platform::current_exe() else {
        return;
    };
    let Ok(mut file) = std::fs::File::open(path) else {
        return;
    };
    let mut buf = vec![0u8; 1 << 20];
    while matches!(file.read(&mut buf), Ok(n) if n > 0) {}
}
