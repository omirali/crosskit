// Which writing systems a chunk's text is in, as far as this program can actually tell.
//
// This replaces a field called `language` whose value was `"en"` when the text was ASCII
// and `"und"` otherwise, decided once for a whole file. That field claimed something nobody
// had computed: English source code was `"en"` by coincidence, Russian prose was `"und"`,
// and a file holding both was `"und"` entirely. A graph that stores provenance cannot also
// store a guess under the name of a fact.
//
// What is computed here is narrow and true: which of the two scripts Crosskit is able to
// normalize occur in this chunk's text. It is not language detection, it does not become
// language detection by being read often, and the name says so.

/// The scripts found in one chunk's text.
///
/// Per chunk, not per file: a file of Russian prose with English identifiers in it has
/// chunks of each kind, and a single answer for the whole file would be wrong for most of
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkScript {
    /// Latin letters, no Cyrillic.
    Latin,
    /// Cyrillic letters, no Latin.
    Cyrillic,
    /// Both, which is the ordinary case for this corpus and the case a single choice of
    /// normalization cannot serve.
    LatinAndCyrillic,
    /// Neither script has a letter here. Digits, punctuation and a script Crosskit does
    /// not normalize all land in this one value, because telling them apart would be a
    /// claim this program has not earned.
    Neither,
}

impl ChunkScript {
    /// Reads the text once. A chunk is at most a few kilobytes, and both answers are known
    /// as soon as one letter of each script has been seen.
    pub fn of(text: &str) -> Self {
        let mut latin = false;
        let mut cyrillic = false;
        for character in text.chars() {
            if is_latin(character) {
                latin = true;
            } else if is_cyrillic(character) {
                cyrillic = true;
            }
            if latin && cyrillic {
                return Self::LatinAndCyrillic;
            }
        }
        match (latin, cyrillic) {
            (true, true) => Self::LatinAndCyrillic,
            (true, false) => Self::Latin,
            (false, true) => Self::Cyrillic,
            (false, false) => Self::Neither,
        }
    }

    /// How the value is stored and read back. Stable: a migration and a fresh index must
    /// write the same string for the same text.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Latin => "latin",
            Self::Cyrillic => "cyrillic",
            Self::LatinAndCyrillic => "latin+cyrillic",
            Self::Neither => "neither",
        }
    }
}

/// Basic Latin plus the accented letters of Latin-1 Supplement, Latin Extended-A and
/// Latin Extended-B. `is_alphabetic` excludes the multiplication and division signs, which
/// sit inside that range and are not letters.
fn is_latin(character: char) -> bool {
    character.is_ascii_alphabetic()
        || (matches!(character, '\u{00C0}'..='\u{024F}') && character.is_alphabetic())
}

/// Cyrillic and Cyrillic Supplement.
fn is_cyrillic(character: char) -> bool {
    matches!(character, '\u{0400}'..='\u{04FF}' | '\u{0500}'..='\u{052F}')
}

#[cfg(test)]
mod tests {
    use super::ChunkScript;

    #[test]
    fn each_script_is_named_by_what_the_text_actually_holds() {
        for (text, expected) in [
            ("plain ascii prose", ChunkScript::Latin),
            ("русская проза", ChunkScript::Cyrillic),
            ("the walker обходит tree", ChunkScript::LatinAndCyrillic),
            ("1234 — 5678 … ()[]{}", ChunkScript::Neither),
            ("", ChunkScript::Neither),
        ] {
            assert_eq!(ChunkScript::of(text), expected, "{text:?}");
        }
    }

    /// The field this replaced answered for a whole file, so a file of Russian prose with
    /// one English identifier in it was `"und"` from end to end. Per chunk, each piece says
    /// what it holds.
    #[test]
    fn a_mixed_file_is_not_one_answer() {
        let russian = "Сверщик обходит индекс и сообщает об удалённых локациях.";
        let code = "fn reconcile(&self) -> Result<(), Error> { Ok(()) }";
        assert_eq!(ChunkScript::of(russian), ChunkScript::Cyrillic);
        assert_eq!(ChunkScript::of(code), ChunkScript::Latin);
        assert_eq!(
            ChunkScript::of(&format!("{russian} {code}")),
            ChunkScript::LatinAndCyrillic
        );
    }

    /// Accented Latin is Latin, and the two signs that sit among those letters are not
    /// letters at all.
    #[test]
    fn accented_latin_counts_and_arithmetic_signs_do_not() {
        assert_eq!(ChunkScript::of("café naïve Straße"), ChunkScript::Latin);
        assert_eq!(ChunkScript::of("2 × 3 ÷ 4"), ChunkScript::Neither);
    }

    /// A script this program cannot normalize is not quietly called one it can.
    #[test]
    fn an_unsupported_script_is_neither_rather_than_a_guess() {
        assert_eq!(ChunkScript::of("κόσμος"), ChunkScript::Neither);
        assert_eq!(ChunkScript::of("日本語"), ChunkScript::Neither);
    }

    #[test]
    fn the_stored_spelling_is_stable() {
        for (script, stored) in [
            (ChunkScript::Latin, "latin"),
            (ChunkScript::Cyrillic, "cyrillic"),
            (ChunkScript::LatinAndCyrillic, "latin+cyrillic"),
            (ChunkScript::Neither, "neither"),
        ] {
            assert_eq!(script.as_str(), stored);
        }
    }
}
