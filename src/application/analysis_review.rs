// What waits for the owner to decide, and what one decision covers.
//
// Stage 23 stopped queueing jobs for revisions no rule covers. What is left is a question
// rather than a queue: which kinds of thing on this machine has nobody said anything about?
// The answer has to be answerable in a sitting, so it arrives grouped — by extension and by
// directory — with what each group would cost. A list of twenty-four thousand revisions is
// not a decision anybody makes.
//
// Nothing here holds the revisions. The inventory is read in bounded pages and folded into
// counters as it goes, because the whole point of this stage is that the number is large.
use std::collections::BTreeMap;

use crate::domain::{AnalysisRules, NormalizedPath};

use super::repository::RepositoryError;

/// How many revisions one page of the inventory carries. Large enough that a tree of tens
/// of thousands is a few hundred statements, small enough that a page is never the tree.
const PAGE: usize = 1_000;

/// One current revision and where its file is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InventoriedRevision {
    revision_id: String,
    path: NormalizedPath,
}

impl InventoriedRevision {
    pub fn new(revision_id: impl Into<String>, path: NormalizedPath) -> Self {
        Self {
            revision_id: revision_id.into(),
            path,
        }
    }

    pub fn revision_id(&self) -> &str {
        &self.revision_id
    }

    pub fn path(&self) -> &NormalizedPath {
        &self.path
    }
}

/// The graph's list of what exists now, read a page at a time.
pub trait RevisionInventory {
    /// Current revisions with the path their file is present at, ordered by revision id and
    /// starting strictly after `after`. An empty `after` starts at the beginning, and a
    /// page shorter than `limit` is the last one.
    fn current_revision_page(
        &self,
        after: &str,
        limit: usize,
    ) -> Result<Vec<InventoriedRevision>, RepositoryError>;

    /// Queues the outbox entry of every named revision that has none, and answers how many
    /// were queued. Each call is one transaction, so a backfill interrupted halfway leaves
    /// whole pages rather than half a page.
    fn queue_insight_jobs(&self, revision_ids: &[String]) -> Result<usize, RepositoryError>;
}

/// One group of revisions nobody has decided about, and the mask that would cover it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReviewGroup {
    mask: String,
    revisions: usize,
}

impl ReviewGroup {
    pub fn mask(&self) -> &str {
        &self.mask
    }

    pub fn revisions(&self) -> usize {
        self.revisions
    }
}

/// What waits for a decision, grouped two ways because a person decides both ways: "all
/// YAML, wherever it is" and "everything in this project".
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AnalysisReview {
    covered: usize,
    waiting: usize,
    by_extension: Vec<ReviewGroup>,
    by_directory: Vec<ReviewGroup>,
}

impl AnalysisReview {
    /// Revisions an automatic or approved rule already covers.
    pub fn covered(&self) -> usize {
        self.covered
    }

    /// Revisions no rule covers. These have no job and never had one.
    pub fn waiting(&self) -> usize {
        self.waiting
    }

    /// The waiting revisions by the extension of their path, largest group first.
    pub fn by_extension(&self) -> &[ReviewGroup] {
        &self.by_extension
    }

    /// And by the directory they lie in, largest first. A directory here is the one the
    /// file is in, not every ancestor: a person approving `**/docs/**` writes that mask,
    /// and offering them every level of every path would be the list this replaces.
    pub fn by_directory(&self) -> &[ReviewGroup] {
        &self.by_directory
    }
}

/// Reads the inventory and folds it into the review.
pub struct AnalysisReviewService;

impl AnalysisReviewService {
    /// What waits for a decision, counting only paths this run may see at all.
    pub fn execute(
        &self,
        inventory: &dyn RevisionInventory,
        rules: &AnalysisRules,
        is_visible: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<AnalysisReview, RepositoryError> {
        let mut review = AnalysisReview::default();
        let mut extensions: BTreeMap<String, usize> = BTreeMap::new();
        let mut directories: BTreeMap<String, usize> = BTreeMap::new();

        for_each_revision(inventory, |revision| {
            if !is_visible(revision.path()) {
                return;
            }
            if rules.cover(revision.path()) {
                review.covered += 1;
                return;
            }
            review.waiting += 1;
            *extensions
                .entry(extension_mask(revision.path()))
                .or_default() += 1;
            *directories
                .entry(directory_mask(revision.path()))
                .or_default() += 1;
        })?;

        review.by_extension = largest_first(extensions);
        review.by_directory = largest_first(directories);
        Ok(review)
    }

    /// Queues the jobs a newly approved mask asks for, in pages, and answers how many.
    ///
    /// After the fact and never in bulk: the revisions already exist, so approving a mask
    /// has to reach back over them. Each page is one transaction, which is what makes an
    /// interrupted approval a partial backfill that the next one finishes rather than a
    /// half-written one.
    pub fn backfill(
        &self,
        inventory: &dyn RevisionInventory,
        covers: &dyn Fn(&NormalizedPath) -> bool,
    ) -> Result<usize, RepositoryError> {
        let mut queued = 0;
        let mut after = String::new();
        loop {
            let page = inventory.current_revision_page(&after, PAGE)?;
            let Some(last) = page.last() else {
                return Ok(queued);
            };
            after = last.revision_id().to_string();
            let short = page.len() < PAGE;
            // Only the ids of one page are ever held, and they are written before the next
            // page is read.
            let wanted: Vec<String> = page
                .iter()
                .filter(|revision| covers(revision.path()))
                .map(|revision| revision.revision_id().to_string())
                .collect();
            queued += inventory.queue_insight_jobs(&wanted)?;
            if short {
                return Ok(queued);
            }
        }
    }
}

/// Walks the whole inventory in pages, handing each revision to `see` and keeping none.
fn for_each_revision(
    inventory: &dyn RevisionInventory,
    mut see: impl FnMut(&InventoriedRevision),
) -> Result<(), RepositoryError> {
    let mut after = String::new();
    loop {
        let page = inventory.current_revision_page(&after, PAGE)?;
        let Some(last) = page.last() else {
            return Ok(());
        };
        after = last.revision_id().to_string();
        let short = page.len() < PAGE;
        for revision in &page {
            see(revision);
        }
        if short {
            return Ok(());
        }
    }
}

/// The mask that would cover every file of this kind, wherever it is.
///
/// Where the name ends is [`NormalizedPath::extension`]'s answer, shared with the other
/// question asked of a suffix — which language the name declares, in
/// [`crate::domain::SourceLanguage`]. The cut is one rule and lives in one place; what the
/// two do with it differs, and deliberately. A mask is matched against paths as they are
/// written, so the suffix goes into it with the case its author gave it, while the language
/// table folds case: `**/*.PY` and `**/*.py` are two masks and one language.
fn extension_mask(path: &NormalizedPath) -> String {
    match path.extension() {
        Some(extension) => format!("**/*.{extension}"),
        // A file with no extension cannot be named by one, so its own directory is the
        // smallest honest mask for it.
        None => directory_mask(path),
    }
}

/// The mask that would cover this file's directory and everything under it.
fn directory_mask(path: &NormalizedPath) -> String {
    match path.parent() {
        Some(parent) => format!("{}/**", parent.as_str().trim_end_matches('/')),
        None => "/**".to_string(),
    }
}

/// Largest group first, and ties by mask so two runs of one tree print one order.
fn largest_first(counted: BTreeMap<String, usize>) -> Vec<ReviewGroup> {
    let mut groups: Vec<ReviewGroup> = counted
        .into_iter()
        .map(|(mask, revisions)| ReviewGroup { mask, revisions })
        .collect();
    groups.sort_by(|left, right| {
        right
            .revisions
            .cmp(&left.revisions)
            .then_with(|| left.mask.cmp(&right.mask))
    });
    groups
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::{
        AnalysisReviewService, InventoriedRevision, PAGE, RepositoryError, RevisionInventory,
    };
    use crate::domain::{AnalysisRules, NormalizedPath, PathPattern};

    fn path(text: &str) -> NormalizedPath {
        NormalizedPath::from_absolute(text).unwrap()
    }

    fn rules(masks: &[&str]) -> AnalysisRules {
        AnalysisRules::new(
            masks
                .iter()
                .map(|mask| PathPattern::parse(mask).unwrap())
                .collect(),
            Vec::new(),
        )
    }

    /// An inventory of `count` revisions that records the size of every page it was asked
    /// for, so a caller that read the whole thing at once is visible as one page.
    struct CountingInventory {
        count: usize,
        pages: RefCell<Vec<usize>>,
        queued: RefCell<Vec<String>>,
    }

    impl CountingInventory {
        fn of(count: usize) -> Self {
            Self {
                count,
                pages: RefCell::new(Vec::new()),
                queued: RefCell::new(Vec::new()),
            }
        }

        fn revision(&self, ordinal: usize) -> InventoriedRevision {
            let suffix = match ordinal % 4 {
                0 => "md",
                _ => "py",
            };
            InventoriedRevision::new(
                format!("revision:{ordinal:08}"),
                path(&format!("/tree/part{}/file{ordinal}.{suffix}", ordinal % 3)),
            )
        }
    }

    impl RevisionInventory for CountingInventory {
        fn current_revision_page(
            &self,
            after: &str,
            limit: usize,
        ) -> Result<Vec<InventoriedRevision>, RepositoryError> {
            self.pages.borrow_mut().push(limit);
            let first = match after.is_empty() {
                true => 0,
                false => after
                    .rsplit(':')
                    .next()
                    .and_then(|ordinal| ordinal.parse::<usize>().ok())
                    .map_or(0, |ordinal| ordinal + 1),
            };
            Ok((first..self.count.min(first + limit))
                .map(|ordinal| self.revision(ordinal))
                .collect())
        }

        fn queue_insight_jobs(&self, revision_ids: &[String]) -> Result<usize, RepositoryError> {
            self.queued
                .borrow_mut()
                .extend(revision_ids.iter().cloned());
            Ok(revision_ids.len())
        }
    }

    /// The review of a tree larger than one page reads it in pages and never asks for more
    /// than one at a time. This is the property the whole stage rests on: the number of
    /// revisions waiting is large by design, and a review that held them would be the cost
    /// this stage exists to avoid, paid in memory instead of in a subscription.
    #[test]
    fn a_tree_larger_than_one_page_is_read_in_pages_and_never_held() {
        let inventory = CountingInventory::of(PAGE * 3 + 7);

        let review = AnalysisReviewService
            .execute(&inventory, &rules(&["**/*.md"]), &|_| true)
            .unwrap();

        assert_eq!(review.covered() + review.waiting(), PAGE * 3 + 7);
        let pages = inventory.pages.borrow();
        assert!(pages.len() >= 4, "read in pages: {pages:?}");
        assert!(
            pages.iter().all(|asked| *asked == PAGE),
            "no page may be larger than the bound: {pages:?}"
        );
    }

    /// The groups are what a decision is made from: one line per kind, largest first, and
    /// the counts add up to what waits.
    #[test]
    fn what_waits_is_grouped_by_kind_and_by_directory() {
        let inventory = CountingInventory::of(12);

        let review = AnalysisReviewService
            .execute(&inventory, &rules(&["**/*.md"]), &|_| true)
            .unwrap();

        assert_eq!(review.covered(), 3);
        assert_eq!(review.waiting(), 9);
        assert_eq!(review.by_extension()[0].mask(), "**/*.py");
        assert_eq!(review.by_extension()[0].revisions(), 9);
        assert_eq!(
            review
                .by_directory()
                .iter()
                .map(|g| g.revisions())
                .sum::<usize>(),
            9
        );
    }

    /// A path this run may not see is not on anybody's list: the review counts what this
    /// context holds, not what the graph remembers from another one.
    #[test]
    fn a_path_this_run_may_not_see_is_neither_covered_nor_waiting() {
        let inventory = CountingInventory::of(12);

        let review = AnalysisReviewService
            .execute(&inventory, &rules(&["**/*.md"]), &|path| {
                path.as_str().contains("/part0/")
            })
            .unwrap();

        assert!(review.covered() + review.waiting() < 12);
    }

    /// The backfill writes page by page, so an approval interrupted halfway leaves whole
    /// pages queued rather than one half-written batch.
    #[test]
    fn the_backfill_queues_page_by_page_rather_than_all_at_once() {
        let inventory = CountingInventory::of(PAGE * 2 + 5);

        let queued = AnalysisReviewService
            .backfill(&inventory, &|path| path.as_str().ends_with(".py"))
            .unwrap();

        assert_eq!(queued, inventory.queued.borrow().len());
        assert!(queued > 0);
        assert!(
            inventory.pages.borrow().len() >= 3,
            "written as it was read: {:?}",
            inventory.pages.borrow()
        );
    }
}
