// Ranking the conversations of a corpus against one question.
//
// A question becomes a list of weighted terms and nothing else. Its own words carry weight 1;
// a term the association graph put next to one of them carries a share of that, so the graph
// enters the ranking in exactly one place and can be turned off by handing over the question's
// own terms alone. That is what makes the two columns of the report comparable: they differ by
// the expansion and by nothing else.
use std::cmp::Ordering;

use super::chat_corpus::ChatCorpus;
use super::term_index::TermIndex;
use crate::domain::conversation_rank::ScoringRule;
use crate::domain::term_window::TermId;

/// How far down the page a measurement looks: as many results as a person reads at once, and
/// the same five Stage 29 counted precision@5 over.
pub(crate) const PAGE: usize = 5;

/// One conversation as an answer to one question.
#[derive(Debug, Clone)]
pub(crate) struct Ranked {
    path: String,
    name: String,
    score: f64,
}

impl Ranked {
    pub(crate) fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    pub(crate) fn score(&self) -> f64 {
        self.score
    }
}

/// The corpus and its terms, ready to be asked.
pub(crate) struct Ranking<'a> {
    corpus: &'a ChatCorpus,
    index: &'a TermIndex,
}

impl<'a> Ranking<'a> {
    pub(crate) fn over(corpus: &'a ChatCorpus, index: &'a TermIndex) -> Self {
        Self { corpus, index }
    }

    /// Every conversation that scored anything for one weighted question, best first.
    ///
    /// The whole ranking and not a page of it: a corpus of tens of conversations costs nothing
    /// to hold, and the count of what ties for first place cannot be read off a page that was
    /// already cut. A conversation that scores nothing is left out rather than ranked last —
    /// a result with no term of the question in it is not an answer, and a page padded with
    /// them makes "the answer was in the five" mean nothing.
    pub(crate) fn ranked(&self, asked: &[(TermId, f64)], rule: ScoringRule) -> Vec<Ranked> {
        let mut scored: Vec<Ranked> = Vec::with_capacity(self.corpus.transcripts().len());
        for (transcript, conversation) in self
            .corpus
            .transcripts()
            .iter()
            .zip(self.index.conversations())
        {
            let mut score = 0.0;
            for (term, share) in asked {
                let presence = conversation.presence_of(*term);
                if presence.is_absent() {
                    continue;
                }
                score += share * rule.score(presence);
            }
            if score <= 0.0 {
                continue;
            }
            scored.push(Ranked {
                path: transcript.path().to_string(),
                name: transcript
                    .conversation()
                    .name()
                    .map(|name| name.value().to_string())
                    .unwrap_or_else(|| transcript.file_name().to_string()),
                score,
            });
        }
        // Equal scores keep one order across runs, so a table of numbers is a table of the
        // same numbers tomorrow.
        scored.sort_by(|left, right| {
            right
                .score
                .partial_cmp(&left.score)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.path.cmp(&right.path))
        });
        scored
    }
}

/// One column of numbers over the whole question set.
#[derive(Debug, Clone, Default)]
pub(crate) struct Column {
    places: Vec<Option<usize>>,
    hits_in_page: Vec<usize>,
    tied_at_the_top: usize,
    firsts_that_are_not_luck: usize,
}

impl Column {
    /// Records where the expected answer came in one ranking.
    ///
    /// Two things are counted about first place, not one. A weighting under which many
    /// conversations score the same puts the answer first by the order paths happen to sort
    /// in, so `tied_at_the_top` says how often that happened at all, and
    /// `firsts_that_are_not_luck` counts only the questions where no wrong conversation shared
    /// the top score — the precision that survives any tie-break whatsoever.
    pub(crate) fn add(&mut self, ranked: &[Ranked], answers: &[String]) {
        let correct = |found: &Ranked| answers.iter().any(|answer| answer == found.path());
        if let [first, second, ..] = ranked
            && first.score() == second.score()
        {
            self.tied_at_the_top += 1;
        }
        if let [first, rest @ ..] = ranked
            && correct(first)
            && !rest
                .iter()
                .any(|found| found.score() == first.score() && !correct(found))
        {
            self.firsts_that_are_not_luck += 1;
        }
        let page = &ranked[..ranked.len().min(PAGE)];
        self.places
            .push(page.iter().position(correct).map(|index| index + 1));
        self.hits_in_page
            .push(page.iter().filter(|found| correct(found)).count());
    }

    /// How many questions had something else sharing the top score.
    ///
    /// An upper bound on how often a tie-break could have mattered, and **not** a count of
    /// luck: this also grows when the top score is shared by two conversations that are both
    /// right, which the first question of the set is by construction. What luck was worth is
    /// the gap between `precision_at_one` and `precision_at_one_outright`.
    pub(crate) fn tied_at_the_top(&self) -> usize {
        self.tied_at_the_top
    }

    /// precision@1 counted only where the answer outscored every wrong conversation outright.
    /// This is the number that holds whatever the tie-break is, and it is the one a decision
    /// should rest on.
    pub(crate) fn precision_at_one_outright(&self) -> f64 {
        if self.places.is_empty() {
            return 0.0;
        }
        self.firsts_that_are_not_luck as f64 / self.places.len() as f64
    }

    pub(crate) fn place(&self, question: usize) -> Option<usize> {
        self.places.get(question).copied().flatten()
    }

    pub(crate) fn precision_at_one(&self) -> f64 {
        self.mean(|place| usize::from(place == Some(1)) as f64)
    }

    pub(crate) fn precision_at_five(&self) -> f64 {
        if self.hits_in_page.is_empty() {
            return 0.0;
        }
        self.hits_in_page
            .iter()
            .map(|hits| *hits as f64 / PAGE as f64)
            .sum::<f64>()
            / self.hits_in_page.len() as f64
    }

    /// How many questions had their answer somewhere in the page.
    pub(crate) fn in_page(&self) -> usize {
        self.places.iter().filter(|place| place.is_some()).count()
    }

    pub(crate) fn questions(&self) -> usize {
        self.places.len()
    }

    fn mean(&self, of: impl Fn(Option<usize>) -> f64) -> f64 {
        if self.places.is_empty() {
            return 0.0;
        }
        self.places.iter().copied().map(of).sum::<f64>() / self.places.len() as f64
    }
}
