//! Formats sizes, rates, durations and dates for display.

use chrono::{DateTime, Local, TimeZone};
pub use velo_core::format::{duration, percent, rate, size};

/// A Unix time in seconds as a local date and time, such as `Oct 26 2026 14:03`.
pub fn date(unix_seconds: i64) -> String {
    date_in(unix_seconds, &Local)
}

/// [`date`] in the time zone `zone`.
fn date_in<Z: TimeZone>(unix_seconds: i64, zone: &Z) -> String
where
    Z::Offset: std::fmt::Display,
{
    DateTime::from_timestamp(unix_seconds, 0)
        .map(|utc| utc.with_timezone(zone).format("%b %d %Y %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use chrono::{FixedOffset, Utc};

    use super::*;

    #[test]
    fn dates_are_shown_in_the_given_zone() {
        // 2026-10-26 07:03:00 UTC.
        let time = 1_792_998_180;
        assert_eq!(date_in(time, &Utc), "Oct 26 2026 07:03");
        let vietnam = FixedOffset::east_opt(7 * 3600).unwrap();
        assert_eq!(date_in(time, &vietnam), "Oct 26 2026 14:03");
    }
}
