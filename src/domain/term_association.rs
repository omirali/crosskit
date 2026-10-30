// What a pair of terms is worth, once you know how often each of them turns up on its own.
//
// Counting how often two terms share a window answers the wrong question. In a corpus of one
// person's working chats the most frequent pairs are the ones where both terms are
// everywhere: they share a window because everything shares a window with them, and a graph
// built on raw counts is a picture of the corpus's common words.
//
// Pointwise mutual information asks the other question — how much more often do these two
// occur together than they would if they were unrelated:
//
//   pmi(x, y) = p(x, y) · log2( p(x, y) / (p(x) · p(y)) )
//
// The leading `p(x, y)` is what makes this the *local* form: a pair seen three times in a
// corpus can have a spectacular ratio and still be noise, and multiplying by its own
// probability puts it back where it belongs. It is the same principle that already gives the
// preposition `в` a BM25 weight of 1.47 against 2.93 for `контрагент`: a term that is
// everywhere has told you nothing by being there.

/// How often terms and pairs of terms occurred, over the windows of one corpus.
///
/// The counts come from outside: this type does the arithmetic and holds no table, because
/// the table is millions of entries and belongs to whoever is building it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowCounts {
    windows: u64,
}

/// A count that cannot be turned into a probability.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociationError {
    /// No windows, so nothing has a probability.
    NoWindows,
    /// A term or pair counted more times than there are windows: a window holds each term
    /// once, so this is a counting mistake and not a weight worth reporting.
    CountAboveWindows { counted: u64, windows: u64 },
    /// A term of the pair never occurred, which makes the pair impossible.
    TermNeverOccurred,
}

impl WindowCounts {
    pub fn over(windows: u64) -> Result<Self, AssociationError> {
        if windows == 0 {
            return Err(AssociationError::NoWindows);
        }
        Ok(Self { windows })
    }

    /// The weight of one pair, from how many windows held each term and how many held both.
    ///
    /// Positive when the two occur together more than chance would have it, negative when
    /// they occur together less, and near zero for a pair of two very common terms.
    pub fn pointwise_mutual_information(
        self,
        left_windows: u64,
        right_windows: u64,
        together_windows: u64,
    ) -> Result<f64, AssociationError> {
        for counted in [left_windows, right_windows, together_windows] {
            if counted > self.windows {
                return Err(AssociationError::CountAboveWindows {
                    counted,
                    windows: self.windows,
                });
            }
        }
        if left_windows == 0 || right_windows == 0 {
            return Err(AssociationError::TermNeverOccurred);
        }
        if together_windows == 0 {
            return Ok(0.0);
        }
        let total = self.windows as f64;
        let joint = together_windows as f64 / total;
        let left = left_windows as f64 / total;
        let right = right_windows as f64 / total;
        Ok(joint * (joint / (left * right)).log2())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rounded to six places, which is more than any decision here rests on and enough for a
    /// number worked out by hand to match.
    fn close(left: f64, right: f64) {
        assert!(
            (left - right).abs() < 1e-6,
            "ожидалось {right}, получено {left}"
        );
    }

    #[test]
    fn a_pair_that_always_occurs_together_is_worth_its_own_probability() {
        // 100 windows, both terms in the same 10 of them. p(x)=p(y)=p(x,y)=0.1, so the ratio
        // is 0.1/0.01 = 10 and the weight is 0.1·log2(10).
        let counts = WindowCounts::over(100).expect("окна");
        let weight = counts
            .pointwise_mutual_information(10, 10, 10)
            .expect("вес пары");
        close(weight, 0.1 * 10f64.log2());
        close(weight, 0.332192809);
    }

    #[test]
    fn two_terms_that_are_everywhere_are_worth_nothing_together() {
        // 100 windows, each term in 50, together in 25 — exactly what independence predicts.
        let counts = WindowCounts::over(100).expect("окна");
        let weight = counts
            .pointwise_mutual_information(50, 50, 25)
            .expect("вес пары");
        close(weight, 0.0);
    }

    #[test]
    fn a_pair_seen_less_than_chance_predicts_weighs_negative() {
        // Each term in half the windows, together in only a tenth: 0.1·log2(0.1/0.25).
        let counts = WindowCounts::over(100).expect("окна");
        let weight = counts
            .pointwise_mutual_information(50, 50, 10)
            .expect("вес пары");
        close(weight, 0.1 * (0.1f64 / 0.25).log2());
        assert!(weight < 0.0);
    }

    /// The claim Stage 31 started from, checked on its own numbers: a term in 55 of 57
    /// conversations paired with one in 12 is worth far less than a pair of two terms that
    /// are each rare and mostly occur together.
    #[test]
    fn a_rare_pair_outweighs_a_pair_with_a_term_that_is_everywhere() {
        let counts = WindowCounts::over(57).expect("окна");
        let common = counts
            .pointwise_mutual_information(55, 12, 12)
            .expect("вес пары");
        let rare = counts
            .pointwise_mutual_information(12, 11, 9)
            .expect("вес пары");
        let times = rare / common;
        assert!(
            (28.0..29.0).contains(&times),
            "пара из редких терминов должна обойти пару с вездесущим термином в 28–29 раз, \
             вышло {times} ({rare} против {common})"
        );
    }

    #[test]
    fn a_pair_that_never_occurred_together_weighs_nothing() {
        let counts = WindowCounts::over(100).expect("окна");
        close(
            counts
                .pointwise_mutual_information(10, 10, 0)
                .expect("вес пары"),
            0.0,
        );
    }

    #[test]
    fn counts_that_cannot_be_are_refused_rather_than_weighed() {
        assert_eq!(WindowCounts::over(0), Err(AssociationError::NoWindows));
        let counts = WindowCounts::over(10).expect("окна");
        assert_eq!(
            counts.pointwise_mutual_information(11, 3, 3),
            Err(AssociationError::CountAboveWindows {
                counted: 11,
                windows: 10
            })
        );
        assert_eq!(
            counts.pointwise_mutual_information(0, 3, 0),
            Err(AssociationError::TermNeverOccurred)
        );
    }
}
