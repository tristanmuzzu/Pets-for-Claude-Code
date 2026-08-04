//! A short record of what the overlay did, so "it just stopped" has an answer.
//!
//! This app has no console: it is a GUI-subsystem binary whose whole job is to
//! sit quietly in a corner. When it misbehaves there is nothing to look at, and
//! the interesting failures are the ones where it keeps running while doing
//! nothing useful. A window can be present, visible, correctly positioned, and
//! painting an empty page, and from outside that is indistinguishable from a
//! bug in the hooks.
//!
//! Deliberately small: one line per event, bounded size, no levels, no
//! configuration. It exists to be read once, after something went wrong.

use crate::state::{now_ms, root};
use std::fs::{self, OpenOptions};
use std::io::Write;

/// Kept small enough to read in one sitting and to never matter on disk.
const MAX_BYTES: u64 = 256 * 1024;

pub fn path() -> std::path::PathBuf {
    root().join("log.txt")
}

/// Appends one line, trimming the file when it gets long.
///
/// Every failure here is swallowed. Logging that can take the app down with it
/// is worse than no logging.
pub fn write(line: &str) {
    let path = path();
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    rotate(&path);
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(file, "{} {line}", stamp(now_ms()));
}

/// Keeps the newest half, cut at a line boundary.
///
/// Halving rather than truncating means the trim happens rarely, and cutting
/// at a newline means the oldest surviving line is a whole one.
fn rotate(path: &std::path::Path) {
    let Ok(meta) = fs::metadata(path) else {
        return;
    };
    if meta.len() <= MAX_BYTES {
        return;
    }
    let Ok(text) = fs::read_to_string(path) else {
        return;
    };
    let midpoint = text.len() / 2;
    let cut = match text[midpoint..].find('\n') {
        Some(offset) => midpoint + offset + 1,
        None => return,
    };
    let _ = fs::write(path, &text[cut..]);
}

/// `2026-08-04 18:52:56` in UTC, from milliseconds since the epoch.
///
/// Hand-rolled because the alternative is a date crate for one format string,
/// and this file is meant to be readable rather than exact.
fn stamp(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = ((secs / 3600) % 24, (secs / 60) % 60, secs % 60);
    let mut days = secs / 86_400;
    let mut year = 1970;
    loop {
        let len = if leap(year) { 366 } else { 365 };
        if days < len {
            break;
        }
        days -= len;
        year += 1;
    }
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 0;
    while month < 12 && days >= months[month] {
        days -= months[month];
        month += 1;
    }
    format!(
        "{year}-{:02}-{:02} {h:02}:{m:02}:{s:02}Z",
        month + 1,
        days + 1
    )
}

fn leap(year: u64) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stamp_is_a_real_date() {
        assert_eq!(stamp(0), "1970-01-01 00:00:00Z");
        // 2026-08-04 18:52:56 UTC
        assert_eq!(stamp(1_785_869_576_000), "2026-08-04 18:52:56Z");
    }

    #[test]
    fn leap_years_are_counted() {
        assert!(leap(2024));
        assert!(!leap(2100));
        assert!(leap(2000));
        // 2024-02-29, a date that only exists if leap years work.
        assert_eq!(stamp(1_709_164_800_000), "2024-02-29 00:00:00Z");
    }
}
