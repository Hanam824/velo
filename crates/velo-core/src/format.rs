//! Formats sizes, rates, durations and percentages for display, the same way in the app and
//! in `velo-cli`.

/// A byte count with a binary unit, such as `512 B`, `1.50 KB` or `2.25 GB`.
pub fn size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1024.0;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.2} {}", UNITS[unit])
}

/// A transfer rate such as `1.25 MB/sec`; empty for 0.
pub fn rate(bytes_per_second: u64) -> String {
    if bytes_per_second == 0 {
        return String::new();
    }
    format!("{}/sec", size(bytes_per_second))
}

/// A duration such as `45 sec`, `3 min 05 sec` or `2 hr 10 min`.
pub fn duration(seconds: u64) -> String {
    let (hours, minutes, seconds) = (seconds / 3600, seconds / 60 % 60, seconds % 60);
    if hours > 0 {
        format!("{hours} hr {minutes:02} min")
    } else if minutes > 0 {
        format!("{minutes} min {seconds:02} sec")
    } else {
        format!("{seconds} sec")
    }
}

/// `done` as a percentage of `total` with two decimals, such as `45.21%`.
pub fn percent(done: u64, total: u64) -> String {
    if total == 0 {
        return "100.00%".into();
    }
    // Floor rather than round, so 99.999% doesn't show as finished.
    let hundredths = (u128::from(done) * 10_000 / u128::from(total)) as u64;
    format!("{}.{:02}%", hundredths / 100, hundredths % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_use_binary_units() {
        assert_eq!(size(0), "0 B");
        assert_eq!(size(1023), "1023 B");
        assert_eq!(size(1536), "1.50 KB");
        assert_eq!(size(5 * 1024 * 1024 + 256 * 1024), "5.25 MB");
        assert_eq!(size(3 << 30), "3.00 GB");
        assert_eq!(size(u64::MAX), "16777216.00 TB");
    }

    #[test]
    fn rates_and_durations() {
        assert_eq!(rate(0), "");
        assert_eq!(rate(2048), "2.00 KB/sec");
        assert_eq!(duration(45), "45 sec");
        assert_eq!(duration(185), "3 min 05 sec");
        assert_eq!(duration(2 * 3600 + 10 * 60 + 59), "2 hr 10 min");
    }

    #[test]
    fn percentages_never_round_up_to_finished() {
        assert_eq!(percent(4521, 10_000), "45.21%");
        assert_eq!(percent(99_999, 100_000), "99.99%");
        assert_eq!(percent(1, 1), "100.00%");
        assert_eq!(percent(0, 0), "100.00%");
        assert_eq!(percent(u64::MAX / 2, u64::MAX), "49.99%");
    }
}
