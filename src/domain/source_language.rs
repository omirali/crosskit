// Which language a file is written in as far as its *name* declares it, and no further.
//
// ADR 0012 removed a field called `language` that held `"en"` when a file's text was ASCII
// and `"und"` when it was not. Nobody had computed that, and a graph which stores
// provenance cannot store a guess under the name of a fact. The same rule governs this
// type, so what it claims is said in one line: the language named by the suffix of a path,
// looked up in the table below, and nothing whatever read out of the file. A file called
// `.h` may hold C or C++, a file called `.ts` may hold TypeScript or a Qt translation, and
// a file called `.py` may hold a shell script somebody renamed. The suffix is what its
// author wrote down; this type repeats that and stops there.
//
// It exists for a question that cannot be asked yet. When an extractor that understands
// syntax arrives, the files written in the language it understands have to be read again —
// and finding them by walking the tree means opening every file on the machine, while
// finding them by a field means one query. That is the whole of the value today.
use super::source_path::NormalizedPath;

/// The version of the suffix table below, recorded by everything written from it.
///
/// It is not the extractor's version and says nothing about chunking: the same bytes, split
/// the same way, get a different answer here as soon as a row is added or a disputed suffix
/// is decided differently. Whatever carries this number can say which table labelled it,
/// and so which of its records were written before the change and which after — which is
/// the only thing that makes a table anybody is expected to extend safe to extend.
pub const SUFFIX_TABLE_VERSION: u32 = 1;

/// A language named by a path's suffix.
///
/// Closed, and closed on purpose: every variant is a language measured on the tree this
/// program indexes, and a language nobody here writes would be a row nobody could check.
/// The counts are in the table below. A suffix that is not in the table has no variant and
/// no placeholder value — see [`SourceLanguage::of_path`], which answers `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceLanguage {
    Css,
    Go,
    Html,
    JavaScript,
    Json,
    Markdown,
    Php,
    Python,
    Rust,
    Scss,
    TypeScript,
    Volt,
    Vue,
    Yaml,
}

/// Suffix to language, lowercase, and the whole of what this module knows.
///
/// A table rather than a `match` because that is what it is: rows a reader can count, and
/// the thing [`SUFFIX_TABLE_VERSION`] versions. Every row was measured on the tree this
/// program indexes as its policy admits it — 30 166 files — and a language earns a place at
/// a hundred files of it or more:
///
/// | language | suffixes | files |
/// | --- | --- | --- |
/// | PHP | `php` 4 129, `phtml` 61 | 4 190 |
/// | YAML | `yaml` 3 773, `yml` 243 | 4 016 |
/// | Python | `py` | 3 363 |
/// | JavaScript | `js` 3 200, `mjs` 7 | 3 207 |
/// | HTML | `html` | 1 749 |
/// | Vue | `vue` | 1 693 |
/// | TypeScript | `ts` 973, `tsx` 772, `mts` 1 | 1 746 |
/// | Markdown | `md` | 1 675 |
/// | Go | `go` | 629 |
/// | Volt | `volt` | 480 |
/// | CSS | `css` | 446 |
/// | JSON | `json` | 333 |
/// | Rust | `rs` | 224 |
/// | SCSS | `scss` | 112 |
///
/// What was measured and deliberately left out: `sql` (83), `tpl` (73), `sh` (59), `toml`
/// (34), `swift` (19), `mako` (6), `scm` (4), `jsonl` (1) — all under the line — and `c`,
/// `h`, `cpp`, `m` and `pl`, of which this tree holds none at all. A row for a language
/// nobody here writes is a rule no measurement on this machine could ever check.
const SUFFIX_TABLE: [(&str, SourceLanguage); 19] = [
    ("css", SourceLanguage::Css),
    ("go", SourceLanguage::Go),
    ("html", SourceLanguage::Html),
    ("js", SourceLanguage::JavaScript),
    ("json", SourceLanguage::Json),
    ("md", SourceLanguage::Markdown),
    ("mjs", SourceLanguage::JavaScript),
    ("mts", SourceLanguage::TypeScript),
    ("php", SourceLanguage::Php),
    ("phtml", SourceLanguage::Php),
    ("py", SourceLanguage::Python),
    ("rs", SourceLanguage::Rust),
    ("scss", SourceLanguage::Scss),
    ("ts", SourceLanguage::TypeScript),
    ("tsx", SourceLanguage::TypeScript),
    ("volt", SourceLanguage::Volt),
    ("vue", SourceLanguage::Vue),
    ("yaml", SourceLanguage::Yaml),
    ("yml", SourceLanguage::Yaml),
];

impl SourceLanguage {
    /// What this path's suffix names, or nothing.
    ///
    /// `None` is the ordinary answer and it is not a failure: a file with no suffix, a file
    /// whose name begins with a dot and has no other, and a file whose suffix nobody put in
    /// the table all land there. It is deliberately not a variant called `Unknown` — an
    /// index that stores "unknown" cannot later be told apart from an index written before
    /// the row existed, while an absent value simply is not there and says so.
    ///
    /// Only the last suffix is read, so `worker.d.ts` is TypeScript and `archive.tar.gz` is
    /// nothing. Case is folded, because these files are written on a filesystem that does
    /// not distinguish `.SQL` from `.sql` and neither does their author.
    pub fn of_path(path: &NormalizedPath) -> Option<Self> {
        let suffix = path.extension()?;
        SUFFIX_TABLE
            .iter()
            .find(|(named, _)| named.eq_ignore_ascii_case(suffix))
            .map(|(_, language)| *language)
    }

    /// The language the names of one file agree on.
    ///
    /// A file is an inode and may be at several paths at once — a hard link, or a copy the
    /// scan met under two names — and then there is no single suffix to read. A name that
    /// claims nothing is passed over, because `x.py` beside a `README` hard-linked to it is
    /// not a disagreement. Two names claiming *different* languages are, and then the file
    /// is claimed by neither: nothing in either name outranks the other.
    pub fn of_paths<'a>(paths: impl IntoIterator<Item = &'a NormalizedPath>) -> Option<Self> {
        let mut claimed: Option<Self> = None;
        for path in paths {
            let Some(language) = Self::of_path(path) else {
                continue;
            };
            match claimed {
                None => claimed = Some(language),
                Some(already) if already == language => {}
                Some(_) => return None,
            }
        }
        claimed
    }

    /// How the value is written down and looked up again. Stable: a rebuilt index and an
    /// index written file by file must spell the same language the same way, or a query
    /// would reach one and not the other.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Css => "css",
            Self::Go => "go",
            Self::Html => "html",
            Self::JavaScript => "javascript",
            Self::Json => "json",
            Self::Markdown => "markdown",
            Self::Php => "php",
            Self::Python => "python",
            Self::Rust => "rust",
            Self::Scss => "scss",
            Self::TypeScript => "typescript",
            Self::Volt => "volt",
            Self::Vue => "vue",
            Self::Yaml => "yaml",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{SUFFIX_TABLE, SourceLanguage};
    use crate::domain::NormalizedPath;

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    fn language_of(text: &str) -> Option<SourceLanguage> {
        SourceLanguage::of_path(&path(text))
    }

    #[test]
    fn a_suffix_in_the_table_names_its_language() {
        for (text, expected) in [
            ("/tree/app/Controller.php", SourceLanguage::Php),
            ("/tree/deploy/values.yaml", SourceLanguage::Yaml),
            ("/tree/deploy/values.yml", SourceLanguage::Yaml),
            ("/tree/backend/api/routes.py", SourceLanguage::Python),
            ("/tree/web/main.js", SourceLanguage::JavaScript),
            ("/tree/web/App.vue", SourceLanguage::Vue),
            ("/tree/web/index.html", SourceLanguage::Html),
            ("/tree/src/lib.rs", SourceLanguage::Rust),
        ] {
            assert_eq!(language_of(text), Some(expected), "{text}");
        }
    }

    /// The point of the whole type: a suffix nobody wrote a row for is absent, not a value
    /// meaning absent. A reader can tell "this file is not in a language the table knows"
    /// from "this document was written before the field existed" only while the second one
    /// has no spelling of its own.
    #[test]
    fn a_suffix_outside_the_table_names_nothing() {
        for text in [
            "/tree/db/schema.sql",
            "/tree/bin/release.sh",
            "/tree/Cargo.toml",
            "/tree/notes/session.jsonl",
            "/tree/image.png",
        ] {
            assert_eq!(language_of(text), None, "{text}");
        }
    }

    /// A name with no suffix at all, and a name that is nothing but a suffix. `.gitignore`
    /// is the second: its dot opens the name rather than separating a suffix from it, and
    /// reading it as one would put every dotfile in whatever row happened to match.
    #[test]
    fn a_name_without_a_suffix_and_a_name_that_is_only_a_dot_file_name_nothing() {
        for text in [
            "/tree/Makefile",
            "/tree/README",
            "/tree/.gitignore",
            "/tree/.bashrc",
            "/tree/.env",
        ] {
            assert_eq!(language_of(text), None, "{text}");
        }
    }

    /// Only the last suffix is read. `.d.ts` is TypeScript because `.ts` is, and `.tar.gz`
    /// is nothing because `.gz` is nothing — neither is a row of its own.
    #[test]
    fn only_the_last_suffix_of_a_compound_name_is_read() {
        assert_eq!(
            language_of("/tree/types/worker.d.ts"),
            Some(SourceLanguage::TypeScript)
        );
        assert_eq!(
            language_of("/tree/dist/bundle.min.js"),
            Some(SourceLanguage::JavaScript)
        );
        assert_eq!(language_of("/tree/backup/archive.tar.gz"), None);
    }

    /// These files are written on a filesystem that does not tell `.PY` from `.py`.
    #[test]
    fn the_suffix_is_read_without_regard_to_case() {
        assert_eq!(
            language_of("/tree/OLD/Report.PY"),
            Some(SourceLanguage::Python)
        );
        assert_eq!(
            language_of("/tree/OLD/Index.HTML"),
            Some(SourceLanguage::Html)
        );
    }

    /// A file at several paths: names that claim nothing are passed over, and two names
    /// claiming different languages leave the file claimed by neither.
    #[test]
    fn a_file_known_by_several_names_is_claimed_only_where_they_do_not_disagree() {
        let agreeing = [path("/tree/a/worker.py"), path("/tree/b/worker.py")];
        assert_eq!(
            SourceLanguage::of_paths(agreeing.iter()),
            Some(SourceLanguage::Python)
        );

        let silent_partner = [path("/tree/a/worker.py"), path("/tree/b/LICENSE")];
        assert_eq!(
            SourceLanguage::of_paths(silent_partner.iter()),
            Some(SourceLanguage::Python)
        );

        let disagreeing = [path("/tree/a/worker.py"), path("/tree/b/worker.rs")];
        assert_eq!(SourceLanguage::of_paths(disagreeing.iter()), None);

        assert_eq!(SourceLanguage::of_paths(Vec::new()), None);
    }

    /// The spelling is what a query is written against and what a future pass selects by,
    /// so it is pinned here rather than left to whatever `Debug` prints.
    #[test]
    fn the_stored_spelling_is_stable() {
        for (language, stored) in [
            (SourceLanguage::Css, "css"),
            (SourceLanguage::Go, "go"),
            (SourceLanguage::Html, "html"),
            (SourceLanguage::JavaScript, "javascript"),
            (SourceLanguage::Json, "json"),
            (SourceLanguage::Markdown, "markdown"),
            (SourceLanguage::Php, "php"),
            (SourceLanguage::Python, "python"),
            (SourceLanguage::Rust, "rust"),
            (SourceLanguage::Scss, "scss"),
            (SourceLanguage::TypeScript, "typescript"),
            (SourceLanguage::Volt, "volt"),
            (SourceLanguage::Vue, "vue"),
            (SourceLanguage::Yaml, "yaml"),
        ] {
            assert_eq!(language.as_str(), stored, "{language:?}");
        }
    }

    /// The table is data a person extends, so the two mistakes an extension makes are
    /// checked here: a row spelled with its dot, and the same suffix answered twice.
    #[test]
    fn every_row_is_a_bare_lowercase_suffix_named_once() {
        for (suffix, _) in SUFFIX_TABLE {
            assert!(
                !suffix.is_empty() && !suffix.contains('.'),
                "a row holds a suffix without its dot: {suffix:?}"
            );
            assert_eq!(
                suffix.to_ascii_lowercase(),
                suffix,
                "a row is lowercase, because lookup folds case: {suffix:?}"
            );
            assert_eq!(
                SUFFIX_TABLE
                    .iter()
                    .filter(|(named, _)| *named == suffix)
                    .count(),
                1,
                "{suffix:?} is answered by two rows"
            );
        }
    }
}
