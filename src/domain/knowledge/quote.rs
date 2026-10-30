// Whether a quote proves its fact. Two rules.
//
// The first: the quote is the source's own words. The rule is the narrowest that survives how
// text is written down: runs of whitespace are one space, and nothing else is forgiven. A model that copies a sentence across a line break of a
// Markdown file writes a space where the file has a newline and two spaces of indentation; that
// is the same words. A model that drops the asterisks of `**БИН**`, straightens «ёлочки» into
// quotes or fixes a typo has written its own sentence, and a fact resting on it is refused. The
// stricter rule costs facts and is counted; the looser one would make the quote a paraphrase,
// and a paraphrase proves nothing about where a fact came from.

// The second: the quote names both ends of its fact. A quote is found in its part and still
// proves nothing about «Контрагент — уникален по — БИН» when it is «БИН»: that word occurs in
// every part about it. An end is named when every word of its key is a word of the quote, both
// reduced by the same analyzer, or when the quote holds one of the ways the answer wrote it.

/// Every run of whitespace as one space, and none at either end.
pub fn collapse_whitespace(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Whether `quote`, read with whitespace collapsed, occurs in `text` read the same way. An
/// empty quote occurs nowhere: it would prove any fact about any part.
pub fn quote_occurs_in(quote: &str, text: &str) -> bool {
    let quote = collapse_whitespace(quote);
    !quote.is_empty() && collapse_whitespace(text).contains(&quote)
}

/// Whether a quote names an entity. `quote_words` is the quote reduced to key words by the same
/// function the entity's key was made with; `key_words` are the words of that key; `spellings`
/// are the ways the answer wrote the entity, matched ignoring case and runs of whitespace.
pub fn quote_names(
    quote: &str,
    quote_words: &[&str],
    key_words: &[&str],
    spellings: &[&str],
) -> bool {
    let by_key = !key_words.is_empty() && key_words.iter().all(|word| quote_words.contains(word));
    if by_key {
        return true;
    }
    let quote = collapse_whitespace(quote).to_lowercase();
    spellings.iter().any(|spelling| {
        let spelling = collapse_whitespace(spelling).to_lowercase();
        !spelling.is_empty() && quote.contains(&spelling)
    })
}

#[cfg(test)]
mod tests {
    use super::{collapse_whitespace, quote_names, quote_occurs_in};

    const PART: &str = "## Реестр\n\nКонтрагент уникален по БИН:\n  два контрагента с одним **БИН** — ошибка «склейки».";

    #[test]
    fn the_same_words_across_a_line_break_are_the_same_quote() {
        assert!(quote_occurs_in("Контрагент уникален по БИН", PART));
        assert!(quote_occurs_in(
            "Контрагент уникален по БИН: два контрагента",
            PART
        ));
        assert!(quote_occurs_in("  уникален\tпо   БИН ", PART));
    }

    /// Each of these is a sentence the model wrote, not one the file holds.
    #[test]
    fn anything_but_whitespace_that_differs_is_not_the_source() {
        for rewritten in [
            "контрагент уникален по БИН",
            "два контрагента с одним БИН",
            "ошибка \"склейки\"",
            "Контрагент уникален по ИИН",
            "Контрагент уникален по БИН.",
        ] {
            assert!(!quote_occurs_in(rewritten, PART), "{rewritten:?}");
        }
    }

    #[test]
    fn an_end_is_named_by_every_word_of_its_key_or_by_a_written_form() {
        let quote = "Контрагенты уникальны по БИН";
        let words = ["контрагент", "уникальн", "по", "бин"];

        assert!(quote_names(quote, &words, &["контрагент"], &["Контрагент"]));
        assert!(quote_names(quote, &words, &["бин"], &[]));
        // Every word of the key, not one of them.
        assert!(!quote_names(
            quote,
            &words,
            &["реестр", "контрагент"],
            &["Реестр контрагентов"]
        ));
        // A written form found in the quote, whatever the key says.
        assert!(quote_names(
            "ИИН/БИН проверены",
            &["иин", "бин", "провер"],
            &["бизнес", "номер"],
            &["иин/бин"]
        ));
        assert!(!quote_names(
            "БИН",
            &["бин"],
            &["контрагент"],
            &["Контрагент"]
        ));
        assert!(!quote_names("БИН", &["бин"], &[], &[""]));
    }

    #[test]
    fn an_empty_quote_proves_nothing() {
        assert!(!quote_occurs_in("", PART));
        assert!(!quote_occurs_in(" \n ", PART));
        assert_eq!(collapse_whitespace(" a \n\n b\t"), "a b");
    }
}
