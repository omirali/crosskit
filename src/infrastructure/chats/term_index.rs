// The corpus with its words turned into terms, once, so every parameter of the measurement
// is swept over the same reading of the same text.
//
// Taking the corpus through the analyzer costs seconds; sweeping nine combinations of window
// size and threshold over it costs nothing if the terms are already there. So the text is
// read once into sequences of term identifiers and the text itself is let go.
//
// Two things are kept per conversation, and they answer two different questions. The
// *sequences* say what followed what, which is the only thing a window can be cut from. The
// *presence* says, per term, in how many distinct replies of each party it occurred — which
// is what the ranking counts, and it is deliberately not a count of occurrences.
use std::collections::HashMap;

use super::chat_corpus::ChatCorpus;
use crate::domain::conversation::Speaker;
use crate::domain::conversation_rank::TermPresence;
use crate::domain::term_window::TermId;
use crate::infrastructure::search::term_stream::TermReader;

/// Every distinct term of the corpus, and the identifier each was given.
#[derive(Debug, Default)]
pub(crate) struct Vocabulary {
    keys: Vec<String>,
    identifiers: HashMap<String, TermId>,
}

impl Vocabulary {
    fn intern(&mut self, key: &str) -> TermId {
        if let Some(known) = self.identifiers.get(key) {
            return *known;
        }
        let id = TermId::new(
            u32::try_from(self.keys.len()).expect("словарь корпуса не превышает 4 млрд терминов"),
        );
        self.keys.push(key.to_string());
        self.identifiers.insert(key.to_string(), id);
        id
    }

    /// The identifier of a term already in the vocabulary; a question word the corpus never
    /// used has none, which is itself an answer.
    pub(crate) fn identifier(&self, key: &str) -> Option<TermId> {
        self.identifiers.get(key).copied()
    }

    pub(crate) fn key(&self, id: TermId) -> &str {
        &self.keys[id.index() as usize]
    }

    pub(crate) fn len(&self) -> usize {
        self.keys.len()
    }
}

/// One conversation, as terms.
#[derive(Debug, Default)]
pub(crate) struct IndexedConversation {
    /// The terms of its name, distinct and in order.
    name_terms: Vec<TermId>,
    /// One sequence per reply, with the speaker, in the order they were said.
    replies: Vec<(Speaker, Vec<TermId>)>,
    /// Per term, in how many distinct replies of each party it occurred.
    presence: HashMap<TermId, (u32, u32)>,
}

impl IndexedConversation {
    pub(crate) fn replies(&self) -> &[(Speaker, Vec<TermId>)] {
        &self.replies
    }

    pub(crate) fn presence_of(&self, term: TermId) -> TermPresence {
        let (owner, model) = self.presence.get(&term).copied().unwrap_or_default();
        TermPresence::new(self.name_terms.contains(&term), owner, model)
    }

    /// Every term this conversation used anywhere, which is what a document frequency counts.
    fn terms_used(&self) -> impl Iterator<Item = TermId> + '_ {
        self.presence
            .keys()
            .copied()
            .chain(self.name_terms.iter().copied())
    }
}

/// The whole corpus as terms, in the order of `ChatCorpus::transcripts`.
pub(crate) struct TermIndex {
    /// The reduction this index was read with, kept so a question cannot be asked under a
    /// different one. Asking with a key rule the corpus was not indexed under finds nothing
    /// and looks exactly like a corpus that holds nothing, which is why the reader lives here
    /// and not at the call site.
    reader: TermReader,
    vocabulary: Vocabulary,
    conversations: Vec<IndexedConversation>,
    /// Per term, in how many distinct conversations it occurred. This is what the threshold
    /// `K` is applied to.
    conversations_with: Vec<u32>,
    term_occurrences: u64,
}

impl TermIndex {
    pub(crate) fn of(corpus: &ChatCorpus, mut reader: TermReader) -> Self {
        let mut vocabulary = Vocabulary::default();
        let mut conversations = Vec::with_capacity(corpus.transcripts().len());
        let mut term_occurrences = 0;
        let mut words = Vec::new();

        for transcript in corpus.transcripts() {
            let mut indexed = IndexedConversation::default();
            if let Some(name) = transcript.conversation().name() {
                words.clear();
                reader.terms_into(name.value(), &mut words);
                for word in &words {
                    let id = vocabulary.intern(word);
                    if !indexed.name_terms.contains(&id) {
                        indexed.name_terms.push(id);
                    }
                }
            }
            for reply in transcript.conversation().replies() {
                words.clear();
                reader.terms_into(reply.text(), &mut words);
                term_occurrences += words.len() as u64;
                let mut sequence = Vec::with_capacity(words.len());
                // A `Vec` and not a set: a reply is tens of terms, a linear scan of tens of
                // `u32` beats hashing them, and the corpus is millions of terms.
                let mut seen_here: Vec<TermId> = Vec::new();
                for word in &words {
                    let id = vocabulary.intern(word);
                    sequence.push(id);
                    if seen_here.contains(&id) {
                        continue;
                    }
                    seen_here.push(id);
                    let counted = indexed.presence.entry(id).or_default();
                    match reply.speaker() {
                        Speaker::Owner => counted.0 += 1,
                        Speaker::Model => counted.1 += 1,
                    }
                }
                indexed.replies.push((reply.speaker(), sequence));
            }
            conversations.push(indexed);
        }

        // Which conversation last counted each term, so a term used a thousand times in one
        // conversation adds one to its document frequency without a set per conversation.
        let mut conversations_with = vec![0u32; vocabulary.len()];
        let mut counted_in = vec![u32::MAX; vocabulary.len()];
        for (ordinal, indexed) in conversations.iter().enumerate() {
            let ordinal = u32::try_from(ordinal).expect("бесед в корпусе меньше 4 млрд");
            for term in indexed.terms_used() {
                let slot = &mut counted_in[term.index() as usize];
                if *slot == ordinal {
                    continue;
                }
                *slot = ordinal;
                conversations_with[term.index() as usize] += 1;
            }
        }

        Self {
            reader,
            vocabulary,
            conversations,
            conversations_with,
            term_occurrences,
        }
    }

    /// One question as terms this index knows, at one threshold.
    ///
    /// Read through the index's own reader, so the question and the corpus are always reduced
    /// the same way. A word the corpus never used has no identifier, and that is an answer in
    /// itself rather than an error.
    pub(crate) fn terms_of(&mut self, question: &str, least_conversations: u32) -> Vec<TermId> {
        let mut words = Vec::new();
        self.reader.terms_into(question, &mut words);
        let mut terms: Vec<TermId> = Vec::new();
        for word in &words {
            let Some(term) = self.vocabulary.identifier(word) else {
                continue;
            };
            if !self.used_in_at_least(term, least_conversations) || terms.contains(&term) {
                continue;
            }
            terms.push(term);
        }
        terms
    }

    pub(crate) fn vocabulary(&self) -> &Vocabulary {
        &self.vocabulary
    }

    pub(crate) fn conversations(&self) -> &[IndexedConversation] {
        &self.conversations
    }

    pub(crate) fn term_occurrences(&self) -> u64 {
        self.term_occurrences
    }

    /// Whether a term is used in at least `least` distinct conversations. A term one
    /// conversation used is a term no question can be answered by comparing conversations on.
    pub(crate) fn used_in_at_least(&self, term: TermId, least: u32) -> bool {
        self.conversations_with
            .get(term.index() as usize)
            .copied()
            .unwrap_or_default()
            >= least
    }

    /// How many terms survive a threshold, which is the vocabulary the graph is built over.
    pub(crate) fn vocabulary_at(&self, least: u32) -> usize {
        self.conversations_with
            .iter()
            .filter(|conversations| **conversations >= least)
            .count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A conversation built by hand, so the counting rule can be checked without a corpus.
    fn indexed(
        replies: &[(Speaker, &str)],
        name: Option<&str>,
    ) -> (Vocabulary, IndexedConversation) {
        let mut reader = TermReader::with_built_in_packs();
        let mut vocabulary = Vocabulary::default();
        let mut conversation = IndexedConversation::default();
        let mut words = Vec::new();
        if let Some(name) = name {
            words.clear();
            reader.terms_into(name, &mut words);
            for word in &words {
                let id = vocabulary.intern(word);
                if !conversation.name_terms.contains(&id) {
                    conversation.name_terms.push(id);
                }
            }
        }
        for (speaker, text) in replies {
            words.clear();
            reader.terms_into(text, &mut words);
            let mut sequence = Vec::new();
            let mut seen: Vec<TermId> = Vec::new();
            for word in &words {
                let id = vocabulary.intern(word);
                sequence.push(id);
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id);
                let counted = conversation.presence.entry(id).or_default();
                match speaker {
                    Speaker::Owner => counted.0 += 1,
                    Speaker::Model => counted.1 += 1,
                }
            }
            conversation.replies.push((*speaker, sequence));
        }
        (vocabulary, conversation)
    }

    /// The rule the whole ranking rests on, checked where it is actually applied: a term said
    /// four hundred times inside one pasted log is one reply, and a term the owner said in
    /// three separate replies is three.
    #[test]
    fn a_term_is_counted_once_per_reply_however_often_it_occurs_in_it() {
        let pasted = "контрагент ".repeat(400);
        let (vocabulary, conversation) = indexed(&[(Speaker::Owner, &pasted)], None);
        let term = vocabulary.identifier("контрагент").expect("термин");
        assert_eq!(conversation.presence_of(term).owner_replies(), 1);

        let (vocabulary, conversation) = indexed(
            &[
                (Speaker::Owner, "контрагенты"),
                (Speaker::Owner, "про контрагента"),
                (Speaker::Owner, "контрагентам"),
            ],
            None,
        );
        let term = vocabulary.identifier("контрагент").expect("термин");
        assert_eq!(conversation.presence_of(term).owner_replies(), 3);
    }

    #[test]
    fn the_two_parties_are_counted_apart() {
        let (vocabulary, conversation) = indexed(
            &[
                (Speaker::Owner, "договор"),
                (Speaker::Model, "договор"),
                (Speaker::Model, "договор"),
            ],
            None,
        );
        let term = vocabulary.identifier("договор").expect("термин");
        let presence = conversation.presence_of(term);
        assert_eq!(presence.owner_replies(), 1);
        assert_eq!(presence.model_replies(), 2);
        assert!(!presence.in_name());
    }

    #[test]
    fn a_term_of_the_name_is_known_to_be_in_the_name() {
        let (vocabulary, conversation) =
            indexed(&[(Speaker::Owner, "шаг два")], Some("Сверка: договор"));
        let named = vocabulary.identifier("договор").expect("термин имени");
        assert!(conversation.presence_of(named).in_name());
        assert_eq!(conversation.presence_of(named).owner_replies(), 0);

        let spoken = vocabulary.identifier("шаг").expect("термин реплики");
        assert!(!conversation.presence_of(spoken).in_name());
    }

    #[test]
    fn the_sequence_of_a_reply_keeps_repeats_because_a_window_is_cut_from_it() {
        let (_, conversation) = indexed(&[(Speaker::Owner, "раз два раз два раз")], None);
        assert_eq!(conversation.replies()[0].1.len(), 5);
    }
}
