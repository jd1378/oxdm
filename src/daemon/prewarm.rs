//! Warm the page cache for the first window.
//!
//! Every window is a fresh process, and the first one after a boot pays
//! for disk the later ones do not: its share of the executable, and the
//! headers of every system font, which the text engine scans once per
//! process before it can shape a character. Measured: the scan is
//! 5 ms warm and over 130 ms cold, with the executable's pages on top.
//!
//! Done here, once, when the daemon starts, on a thread nothing waits
//! for. The reads are the same ones the window would make, so nothing
//! is pulled in that the window would not have read itself. A failure
//! costs nothing but the warm-up.

use std::io::Read;

pub fn spawn() {
    let _ = std::thread::Builder::new()
        .name("oxdm-prewarm".into())
        .spawn(|| {
            // The daemon has its own startup to get through first, and
            // this can wait a moment without missing the first click.
            std::thread::sleep(std::time::Duration::from_secs(2));
            touch_executable();
            touch_system_fonts();
        });
}

/// The daemon runs from this file too, but only its own pages: the
/// windows' code is elsewhere in it and untouched until one starts.
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

/// The same scan the text engine does, through the same library, so
/// the pages it reads are the ones the window will read.
fn touch_system_fonts() {
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    tracing::debug!(faces = db.len(), "system fonts warmed");
}

#[cfg(test)]
mod tests {
    /// The scan must not panic on whatever fonts the machine has, and
    /// must find at least the ones a desktop cannot be without.
    #[test]
    fn the_font_scan_runs() {
        super::touch_system_fonts();
        super::touch_executable();
    }
}
