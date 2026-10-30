// Today, as this machine's clock and zone say it is.
//
// `localtime_r` and not a crate: the C library already knows the zone the machine is set to,
// with its rules and its database, and `libc` is already a dependency. What comes back is the
// date the local clock shows and the zone's distance from UTC at this moment — the two things
// a period is counted from. `TZ` in the environment changes both, which is what makes a test
// of "this week" independent of where it runs.
use crate::domain::{CivilDate, LocalDay};

/// The local clock could not be read. It names no detail, because there is none a person
/// could act on: the C library either answers or it does not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LocalTimeUnavailable;

impl std::fmt::Display for LocalTimeUnavailable {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("the local date and time zone of this machine could not be read")
    }
}

impl std::error::Error for LocalTimeUnavailable {}

pub fn local_today() -> Result<LocalDay, LocalTimeUnavailable> {
    // SAFETY: `time` with a null pointer only returns the current time.
    let now = unsafe { libc::time(std::ptr::null_mut()) };
    // SAFETY: an all-zero `tm` is a valid value of a plain C struct of integers and a pointer,
    // and `localtime_r` writes it in full before it is read. The reentrant form is used because
    // `localtime` shares one static buffer between every thread of the process.
    let mut local: libc::tm = unsafe { std::mem::zeroed() };
    let filled = unsafe { libc::localtime_r(&now, &mut local) };
    if filled.is_null() {
        return Err(LocalTimeUnavailable);
    }
    let date = CivilDate::new(
        local.tm_year + 1900,
        u8::try_from(local.tm_mon + 1).map_err(|_| LocalTimeUnavailable)?,
        u8::try_from(local.tm_mday).map_err(|_| LocalTimeUnavailable)?,
    )
    .ok_or(LocalTimeUnavailable)?;
    let offset_seconds = i32::try_from(local.tm_gmtoff).map_err(|_| LocalTimeUnavailable)?;
    Ok(LocalDay::new(date, offset_seconds))
}

#[cfg(test)]
mod tests {
    use super::local_today;

    /// Whatever the zone, today is a real date and the offset is one a zone can have.
    #[test]
    fn today_is_a_date_with_an_offset_a_zone_can_have() {
        let today = local_today().unwrap();
        assert!(today.date().year() >= 2026);
        let offset = today_offset_seconds();
        assert!((-12 * 3600..=14 * 3600).contains(&offset), "{offset}");
    }

    fn today_offset_seconds() -> i32 {
        let today = local_today().unwrap();
        let period = crate::domain::Period::of_phrase(crate::domain::PeriodPhrase::Today, today);
        period.offset_seconds()
    }
}
