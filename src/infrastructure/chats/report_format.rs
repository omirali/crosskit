// How the measurement writes numbers, in one place, so two tables cannot disagree about what
// a megabyte is.
//
// Bytes are printed twice where a limit is involved and once everywhere else. The 16 MiB
// content limit is binary and file sizes are quoted decimally by every tool the owner looks
// at, and Stage 31 started from a report that said 58.5 MB and 61.4 MB about the same file.
// They are the same file.

/// `1 234 567`, grouped with a non-breaking space so a column of numbers reads as a column.
pub(crate) fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (position, digit) in digits.chars().enumerate() {
        if position > 0 && (digits.len() - position).is_multiple_of(3) {
            out.push('\u{00a0}');
        }
        out.push(digit);
    }
    out
}

/// Decimal megabytes, as a file listing quotes them.
pub(crate) fn megabytes(bytes: u64) -> String {
    format!("{:.1} МБ", bytes as f64 / 1_000_000.0)
}

/// Decimal kilobytes, for the numbers a megabyte rounds to nothing.
pub(crate) fn kilobytes(bytes: u64) -> String {
    format!("{:.1} КБ", bytes as f64 / 1_000.0)
}

/// Binary mebibytes, which is what the content limit is written in.
pub(crate) fn mebibytes(bytes: u64) -> String {
    format!("{:.1} МиБ", bytes as f64 / (1024.0 * 1024.0))
}

/// A share of a whole, or a dash when there is no whole to be a share of.
pub(crate) fn share(part: u64, whole: u64) -> String {
    if whole == 0 {
        return "—".to_string();
    }
    format!("{:.2} %", part as f64 * 100.0 / whole as f64)
}

/// The middle value of a set of counts, which says more about a corpus than the mean when one
/// file is twenty times the size of the rest.
pub(crate) fn median(mut values: Vec<u64>) -> u64 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_are_grouped_by_three_from_the_right() {
        assert_eq!(grouped(7), "7");
        assert_eq!(grouped(1_000), "1\u{00a0}000");
        assert_eq!(grouped(1_234_567), "1\u{00a0}234\u{00a0}567");
    }

    /// The two units the report has to keep apart, on the file that made it necessary.
    #[test]
    fn the_same_file_is_both_numbers_the_owner_saw() {
        assert_eq!(megabytes(61_384_989), "61.4 МБ");
        assert_eq!(mebibytes(61_384_989), "58.5 МиБ");
    }

    #[test]
    fn a_share_of_nothing_is_not_a_number() {
        assert_eq!(share(0, 0), "—");
        assert_eq!(share(1, 4), "25.00 %");
    }

    #[test]
    fn the_median_of_an_even_set_takes_the_upper_middle() {
        assert_eq!(median(Vec::new()), 0);
        assert_eq!(median(vec![5, 1, 3]), 3);
        assert_eq!(median(vec![4, 1, 3, 2]), 3);
    }
}
