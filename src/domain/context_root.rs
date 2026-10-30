// What a context answers when a command that walks asks it where to start.
//
// A context is a list of `include` patterns, and a pass is a walk of one directory. The two
// are not the same thing: a pattern of literal segments names a directory, and a pattern
// with a glob in it covers many and names none. Until Stage 35 nothing ever had to ask,
// because the directory always came from the command line; now the context is asked, and
// the difference has to be a type rather than an assumption.
use super::index_policy::PathPattern;

/// One `include` of a context, as a command that has to walk it sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextRoot {
    /// A directory a pass can start from.
    Directory(String),
    /// A pattern that covers files without naming a directory. A pass cannot start here,
    /// and the pattern is carried so it can be named: a root that is skipped silently is
    /// how eight roots went unindexed for fifteen stages.
    Mask(String),
}

impl ContextRoot {
    pub fn of(pattern: &PathPattern) -> Self {
        match pattern.as_directory() {
            Some(directory) => Self::Directory(directory),
            None => Self::Mask(pattern.as_str().to_string()),
        }
    }

    /// The directory to walk, or nothing when this include names none.
    pub fn directory(&self) -> Option<&str> {
        match self {
            Self::Directory(path) => Some(path),
            Self::Mask(_) => None,
        }
    }

    /// How this include is written, whichever kind it is. Every report names the include as
    /// the configuration spells it, so a person can find the line to change.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Directory(path) => path,
            Self::Mask(pattern) => pattern,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ContextRoot;
    use crate::domain::PathPattern;

    fn root_of(pattern: &str) -> ContextRoot {
        ContextRoot::of(&PathPattern::parse(pattern).unwrap())
    }

    #[test]
    fn an_include_of_literal_segments_is_a_directory_a_pass_can_start_from() {
        assert_eq!(
            root_of("/Users/alice/Projects/thing"),
            ContextRoot::Directory("/Users/alice/Projects/thing".to_string())
        );
        assert_eq!(
            root_of("/Users/alice/Projects/thing/").directory(),
            Some("/Users/alice/Projects/thing")
        );
    }

    /// The whole point of the type: a glob covers many directories, and the longest literal
    /// part of it is a different pattern covering different files.
    #[test]
    fn an_include_with_a_glob_in_it_names_no_directory_and_is_carried_as_a_mask() {
        assert_eq!(
            root_of("/tree/*/src"),
            ContextRoot::Mask("/tree/*/src".to_string())
        );
        assert_eq!(root_of("/tree/*/src").directory(), None);
        assert_eq!(root_of("**/notes").directory(), None);
        assert_eq!(root_of("/tree/**").directory(), None);
    }

    /// Whichever kind it is, it is named the way the configuration writes it, so the line
    /// to change can be found.
    #[test]
    fn every_include_can_be_named() {
        assert_eq!(root_of("/tree/work").as_str(), "/tree/work");
        assert_eq!(root_of("/tree/*/src").as_str(), "/tree/*/src");
    }
}
