// One root walked the way a pass walks it, counting instead of writing.
//
// The census answers the first two arrows of the funnel — what the policy did with every
// entry, and what reading the admitted ones produced — and it answers them with the
// product's own parts: the descriptor-relative walker, `IndexPolicy::decide`,
// `generated_directory_marker`, `ExtractionProfile` and `SourceEntry::read_evidence`. A
// census that applied rules of its own would measure a program nobody runs, which is the
// mistake Stage 31's corpus walk made and Stage 33's verdict named.
//
// The one deliberate difference from a pass: an excluded directory is still descended
// into. A pass skips it, and that is right for a pass; an audit that skipped it could not
// say how many files the exclusion took, and "files on disk" would be a number copied from
// the configuration instead of one counted now.
use std::collections::BTreeSet;

use crate::application::{
    Admission, ContentSkipReason, EvidenceRead, IndexError, SourceEntry, SourceVisitor,
    SourceWalker,
};
use crate::domain::{
    ExtractionProfile, FileKind, IndexPolicy, NormalizedPath, PolicyClass, PolicyRule,
    generated_directory_marker,
};

use super::funnel::{ContentOutcome, PolicySieve};

/// The identity every described entry is described under. Nothing is published, so the
/// identity is never resolved against the graph and never stored; it exists because
/// `FileLocation` cannot be built without one.
const CENSUS_FILE_ID: &str = "file:coverage-audit";

/// How many failing paths are kept for the report. A failure means the walk cannot vouch
/// for a subtree, so the first few say what kind of thing is failing and the count says how
/// much of it there is.
const FAILURES_KEPT: usize = 10;

pub(super) struct TreeCensus<'a> {
    /// The policy with this root granted Include, exactly as `initial_scan` and `reconcile`
    /// build it.
    policy: &'a IndexPolicy,
    /// Exclude rules the walk learned from the tree: one per directory carrying a marker.
    /// A pass adds these to its policy for everything downstream; the census keeps them
    /// apart because "a tool made this directory" and "a rule in the configuration names
    /// it" are two different answers to the owner's question.
    generated: Vec<PolicyRule>,
    sieve: PolicySieve,
    content: ContentOutcome,
    /// Every path the policy would let into the graph. Kept so the funnel's gap can be
    /// turned into a list of files with a reason each, rather than a difference of two
    /// totals.
    admitted_paths: BTreeSet<String>,
    failures: Vec<String>,
    failure_count: usize,
}

impl<'a> TreeCensus<'a> {
    pub(super) fn new(policy: &'a IndexPolicy) -> Self {
        Self {
            policy,
            generated: Vec::new(),
            sieve: PolicySieve::default(),
            content: ContentOutcome::default(),
            admitted_paths: BTreeSet::new(),
            failures: Vec::new(),
            failure_count: 0,
        }
    }

    fn is_under_generated(&self, path: &NormalizedPath) -> bool {
        self.generated
            .iter()
            .any(|rule| rule.pattern().matches(path))
    }

    fn note_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        self.failure_count += 1;
        if self.failures.len() < FAILURES_KEPT {
            self.failures.push(format!("{path}: {error}"));
        }
    }

    /// Reads the content of one admitted file and credits the outcome. The file is read
    /// through the same call a pass reads it through, so "binary" and "not UTF-8" mean here
    /// what they mean in the index.
    fn read_content_of(&mut self, entry: &dyn SourceEntry) {
        let location = match entry.location(CENSUS_FILE_ID) {
            Ok(location) => location,
            Err(error) => {
                self.content.record_unreadable();
                let path = entry.path().clone();
                self.note_failure(&path, error);
                return;
            }
        };
        if location.kind() != FileKind::RegularFile {
            self.content.record_not_a_regular_file();
            return;
        }
        let limit = self.policy.max_content_bytes();
        if !ExtractionProfile::of_path(location.path()).may_read(location.size_bytes(), limit) {
            self.content.record_over_the_content_limit();
            return;
        }

        match entry.read_evidence(&location, limit) {
            Ok(EvidenceRead::Commit(evidence)) => match evidence.chunks().len() {
                0 => self.content.record_held_no_text(location.size_bytes()),
                chunks => self.content.record_read(chunks, location.size_bytes()),
            },
            Ok(EvidenceRead::Skipped(ContentSkipReason::Binary)) => self.content.record_binary(),
            Ok(EvidenceRead::Skipped(ContentSkipReason::NotUtf8)) => self.content.record_not_utf8(),
            Ok(EvidenceRead::Skipped(ContentSkipReason::TooLarge)) => {
                self.content.record_over_the_content_limit();
            }
            Ok(EvidenceRead::Skipped(ContentSkipReason::ChangedWhileReading)) => {
                self.content.record_changed_while_reading();
            }
            Err(error) => {
                self.content.record_unreadable();
                let path = entry.path().clone();
                self.note_failure(&path, error);
            }
        }
    }

    fn into_report(self) -> TreeReport {
        TreeReport {
            sieve: self.sieve,
            content: self.content,
            admitted_paths: self.admitted_paths,
            failures: self.failures,
            failure_count: self.failure_count,
        }
    }
}

impl SourceVisitor for TreeCensus<'_> {
    /// Nothing is refused. A pass answers `Skip` here and stops looking; the census looks,
    /// and records in `visit_entry` what the pass would have answered.
    fn admit(&mut self, _path: &NormalizedPath) -> Admission {
        Admission::Inspect
    }

    /// The marker is asked for exactly where a pass asks for it, and only of a directory a
    /// pass would have reached: one already inside a marked directory, or one an Exclude
    /// rule covers, is never opened by a pass and so never gets to declare itself
    /// generated.
    fn admit_directory(
        &mut self,
        path: &NormalizedPath,
        holds: &dyn Fn(&str) -> bool,
    ) -> Admission {
        let reachable = !self.is_under_generated(path)
            && self.policy.decide(path).class() != PolicyClass::Exclude;
        if reachable && generated_directory_marker(holds).is_some() {
            self.sieve.record_generated_directory();
            self.generated
                .push(IndexPolicy::generated_directory_rule(path));
        }
        Admission::Inspect
    }

    fn visit_entry(&mut self, entry: &dyn SourceEntry) -> Result<(), IndexError> {
        let path = entry.path();
        if self.is_under_generated(path) {
            self.sieve.record_under_generated_directory();
            return Ok(());
        }

        let decision = self.policy.decide(path);
        match decision.class() {
            PolicyClass::Exclude => {
                match decision.rule() {
                    Some(rule) => self.sieve.record_excluded_by(rule.pattern().as_str()),
                    None => self.sieve.record_excluded_by_default(),
                }
                return Ok(());
            }
            PolicyClass::Metadata => {
                self.sieve.record_metadata_only();
                self.admitted_paths.insert(path.as_str().to_string());
                return Ok(());
            }
            PolicyClass::Include => {
                self.sieve.record_admitted();
                self.admitted_paths.insert(path.as_str().to_string());
            }
        }

        self.read_content_of(entry);
        Ok(())
    }

    fn skip_unrepresentable_name(&mut self) {
        self.sieve.record_unrepresentable_name();
    }

    fn record_failure(&mut self, path: &NormalizedPath, error: IndexError) {
        let path = path.clone();
        self.note_failure(&path, error);
    }
}

/// What one root's tree holds, counted now.
pub(super) struct TreeReport {
    pub(super) sieve: PolicySieve,
    pub(super) content: ContentOutcome,
    pub(super) admitted_paths: BTreeSet<String>,
    pub(super) failures: Vec<String>,
    pub(super) failure_count: usize,
}

/// Walks one already resolved root under the configured policy and counts what a pass
/// would have done with every entry. Writes nothing and opens no database.
pub(super) fn census_of_root(
    walker: &impl SourceWalker,
    configured: &IndexPolicy,
    root: &NormalizedPath,
) -> Result<TreeReport, IndexError> {
    let policy = configured.with_scan_root(root);
    let mut census = TreeCensus::new(&policy);
    walker.walk(root, &mut census)?;
    Ok(census.into_report())
}
