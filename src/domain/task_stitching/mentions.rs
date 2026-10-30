// What a piece of text says about files and trackers: the paths it names, the files a patch in
// it changes, the tracker links it carries — and the key two paths are compared by.
//
// A path is compared by its last two components after it is made absolute. Two sessions name
// one file in different ways — `@notes/handoff.md` from the project directory, the absolute
// path from a tool — and the parent directory plus the name is what both spellings keep. One
// component alone is not a key: `handoff.md` exists in many projects.
use std::path::{Component, Path, PathBuf};

/// Files every session reads or is given whatever it works on. A session that names one of
/// them says nothing about which task it belongs to.
const INSTRUCTION_FILES: [&str; 4] = ["CLAUDE.md", "AGENTS.md", "MEMORY.md", ".gitignore"];

/// Extensions a bare file name is accepted with when it carries no directory.
const FILE_EXTENSIONS: [&str; 16] = [
    "md", "txt", "sql", "py", "rs", "php", "ts", "tsx", "js", "json", "yaml", "yml", "toml",
    "xlsx", "csv", "env",
];

/// Prefixes of a tracker task link. Only the link to one task is a signal, so a prefix names
/// the part of the address under which tasks live.
const TRACKER_PREFIXES: [&str; 4] = [
    "https://app.asana.com/",
    "https://linear.app/",
    "https://github.com/",
    "https://gitlab.com/",
];

/// The patch markers of `apply_patch` that name a file the patch changes.
#[cfg(test)]
const PATCH_MARKERS: [&str; 3] = ["*** Add File: ", "*** Update File: ", "*** Delete File: "];

/// Every path-like token in `text`, as written: a token with a directory separator, or a bare
/// name with a known extension. Addresses with a scheme are links, not paths.
pub(crate) fn path_mentions(text: &str) -> Vec<String> {
    tokens(text)
        .filter(|token| !token.contains("://"))
        .map(|token| token.trim_start_matches('@'))
        .filter(|token| looks_like_a_path(token))
        .map(str::to_string)
        .collect()
}

/// Every file a patch inside `text` adds, updates or deletes. The patch may sit inside a quoted
/// string, where its line breaks are the two characters `\n`.
#[cfg(test)]
pub(crate) fn patched_files(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    for marker in PATCH_MARKERS {
        for (start, _) in text.match_indices(marker) {
            let rest = &text[start + marker.len()..];
            let end = ["\\n", "\n", "\""]
                .iter()
                .filter_map(|stop| rest.find(stop))
                .min()
                .unwrap_or(rest.len());
            let path = rest[..end].trim();
            if !path.is_empty() {
                found.push(path.to_string());
            }
        }
    }
    found
}

/// Every tracker task link in `text`, without its query and fragment.
pub(crate) fn tracker_links(text: &str) -> Vec<String> {
    tokens(text)
        .filter(|token| {
            TRACKER_PREFIXES
                .iter()
                .any(|prefix| token.starts_with(prefix))
        })
        .map(|token| {
            let end = token.find(['?', '#']).unwrap_or(token.len());
            token[..end].trim_end_matches('/').to_string()
        })
        .filter(|link| is_a_task_link(link))
        .collect()
}

/// The key `path` is compared by: its last two components after it is made absolute against
/// `working_directory` and `home`. Nothing for an instruction file, or for a bare name with no
/// directory to resolve it against. A relative path of two components or more needs no
/// directory: its last two are already the key.
pub(crate) fn file_key(path: &str, working_directory: Option<&str>, home: &str) -> Option<String> {
    let absolute = if let Some(rest) = path.strip_prefix("~/") {
        Path::new(home).join(rest)
    } else if Path::new(path).is_absolute() {
        PathBuf::from(path)
    } else if let Some(directory) = working_directory {
        Path::new(directory).join(path)
    } else {
        PathBuf::from(path)
    };
    let mut names: Vec<&str> = Vec::new();
    for component in absolute.components() {
        match component {
            Component::Normal(name) => names.push(name.to_str()?),
            Component::ParentDir => {
                names.pop();
            }
            _ => {}
        }
    }
    let name = *names.last()?;
    if INSTRUCTION_FILES.contains(&name) || names.len() < 2 {
        return None;
    }
    Some(format!("{}/{name}", names[names.len() - 2]))
}

fn tokens(text: &str) -> impl Iterator<Item = &str> {
    text.split(|c: char| c.is_whitespace() || "\"'`()<>[]{},;|".contains(c))
        .map(|token| token.trim_end_matches(['.', ':', '!', '?']))
        .filter(|token| !token.is_empty())
}

fn looks_like_a_path(token: &str) -> bool {
    let name = token.rsplit('/').next().unwrap_or(token);
    let has_extension = name
        .rsplit_once('.')
        .is_some_and(|(stem, extension)| !stem.is_empty() && FILE_EXTENSIONS.contains(&extension));
    if token.contains('/') {
        return token.len() > 3 && !name.is_empty();
    }
    has_extension
}

/// A link names one task when it goes deeper than the tracker's top level: a project page of a
/// tracker is shared by every task in it.
fn is_a_task_link(link: &str) -> bool {
    let path = link.split_once("://").map_or(link, |(_, rest)| rest);
    path.split('/').filter(|part| !part.is_empty()).count() >= 4
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOME: &str = "/Users/alice";

    #[test]
    fn a_path_is_found_with_or_without_a_directory_and_without_its_punctuation() {
        let text = "Прочитай @notes/handoff.md и потом plan.sql, а ещё /tmp/run/out. Не README";
        assert_eq!(
            path_mentions(text),
            ["notes/handoff.md", "plan.sql", "/tmp/run/out"]
        );
    }

    #[test]
    fn a_link_is_not_a_path() {
        assert!(path_mentions("см. https://example.com/a/b.md").is_empty());
    }

    #[test]
    fn a_patch_inside_a_quoted_string_names_its_files() {
        let text = r#"const patch = "*** Begin Patch\n*** Add File: /w/payments-api/plan.md\n+x\n*** Update File: src/cart.rs\n""#;
        assert_eq!(
            patched_files(text),
            ["/w/payments-api/plan.md", "src/cart.rs"]
        );
    }

    #[test]
    fn a_tracker_link_keeps_the_task_and_drops_the_query() {
        let text = "задача https://app.asana.com/1/42/project/7/task/9?focus=true, и всё";
        assert_eq!(
            tracker_links(text),
            ["https://app.asana.com/1/42/project/7/task/9"]
        );
        assert!(tracker_links("https://app.asana.com/").is_empty());
    }

    #[test]
    fn two_spellings_of_one_file_share_a_key() {
        let relative = file_key("notes/handoff.md", Some("/w/payments-api"), HOME);
        let absolute = file_key("/w/payments-api/notes/handoff.md", None, HOME);
        assert_eq!(relative.as_deref(), Some("notes/handoff.md"));
        assert_eq!(relative, absolute);
        assert_eq!(
            file_key("~/w/payments-api/handoff.md", None, HOME).as_deref(),
            Some("payments-api/handoff.md")
        );
    }

    #[test]
    fn a_file_named_without_a_place_to_resolve_it_or_an_instruction_file_has_no_key() {
        assert_eq!(file_key("handoff.md", None, HOME), None);
        assert_eq!(
            file_key("notes/handoff.md", None, HOME).as_deref(),
            Some("notes/handoff.md")
        );
        assert_eq!(file_key("/w/payments-api/CLAUDE.md", None, HOME), None);
        assert_eq!(
            file_key("handoff.md", Some("/w/payments-api"), HOME).as_deref(),
            Some("payments-api/handoff.md")
        );
    }
}
