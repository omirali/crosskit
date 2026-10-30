// The owner's marks on the list: which conversations of the project belong to the seed's task.
//
// The list is one file the owner keeps: the measurement writes it, the owner fills the last
// column, and the next run reads the marks back before it writes the list again, so a rerun
// keeps them. The marks are the truth the measurement is scored against.
use std::collections::BTreeMap;
use std::path::Path;

/// What the owner said of one conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mark {
    InTask,
    NotInTask,
}

impl Mark {
    fn parse(cell: &str) -> Option<Self> {
        match cell.trim().to_lowercase().as_str() {
            "да" | "yes" | "+" => Some(Self::InTask),
            "нет" | "no" | "-" => Some(Self::NotInTask),
            _ => None,
        }
    }

    pub(super) fn as_str(self) -> &'static str {
        match self {
            Self::InTask => "да",
            Self::NotInTask => "нет",
        }
    }
}

/// The marks in the list at `path`, by session id. A list that is not there yet has none.
///
/// A row is read by its last two cells: the session id and the mark. Every other cell is the
/// measurement's own and may change between runs.
pub(super) fn marks_in(path: &Path) -> BTreeMap<String, Mark> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return BTreeMap::new();
    };
    text.lines()
        .filter_map(|line| {
            let cells: Vec<&str> = line.trim().trim_matches('|').split('|').collect();
            let [.., id, mark] = cells.as_slice() else {
                return None;
            };
            Some((id.trim().to_string(), Mark::parse(mark)?))
        })
        .collect()
}

/// How a set of conversations stitching put in the task fares against the owner's marks.
#[derive(Debug, Default, PartialEq, Eq)]
pub(super) struct Score {
    /// Found and marked as the task's.
    pub(super) right: usize,
    /// Found and marked as another task's.
    pub(super) wrong: usize,
    /// Found and not marked.
    pub(super) unmarked: usize,
    /// Marked as the task's and not found.
    pub(super) missed: usize,
}

impl Score {
    pub(super) fn of(found: &[&str], marks: &BTreeMap<String, Mark>) -> Self {
        let mut score = Self::default();
        for id in found {
            match marks.get(*id) {
                Some(Mark::InTask) => score.right += 1,
                Some(Mark::NotInTask) => score.wrong += 1,
                None => score.unmarked += 1,
            }
        }
        score.missed = marks
            .iter()
            .filter(|(id, mark)| **mark == Mark::InTask && !found.contains(&id.as_str()))
            .count();
        score
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mark_is_read_from_the_last_cell_and_its_id_from_the_one_before() {
        let path = std::env::temp_dir().join(format!("stitch-marks-{}.md", std::process::id()));
        std::fs::write(
            &path,
            "| начало | имя | id | в задаче? |\n| --- | --- | --- | --- |\n\
             | 1 | Двойная скидка | a1 | да |\n| 2 | Отчёт | b2 | нет |\n| 3 | Возвраты | c3 | |\n",
        )
        .expect("a temporary file is writable");
        let marks = marks_in(&path);
        std::fs::remove_file(&path).expect("the temporary file is removable");
        assert_eq!(marks.get("a1"), Some(&Mark::InTask));
        assert_eq!(marks.get("b2"), Some(&Mark::NotInTask));
        assert_eq!(marks.len(), 2);
    }

    #[test]
    fn a_score_counts_right_wrong_unmarked_and_missed() {
        let marks: BTreeMap<String, Mark> = [
            ("a".to_string(), Mark::InTask),
            ("b".to_string(), Mark::NotInTask),
            ("c".to_string(), Mark::InTask),
        ]
        .into();
        assert_eq!(
            Score::of(&["a", "b", "d"], &marks),
            Score {
                right: 1,
                wrong: 1,
                unmarked: 1,
                missed: 1
            }
        );
    }
}
