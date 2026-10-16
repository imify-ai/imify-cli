//! Timestamps in the output format: RFC 3339 in UTC with second precision.

use std::time::{SystemTime, UNIX_EPOCH};

const SECONDS_PER_DAY: i64 = 86_400;

/// Formats `time` as RFC 3339 in UTC, such as `2026-10-16T12:00:00Z`. Fractions of a
/// second are dropped.
pub fn rfc3339(time: SystemTime) -> String {
    let seconds = unix_seconds(time);
    let (year, month, day) = civil_from_days(seconds.div_euclid(SECONDS_PER_DAY));
    let second_of_day = seconds.rem_euclid(SECONDS_PER_DAY);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        second_of_day % 3600 / 60,
        second_of_day % 60
    )
}

/// Whole seconds since the Unix epoch, rounded towards the past.
fn unix_seconds(time: SystemTime) -> i64 {
    match time.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_secs()).unwrap_or(i64::MAX),
        Err(before) => {
            let before = before.duration();
            let whole = i64::try_from(before.as_secs()).unwrap_or(i64::MAX);
            if before.subsec_nanos() == 0 {
                -whole
            } else {
                -whole - 1
            }
        }
    }
}

/// The proleptic Gregorian date of `days` since 1970-01-01 (H. Hinnant's `civil_from_days`).
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn at(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn the_epoch_is_1970_01_01_midnight() {
        assert_eq!(rfc3339(UNIX_EPOCH), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_known_instant_formats_with_date_and_time() {
        assert_eq!(rfc3339(at(1_792_152_000)), "2026-10-16T12:00:00Z");
    }

    #[test]
    fn a_leap_day_formats_as_february_29() {
        assert_eq!(rfc3339(at(1_709_208_000)), "2024-02-29T12:00:00Z");
    }

    #[test]
    fn the_last_second_of_a_year_formats_as_december_31() {
        assert_eq!(rfc3339(at(4_102_444_799)), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn fractions_of_a_second_are_dropped() {
        let time = at(1_792_152_000) + Duration::from_millis(999);

        assert_eq!(rfc3339(time), "2026-10-16T12:00:00Z");
    }

    #[test]
    fn an_instant_before_the_epoch_rounds_towards_the_past() {
        let time = UNIX_EPOCH - Duration::from_millis(500);

        assert_eq!(rfc3339(time), "1969-12-31T23:59:59Z");
    }
}
