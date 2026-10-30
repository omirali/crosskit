// The folder Claude Code files a session's transcripts under, and the places the system keeps
// for throwaway work.
//
// The folder is named after the directory the session was started in, and the name is computed
// here from a directory, never read back into one. The encoding loses what it replaces: every
// `/`, `-`, `.`, `_` and space becomes the same dash, so `-a-b` is `/a/b`, `/a-b`, `/a.b` and
// `/a_b` at once. Anything that decoded folder names would be choosing among those, so nothing
// does: `setup` computes the name of a directory it already knows and compares (ADR 0042), and
// Stage 38's measurement did the same.
use super::source_path::NormalizedPath;

/// The folder name the tool gives a directory.
///
/// The rule is the tool's own, read out of its code (Claude Code 2.1.41, the function that
/// builds `~/.claude/projects/<name>`): `directory.replace(/[^a-zA-Z0-9]/g, "-")`, with no
/// shortening of long paths in that version. A JavaScript regular expression without the `u`
/// flag works on UTF-16 code units, so a letter outside ASCII becomes one dash and a character
/// outside the Basic Multilingual Plane becomes two — which is why this counts `len_utf16`, and
/// neither characters nor bytes.
pub fn transcript_folder_name(directory: &str) -> String {
    let mut name = String::with_capacity(directory.len());
    for character in directory.chars() {
        if character.is_ascii_alphanumeric() {
            name.push(character);
            continue;
        }
        for _ in 0..character.len_utf16() {
            name.push('-');
        }
    }
    name
}

/// The directory, of `directories`, whose sessions a transcript folder holds: the one the folder
/// is named after, or the deepest one it is named after a subdirectory of — a session started
/// in `payments-api/src` is filed under `…-payments-api-src`. Computed in the encoding's own
/// direction, as everything here is: each directory is encoded and compared, nothing decoded.
pub fn project_of_folder<'d>(
    folder_name: &str,
    directories: impl IntoIterator<Item = &'d str>,
) -> Option<&'d str> {
    directories
        .into_iter()
        .filter(|directory| {
            let encoded = transcript_folder_name(directory.trim_end_matches('/'));
            folder_name == encoded
                || folder_name
                    .strip_prefix(&encoded)
                    .is_some_and(|rest| rest.starts_with('-'))
        })
        .max_by_key(|directory| directory.len())
}

/// Where the operating system keeps throwaway work: the per-user temporary directory macOS
/// hands out under `/var/folders`, and the shared temporary directories. These are places in
/// the layout of the system, fixed by it the way `~/Documents` is fixed by the home layout, and
/// not names somebody chose — which is why a list is the rule here. On macOS `/var` and `/tmp` are links into `/private`; a directory is recorded by the
/// tool after resolving them, so both spellings are listed.
const TEMPORARY_PLACES: [&str; 6] = [
    "/private/var/folders",
    "/private/tmp",
    "/private/var/tmp",
    "/var/folders",
    "/tmp",
    "/var/tmp",
];

/// Whether a working directory lies in a temporary place. Compared by whole segments, so
/// `/tmpwork` is not `/tmp`.
pub fn is_temporary_place(directory: &NormalizedPath) -> bool {
    TEMPORARY_PLACES.iter().any(|place| {
        NormalizedPath::from_absolute(place).is_ok_and(|place| directory.is_within(&place))
    })
}

/// Whether a transcript folder's name is the encoding of a directory inside a temporary place.
///
/// This is the one question asked of a folder name without the directory in hand, and it is
/// asked in the encoding's own direction: the temporary place is encoded and compared as a
/// prefix, followed by the dash every separator became. Nothing is decoded.
pub fn is_temporary_folder_name(name: &str) -> bool {
    TEMPORARY_PLACES.iter().any(|place| {
        let encoded = transcript_folder_name(place);
        name == encoded
            || name
                .strip_prefix(&encoded)
                .is_some_and(|rest| rest.starts_with('-'))
    })
}

#[cfg(test)]
mod tests {
    use super::{
        is_temporary_folder_name, is_temporary_place, project_of_folder, transcript_folder_name,
    };
    use crate::domain::NormalizedPath;

    #[test]
    fn a_folder_of_conversations_belongs_to_the_directory_it_is_named_after() {
        let roots = [
            "/Users/alice/Projects/payments-api",
            "/Users/alice/Projects/payments",
        ];
        assert_eq!(
            project_of_folder("-Users-alice-Projects-payments-api", roots),
            Some("/Users/alice/Projects/payments-api")
        );
        assert_eq!(
            project_of_folder("-Users-alice-Projects-payments-api-src", roots),
            Some("/Users/alice/Projects/payments-api"),
            "сессия, начатая в подкаталоге, — того же проекта"
        );
        assert_eq!(project_of_folder("-Users-alice-notes", roots), None);
    }

    fn path(value: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(value).expect("absolute")
    }

    #[test]
    fn every_character_outside_ascii_letters_and_digits_becomes_a_dash() {
        assert_eq!(
            transcript_folder_name("/Users/alice/Projects/payments-api"),
            "-Users-alice-Projects-payments-api"
        );
        assert_eq!(
            transcript_folder_name("/Users/alice/My Notes/v1.2_final"),
            "-Users-alice-My-Notes-v1-2-final"
        );
    }

    /// Four different directories, one folder. This is why a folder name is never decoded:
    /// any decoding would pick one of them, and three would be wrong.
    #[test]
    fn a_dash_in_a_folder_name_does_not_say_what_it_replaced() {
        let spellings = [
            "/Users/alice/Projects/payments/api",
            "/Users/alice/Projects/payments-api",
            "/Users/alice/Projects/payments.api",
            "/Users/alice/Projects/payments_api",
        ];
        for spelling in spellings {
            assert_eq!(
                transcript_folder_name(spelling),
                "-Users-alice-Projects-payments-api",
                "{spelling}"
            );
        }
    }

    /// Counted in UTF-16 code units, as the tool's regular expression counts them: a Cyrillic
    /// letter is one unit and one dash, a character outside the Basic Multilingual Plane is two.
    #[test]
    fn a_letter_outside_ascii_becomes_one_dash_per_utf16_unit() {
        assert_eq!(
            transcript_folder_name("/Users/alice/Проекты/app"),
            "-Users-alice---------app"
        );
        assert_eq!(transcript_folder_name("/tmp/a😀b"), "-tmp-a--b");
        assert_eq!(
            transcript_folder_name("/tmp/\u{e9}"),
            "-tmp--",
            "a precomposed letter is one unit"
        );
    }

    #[test]
    fn a_temporary_place_is_matched_by_whole_segments() {
        assert!(is_temporary_place(&path(
            "/private/var/folders/xy/abc_123/T/pytest-of-alice/pytest-0/test_one0"
        )));
        assert!(is_temporary_place(&path(
            "/private/tmp/tool-501/session/scratchpad"
        )));
        assert!(is_temporary_place(&path("/tmp")));
        assert!(!is_temporary_place(&path("/tmpwork/app")));
        assert!(!is_temporary_place(&path("/Users/alice/tmp/app")));
        assert!(!is_temporary_place(&path(
            "/Users/alice/Projects/payments-api"
        )));
    }

    #[test]
    fn a_temporary_folder_name_is_recognised_by_its_encoded_prefix() {
        let pytest = transcript_folder_name(
            "/private/var/folders/xy/abc_123/T/pytest-of-alice/pytest-0/test_one0",
        );
        assert!(is_temporary_folder_name(&pytest));
        assert!(is_temporary_folder_name(&transcript_folder_name(
            "/private/tmp/tool-501/-Users-alice-Projects-app/0f3a/scratchpad/probe"
        )));
        assert!(!is_temporary_folder_name(
            "-Users-alice-Projects-payments-api"
        ));
        assert!(
            !is_temporary_folder_name("-private-tmpwork"),
            "a prefix is matched up to the dash a separator became"
        );
    }
}
