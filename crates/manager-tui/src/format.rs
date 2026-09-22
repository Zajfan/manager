//! Turning numbers into short text for narrow columns.

use std::time::SystemTime;

use chrono::{DateTime, Local};

/// Compact sizes that fit a 7-character column: `999`, `12.3K`, `456K`, `1.5M`, `12G`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 6] = ["K", "M", "G", "T", "P", "E"];
    if bytes < 1000 {
        return bytes.to_string();
    }
    let mut value = bytes as f64;
    for unit in UNITS {
        value /= 1024.0;
        if value < 9.95 {
            return format!("{value:.1}{unit}");
        }
        if value < 999.5 {
            return format!("{value:.0}{unit}");
        }
    }
    unreachable!("u64 can't exceed 16 EiB")
}

/// Local time as `2026-09-22 14:03`, or blank when unknown.
pub fn date(time: Option<SystemTime>) -> String {
    time.map(|t| {
        DateTime::<Local>::from(t)
            .format("%Y-%m-%d %H:%M")
            .to_string()
    })
    .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let cases = [
            (0, "0"),
            (999, "999"),
            (1000, "1.0K"),
            (1536, "1.5K"),
            (10 * 1024, "10K"),
            (999 * 1024, "999K"),
            (1000 * 1024, "1.0M"),
            (5 * 1024 * 1024 * 1024, "5.0G"),
            (u64::MAX, "16E"),
        ];
        for (bytes, expected) in cases {
            assert_eq!(size(bytes), expected, "{bytes} bytes");
            assert!(size(bytes).len() <= 7);
        }
    }

    #[test]
    fn unknown_date_is_blank() {
        assert_eq!(date(None), "");
        assert_eq!(date(Some(SystemTime::now())).len(), 16);
    }
}
