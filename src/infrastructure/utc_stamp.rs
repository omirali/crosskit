// A moment written into a file name: `20260922T143000Z` from a UNIX timestamp in
// milliseconds.
//
// Written out rather than taken from a crate: a name that sorts and reads is worth twenty
// lines, and a date library would be a dependency this program has no other use for. The
// civil date comes from `CivilDate`, the program's one days-from-epoch algorithm; a
// timestamp before the epoch is not a moment this program can be running at, so it is
// clamped rather than given a second code path.
//
// Two things are kept beside what they replace under a name of this shape — a write-ahead
// log a recovery moved aside, and a policy file `setup` replaced — and both have the same
// reason for the moment being in the name: the file they keep must not be overwritten by
// the next one.
use crate::domain::CivilDate;

/// `20260922T143000Z` from a UNIX timestamp in milliseconds.
pub fn utc_stamp(unix_ms: i64) -> String {
    let seconds = unix_ms.max(0) / 1000;
    let days = seconds / 86_400;
    let second_of_day = seconds % 86_400;
    let date = CivilDate::from_days(days);
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        date.year(),
        date.month(),
        date.day(),
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

/// `2026-09-22 14:30:00Z` from a UNIX timestamp in milliseconds: the same moment, for a line
/// a person reads rather than a file name that has to sort.
pub fn utc_moment(unix_ms: i64) -> String {
    let stamp = utc_stamp(unix_ms);
    format!(
        "{}-{}-{} {}:{}:{}Z",
        &stamp[0..4],
        &stamp[4..6],
        &stamp[6..8],
        &stamp[9..11],
        &stamp[11..13],
        &stamp[13..15]
    )
}

#[cfg(test)]
mod tests {
    use super::{utc_moment, utc_stamp};

    #[test]
    fn a_moment_for_a_person_reads_like_a_date_and_a_time() {
        assert_eq!(utc_moment(1_790_087_400_000), "2026-09-22 14:30:00Z");
    }

    #[test]
    fn a_stamp_names_the_moment_in_utc() {
        assert_eq!(utc_stamp(0), "19700101T000000Z");
        // 2026-09-22 14:30:00 UTC
        assert_eq!(utc_stamp(1_790_087_400_000), "20260922T143000Z");
        assert_eq!(utc_stamp(-1), "19700101T000000Z");
    }

    /// Two moments a second apart are two names. The point is the rule, not the
    /// resolution: a name that repeats would overwrite the file it was meant to keep.
    #[test]
    fn a_second_apart_is_a_different_name() {
        assert_ne!(utc_stamp(1_790_087_400_000), utc_stamp(1_790_087_401_000));
    }
}
