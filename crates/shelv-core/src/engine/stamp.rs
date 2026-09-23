//! Naming a folder after the moment it was made.
//!
//! Shelv writes two kinds of timestamped folder — a trash folder per run
//! that deletes, and a snapshot folder per run that snapshots — and both
//! want the same name, so it lives here rather than in either.
//!
//! **UTC, with the `Z` to say so.** Local time would be friendlier to read,
//! and getting it right means a timezone database and a rule about what
//! happens to the hour that occurs twice every autumn. Two folders from one
//! run an hour apart that sort into the wrong order, or collide outright, is
//! a worse outcome than a name an hour off what the clock on the wall said.
//! The `Z` is there so nobody has to guess which was chosen.
//!
//! The format sorts lexicographically in chronological order, which is what
//! makes a directory listing of snapshots useful without any tooling.
//!
//! The calendar arithmetic itself lives in [`crate::civil`], which the
//! scheduler also uses — for the opposite purpose, since a schedule is
//! about a day on the wall rather than a day in UTC.

/// Formats a Unix timestamp as `YYYY-MM-DDTHHMMSSZ`.
///
/// Colons are left out because Windows does not allow them in a filename.
///
/// Timestamps before 1970 are not expected — they would mean the system
/// clock is badly wrong — but they format rather than panicking, because a
/// backup refusing to run over a clock reading is worse than a folder with
/// an odd name.
#[must_use]
#[allow(
    clippy::integer_division,
    reason = "every division here is deliberately truncating calendar arithmetic"
)]
pub fn folder_name(unix_seconds: i64) -> String {
    let days = unix_seconds.div_euclid(86_400);
    let seconds = unix_seconds.rem_euclid(86_400);
    let (year, month, day) = crate::civil::civil_from_days(days);
    let hour = seconds / 3_600;
    let minute = (seconds % 3_600) / 60;
    let second = seconds % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}{minute:02}{second:02}Z")
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

    #[test]
    fn the_epoch_formats_as_itself() {
        assert_eq!(folder_name(0), "1970-01-01T000000Z");
    }

    #[test]
    fn a_known_moment_formats_correctly() {
        // 2025-09-22T07:34:12Z, checked against `date -u -d @1758526452`.
        assert_eq!(folder_name(1_758_526_452), "2025-09-22T073412Z");
    }

    #[test]
    fn a_leap_day_is_a_real_day() {
        // 2024-02-29T00:00:00Z. The approximation that makes February 29
        // disappear shows up here and nowhere else.
        assert_eq!(folder_name(1_709_164_800), "2024-02-29T000000Z");
    }

    #[test]
    fn the_end_of_a_century_is_not_a_leap_year_unless_it_divides_by_four_hundred() {
        // 2000 is a leap year; 1900 and 2100 are not. 2100-03-01T00:00:00Z
        // is the day the simple rule gets wrong.
        assert_eq!(folder_name(951_782_400), "2000-02-29T000000Z");
        assert_eq!(folder_name(4_107_542_400), "2100-03-01T000000Z");
    }

    #[test]
    fn names_sort_in_the_order_the_runs_happened() {
        let mut names = vec![
            folder_name(1_758_526_452),
            folder_name(0),
            folder_name(1_709_164_800),
        ];
        names.sort();
        assert_eq!(
            names,
            vec![
                "1970-01-01T000000Z",
                "2024-02-29T000000Z",
                "2025-09-22T073412Z"
            ]
        );
    }

    #[test]
    fn a_name_contains_nothing_windows_refuses_in_a_filename() {
        let name = folder_name(1_758_526_452);
        assert!(
            !name.contains([':', '/', '\\', '*', '?', '"', '<', '>', '|']),
            "{name}"
        );
    }
}
