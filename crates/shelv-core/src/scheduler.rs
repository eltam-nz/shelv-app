//! When a rule is due.
//!
//! This is the whole of the decision that starts a backup nobody asked for,
//! and it is deliberately a pure function: no store, no filesystem, no clock
//! of its own. Everything it needs arrives as an argument, so the question
//! "would this rule run right now?" is answerable in a test as a table of
//! dates rather than by waiting.
//!
//! **A schedule says how often, not when.** `Daily` means the last
//! successful run was on an earlier local calendar day; `Weekly` an earlier
//! week, `Monthly` an earlier month. There is no firing time, and that is
//! not a simplification for its own sake — see `docs/M3.md`. Three things
//! follow from it:
//!
//! * **Catch-up is arithmetic, not a feature.** A machine that was off for a
//!   week crossed six day boundaries; when it comes back the rule is due.
//!   Nothing was missed, only delayed, so there is nothing to make up.
//! * **No timezone database.** The only thing needed from the operating
//!   system is the offset in effect at a given instant, which handles
//!   daylight saving by construction because the OS knows what the offset
//!   *was*.
//! * **Connecting a drive is not a separate trigger.** It is an opportunity
//!   to notice that a rule is already due, which is what `docs/PLAN.md`
//!   §1.1d means by frequency becoming a floor rather than an alarm clock.

use serde::{Deserialize, Serialize};

use crate::civil::{month_of, week_of, Date};
use crate::model::{RuleSpec, Schedule};
use crate::platform::LocalTime;

/// Whether a rule wants to run, and if not, what would change that.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Due {
    /// Its period has passed. Whether it *can* run is a separate question —
    /// the drives may be absent — and deliberately not answered here.
    Now,
    /// Not until this local date, which is the start of its next period.
    On {
        /// The first day the rule will be due again.
        date: Date,
    },
    /// Never on its own.
    Never {
        /// Why, so the table can say which.
        reason: NeverAutomatic,
    },
}

/// Why a rule will never start by itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, ts_rs::TS)]
#[ts(export, export_to = "../../../src/types/")]
#[serde(rename_all = "snake_case")]
pub enum NeverAutomatic {
    /// Switched off. Nothing about it is evaluated.
    Disabled,
    /// Set to run only when asked.
    Manual,
    /// Set to a custom schedule, which Shelv cannot evaluate yet. The rule
    /// can still be run by hand; it simply never starts on its own, and the
    /// UI says so rather than showing a date it would miss.
    UnsupportedSchedule,
}

/// Whether `spec` is due at `now`.
///
/// `last_success` is when the rule last finished a run that copied
/// everything it planned to — not the last run. A rule whose recent runs all
/// failed stays due, which is what keeps a broken rule from going quiet.
pub fn due(spec: &RuleSpec, last_success: Option<i64>, now: i64, zone: &dyn LocalTime) -> Due {
    if !spec.enabled {
        return Due::Never {
            reason: NeverAutomatic::Disabled,
        };
    }

    let period = match spec.schedule {
        Schedule::Manual => {
            return Due::Never {
                reason: NeverAutomatic::Manual,
            }
        }
        Schedule::Cron(_) => {
            return Due::Never {
                reason: NeverAutomatic::UnsupportedSchedule,
            }
        }
        Schedule::Daily => Period::Day,
        Schedule::Weekly => Period::Week,
        Schedule::Monthly => Period::Month,
    };

    // A rule that has never succeeded is due, so one created this morning
    // backs up this morning rather than tomorrow.
    let Some(last) = last_success else {
        return Due::Now;
    };

    let today = Date::local(now, zone.utc_offset_seconds(now));
    let then = Date::local(last, zone.utc_offset_seconds(last));

    if period.index(today) > period.index(then) {
        Due::Now
    } else {
        Due::On {
            date: period.next_start(today),
        }
    }
}

/// The unit a schedule counts in.
#[derive(Debug, Clone, Copy)]
enum Period {
    Day,
    Week,
    Month,
}

impl Period {
    /// Which period a date falls in. Only ever compared, never shown.
    fn index(self, date: Date) -> i64 {
        match self {
            Self::Day => date.to_days(),
            Self::Week => week_of(date.to_days()),
            Self::Month => month_of(date),
        }
    }

    /// The first day of the period after the one `date` is in.
    fn next_start(self, date: Date) -> Date {
        match self {
            Self::Day => Date::from_days(date.to_days() + 1),
            // Weeks start on Monday, so this lands on the next one however
            // far through the week the date is.
            Self::Week => Date::from_days(week_start(week_of(date.to_days()) + 1)),
            Self::Month if date.month == 12 => Date {
                year: date.year + 1,
                month: 1,
                day: 1,
            },
            Self::Month => Date {
                year: date.year,
                month: date.month + 1,
                day: 1,
            },
        }
    }
}

/// The Monday that opens a week, as a day count.
const fn week_start(week: i64) -> i64 {
    week * 7 - 3
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
    use crate::civil::days_from_civil;
    use crate::model::{Layout, Packaging, PlaceholderPolicy, Retention, VolumePath};
    use crate::platform::FixedOffset;
    use std::path::PathBuf;

    const UTC: FixedOffset = FixedOffset(0);

    /// An instant at a given UTC hour on a given date.
    fn at(year: i32, month: u32, day: u32, hour: i64) -> i64 {
        days_from_civil(i64::from(year), i64::from(month), i64::from(day)) * 86_400 + hour * 3600
    }

    /// Midday UTC, which keeps a test from accidentally landing on the
    /// midnight boundary when it means to test the day.
    fn midday(year: i32, month: u32, day: u32) -> i64 {
        at(year, month, day, 12)
    }

    fn spec(schedule: Schedule) -> RuleSpec {
        RuleSpec {
            name: "Photos".to_owned(),
            enabled: true,
            source: VolumePath {
                volume: crate::model::VolumeId(1),
                relative: PathBuf::from("Pictures"),
            },
            layout: Layout::Mirror,
            packaging: Packaging::Files,
            retention: Retention::Unlimited,
            schedule,
            run_on_connect: true,
            placeholders: PlaceholderPolicy::Hydrate,
            hydrate_budget_bytes: None,
            follow_symlinks: false,
            excludes: Vec::new(),
        }
    }

    fn date(year: i32, month: u32, day: u32) -> Date {
        Date { year, month, day }
    }

    #[test]
    fn a_rule_that_has_never_succeeded_is_due() {
        // Otherwise a rule created this morning waits until tomorrow, and
        // the person who just made it sees nothing happen.
        assert_eq!(
            due(&spec(Schedule::Daily), None, midday(2025, 9, 22), &UTC),
            Due::Now
        );
    }

    #[test]
    fn a_daily_rule_is_due_once_the_date_changes() {
        let rule = spec(Schedule::Daily);
        let yesterday = midday(2025, 9, 21);
        let today = midday(2025, 9, 22);

        // Ten hours later on the same day is not a new day.
        assert_eq!(
            due(&rule, Some(today), today + 10 * 3600, &UTC),
            Due::On {
                date: date(2025, 9, 23)
            }
        );
        assert_eq!(due(&rule, Some(yesterday), today, &UTC), Due::Now);
    }

    #[test]
    fn two_hours_apart_across_midnight_is_a_new_day() {
        // The period is a calendar day, not twenty-four hours. A run at
        // 23:30 does not hold off the next one until 23:30 tomorrow.
        let rule = spec(Schedule::Daily);
        let late = midday(2025, 9, 21) + 11 * 3600 + 1800;
        let early = late + 2 * 3600;
        assert_eq!(due(&rule, Some(late), early, &UTC), Due::Now);
    }

    #[test]
    fn a_weekly_rule_waits_for_monday() {
        let rule = spec(Schedule::Weekly);
        // 2025-09-22 was a Monday; the 28th is the Sunday that ends its week.
        let monday = midday(2025, 9, 22);
        let sunday = midday(2025, 9, 28);
        let next_monday = midday(2025, 9, 29);

        assert_eq!(
            due(&rule, Some(monday), sunday, &UTC),
            Due::On {
                date: date(2025, 9, 29)
            },
            "still the same week"
        );
        assert_eq!(due(&rule, Some(monday), next_monday, &UTC), Due::Now);
    }

    #[test]
    fn a_monthly_rule_is_due_on_the_first_not_thirty_days_later() {
        let rule = spec(Schedule::Monthly);
        let late_january = midday(2025, 1, 31);
        let early_february = midday(2025, 2, 1);

        assert_eq!(
            due(&rule, Some(late_january), early_february, &UTC),
            Due::Now
        );
        assert_eq!(
            due(&rule, Some(early_february), midday(2025, 2, 28), &UTC),
            Due::On {
                date: date(2025, 3, 1)
            },
            "February is short, and the next period still starts in March"
        );
    }

    #[test]
    fn a_month_that_ends_the_year_rolls_over() {
        let rule = spec(Schedule::Monthly);
        assert_eq!(
            due(&rule, Some(midday(2025, 12, 3)), midday(2025, 12, 31), &UTC),
            Due::On {
                date: date(2026, 1, 1)
            }
        );
    }

    #[test]
    fn a_week_that_ends_the_year_rolls_over() {
        let rule = spec(Schedule::Weekly);
        // 2025-12-29 is a Monday, so its week runs into January.
        assert_eq!(
            due(
                &rule,
                Some(midday(2025, 12, 29)),
                midday(2025, 12, 31),
                &UTC
            ),
            Due::On {
                date: date(2026, 1, 5)
            }
        );
    }

    #[test]
    fn the_day_is_the_one_where_the_machine_is() {
        // New Zealand is twelve hours ahead, so its midnight is midday UTC.
        // Two instants an hour either side of it are the same day in UTC and
        // different days in Auckland — and the person looking at the screen
        // is in Auckland.
        let rule = spec(Schedule::Daily);
        let nz = FixedOffset(12 * 3600);
        let ran = at(2025, 9, 22, 11);
        let now = at(2025, 9, 22, 13);

        assert_eq!(
            due(&rule, Some(ran), now, &UTC),
            Due::On {
                date: date(2025, 9, 23)
            },
            "still the 22nd in UTC"
        );
        assert_eq!(
            due(&rule, Some(ran), now, &nz),
            Due::Now,
            "the 22nd and then the 23rd in Auckland"
        );
    }

    /// A zone that changes offset at a fixed instant, which is what a
    /// daylight-saving transition is.
    struct Shifting {
        before: i32,
        after: i32,
        at: i64,
    }

    impl LocalTime for Shifting {
        fn utc_offset_seconds(&self, unix_seconds: i64) -> i32 {
            if unix_seconds < self.at {
                self.before
            } else {
                self.after
            }
        }
    }

    #[test]
    fn a_clock_going_forward_does_not_skip_a_day() {
        // US Eastern's spring transition: 2026-03-08, 02:00 local becomes
        // 03:00, so the offset goes from -5 to -4 at 07:00Z. A rule that
        // succeeded on the 7th must be due on the 8th — the hour that never
        // happened must not take the day with it.
        let rule = spec(Schedule::Daily);
        let zone = Shifting {
            before: -5 * 3600,
            after: -4 * 3600,
            at: at(2026, 3, 8, 7),
        };

        // 20:00 local on the 7th, then 09:00 local on the 8th.
        let saturday = at(2026, 3, 8, 1);
        let sunday = at(2026, 3, 8, 13);
        assert_eq!(due(&rule, Some(saturday), sunday, &zone), Due::Now);
    }

    #[test]
    fn a_clock_going_back_does_not_run_a_day_twice() {
        // The autumn transition, where 01:30 local happens twice: -4 back to
        // -5 at 06:00Z on 2026-11-01. Both instants are 01:30 on the same
        // local day, so a daily rule that ran in the first pass must not run
        // again in the second.
        let rule = spec(Schedule::Daily);
        let zone = Shifting {
            before: -4 * 3600,
            after: -5 * 3600,
            at: at(2026, 11, 1, 6),
        };

        let first_pass = at(2026, 11, 1, 5) + 1800;
        let second_pass = at(2026, 11, 1, 6) + 1800;
        assert_eq!(
            due(&rule, Some(first_pass), second_pass, &zone),
            Due::On {
                date: date(2026, 11, 2)
            },
            "the same local day, twice over"
        );
    }

    #[test]
    fn a_disabled_rule_is_never_due_whatever_its_schedule() {
        let mut rule = spec(Schedule::Daily);
        rule.enabled = false;
        assert_eq!(
            due(&rule, None, midday(2025, 9, 22), &UTC),
            Due::Never {
                reason: NeverAutomatic::Disabled
            }
        );
    }

    #[test]
    fn a_manual_rule_is_never_due() {
        assert_eq!(
            due(&spec(Schedule::Manual), None, midday(2025, 9, 22), &UTC),
            Due::Never {
                reason: NeverAutomatic::Manual
            }
        );
    }

    #[test]
    fn a_custom_schedule_is_refused_rather_than_guessed_at() {
        // Cron is in the data model and nothing evaluates it. Treating it
        // as daily would run a backup on a schedule nobody chose; treating
        // it as manual would hide that the setting does nothing.
        assert_eq!(
            due(
                &spec(Schedule::Cron("0 3 * * 1".to_owned())),
                None,
                midday(2025, 9, 22),
                &UTC
            ),
            Due::Never {
                reason: NeverAutomatic::UnsupportedSchedule
            }
        );
    }

    #[test]
    fn a_rule_whose_runs_all_failed_stays_due() {
        // `last_success` is the argument, not `last_run`, and this is why:
        // a rule failing every night must keep asking rather than going
        // quiet after the first attempt.
        let rule = spec(Schedule::Daily);
        assert_eq!(due(&rule, None, midday(2025, 9, 22), &UTC), Due::Now);
    }
}
