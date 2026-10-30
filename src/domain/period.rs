// The stretch of time a question is about, worked out by this program and never by a model.
//
// A person asking "what did I work on last week?" means seven particular days, and which
// seven depends on what today is and where the machine is. Both are facts of this machine, so
// the period is computed here from them and printed, and the person sees how they were
// understood before they read the answer. A model handed the words "last week" would guess a
// date from the text it was given — which is how the owner got a file of the current week as
// the first point of an answer about the previous one.
//
// A period is a half-open interval of moments, `[start, end)`, between two local midnights.
// The offset is the one the machine had when the question was asked and it is used for both
// ends: a week that crosses a daylight-saving change is off by that hour at one end. The
// owner's zone has no such change, and a second offset would be a second answer to "which
// zone is this" that nothing here could show the person.
use std::fmt;

use super::calendar::CivilDate;

/// What a phrase of a question names. A closed set: the words that name them are data, in
/// the language packs, and a phrase this set has no member for is not a period.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PeriodPhrase {
    Today,
    Yesterday,
    ThisWeek,
    LastWeek,
    ThisMonth,
    LastMonth,
    /// A month by its name, the nearest one that has begun. One to twelve.
    Month(u8),
}

impl PeriodPhrase {
    /// The phrase a key of a phrase file names, and nothing for a key no build of this
    /// program knows. `month-09` and not `september`: the file is per language, and the key is
    /// the one thing every language's file must spell the same way.
    pub fn from_key(key: &str) -> Option<Self> {
        match key {
            "today" => Some(Self::Today),
            "yesterday" => Some(Self::Yesterday),
            "this-week" => Some(Self::ThisWeek),
            "last-week" => Some(Self::LastWeek),
            "this-month" => Some(Self::ThisMonth),
            "last-month" => Some(Self::LastMonth),
            _ => {
                let digits = key.strip_prefix("month-")?;
                if digits.len() != 2 || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
                    return None;
                }
                let month: u8 = digits.parse().ok()?;
                (1..=12).contains(&month).then_some(Self::Month(month))
            }
        }
    }

    /// How the period line says what the phrase was read as.
    pub fn reading(self) -> String {
        match self {
            Self::Today => "today".to_string(),
            Self::Yesterday => "yesterday".to_string(),
            Self::ThisWeek => "this week, Mon–Sun".to_string(),
            Self::LastWeek => "last week, Mon–Sun".to_string(),
            Self::ThisMonth => "this month".to_string(),
            Self::LastMonth => "last month".to_string(),
            Self::Month(month) => {
                let named =
                    CivilDate::new(2000, month, 1).map_or("?", CivilDate::month_abbreviation);
                format!("{named}, the nearest one that has begun")
            }
        }
    }
}

/// Today on this machine: the date the local clock shows and how far the local zone is
/// from UTC at this moment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalDay {
    date: CivilDate,
    offset_seconds: i32,
}

impl LocalDay {
    pub fn new(date: CivilDate, offset_seconds: i32) -> Self {
        Self {
            date,
            offset_seconds,
        }
    }

    #[cfg(test)]
    pub fn date(self) -> CivilDate {
        self.date
    }
}

/// Where a period came from, which the period line prints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodOrigin {
    /// Read off a phrase of the question.
    Phrase(PeriodPhrase),
    /// Given by `--since` and `--until`. When the question also held a phrase, the flags win
    /// and the phrase is named as set aside, so nobody wonders which of the two was used.
    Flags { set_aside: Option<PeriodPhrase> },
}

/// A stretch of whole local days, first to last inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Period {
    first_day: CivilDate,
    last_day: CivilDate,
    offset_seconds: i32,
    origin: PeriodOrigin,
}

impl Period {
    /// The period a phrase names, counted from `today`.
    pub fn of_phrase(phrase: PeriodPhrase, today: LocalDay) -> Self {
        let date = today.date;
        let (first_day, last_day) = match phrase {
            PeriodPhrase::Today => (date, date),
            PeriodPhrase::Yesterday => (date.plus_days(-1), date.plus_days(-1)),
            PeriodPhrase::ThisWeek => {
                let monday = date.monday_of_its_week();
                (monday, monday.plus_days(6))
            }
            PeriodPhrase::LastWeek => {
                let monday = date.monday_of_its_week().plus_days(-7);
                (monday, monday.plus_days(6))
            }
            PeriodPhrase::ThisMonth => (date.first_of_its_month(), date.last_of_its_month()),
            PeriodPhrase::LastMonth => {
                let first = date.first_of_previous_month();
                (first, first.last_of_its_month())
            }
            PeriodPhrase::Month(month) => {
                // The nearest one that has begun: September asked in October is this year's,
                // and so is October asked in October — a month that is going on is not in the
                // future, and a person who meant a year ago says the year.
                let year = if month <= date.month() {
                    date.year()
                } else {
                    date.year() - 1
                };
                let first = CivilDate::new(year, month, 1).unwrap_or(date.first_of_its_month());
                (first, first.last_of_its_month())
            }
        };
        Self {
            first_day,
            last_day,
            offset_seconds: today.offset_seconds,
            origin: PeriodOrigin::Phrase(phrase),
        }
    }

    /// The period two flags give, both days inclusive. `--since` alone runs to the end of
    /// today; `--until` alone is refused rather than read as "since the beginning of time",
    /// which would be a period in name only.
    pub fn of_flags(
        since: Option<CivilDate>,
        until: Option<CivilDate>,
        today: LocalDay,
        set_aside: Option<PeriodPhrase>,
    ) -> Result<Self, PeriodError> {
        let Some(first_day) = since else {
            return Err(PeriodError::UntilWithoutSince);
        };
        let last_day = until.unwrap_or(today.date);
        if last_day < first_day {
            return Err(PeriodError::EndsBeforeItStarts {
                since: first_day,
                until: last_day,
            });
        }
        Ok(Self {
            first_day,
            last_day,
            offset_seconds: today.offset_seconds,
            origin: PeriodOrigin::Flags { set_aside },
        })
    }

    /// The period a question is about, if any: the flags when there are any, otherwise the
    /// phrase, otherwise none — and none is the ordinary case, answered exactly as before.
    pub fn resolve(
        since: Option<CivilDate>,
        until: Option<CivilDate>,
        phrase: Option<PeriodPhrase>,
        today: LocalDay,
    ) -> Result<Option<Self>, PeriodError> {
        if since.is_some() || until.is_some() {
            return Self::of_flags(since, until, today, phrase).map(Some);
        }
        Ok(phrase.map(|phrase| Self::of_phrase(phrase, today)))
    }

    /// The first moment of the period, in milliseconds since the epoch.
    pub fn start_unix_ms(&self) -> i64 {
        self.midnight_before(self.first_day)
    }

    /// The first moment after the period: midnight at the end of its last day.
    pub fn end_unix_ms(&self) -> i64 {
        self.midnight_before(self.last_day.plus_days(1))
    }

    /// Whether a moment falls inside. Half-open, so a moment at midnight belongs to the day
    /// that starts there and to no other.
    pub fn contains(&self, at_unix_ms: i64) -> bool {
        (self.start_unix_ms()..self.end_unix_ms()).contains(&at_unix_ms)
    }

    pub fn first_day(&self) -> CivilDate {
        self.first_day
    }

    pub fn last_day(&self) -> CivilDate {
        self.last_day
    }

    #[cfg(test)]
    pub fn offset_seconds(&self) -> i32 {
        self.offset_seconds
    }

    pub fn origin(&self) -> PeriodOrigin {
        self.origin
    }

    /// A moment as the local clock of this period showed it: the day, the hour and the
    /// minute. What a source line prints beside the event that put it in the period.
    pub fn local_time_of(&self, at_unix_ms: i64) -> (CivilDate, u8, u8) {
        let local_seconds = at_unix_ms.div_euclid(1000) + i64::from(self.offset_seconds);
        let date = CivilDate::from_days(local_seconds.div_euclid(86_400));
        let second_of_day = local_seconds.rem_euclid(86_400);
        (
            date,
            (second_of_day / 3600) as u8,
            ((second_of_day % 3600) / 60) as u8,
        )
    }

    fn midnight_before(&self, day: CivilDate) -> i64 {
        (day.days() * 86_400 - i64::from(self.offset_seconds)) * 1000
    }
}

/// `21–27 Sep 2026`, `28 Sep – 4 Oct 2026`, `29 Dec 2025 – 4 Jan 2026`, `1 Oct 2026`: the
/// days, written the way the period line and the model's line both write them.
impl fmt::Display for Period {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (first, last) = (self.first_day, self.last_day);
        if first == last {
            return write!(
                formatter,
                "{} {} {}",
                first.day(),
                first.month_abbreviation(),
                first.year()
            );
        }
        if first.year() != last.year() {
            return write!(
                formatter,
                "{} {} {} – {} {} {}",
                first.day(),
                first.month_abbreviation(),
                first.year(),
                last.day(),
                last.month_abbreviation(),
                last.year()
            );
        }
        if first.month() != last.month() {
            return write!(
                formatter,
                "{} {} – {} {} {}",
                first.day(),
                first.month_abbreviation(),
                last.day(),
                last.month_abbreviation(),
                last.year()
            );
        }
        write!(
            formatter,
            "{}–{} {} {}",
            first.day(),
            last.day(),
            last.month_abbreviation(),
            last.year()
        )
    }
}

/// What two flags cannot say together.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PeriodError {
    UntilWithoutSince,
    EndsBeforeItStarts { since: CivilDate, until: CivilDate },
}

impl fmt::Display for PeriodError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UntilWithoutSince => formatter.write_str(
                "--until needs --since: a period with no first day is every moment ever recorded",
            ),
            Self::EndsBeforeItStarts { since, until } => write!(
                formatter,
                "the period would end on {until}, before it starts on {since}"
            ),
        }
    }
}

impl std::error::Error for PeriodError {}

#[cfg(test)]
mod tests {
    use super::{CivilDate, LocalDay, Period, PeriodError, PeriodOrigin, PeriodPhrase};

    fn date(year: i32, month: u8, day: u8) -> CivilDate {
        CivilDate::new(year, month, day).unwrap()
    }

    /// The machine this program is measured on keeps UTC+05:00.
    const OFFSET: i32 = 5 * 3600;

    fn on(year: i32, month: u8, day: u8) -> LocalDay {
        LocalDay::new(date(year, month, day), OFFSET)
    }

    fn days(period: &Period) -> (CivilDate, CivilDate) {
        (period.first_day(), period.last_day())
    }

    /// The case that started the stage: on Thursday 1 October 2026 the last week is 21–27
    /// September, and 30 September belongs to this one.
    #[test]
    fn last_week_is_the_monday_to_sunday_before_this_one() {
        let today = on(2026, 10, 1);
        let last = Period::of_phrase(PeriodPhrase::LastWeek, today);
        assert_eq!(days(&last), (date(2026, 9, 21), date(2026, 9, 27)));
        assert_eq!(last.to_string(), "21–27 Sep 2026");

        let this = Period::of_phrase(PeriodPhrase::ThisWeek, today);
        assert_eq!(days(&this), (date(2026, 9, 28), date(2026, 10, 4)));
        assert_eq!(this.to_string(), "28 Sep – 4 Oct 2026");

        let thirtieth = (date(2026, 9, 30).days() * 86_400 + 12 * 3600) * 1000;
        assert!(this.contains(thirtieth) && !last.contains(thirtieth));
    }

    /// Monday is the first day of its own week, and Sunday the last.
    #[test]
    fn on_a_monday_and_on_a_sunday_the_week_is_the_same_week() {
        for today in [on(2026, 9, 28), on(2026, 10, 4)] {
            assert_eq!(
                days(&Period::of_phrase(PeriodPhrase::ThisWeek, today)),
                (date(2026, 9, 28), date(2026, 10, 4))
            );
            assert_eq!(
                days(&Period::of_phrase(PeriodPhrase::LastWeek, today)),
                (date(2026, 9, 21), date(2026, 9, 27))
            );
        }
    }

    /// Midnight belongs to the day it starts, in local time: the last millisecond of Sunday is
    /// last week, the first of Monday is not.
    #[test]
    fn the_boundary_is_local_midnight_and_the_interval_is_half_open() {
        let last = Period::of_phrase(PeriodPhrase::LastWeek, on(2026, 10, 1));
        let monday_midnight_utc = (date(2026, 9, 28).days() * 86_400 - i64::from(OFFSET)) * 1000;
        assert_eq!(last.end_unix_ms(), monday_midnight_utc);
        assert!(last.contains(monday_midnight_utc - 1));
        assert!(!last.contains(monday_midnight_utc));
        assert!(last.contains(last.start_unix_ms()));
        assert!(!last.contains(last.start_unix_ms() - 1));
        // 23:30 on Sunday local is 18:30 UTC on Sunday: inside. 00:30 Monday local is 19:30 UTC
        // on Sunday: outside, although a UTC calendar would call it Sunday.
        assert!(last.contains(monday_midnight_utc - 30 * 60 * 1000));
        assert!(!last.contains(monday_midnight_utc + 30 * 60 * 1000));
        assert_eq!(
            last.local_time_of(monday_midnight_utc + 30 * 60 * 1000),
            (date(2026, 9, 28), 0, 30)
        );
    }

    #[test]
    fn today_and_yesterday_are_one_local_day_each() {
        let today = on(2026, 10, 1);
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Today, today)),
            (date(2026, 10, 1), date(2026, 10, 1))
        );
        let yesterday = Period::of_phrase(PeriodPhrase::Yesterday, today);
        assert_eq!(days(&yesterday), (date(2026, 9, 30), date(2026, 9, 30)));
        assert_eq!(yesterday.to_string(), "30 Sep 2026");
        assert_eq!(
            yesterday.end_unix_ms() - yesterday.start_unix_ms(),
            86_400_000
        );
        // Yesterday of the first of a month is the last of the month before.
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Yesterday, on(2026, 3, 1))),
            (date(2026, 2, 28), date(2026, 2, 28))
        );
    }

    #[test]
    fn a_month_ends_on_its_own_last_day() {
        let today = on(2026, 10, 1);
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::LastMonth, today)),
            (date(2026, 9, 1), date(2026, 9, 30))
        );
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::ThisMonth, on(2028, 2, 10))),
            (date(2028, 2, 1), date(2028, 2, 29))
        );
        assert_eq!(
            Period::of_phrase(PeriodPhrase::LastMonth, today).to_string(),
            "1–30 Sep 2026"
        );
    }

    /// January's last month is December of the year before, and a week can span the two.
    #[test]
    fn the_turn_of_the_year_is_an_ordinary_boundary() {
        let today = on(2026, 1, 2);
        let last_month = Period::of_phrase(PeriodPhrase::LastMonth, today);
        assert_eq!(days(&last_month), (date(2025, 12, 1), date(2025, 12, 31)));
        let this_week = Period::of_phrase(PeriodPhrase::ThisWeek, today);
        assert_eq!(days(&this_week), (date(2025, 12, 29), date(2026, 1, 4)));
        assert_eq!(this_week.to_string(), "29 Dec 2025 – 4 Jan 2026");
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Yesterday, on(2026, 1, 1))),
            (date(2025, 12, 31), date(2025, 12, 31))
        );
    }

    /// A month by name is the nearest one that has begun: this year's when it has, last
    /// year's when it has not yet.
    #[test]
    fn a_named_month_is_the_nearest_that_has_begun() {
        let today = on(2026, 10, 1);
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Month(9), today)),
            (date(2026, 9, 1), date(2026, 9, 30))
        );
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Month(10), today)),
            (date(2026, 10, 1), date(2026, 10, 31))
        );
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Month(11), today)),
            (date(2025, 11, 1), date(2025, 11, 30))
        );
        assert_eq!(
            days(&Period::of_phrase(PeriodPhrase::Month(2), on(2029, 1, 5))),
            (date(2028, 2, 1), date(2028, 2, 29))
        );
    }

    /// Flags win over a phrase, and the phrase they set aside is kept to be named.
    #[test]
    fn flags_win_over_a_phrase_and_say_which_phrase_they_set_aside() {
        let today = on(2026, 10, 1);
        let period = Period::resolve(
            Some(date(2026, 9, 1)),
            Some(date(2026, 9, 3)),
            Some(PeriodPhrase::LastWeek),
            today,
        )
        .unwrap()
        .unwrap();
        assert_eq!(days(&period), (date(2026, 9, 1), date(2026, 9, 3)));
        assert_eq!(
            period.origin(),
            PeriodOrigin::Flags {
                set_aside: Some(PeriodPhrase::LastWeek)
            }
        );

        let since_only = Period::resolve(Some(date(2026, 9, 28)), None, None, today)
            .unwrap()
            .unwrap();
        assert_eq!(days(&since_only), (date(2026, 9, 28), date(2026, 10, 1)));

        assert_eq!(Period::resolve(None, None, None, today).unwrap(), None);
        assert_eq!(
            Period::resolve(None, Some(date(2026, 9, 3)), None, today).unwrap_err(),
            PeriodError::UntilWithoutSince
        );
        assert!(matches!(
            Period::resolve(Some(date(2026, 9, 3)), Some(date(2026, 9, 1)), None, today),
            Err(PeriodError::EndsBeforeItStarts { .. })
        ));
    }

    #[test]
    fn a_phrase_key_is_one_of_the_closed_set() {
        assert_eq!(
            PeriodPhrase::from_key("last-week"),
            Some(PeriodPhrase::LastWeek)
        );
        assert_eq!(
            PeriodPhrase::from_key("month-09"),
            Some(PeriodPhrase::Month(9))
        );
        for unknown in [
            "month-9",
            "month-13",
            "month-00",
            "fortnight",
            "",
            "month-+1",
        ] {
            assert_eq!(PeriodPhrase::from_key(unknown), None, "{unknown}");
        }
    }
}
