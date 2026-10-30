// The unit inside which two terms count as related.
//
// Co-occurrence has one parameter that decides everything, and it is not the weighting: it
// is how big the unit of text is. Two terms in the same 4 KB chunk are barely evidence of
// anything, because a chunk of a transcript holds a pasted log and half a file. Stage 31
// measured the same corpus three ways and the unit moved the edge count by an order of
// magnitude while the vocabulary barely moved.
//
// So the unit is a window of a fixed number of terms, cut through a reply, with a ceiling on
// how many distinct terms one window may contribute. The ceiling is not tidiness: without
// it, the top 1% of replies — pasted logs and dumps — produce 44.7% of all edges, and the
// graph is a picture of what somebody pasted rather than of what they talked about.
use std::fmt;

/// A term, as a position in the vocabulary of one measurement.
///
/// An integer rather than a string: a window is compared against every other term in it, a
/// corpus is millions of windows, and nothing here needs to know what the word was.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TermId(u32);

impl TermId {
    pub fn new(index: u32) -> Self {
        Self(index)
    }

    pub fn index(self) -> u32 {
        self.0
    }
}

/// How a reply is cut into windows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowShape {
    length: usize,
    ceiling: usize,
}

/// A window shape that could not be one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowShapeError {
    /// A window of no terms holds no pairs.
    LengthIsZero,
    /// A ceiling of one term admits no pair, so the whole graph would be empty.
    CeilingBelowTwo,
}

impl fmt::Display for WindowShapeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::LengthIsZero => write!(formatter, "окно длиной 0 терминов не содержит пар"),
            Self::CeilingBelowTwo => {
                write!(
                    formatter,
                    "потолок меньше 2 терминов не допускает ни одной пары"
                )
            }
        }
    }
}

impl WindowShape {
    /// `length` terms per window, at most `ceiling` distinct terms contributed by one.
    pub fn new(length: usize, ceiling: usize) -> Result<Self, WindowShapeError> {
        if length == 0 {
            return Err(WindowShapeError::LengthIsZero);
        }
        if ceiling < 2 {
            return Err(WindowShapeError::CeilingBelowTwo);
        }
        Ok(Self { length, ceiling })
    }

    pub fn length(self) -> usize {
        self.length
    }

    pub fn ceiling(self) -> usize {
        self.ceiling
    }

    /// How many pairs one full window can contribute at most. The ceiling is what makes this
    /// a constant rather than a function of how long a pasted log was.
    pub fn most_pairs_per_window(self) -> usize {
        let held = self.length.min(self.ceiling);
        held * held.saturating_sub(1) / 2
    }
}

/// Cuts `terms` into windows and hands each one's distinct terms to `window`.
///
/// Windows do not overlap: an overlapping stride would count the same pair once per step and
/// make a term's weight depend on where in the reply it happened to fall. A window repeats no
/// term, because a pair of a term with itself is not a relation, and a term said twice inside
/// one window is still one occurrence of it there. A window holding more distinct terms than
/// the ceiling is handed over in consecutive pieces of at most that many.
pub fn windows_of(terms: &[TermId], shape: WindowShape, mut window: impl FnMut(&[TermId])) {
    let mut distinct: Vec<TermId> = Vec::with_capacity(shape.length.min(shape.ceiling));
    for slice in terms.chunks(shape.length) {
        distinct.clear();
        for term in slice {
            if distinct.contains(term) {
                continue;
            }
            distinct.push(*term);
            if distinct.len() == shape.ceiling {
                window(&distinct);
                distinct.clear();
            }
        }
        if distinct.len() >= 2 {
            window(&distinct);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(values: &[u32]) -> Vec<TermId> {
        values.iter().copied().map(TermId::new).collect()
    }

    fn cut(values: &[u32], length: usize, ceiling: usize) -> Vec<Vec<u32>> {
        let shape = WindowShape::new(length, ceiling).expect("форма окна");
        let mut cut = Vec::new();
        windows_of(&ids(values), shape, |window| {
            cut.push(window.iter().copied().map(TermId::index).collect())
        });
        cut
    }

    #[test]
    fn a_reply_is_cut_into_windows_that_do_not_overlap() {
        assert_eq!(cut(&[1, 2, 3, 4, 5], 2, 20), vec![vec![1, 2], vec![3, 4]]);
    }

    #[test]
    fn a_term_repeated_inside_one_window_is_held_once() {
        assert_eq!(cut(&[7, 7, 7, 8], 4, 20), vec![vec![7, 8]]);
        // One term said four times is one term, so the window holds no pair at all.
        assert!(cut(&[7, 7, 7, 7], 4, 20).is_empty());
    }

    #[test]
    fn a_window_wider_than_the_ceiling_is_cut_further() {
        assert_eq!(
            cut(&[1, 2, 3, 4, 5, 6], 6, 2),
            vec![vec![1, 2], vec![3, 4], vec![5, 6]]
        );
    }

    #[test]
    fn a_window_left_with_one_term_contributes_nothing() {
        assert!(cut(&[1], 4, 20).is_empty());
        assert_eq!(cut(&[1, 2, 3], 2, 20), vec![vec![1, 2]]);
    }

    #[test]
    fn the_ceiling_bounds_what_one_window_can_cost() {
        let shape = WindowShape::new(80, 20).expect("форма окна");
        assert_eq!(shape.most_pairs_per_window(), 190);
        let shape = WindowShape::new(20, 80).expect("форма окна");
        assert_eq!(shape.most_pairs_per_window(), 190);
    }

    #[test]
    fn a_shape_that_admits_no_pair_is_refused() {
        assert_eq!(WindowShape::new(0, 20), Err(WindowShapeError::LengthIsZero));
        assert_eq!(
            WindowShape::new(20, 1),
            Err(WindowShapeError::CeilingBelowTwo)
        );
    }
}
