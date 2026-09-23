//! Calendar arithmetic.
//!
//! Two parts of Shelv need to turn an instant into a date: the folder names
//! a run writes, which are UTC, and the scheduler, which is emphatically
//! not — "daily" means a day on the wall, in the place the machine is.
//!
//! Neither needs a timezone database. A folder name needs no zone at all,
//! and the scheduler needs only the offset the operating system says was in
//! effect at a given instant, which it will answer for any instant,
//! including one on the far side of a daylight-saving change.

use serde::{Deserialize, Serialize};

/// Seconds in a day, as the calendar counts them.
///
/// Leap seconds are not represented in Unix time, so a day is always
/// exactly this, and the arithmetic below is exact rather than approximate.
pub const SECONDS_PER_DAY: i64 = 86_400;

/// A date on somebody's calendar, with no time and no zone.
///
/// Two of these can be compared, and that comparison is the whole of what
/// the scheduler needs: not how far apart two instants are, but whether
/// they fall on different days.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
pub struct Date {
    /// Proleptic Gregorian year.
    #[ts(type = "number")]
    pub year: i32,
    /// 1–12.
    #[ts(type = "number")]
    pub month: u32,
    /// 1–31.
    #[ts(type = "number")]
    pub day: u32,
}

impl Date {
    /// The date at `unix_seconds`, in a zone `offset_seconds` from UTC.
    ///
    /// The offset is the one in effect *at that instant*, not a fixed
    /// property of the zone, which is what makes this correct either side of
    /// a daylight-saving change without knowing the rule that caused it.
    #[must_use]
    pub fn local(unix_seconds: i64, offset_seconds: i32) -> Self {
        Self::from_days(days_from_unix(unix_seconds, offset_seconds))
    }

    /// The date `days` after 1970-01-01.
    /// Out-of-range values saturate rather than wrapping. They cannot arise
    /// from a system clock — the year would have to be beyond two billion —
    /// but a date that clamps is easier to recognise as wrong than one that
    /// silently becomes plausible.
    #[must_use]
    pub fn from_days(days: i64) -> Self {
        let (year, month, day) = civil_from_days(days);
        Self {
            year: i32::try_from(year).unwrap_or(i32::MAX),
            month: u32::try_from(month).unwrap_or(1),
            day: u32::try_from(day).unwrap_or(1),
        }
    }

    /// Days since 1970-01-01.
    #[must_use]
    pub fn to_days(self) -> i64 {
        days_from_civil(
            i64::from(self.year),
            i64::from(self.month),
            i64::from(self.day),
        )
    }
}

/// Days since 1970-01-01 for an instant in a zone `offset_seconds` from UTC.
#[must_use]
pub const fn days_from_unix(unix_seconds: i64, offset_seconds: i32) -> i64 {
    (unix_seconds + offset_seconds as i64).div_euclid(SECONDS_PER_DAY)
}

/// Converts a count of days since 1970-01-01 into a civil date.
///
/// Howard Hinnant's algorithm, which is the standard one: it shifts the year
/// to start in March so that the leap day falls at the end and the month
/// lengths become a simple arithmetic series, then unwinds the shift. It is
/// exact for every date the calendar defines rather than approximating with
/// 365.25, which drifts.
#[must_use]
#[allow(
    clippy::integer_division,
    reason = "every division here is deliberately truncating calendar arithmetic"
)]
pub const fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 {
        shifted
    } else {
        shifted - 146_096
    } / 146_097;
    let day_of_era = shifted - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = if month <= 2 { year + 1 } else { year };
    (year, month, day)
}

/// The inverse: days since 1970-01-01 for a civil date.
#[must_use]
#[allow(
    clippy::integer_division,
    reason = "the same truncating calendar arithmetic, run backwards"
)]
pub const fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Which week a day falls in, counted from an arbitrary but fixed origin.
///
/// Only ever compared with another week number, never displayed, so the
/// origin does not matter — only that weeks start on Monday and that
/// consecutive weeks differ by one. 1970-01-01 was a Thursday, hence the
/// three.
#[must_use]
pub const fn week_of(days: i64) -> i64 {
    (days + 3).div_euclid(7)
}

/// Which month a date falls in, counted from year zero.
///
/// As with [`week_of`], this exists to be compared rather than read.
#[must_use]
pub fn month_of(date: Date) -> i64 {
    i64::from(date.year) * 12 + i64::from(date.month) - 1
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    reason = "a panic in a test is the failure report"
)]
mod tests {
    use super::*;

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn the_epoch_is_the_first_of_january_1970() {
        assert_eq!(Date::from_days(0), date(1970, 1, 1));
        assert_eq!(date(1970, 1, 1).to_days(), 0);
    }

    #[test]
    fn a_date_round_trips_through_days() {
        // Including the two the shifted-year trick is most likely to get
        // wrong: the leap day it moves to the end, and the January that
        // belongs to the previous shifted year.
        for d in [
            date(1970, 1, 1),
            date(2000, 2, 29),
            date(2024, 12, 31),
            date(2025, 1, 1),
            date(2100, 3, 1),
            date(1969, 12, 31),
        ] {
            assert_eq!(Date::from_days(d.to_days()), d, "{d:?}");
        }
    }

    #[test]
    fn a_local_date_is_the_date_where_the_machine_is() {
        // 2025-09-22T07:34:12Z. In UTC+13 that is already the 22nd, late in
        // the evening; in UTC-07:00 it is still the 22nd, early. An hour
        // later in UTC+13 tips it over to the 23rd while UTC stays on the
        // 22nd, which is the whole reason the scheduler cannot use UTC.
        let instant = 1_758_526_452;
        assert_eq!(Date::local(instant, 0), date(2025, 9, 22));
        assert_eq!(Date::local(instant, 13 * 3600), date(2025, 9, 22));
        assert_eq!(Date::local(instant, -7 * 3600), date(2025, 9, 22));

        let later = instant + 17 * 3600;
        assert_eq!(Date::local(later, 0), date(2025, 9, 23));
        assert_eq!(Date::local(later, 13 * 3600), date(2025, 9, 23));
        assert_eq!(Date::local(later, -7 * 3600), date(2025, 9, 22));
    }

    #[test]
    fn weeks_start_on_monday_and_increase_by_one() {
        // 2025-09-22 was a Monday.
        let monday = date(2025, 9, 22).to_days();
        assert_eq!(week_of(monday), week_of(monday + 6), "the week holds");
        assert_eq!(
            week_of(monday + 7),
            week_of(monday) + 1,
            "the next Monday is the next week"
        );
        assert_eq!(
            week_of(monday - 1),
            week_of(monday) - 1,
            "Sunday belongs to the week before"
        );
    }

    #[test]
    fn months_increase_by_one_across_a_year_boundary() {
        assert_eq!(month_of(date(2026, 1, 1)), month_of(date(2025, 12, 31)) + 1);
        assert_eq!(month_of(date(2025, 3, 1)), month_of(date(2025, 2, 28)) + 1);
    }
}
