// The period a question or a query is about, as `ask` and `search` read it and print it.
//
// Both commands take the same two flags and read the same phrases, and both print the same
// line before anything else is shown — so the person sees how they were understood before
// they read an answer that depends on it. The rules are the domain's; what is here is reading
// arguments, reading the clock, and writing lines.
use crate::application::{PeriodEvent, PeriodScope};
use crate::domain::conversation::TouchVerb;
use crate::domain::{CivilDate, FoundPhrase, NormalizedPath, Period, PeriodOrigin};
use crate::infrastructure::{LadybugSearchGraph, built_in_period_phrases, local_today};

use super::refusal::Refusal;

/// `--since` and `--until`, as typed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct PeriodFlags {
    since: Option<CivilDate>,
    until: Option<CivilDate>,
}

impl PeriodFlags {
    fn is_empty(self) -> bool {
        self.since.is_none() && self.until.is_none()
    }

    /// Reads one of the two flags when `argument` is one, taking its value from `remaining`.
    /// `Ok(false)` when the argument is something else.
    pub(super) fn read<'a>(
        &mut self,
        argument: &str,
        remaining: &mut impl Iterator<Item = &'a String>,
        usage: &str,
    ) -> Result<bool, String> {
        let slot = match argument {
            "--since" => &mut self.since,
            "--until" => &mut self.until,
            _ => return Ok(false),
        };
        let value = remaining
            .next()
            .ok_or_else(|| format!("{argument} requires a date, YYYY-MM-DD. {usage}"))?;
        *slot = Some(CivilDate::parse_iso(value).ok_or_else(|| {
            format!("{argument} expects a date as YYYY-MM-DD, received {value:?}. {usage}")
        })?);
        Ok(true)
    }
}

/// What a question is about once its period has been read: the period, if any, the phrase
/// that named one, if any, and the words that are left to look for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct ReadPeriod {
    period: Option<Period>,
    phrase: Option<FoundPhrase>,
    words: String,
}

impl ReadPeriod {
    pub(super) fn period(&self) -> Option<&Period> {
        self.period.as_ref()
    }

    /// The words to look for: the text given, without the phrase when there was one.
    pub(super) fn words(&self) -> &str {
        &self.words
    }

    /// Whether the phrase was taken out of what is looked for, so the command says what it
    /// looked for instead of letting the person assume it was everything they typed.
    pub(super) fn narrowed_words(&self) -> bool {
        self.phrase.is_some()
    }
}

/// Reads the period of `text` and of the flags. The clock is read only when there is a period
/// to count, so a question without one reads nothing it did not read before this stage.
pub(super) fn read_period(flags: PeriodFlags, text: &str) -> Result<ReadPeriod, Refusal> {
    let phrases = built_in_period_phrases().map_err(|error| Refusal::failed(error.to_string()))?;
    let stripped = phrases.strip(text);
    let phrase = stripped.found().cloned();
    if flags.is_empty() && phrase.is_none() {
        return Ok(ReadPeriod {
            period: None,
            phrase: None,
            words: text.to_string(),
        });
    }
    let today = local_today().map_err(|error| Refusal::failed(error.to_string()))?;
    let period = Period::resolve(
        flags.since,
        flags.until,
        phrase.as_ref().map(FoundPhrase::phrase),
        today,
    )
    .map_err(|error| Refusal::usage(error.to_string()))?;
    Ok(ReadPeriod {
        period,
        phrase,
        words: stripped.remaining().to_string(),
    })
}

/// The events of the period, read from the context's graph, and the line saying what the
/// period could not show, printed as soon as it is known — before the page or the answer
/// that depends on it.
pub(super) fn scope_of(
    graph: &LadybugSearchGraph,
    period: Option<&Period>,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
) -> Result<Option<PeriodScope>, String> {
    let scope = gather(graph, period, is_visible)?;
    if let Some(gaps) = scope.as_ref().and_then(|scope| scope.gaps().sentence()) {
        println!("{gaps}");
    }
    Ok(scope)
}

/// The events of the period, without printing anything about them.
pub(super) fn gather(
    graph: &LadybugSearchGraph,
    period: Option<&Period>,
    is_visible: &dyn Fn(&NormalizedPath) -> bool,
) -> Result<Option<PeriodScope>, Refusal> {
    let Some(period) = period else {
        return Ok(None);
    };
    PeriodScope::gather(graph, *period, is_visible)
        .map(Some)
        .map_err(|error| Refusal::of_graph(&error, error.to_string()))
}

/// The lines printed before an answer or a page: the period and how it was read, and what
/// was looked for when the phrase was taken out.
pub(super) fn print_reading(read: &ReadPeriod) {
    if let Some(period) = read.period() {
        println!("{}", period_line(period, read.phrase.as_ref()));
    }
    if read.narrowed_words() {
        println!("Looked for: {}", read.words());
    }
}

/// `Period: 21–27 Sep 2026 (last week, Mon–Sun, local time)`.
fn period_line(period: &Period, phrase: Option<&FoundPhrase>) -> String {
    match period.origin() {
        PeriodOrigin::Phrase(read_as) => {
            format!("Period: {period} ({}, local time)", read_as.reading())
        }
        PeriodOrigin::Flags { set_aside: None } => {
            format!("Period: {period} (--since/--until, local time)")
        }
        PeriodOrigin::Flags { set_aside: Some(_) } => format!(
            "Period: {period} (--since/--until, local time); the flags win over {:?}, which \
             was not used",
            phrase.map_or("", FoundPhrase::as_written)
        ),
    }
}

/// `reply 2026-09-24 14:02`, `edited 2026-09-24 14:02`: what put a source inside the period,
/// in the period's local time.
pub(super) fn event_label(event: PeriodEvent, period: &Period) -> String {
    let (what, at) = match event {
        PeriodEvent::Reply { at_unix_ms } => ("reply", at_unix_ms),
        PeriodEvent::Touch { at_unix_ms, verb } => (verb_word(verb), at_unix_ms),
    };
    let (date, hour, minute) = period.local_time_of(at);
    format!("{what} {date} {hour:02}:{minute:02}")
}

fn verb_word(verb: TouchVerb) -> &'static str {
    match verb {
        TouchVerb::Read => "read",
        TouchVerb::Wrote => "written",
        TouchVerb::Edited => "edited",
    }
}

/// What a period with nothing in it is told: whether nothing happened in it at all, or
/// something did and none of it matched.
pub(super) fn nothing_inside(period: &Period, files_with_events: usize) -> String {
    match files_with_events {
        0 => format!(
            "Nothing in this context has an event in {period}: no reply was said and no file was \
             written or named in a conversation then."
        ),
        files => format!(
            "Nothing inside {period} matched. {files} files have an event in it; nothing from \
             outside the period was used instead."
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::{PeriodFlags, event_label, period_line};
    use crate::application::PeriodEvent;
    use crate::domain::conversation::TouchVerb;
    use crate::domain::{CivilDate, LocalDay, Period, PeriodPhrase, PeriodPhrases};

    fn thursday() -> LocalDay {
        LocalDay::new(CivilDate::new(2026, 10, 1).unwrap(), 5 * 3600)
    }

    /// The line the stage names, word for word.
    #[test]
    fn the_period_line_says_the_days_and_how_they_were_read() {
        let last_week = Period::of_phrase(PeriodPhrase::LastWeek, thursday());
        assert_eq!(
            period_line(&last_week, None),
            "Period: 21–27 Sep 2026 (last week, Mon–Sun, local time)"
        );
    }

    /// Flags beside a phrase: the flags are the period, and the line names the phrase it set
    /// aside.
    #[test]
    fn the_line_names_the_phrase_the_flags_set_aside() {
        let phrases = PeriodPhrases::new(vec![(
            PeriodPhrase::LastWeek,
            "на прошлой неделе".to_string(),
        )])
        .unwrap();
        let stripped = phrases.strip("что было на прошлой неделе");
        let period = Period::resolve(
            CivilDate::new(2026, 9, 1),
            CivilDate::new(2026, 9, 3),
            stripped.found().map(|found| found.phrase()),
            thursday(),
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            period_line(&period, stripped.found()),
            "Period: 1–3 Sep 2026 (--since/--until, local time); the flags win over \
             \"на прошлой неделе\", which was not used"
        );
    }

    #[test]
    fn an_event_is_printed_in_the_periods_local_time() {
        let period = Period::of_phrase(PeriodPhrase::LastWeek, thursday());
        // 09:02 UTC on 24 September is 14:02 at UTC+05:00.
        let at = (CivilDate::new(2026, 9, 24).unwrap().days() * 86_400 + 9 * 3600 + 120) * 1000;
        assert_eq!(
            event_label(PeriodEvent::Reply { at_unix_ms: at }, &period),
            "reply 2026-09-24 14:02"
        );
        assert_eq!(
            event_label(
                PeriodEvent::Touch {
                    at_unix_ms: at,
                    verb: TouchVerb::Edited
                },
                &period
            ),
            "edited 2026-09-24 14:02"
        );
    }

    #[test]
    fn a_flag_takes_a_date_in_one_form() {
        let mut flags = PeriodFlags::default();
        let values = ["2026-09-21".to_string()];
        assert_eq!(flags.read("--since", &mut values.iter(), "usage"), Ok(true));
        assert_eq!(flags.since, CivilDate::new(2026, 9, 21));
        assert_eq!(
            flags.read("--limit", &mut values.iter(), "usage"),
            Ok(false)
        );
        let wrong = ["21.09.2026".to_string()];
        assert!(flags.read("--until", &mut wrong.iter(), "usage").is_err());
        assert!(flags.read("--until", &mut [].iter(), "usage").is_err());
    }
}
