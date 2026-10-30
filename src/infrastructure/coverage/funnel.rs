// The funnel of one root: every entry the tree holds, what the policy did with it, what
// reading it produced, and what the graph kept.
//
// Nothing here walks, reads or queries. It is the arithmetic of the audit, kept apart from
// the measurement so that the one claim the audit makes about itself — that every loss is a
// named class and not a subtraction — is a property a test can check.
use std::collections::BTreeMap;

use crate::infrastructure::ladybug::coverage_queries::GraphCoverage;

/// What the policy did with every non-directory entry of one root.
///
/// The classes are exclusive and exhaustive by construction: each entry is recorded exactly
/// once, and [`PolicySieve::accounted`] is their sum. That is what makes the funnel's first
/// arrow a set of named reasons rather than a remainder.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PolicySieve {
    unrepresentable_names: usize,
    excluded_by_rule: BTreeMap<String, usize>,
    excluded_by_default: usize,
    under_generated_directory: usize,
    generated_directories: usize,
    metadata_only: usize,
    admitted: usize,
}

impl PolicySieve {
    /// An entry whose name is not valid UTF-8. The policy works on strings, so such an
    /// entry is never stat-ed, opened or named — only counted.
    pub(super) fn record_unrepresentable_name(&mut self) {
        self.unrepresentable_names += 1;
    }

    /// An entry an Exclude rule covers, credited to the rule's own pattern.
    pub(super) fn record_excluded_by(&mut self, pattern: &str) {
        *self
            .excluded_by_rule
            .entry(pattern.to_string())
            .or_default() += 1;
    }

    /// An entry no rule covers at all. The policy excludes it by default, and that default
    /// is a reason of its own: nothing in the configuration names these files.
    pub(super) fn record_excluded_by_default(&mut self) {
        self.excluded_by_default += 1;
    }

    /// A directory that said, by the marker in it, that a tool made it.
    pub(super) fn record_generated_directory(&mut self) {
        self.generated_directories += 1;
    }

    pub(super) fn record_under_generated_directory(&mut self) {
        self.under_generated_directory += 1;
    }

    pub(super) fn record_metadata_only(&mut self) {
        self.metadata_only += 1;
    }

    pub(super) fn record_admitted(&mut self) {
        self.admitted += 1;
    }

    /// Every entry this sieve saw. Equal to the number of non-directory entries on disk
    /// under the root, because every one of them lands in exactly one class.
    pub(super) fn accounted(&self) -> usize {
        self.unrepresentable_names
            + self.excluded_by_rule.values().sum::<usize>()
            + self.excluded_by_default
            + self.under_generated_directory
            + self.metadata_only
            + self.admitted
    }

    /// Files the policy allows into the graph: the ones whose content it reads and the ones
    /// it limits to metadata. Both become a `File` node.
    pub(super) fn reaches_the_graph(&self) -> usize {
        self.admitted + self.metadata_only
    }

    pub(super) fn metadata_only(&self) -> usize {
        self.metadata_only
    }

    pub(super) fn unrepresentable_names(&self) -> usize {
        self.unrepresentable_names
    }

    pub(super) fn excluded_by_default(&self) -> usize {
        self.excluded_by_default
    }

    pub(super) fn under_generated_directory(&self) -> usize {
        self.under_generated_directory
    }

    pub(super) fn generated_directories(&self) -> usize {
        self.generated_directories
    }

    /// Every Exclude rule that took something out of this root, with how much, largest
    /// first. A rule that took nothing is not listed: the audit reports what happened, not
    /// what could have.
    pub(super) fn excluded_by_rule(&self) -> Vec<(&str, usize)> {
        let mut rules: Vec<(&str, usize)> = self
            .excluded_by_rule
            .iter()
            .map(|(pattern, count)| (pattern.as_str(), *count))
            .collect();
        rules.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(right.0)));
        rules
    }
}

/// What became of the content of every admitted file.
///
/// None of these classes keeps a file out of the graph — a binary file, an oversized file
/// and a symlink are all published by their metadata. They decide how many revisions and
/// chunks the root produces, which is the funnel's third arrow and not its second.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct ContentOutcome {
    not_a_regular_file: usize,
    over_the_content_limit: usize,
    binary: usize,
    not_utf8: usize,
    changed_while_reading: usize,
    held_no_text: usize,
    read_into_chunks: usize,
    chunks: usize,
    unreadable: usize,
    bytes_read: u64,
}

impl ContentOutcome {
    pub(super) fn record_not_a_regular_file(&mut self) {
        self.not_a_regular_file += 1;
    }

    pub(super) fn record_over_the_content_limit(&mut self) {
        self.over_the_content_limit += 1;
    }

    pub(super) fn record_binary(&mut self) {
        self.binary += 1;
    }

    pub(super) fn record_not_utf8(&mut self) {
        self.not_utf8 += 1;
    }

    pub(super) fn record_changed_while_reading(&mut self) {
        self.changed_while_reading += 1;
    }

    pub(super) fn record_held_no_text(&mut self, bytes: u64) {
        self.held_no_text += 1;
        self.bytes_read += bytes;
    }

    pub(super) fn record_read(&mut self, chunks: usize, bytes: u64) {
        self.read_into_chunks += 1;
        self.chunks += chunks;
        self.bytes_read += bytes;
    }

    /// A file the walk offered and could not describe or open at all. A pass that meets one
    /// of these is incomplete and retires nothing.
    pub(super) fn record_unreadable(&mut self) {
        self.unreadable += 1;
    }

    /// Every admitted file this outcome saw, in exactly one class.
    pub(super) fn accounted(&self) -> usize {
        self.not_a_regular_file
            + self.over_the_content_limit
            + self.binary
            + self.not_utf8
            + self.changed_while_reading
            + self.held_no_text
            + self.read_into_chunks
            + self.unreadable
    }

    /// Admitted files that produced a revision with text in it. This is what the graph
    /// should hold as a current version with chunks.
    pub(super) fn revisions_with_text(&self) -> usize {
        self.read_into_chunks
    }

    pub(super) fn chunks(&self) -> usize {
        self.chunks
    }

    pub(super) fn bytes_read(&self) -> u64 {
        self.bytes_read
    }

    pub(super) fn not_a_regular_file(&self) -> usize {
        self.not_a_regular_file
    }

    pub(super) fn over_the_content_limit(&self) -> usize {
        self.over_the_content_limit
    }

    pub(super) fn binary(&self) -> usize {
        self.binary
    }

    pub(super) fn not_utf8(&self) -> usize {
        self.not_utf8
    }

    pub(super) fn changed_while_reading(&self) -> usize {
        self.changed_while_reading
    }

    pub(super) fn held_no_text(&self) -> usize {
        self.held_no_text
    }

    pub(super) fn unreadable(&self) -> usize {
        self.unreadable
    }
}

/// One root, from the entries on disk to the documents of the search index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct RootFunnel {
    context: String,
    root: String,
    sieve: PolicySieve,
    content: ContentOutcome,
    graph: GraphCoverage,
}

impl RootFunnel {
    pub(super) fn new(
        context: &str,
        root: &str,
        sieve: PolicySieve,
        content: ContentOutcome,
        graph: GraphCoverage,
    ) -> Self {
        Self {
            context: context.to_string(),
            root: root.to_string(),
            sieve,
            content,
            graph,
        }
    }

    pub(super) fn context(&self) -> &str {
        &self.context
    }

    pub(super) fn root(&self) -> &str {
        &self.root
    }

    /// Non-directory entries under the root, counted now rather than taken from anything
    /// written earlier.
    pub(super) fn entries_on_disk(&self) -> usize {
        self.sieve.accounted()
    }

    pub(super) fn sieve(&self) -> &PolicySieve {
        &self.sieve
    }

    pub(super) fn content(&self) -> &ContentOutcome {
        &self.content
    }

    pub(super) fn graph(&self) -> &GraphCoverage {
        &self.graph
    }

    /// The gap the audit exists to name: files the policy would let into the graph, minus
    /// the files the graph actually holds under this root. Positive means the tree has
    /// files nothing ever indexed.
    pub(super) fn files_the_graph_is_missing(&self) -> i64 {
        self.sieve.reaches_the_graph() as i64 - self.graph.files() as i64
    }
}

#[cfg(test)]
mod tests {
    use super::{ContentOutcome, PolicySieve};

    /// The claim the whole audit rests on: an entry is never lost between classes, so the
    /// funnel's first step is a sum of named reasons and never a subtraction.
    #[test]
    fn every_entry_the_sieve_saw_lands_in_exactly_one_class() {
        let mut sieve = PolicySieve::default();
        sieve.record_unrepresentable_name();
        sieve.record_excluded_by("**/.git/**");
        sieve.record_excluded_by("**/.git/**");
        sieve.record_excluded_by("**/node_modules/**");
        sieve.record_excluded_by_default();
        sieve.record_under_generated_directory();
        sieve.record_metadata_only();
        sieve.record_admitted();
        sieve.record_admitted();

        assert_eq!(sieve.accounted(), 9);
        assert_eq!(sieve.reaches_the_graph(), 3);
    }

    /// A marked directory is counted, but it is not an entry: the funnel counts files, and
    /// counting the directory too would make the sum larger than the tree.
    #[test]
    fn a_generated_directory_is_reported_apart_from_the_entries_under_it() {
        let mut sieve = PolicySieve::default();
        sieve.record_generated_directory();
        sieve.record_under_generated_directory();
        sieve.record_under_generated_directory();

        assert_eq!(sieve.generated_directories(), 1);
        assert_eq!(sieve.accounted(), 2);
    }

    /// Rules are reported largest first, and a tie is broken by the pattern so that two
    /// runs of the audit print the same table.
    #[test]
    fn exclusion_rules_are_reported_by_what_they_took_and_then_by_name() {
        let mut sieve = PolicySieve::default();
        for _ in 0..3 {
            sieve.record_excluded_by("**/node_modules/**");
        }
        sieve.record_excluded_by("**/target/**");
        sieve.record_excluded_by("**/.git/**");

        assert_eq!(
            sieve.excluded_by_rule(),
            vec![
                ("**/node_modules/**", 3),
                ("**/.git/**", 1),
                ("**/target/**", 1),
            ]
        );
    }

    #[test]
    fn every_admitted_file_lands_in_exactly_one_content_class() {
        let mut content = ContentOutcome::default();
        content.record_not_a_regular_file();
        content.record_over_the_content_limit();
        content.record_binary();
        content.record_not_utf8();
        content.record_changed_while_reading();
        content.record_held_no_text(0);
        content.record_read(4, 1_024);
        content.record_read(2, 512);
        content.record_unreadable();

        assert_eq!(content.accounted(), 9);
        assert_eq!(content.revisions_with_text(), 2);
        assert_eq!(content.chunks(), 6);
        assert_eq!(content.bytes_read(), 1_536);
    }
}
