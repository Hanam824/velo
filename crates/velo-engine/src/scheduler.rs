//! When a queue's schedule starts or stops it. Pure functions of the local time, so the engine
//! asks them on its clock and the tests can ask them for any moment.

use chrono::{Datelike, Duration, NaiveDateTime};
use velo_core::{Schedule, TimeOfDay};

/// How late a start or stop may still happen, such as after the computer woke from sleep.
const GRACE: Duration = Duration::minutes(5);

/// Whether a queue with `schedule` should start at `now`, given when the schedule last started
/// it.
///
/// That is when today's start time passed less than five minutes ago, today is one of the
/// chosen weekdays (any day for a once-only schedule), and this start hasn't happened yet.
pub fn start_due(schedule: &Schedule, now: NaiveDateTime, last: Option<NaiveDateTime>) -> bool {
    let Some(time) = schedule.start_at else {
        return false;
    };
    let day = now.weekday().num_days_from_monday() as usize;
    (schedule.is_once() || schedule.days[day]) && due(time, now, last)
}

/// Whether a queue with `schedule` should stop at `now`, given when the schedule last stopped
/// it. The stop time applies every day.
pub fn stop_due(schedule: &Schedule, now: NaiveDateTime, last: Option<NaiveDateTime>) -> bool {
    schedule.stop_at.is_some_and(|time| due(time, now, last))
}

/// Whether today's `time` passed less than [`GRACE`] before `now`, and `last` is earlier.
fn due(time: TimeOfDay, now: NaiveDateTime, last: Option<NaiveDateTime>) -> bool {
    let Some(at) = now
        .date()
        .and_hms_opt(u32::from(time.hour), u32::from(time.minute), 0)
    else {
        return false;
    };
    at <= now && now < at + GRACE && last.is_none_or(|last| last < at)
}

#[cfg(test)]
mod tests {
    use chrono::NaiveDate;

    use super::*;

    /// Monday 26 October 2026 at `hour:minute:second`.
    fn monday(hour: u32, minute: u32, second: u32) -> NaiveDateTime {
        NaiveDate::from_ymd_opt(2026, 10, 26)
            .unwrap()
            .and_hms_opt(hour, minute, second)
            .unwrap()
    }

    fn at(hour: u8, minute: u8, days: [bool; 7]) -> Schedule {
        Schedule {
            start_at: TimeOfDay::new(hour, minute),
            days,
            stop_at: TimeOfDay::new(6, 0),
            ..Schedule::default()
        }
    }

    #[test]
    fn a_start_is_due_within_the_grace_period_once() {
        let schedule = at(2, 30, [false; 7]);
        assert!(!start_due(&schedule, monday(2, 29, 59), None));
        assert!(start_due(&schedule, monday(2, 30, 0), None));
        assert!(start_due(&schedule, monday(2, 34, 59), None));
        assert!(!start_due(&schedule, monday(2, 35, 0), None));
        // Already started for this occurrence.
        assert!(!start_due(
            &schedule,
            monday(2, 31, 0),
            Some(monday(2, 30, 1))
        ));
        // Started yesterday: today's start is still due.
        let yesterday = monday(2, 30, 0) - Duration::days(1);
        assert!(start_due(&schedule, monday(2, 31, 0), Some(yesterday)));
    }

    #[test]
    fn weekdays_limit_the_start_but_not_the_stop() {
        let weekends = at(2, 30, [false, false, false, false, false, true, true]);
        assert!(!start_due(&weekends, monday(2, 30, 0), None));
        let mondays = at(2, 30, [true, false, false, false, false, false, false]);
        assert!(start_due(&mondays, monday(2, 30, 0), None));
        assert!(stop_due(&weekends, monday(6, 0, 30), None));
        assert!(!stop_due(
            &weekends,
            monday(6, 1, 0),
            Some(monday(6, 0, 30))
        ));
        assert!(!stop_due(&Schedule::default(), monday(6, 0, 0), None));
        assert!(!start_due(&Schedule::default(), monday(2, 30, 0), None));
    }
}
