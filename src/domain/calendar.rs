// A day of the civil calendar, with nothing about clocks or zones in it.
//
// Written out rather than taken from a crate, for the reason `utc_stamp` gave first: the
// arithmetic is Howard Hinnant's days-from-civil pair, it needs no table and no locale, and a
// date library would be a dependency this program has no other use for. A period of a
// question is counted in these days, and the moment a day starts is the period's business,
// because only it knows the offset.
use std::fmt;

/// One day: a year, a month from one to twelve and a day of that month.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CivilDate {
    year: i32,
    month: u8,
    day: u8,
}

/// English month abbreviations, which is how a terminal line of this program writes a date.
const MONTH_ABBREVIATIONS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

impl CivilDate {
    /// A day, or nothing for a day the calendar does not have — the thirty-first of
    /// September, the twenty-ninth of February of a common year.
    pub fn new(year: i32, month: u8, day: u8) -> Option<Self> {
        if !(1..=12).contains(&month) || day == 0 || day > days_in_month(year, month) {
            return None;
        }
        Some(Self { year, month, day })
    }

    /// `YYYY-MM-DD`, exactly: a date typed on a command line is read in one form, so two
    /// people reading the same flag never read two different days.
    pub fn parse_iso(text: &str) -> Option<Self> {
        let bytes = text.as_bytes();
        if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
            return None;
        }
        let digits = |range: std::ops::Range<usize>| -> Option<u32> {
            let part = text.get(range)?;
            if !part.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let year = i32::try_from(digits(0..4)?).ok()?;
        let month = u8::try_from(digits(5..7)?).ok()?;
        let day = u8::try_from(digits(8..10)?).ok()?;
        Self::new(year, month, day)
    }

    /// The day that is `days` days after 1970-01-01 (`civil_from_days`).
    pub fn from_days(days: i64) -> Self {
        let shifted = days + 719_468;
        let era = shifted.div_euclid(146_097);
        let day_of_era = shifted.rem_euclid(146_097);
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
        Self {
            year: year as i32,
            month: month as u8,
            day: day as u8,
        }
    }

    /// How many days this day is after 1970-01-01 (`days_from_civil`).
    pub fn days(self) -> i64 {
        let year = i64::from(self.year) - i64::from(self.month <= 2);
        let era = year.div_euclid(400);
        let year_of_era = year.rem_euclid(400);
        let month = i64::from(self.month);
        let shifted_month = if month > 2 { month - 3 } else { month + 9 };
        let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(self.day) - 1;
        let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
        era * 146_097 + day_of_era - 719_468
    }

    pub fn year(self) -> i32 {
        self.year
    }

    pub fn month(self) -> u8 {
        self.month
    }

    pub fn day(self) -> u8 {
        self.day
    }

    pub fn plus_days(self, days: i64) -> Self {
        Self::from_days(self.days() + days)
    }

    /// Days since the Monday of this day's week: zero on a Monday, six on a Sunday. A week
    /// here runs Monday to Sunday, which is how the owner's calendar counts it.
    pub fn days_since_monday(self) -> i64 {
        // 1970-01-01 was a Thursday, three days after a Monday.
        (self.days() + 3).rem_euclid(7)
    }

    pub fn monday_of_its_week(self) -> Self {
        self.plus_days(-self.days_since_monday())
    }

    pub fn first_of_its_month(self) -> Self {
        Self { day: 1, ..self }
    }

    pub fn last_of_its_month(self) -> Self {
        Self {
            day: days_in_month(self.year, self.month),
            ..self
        }
    }

    /// The first day of the month before this day's.
    pub fn first_of_previous_month(self) -> Self {
        self.first_of_its_month().plus_days(-1).first_of_its_month()
    }

    /// The month's English abbreviation, as a terminal line writes it.
    pub fn month_abbreviation(self) -> &'static str {
        MONTH_ABBREVIATIONS[usize::from(self.month - 1)]
    }
}

/// `2026-09-21`.
impl fmt::Display for CivilDate {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "{:04}-{:02}-{:02}",
            self.year, self.month, self.day
        )
    }
}

fn is_leap(year: i32) -> bool {
    (year % 4 == 0 && year % 100 != 0) || year % 400 == 0
}

fn days_in_month(year: i32, month: u8) -> u8 {
    match month {
        2 if is_leap(year) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

#[cfg(test)]
mod tests {
    use super::CivilDate;

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::new(year, month, day).unwrap()
    }

    #[test]
    fn days_and_dates_are_one_mapping_in_both_directions() {
        assert_eq!(CivilDate::from_days(0), date(1970, 1, 1));
        assert_eq!(date(1970, 1, 1).days(), 0);
        for days in [-1, 59, 60, 365, 10_957, 20_727, 20_728, 20_819, 20_820] {
            assert_eq!(CivilDate::from_days(days).days(), days, "{days}");
        }
        assert_eq!(date(2026, 10, 1).days(), 20_727);
    }

    #[test]
    fn a_week_starts_on_monday() {
        // 2026-10-01 is a Thursday.
        assert_eq!(date(2026, 10, 1).days_since_monday(), 3);
        assert_eq!(date(2026, 10, 1).monday_of_its_week(), date(2026, 9, 28));
        assert_eq!(date(2026, 9, 28).monday_of_its_week(), date(2026, 9, 28));
        assert_eq!(date(2026, 10, 4).monday_of_its_week(), date(2026, 9, 28));
        // A week that starts in one year and ends in the next.
        assert_eq!(date(2026, 1, 1).monday_of_its_week(), date(2025, 12, 29));
    }

    #[test]
    fn a_month_knows_its_last_day_and_the_month_before_it() {
        assert_eq!(date(2026, 9, 17).last_of_its_month(), date(2026, 9, 30));
        assert_eq!(date(2028, 2, 3).last_of_its_month(), date(2028, 2, 29));
        assert_eq!(date(2026, 2, 3).last_of_its_month(), date(2026, 2, 28));
        assert_eq!(
            date(2026, 1, 15).first_of_previous_month(),
            date(2025, 12, 1)
        );
        assert_eq!(
            date(2026, 3, 31).first_of_previous_month(),
            date(2026, 2, 1)
        );
    }

    #[test]
    fn a_date_is_read_in_one_form_and_must_exist() {
        assert_eq!(CivilDate::parse_iso("2026-09-21"), Some(date(2026, 9, 21)));
        for refused in [
            "2026-9-21",
            "21.09.2026",
            "2026-09-31",
            "2026-02-29",
            "2026-13-01",
            "2026-00-10",
            "+026-09-21",
            "2026-09-2١",
            "",
        ] {
            assert_eq!(CivilDate::parse_iso(refused), None, "{refused:?}");
        }
        assert_eq!(CivilDate::parse_iso("2028-02-29"), Some(date(2028, 2, 29)));
        assert_eq!(date(2026, 9, 1).to_string(), "2026-09-01");
    }
}
