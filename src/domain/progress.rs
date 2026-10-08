//! How far along a transfer is, as a number people read.
//!
//! Rounded down, never to nearest: "100%" is a promise that the file is
//! all there, and a download with its last few kilobytes still in
//! flight rounds up to it. Integer arithmetic, not floats: an `f32`
//! share of a multi-gigabyte file is already `1.0` with hundreds of
//! bytes to go.

/// Tenths of a percent of `done` out of `total`, rounded down; `0`
/// when `total` is. A `done` past `total` reads as complete.
pub fn permille(done: u64, total: u64) -> u16 {
    if total == 0 {
        return 0;
    }
    (u128::from(done.min(total)) * 1000 / u128::from(total)) as u16
}

/// Whole percent of `done` out of `total`, rounded down; `0` when
/// `total` is.
pub fn percent(done: u64, total: u64) -> u8 {
    (permille(done, total) / 10) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_every_byte_reads_as_complete() {
        assert_eq!(percent(999, 1000), 99);
        assert_eq!(permille(9999, 10000), 999);
        assert_eq!(percent(1000, 1000), 100);
        assert_eq!(permille(1000, 1000), 1000);
        // Where an `f32` share already says 1.0.
        let size = 10 * 1024 * 1024 * 1024;
        assert_eq!((((size - 100) as f64 / size as f64) as f32), 1.0);
        assert_eq!(percent(size - 100, size), 99);
        assert_eq!(permille(size - 1, size), 999);
    }

    #[test]
    fn rounds_down() {
        assert_eq!(percent(4, 1000), 0);
        assert_eq!(percent(19, 1000), 1);
        assert_eq!(permille(19, 10000), 1);
        assert_eq!(percent(250, 1000), 25);
        assert_eq!(permille(250, 1000), 250);
    }

    #[test]
    fn edges_do_not_panic_or_overflow() {
        assert_eq!(percent(0, 0), 0);
        assert_eq!(percent(5, 0), 0);
        assert_eq!(percent(1500, 1000), 100);
        assert_eq!(percent(u64::MAX, u64::MAX), 100);
        assert_eq!(percent(u64::MAX - 1, u64::MAX), 99);
    }
}
