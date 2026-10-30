// The co-occurrence graph of one corpus, at one window size and one threshold.
//
// Building it is three passes and no cleverness: cut every reply into windows, count how many
// windows held each term and each pair of terms, then weigh every pair by pointwise mutual
// information. What it costs — how many windows, how many edges, how long — is the
// measurement, so the counters are part of the result and not a debug print.
use std::collections::HashMap;
use std::time::{Duration, Instant};

use super::term_index::TermIndex;
use crate::domain::term_association::WindowCounts;
use crate::domain::term_window::{TermId, WindowShape, windows_of};

/// A weighted association graph over one corpus.
#[derive(Debug)]
pub(crate) struct AssociationGraph {
    shape: WindowShape,
    least_conversations: u32,
    vocabulary: usize,
    windows: u64,
    /// The pairs the same corpus would hold if the unit were the whole reply rather than a
    /// window — the comparison that says what the window is buying.
    pairs_if_the_reply_were_the_unit: u64,
    /// How many windows held each term, by term identifier. A `Vec` and not a map: the
    /// vocabulary is tens of thousands and this is read once per pair.
    windows_with: Vec<u32>,
    weights: HashMap<(u32, u32), f64>,
    built_in: Duration,
}

impl AssociationGraph {
    /// Builds the graph for one window shape and one threshold.
    pub(crate) fn build(index: &TermIndex, shape: WindowShape, least_conversations: u32) -> Self {
        let started = Instant::now();
        let mut windows = 0u64;
        let mut whole_reply_pairs = 0u64;
        let mut windows_with = vec![0u32; index.vocabulary().len()];
        let mut together: HashMap<(u32, u32), u32> = HashMap::new();
        let mut admitted: Vec<TermId> = Vec::new();
        let mut distinct: Vec<TermId> = Vec::new();

        for conversation in index.conversations() {
            for (_, sequence) in conversation.replies() {
                admitted.clear();
                admitted.extend(
                    sequence
                        .iter()
                        .copied()
                        .filter(|term| index.used_in_at_least(*term, least_conversations)),
                );
                distinct.clear();
                distinct.extend_from_slice(&admitted);
                distinct.sort_unstable();
                distinct.dedup();
                let held = distinct.len() as u64;
                whole_reply_pairs += held * held.saturating_sub(1) / 2;

                windows_of(&admitted, shape, |window| {
                    windows += 1;
                    for (position, left) in window.iter().enumerate() {
                        windows_with[left.index() as usize] += 1;
                        for right in &window[position + 1..] {
                            let pair = if left.index() < right.index() {
                                (left.index(), right.index())
                            } else {
                                (right.index(), left.index())
                            };
                            *together.entry(pair).or_default() += 1;
                        }
                    }
                });
            }
        }

        // A corpus with no window at all is a corpus with no graph; the weights stay empty
        // rather than the run ending, because "no windows" is a result the report prints.
        let weights = match WindowCounts::over(windows) {
            Ok(counts) => weigh(&counts, &windows_with, &together),
            Err(_) => HashMap::new(),
        };

        Self {
            shape,
            least_conversations,
            vocabulary: index.vocabulary_at(least_conversations),
            windows,
            pairs_if_the_reply_were_the_unit: whole_reply_pairs,
            windows_with,
            weights,
            built_in: started.elapsed(),
        }
    }

    pub(crate) fn shape(&self) -> WindowShape {
        self.shape
    }

    pub(crate) fn least_conversations(&self) -> u32 {
        self.least_conversations
    }

    pub(crate) fn vocabulary(&self) -> usize {
        self.vocabulary
    }

    pub(crate) fn windows(&self) -> u64 {
        self.windows
    }

    pub(crate) fn edges(&self) -> usize {
        self.weights.len()
    }

    pub(crate) fn pairs_if_the_reply_were_the_unit(&self) -> u64 {
        self.pairs_if_the_reply_were_the_unit
    }

    pub(crate) fn built_in(&self) -> Duration {
        self.built_in
    }

    /// What the tables of this graph hold, counted rather than measured: the process peak is
    /// one number for the whole sweep, and this is the part of it one combination is.
    pub(crate) fn held_bytes(&self) -> u64 {
        let per_edge = (std::mem::size_of::<(u32, u32)>() + std::mem::size_of::<f64>()) as u64;
        self.weights.len() as u64 * per_edge
            + self.windows_with.len() as u64 * std::mem::size_of::<u32>() as u64
    }

    /// For each term asked about, its `most` strongest neighbours, strongest first.
    ///
    /// One pass over every edge for the whole set, because an adjacency list would be a second
    /// copy of a graph of millions of edges and this is asked once per run of the questions.
    /// Only positive weights: a pair that occurs together *less* than chance predicts is
    /// evidence against the two belonging together, and expanding a question by it would be
    /// adding a term because it is unrelated.
    pub(crate) fn strongest_neighbours_of(
        &self,
        terms: &[TermId],
        most: usize,
    ) -> HashMap<TermId, Vec<(TermId, f64)>> {
        let mut found: HashMap<TermId, Vec<(TermId, f64)>> =
            terms.iter().map(|term| (*term, Vec::new())).collect();
        for ((left, right), weight) in &self.weights {
            if *weight <= 0.0 {
                continue;
            }
            if let Some(neighbours) = found.get_mut(&TermId::new(*left)) {
                neighbours.push((TermId::new(*right), *weight));
            }
            if let Some(neighbours) = found.get_mut(&TermId::new(*right)) {
                neighbours.push((TermId::new(*left), *weight));
            }
        }
        for neighbours in found.values_mut() {
            neighbours.sort_by(|left, right| {
                right
                    .1
                    .partial_cmp(&left.1)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| left.0.cmp(&right.0))
            });
            neighbours.truncate(most);
        }
        found
    }
}

/// Every pair's weight, from the counts of the corpus.
fn weigh(
    counts: &WindowCounts,
    windows_with: &[u32],
    together: &HashMap<(u32, u32), u32>,
) -> HashMap<(u32, u32), f64> {
    let mut weights = HashMap::with_capacity(together.len());
    for ((left, right), both) in together {
        let left_windows = windows_with
            .get(*left as usize)
            .copied()
            .unwrap_or_default();
        let right_windows = windows_with
            .get(*right as usize)
            .copied()
            .unwrap_or_default();
        // A count that cannot be turned into a probability is a counting mistake, not a
        // weight: it is left out rather than written down as zero, which would be a claim.
        if let Ok(weight) = counts.pointwise_mutual_information(
            u64::from(left_windows),
            u64::from(right_windows),
            u64::from(*both),
        ) {
            weights.insert((*left, *right), weight);
        }
    }
    weights
}
