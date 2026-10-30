// Every node of a map checked against the reply it names: the reply must exist in the window
// the model was given, and the quote must occur in it by the knowledge layer's rule — runs of
// whitespace forgiven, nothing else (ADR 0048).
use serde_json::Value;

use super::speech::Reply;
use crate::domain::knowledge::quote_occurs_in;

/// What became of the nodes of one answer.
#[derive(Debug, Default, Clone, Copy)]
pub(super) struct Tally {
    pub(super) forks: usize,
    pub(super) branches: usize,
    pub(super) risks: usize,
    pub(super) proved: usize,
    pub(super) no_such_reply: usize,
    pub(super) quote_not_found: usize,
}

impl Tally {
    pub(super) fn add(&mut self, other: Tally) {
        self.forks += other.forks;
        self.branches += other.branches;
        self.risks += other.risks;
        self.proved += other.proved;
        self.no_such_reply += other.no_such_reply;
        self.quote_not_found += other.quote_not_found;
    }

    pub(super) fn nodes(&self) -> usize {
        self.forks + self.branches + self.risks
    }
}

/// Checks every node of `answer` against `window`, marks each with `"proved"`, and counts.
pub(super) fn checked(answer: &mut Value, window: &[&Reply]) -> Tally {
    let mut tally = Tally::default();
    let check = |node: &mut Value, tally: &mut Tally| {
        let reply = node["reply"].as_u64().and_then(|n| usize::try_from(n).ok());
        let quote = node["quote"].as_str().unwrap_or_default().to_string();
        let found = reply.and_then(|number| window.iter().find(|r| r.number == number));
        let proved = match found {
            None => {
                tally.no_such_reply += 1;
                false
            }
            Some(reply) if quote_occurs_in(&quote, &reply.text) => {
                tally.proved += 1;
                true
            }
            Some(_) => {
                tally.quote_not_found += 1;
                false
            }
        };
        node["proved"] = Value::Bool(proved);
    };
    for fork in answer["forks"].as_array_mut().into_iter().flatten() {
        tally.forks += 1;
        check(fork, &mut tally);
        for branch in fork["branches"].as_array_mut().into_iter().flatten() {
            tally.branches += 1;
            check(branch, &mut tally);
        }
    }
    for risk in answer["risks"].as_array_mut().into_iter().flatten() {
        tally.risks += 1;
        check(risk, &mut tally);
    }
    tally
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::domain::conversation::Speaker;

    #[test]
    fn a_node_is_proved_only_by_a_quote_from_the_reply_it_names() {
        let replies = [
            Reply {
                number: 1,
                speaker: Speaker::Owner,
                text: "Почему скидка   считается дважды?".into(),
            },
            Reply {
                number: 2,
                speaker: Speaker::Model,
                text: "Убираю расчёт из cart.rs, оставляю в order.rs.".into(),
            },
        ];
        let window: Vec<&Reply> = replies.iter().collect();
        let mut answer = json!({
            "forks": [{
                "question": "Где считать скидку?", "reply": 1, "quote": "скидка считается дважды",
                "branches": [
                    {"option": "в order.rs", "state": "chosen", "reason": "", "reply": 2, "quote": "оставляю в order.rs"},
                    {"option": "в cart.rs", "state": "rejected", "reason": "", "reply": 2, "quote": "оставляю в cart.rs"}
                ]
            }],
            "risks": [{"description": "", "level": "info", "status": "open", "in_task": true, "reply": 9, "quote": "дважды"}]
        });
        let tally = checked(&mut answer, &window);
        assert_eq!((tally.forks, tally.branches, tally.risks), (1, 2, 1));
        assert_eq!(
            (tally.proved, tally.quote_not_found, tally.no_such_reply),
            (2, 1, 1)
        );
        assert_eq!(answer["forks"][0]["branches"][1]["proved"], false);
    }
}
