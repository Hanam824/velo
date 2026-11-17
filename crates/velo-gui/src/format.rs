//! Formats sizes, rates, durations and dates for display.

use jiff::Timestamp;
use jiff::tz::TimeZone;
pub use velo_core::format::{duration, percent, rate, size};

/// A Unix time in seconds as a local date and time, such as `Oct 26 2026 14:03`.
pub fn date(unix_seconds: i64) -> String {
    date_in(unix_seconds, TimeZone::system())
}

/// [`date`] in the time zone `zone`.
fn date_in(unix_seconds: i64, zone: TimeZone) -> String {
    Timestamp::from_second(unix_seconds)
        .map(|time| {
            time.to_zoned_datetime(zone)
                .strftime("%b %d %Y %H:%M")
                .to_string()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use jiff::tz;

    use super::*;

    #[test]
    fn dates_are_shown_in_the_given_zone() {
        // 2026-10-26 07:03:00 UTC.
        let time = 1_792_998_180;
        assert_eq!(date_in(time, TimeZone::UTC), "Oct 26 2026 07:03");
        let vietnam = TimeZone::fixed(tz::offset(7));
        assert_eq!(date_in(time, vietnam), "Oct 26 2026 14:03");
    }
}
