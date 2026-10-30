// Who acted in a repository, as its reflog records it — Stage 38's measurement only.
//
// `.git/logs/HEAD` is a line per local action naming its committer. A released Crosskit never
// opens it: on a machine with one configured identity it names the same address in every
// repository, a clone of somebody else's project included, so it says nothing about a
// repository that the global `[user]` does not already say about the person. The address is
// reduced to its domain by the product's `Identity`, before this file holds it.
use std::collections::BTreeMap;

use crate::domain::git_identity::{Identity, MailDomain};

/// The identities one reflog records, counted by line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct ReflogIdentities {
    mail: BTreeMap<MailDomain, usize>,
    machine_default: usize,
    without_identity: usize,
}

impl ReflogIdentities {
    /// Every line of `.git/logs/HEAD` is `<old> <new> <name> <<address>> <time> <zone>`, a tab,
    /// and what was done. The address is the last `<…>` before the tab; a line without one is
    /// counted rather than guessed at.
    pub(crate) fn of_reflog(text: &str) -> Self {
        let mut found = Self::default();
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            match committer_of(line).and_then(Identity::of_address) {
                Some(Identity::Mail(domain)) => *found.mail.entry(domain).or_default() += 1,
                Some(Identity::MachineDefault) => found.machine_default += 1,
                None => found.without_identity += 1,
            }
        }
        found
    }

    /// The mail domains the reflog records, each with the number of lines naming it.
    pub(crate) fn domains(&self) -> impl Iterator<Item = (&MailDomain, usize)> {
        self.mail.iter().map(|(domain, lines)| (domain, *lines))
    }

    pub(crate) fn machine_defaults(&self) -> usize {
        self.machine_default
    }

    pub(crate) fn lines_without_identity(&self) -> usize {
        self.without_identity
    }

    pub(crate) fn lines(&self) -> usize {
        self.mail.values().sum::<usize>() + self.machine_default + self.without_identity
    }
}

fn committer_of(line: &str) -> Option<&str> {
    let header = line.split('\t').next().unwrap_or(line);
    let close = header.rfind('>')?;
    let open = header[..close].rfind('<')?;
    Some(&header[open + 1..close])
}

#[cfg(test)]
mod tests {
    use super::ReflogIdentities;

    #[test]
    fn a_reflog_is_counted_by_the_committer_of_each_line() {
        let reflog = "0000 1111 Alice Doe <alice@acme.example> 1786445503 +0500\tclone: from host:acme/app.git\n\
                      1111 2222 Alice Doe <alice@acme.example> 1786445600 +0500\tcommit: first\n\
                      2222 3333 Alice <alice@Alices-Laptop.local> 1786445700 +0500\tcommit: second <draft>\n\
                      3333 3333 git stash <git@stash> 1786445800 +0500\treset: moving to HEAD\n\
                      this line names nobody\n";

        let found = ReflogIdentities::of_reflog(reflog);

        assert_eq!(
            found
                .domains()
                .map(|(domain, lines)| (domain.as_str().to_string(), lines))
                .collect::<Vec<_>>(),
            [("acme.example".to_string(), 2)]
        );
        assert_eq!(found.machine_defaults(), 2);
        assert_eq!(found.lines_without_identity(), 1);
        assert_eq!(found.lines(), 5);
    }
}
