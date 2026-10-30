// Rolling what each directory holds directly up into the volume of every subtree the draft
// will name.
//
// It is kept apart from the walk for one reason: the walk cannot know, while it is offering
// entries, which directories will turn out to be worth naming. A `.git` or a manifest is met
// in the middle of a directory's own listing, after some of its files have already gone by.
// So the walk only tallies each directory, and every subtree total is computed here, from
// the finished tallies — which is also why the answer cannot depend on the order the
// filesystem handed entries back in.
use std::collections::BTreeMap;

use crate::domain::SubtreeVolume;

/// What one directory holds directly: its own entries, not those of its subdirectories.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct DirectoryTally {
    pub(super) files: u64,
    pub(super) bytes: u64,
}

/// The volume of the root, of every candidate and of every child of the root, each covering
/// its whole subtree. Paths are borrowed from the tallies, so nothing is copied to compute
/// them.
pub(super) fn roll_up<'a>(
    root: &'a str,
    directories: &'a BTreeMap<String, DirectoryTally>,
    candidates: impl Iterator<Item = &'a str>,
) -> BTreeMap<&'a str, SubtreeVolume> {
    let mut volumes: BTreeMap<&str, SubtreeVolume> = BTreeMap::new();
    volumes.insert(root, SubtreeVolume::default());
    for candidate in candidates {
        volumes.entry(candidate).or_default();
    }
    for key in directories.keys().map(String::as_str) {
        if let Some(child) = top_level_child(key, root) {
            volumes.entry(child).or_default();
        }
    }

    for (key, tally) in directories {
        for scope in scopes_of(key, root) {
            let Some(volume) = volumes.get_mut(scope) else {
                continue;
            };
            *volume = SubtreeVolume::new(
                volume.files() + tally.files,
                volume.bytes() + tally.bytes,
                volume
                    .deepest_levels()
                    .max(segment_count(key) + 1 - segment_count(scope)),
            );
        }
    }
    volumes
}

/// The directory a path lies in, as a slice of the path itself.
pub(super) fn parent_of(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(cut) => &path[..cut],
        None => path,
    }
}

/// The child of `root` this path lies in, or nothing when the path is the root itself.
fn top_level_child<'a>(path: &'a str, root: &str) -> Option<&'a str> {
    scopes_of(path, root).get(1).copied()
}

/// Every directory from `root` down to `path` itself, as slices of `path`. A path outside
/// `root` has no scopes here rather than a cut at `root`'s width: that width is measured on
/// another string, and on a root named in Cyrillic it can fall inside a character. The walk
/// only ever offers paths within its root, so this answers the caller's question either way.
fn scopes_of<'a>(path: &'a str, root: &str) -> Vec<&'a str> {
    if !path.starts_with(root) {
        return Vec::new();
    }
    let root_length = root.len();
    let mut scopes = vec![&path[..root_length]];
    let mut index = root_length;
    while let Some(offset) = path[index..].find('/') {
        let cut = index + offset;
        if cut > root_length {
            scopes.push(&path[..cut]);
        }
        index = cut + 1;
    }
    if path.len() > root_length {
        scopes.push(path);
    }
    scopes
}

/// How many segments a normalized path has. The filesystem root has none.
fn segment_count(path: &str) -> u32 {
    if path == "/" {
        return 0;
    }
    u32::try_from(path.matches('/').count()).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{DirectoryTally, parent_of, roll_up, scopes_of, segment_count, top_level_child};
    use std::collections::BTreeMap;

    fn tallies(entries: &[(&str, u64, u64)]) -> BTreeMap<String, DirectoryTally> {
        entries
            .iter()
            .map(|(path, files, bytes)| {
                (
                    (*path).to_string(),
                    DirectoryTally {
                        files: *files,
                        bytes: *bytes,
                    },
                )
            })
            .collect()
    }

    #[test]
    fn a_subtree_counts_everything_below_it_and_the_root_counts_all_of_it() {
        let directories = tallies(&[
            ("/r", 1, 10),
            ("/r/a", 2, 20),
            ("/r/a/deep/deeper", 4, 40),
            ("/r/b", 8, 80),
        ]);

        let volumes = roll_up("/r", &directories, ["/r/a"].into_iter());

        let volume = |path: &str| {
            let volume = volumes[path];
            (volume.files(), volume.bytes(), volume.deepest_levels())
        };
        assert_eq!(volume("/r"), (15, 150, 4));
        assert_eq!(volume("/r/a"), (6, 60, 3));
        assert_eq!(volume("/r/b"), (8, 80, 1));
    }

    /// A sibling whose name merely starts like a subtree's is not part of it, although it
    /// sorts between the subtree and its children.
    #[test]
    fn a_name_that_only_begins_like_a_subtree_is_not_inside_it() {
        let directories = tallies(&[("/r/a", 1, 10), ("/r/a-old", 2, 20), ("/r/a/inner", 4, 40)]);

        let volumes = roll_up("/r", &directories, ["/r/a"].into_iter());

        assert_eq!(volumes["/r/a"].files(), 5);
        assert_eq!(volumes["/r/a-old"].files(), 2);
        assert_eq!(volumes["/r"].files(), 7);
    }

    /// The root's width is measured on the root and then used to cut a different string.
    /// The walk only offers paths under its root, so this never happens by itself — but the
    /// same shape, a byte index taken from one string and applied to another, is what cut a
    /// chunk inside a character, and here it would land inside a Cyrillic letter: `/x` is
    /// two bytes and the `ы` of `/ыз` covers the second of them.
    #[test]
    fn a_path_outside_the_root_has_no_scopes_rather_than_a_cut_at_the_root_s_width() {
        assert!(scopes_of("/ыз", "/x").is_empty());
        assert_eq!(top_level_child("/ыз", "/x"), None);
    }

    #[test]
    fn paths_are_cut_the_same_way_whether_the_root_is_the_filesystem_root_or_not() {
        assert_eq!(scopes_of("/a/b/c", "/"), ["/", "/a", "/a/b", "/a/b/c"]);
        assert_eq!(scopes_of("/r/a/b", "/r"), ["/r", "/r/a", "/r/a/b"]);
        assert_eq!(scopes_of("/r", "/r"), ["/r"]);
        assert_eq!(top_level_child("/r/a/b", "/r"), Some("/r/a"));
        assert_eq!(top_level_child("/r", "/r"), None);
        assert_eq!(parent_of("/r/a/b"), "/r/a");
        assert_eq!(parent_of("/a"), "/");
        assert_eq!(segment_count("/"), 0);
        assert_eq!(segment_count("/r/a"), 2);
    }
}
