// The ordered rule that turns what a repository's own files say into a context, or into a
// question for the owner.
//
//   order  signal                                                     answer
//   1      the repository's own `[user]` names a domain that is not    ask
//          the owner's
//   2      `origin` is missing or points at a path on this machine     ask
//   3      `origin` names a host and no group                          ask
//   4      the machine records no single mail domain of the owner      ask
//   5      the group of `origin` is that domain, or its host is the    work
//          domain or a subdomain of it
//   6      any other group, on any host                                ask
//
// Two signals are named here by what they do not decide. The host alone decides nothing: one
// server was measured holding three organisations, so a host shared by a group of rule 5 is
// still rule 6 for every other group on it. The mail in a reflog decides nothing about a
// repository either: it records who acted on this machine, which is one address everywhere.
// It is how the owner's domain is learned, for rules 4 and 5, and that is all.
//
// Nothing is ever decided `personal`. No free signal on a disk says a repository is somebody's
// own — an account on a public forge is not one, because the rule cannot tell whose account it
// is — so the rule asks instead of guessing, and the count of questions is the measurement.
//
// Rule 5 calls the side of the owner's domain `work`, and that is a premise, not a finding: a
// group named after a mail domain is the organisation that issues addresses under it, and the
// owner's configured address is then the employer's. On the owner's machine it holds. Where it
// would not — a person whose git identity is their own domain, with a group of that name — it is
// one question per machine, not one per repository.
use super::side::{AskBecause, ContextSide, DecidedBy, SplitDecision};
use crate::domain::git_identity::{Identity, OwnerDomain};
use crate::domain::git_remote::remote_owner::Origin;

/// What one repository's files say, as far as the split listens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RepositoryEvidence {
    origin: Origin,
    own_identity: Option<Identity>,
}

impl RepositoryEvidence {
    pub(crate) fn new(origin: Origin, own_identity: Option<Identity>) -> Self {
        Self {
            origin,
            own_identity,
        }
    }
}

/// The rule of this file, in its order. The first rule that speaks answers.
pub(crate) fn decide_repository(
    evidence: &RepositoryEvidence,
    owner: &OwnerDomain,
) -> SplitDecision {
    if let (OwnerDomain::One(domain), Some(Identity::Mail(own))) = (owner, &evidence.own_identity)
        && own != domain
    {
        return SplitDecision::Ask(AskBecause::OwnIdentity);
    }
    let group = match &evidence.origin {
        Origin::Owned(group) => group,
        Origin::Ungrouped(_) => return SplitDecision::Ask(AskBecause::NoGroup),
        Origin::NotAHost | Origin::Absent => return SplitDecision::Ask(AskBecause::NoRemote),
    };
    let domain = match owner {
        OwnerDomain::One(domain) => domain,
        OwnerDomain::Unknown => return SplitDecision::Ask(AskBecause::NoOwnerDomain),
        OwnerDomain::Several(_) => return SplitDecision::Ask(AskBecause::SeveralOwnerDomains),
    };
    if group.lies_under(domain) {
        return SplitDecision::decided(ContextSide::Work, DecidedBy::OwnerMailDomain);
    }
    SplitDecision::Ask(AskBecause::ForeignGroup)
}

#[cfg(test)]
mod tests {
    use super::{RepositoryEvidence, decide_repository};
    use crate::domain::context_split::{
        AskBecause, ContextSide, DecidedBy, ReflogIdentities, SplitDecision,
    };
    use crate::domain::git_identity::{Identity, MailDomain, OwnerDomain};
    use crate::domain::git_remote::remote_owner::Origin;

    /// The employer's domain in every test: the synthetic machine's owner works for it.
    const EMPLOYER: &str = "acme.example";

    fn owner_domain() -> OwnerDomain {
        OwnerDomain::of(mail_domains(&[EMPLOYER]))
    }

    fn mail_domains(domains: &[&str]) -> Vec<MailDomain> {
        domains
            .iter()
            .filter_map(|domain| Identity::of_address(&format!("alice@{domain}")))
            .filter_map(|identity| identity.mail_domain().cloned())
            .collect()
    }

    fn origin(url: &str) -> Origin {
        Origin::of_git_config(&format!("[remote \"origin\"]\n\turl = {url}\n"))
    }

    fn repository(url: &str) -> RepositoryEvidence {
        RepositoryEvidence::new(origin(url), None)
    }

    fn work() -> SplitDecision {
        SplitDecision::decided(ContextSide::Work, DecidedBy::OwnerMailDomain)
    }

    #[test]
    fn a_group_named_after_the_owners_domain_is_work_on_any_host() {
        assert_eq!(
            decide_repository(
                &repository("git@git.shared.example:acme.example/platform/app.git"),
                &owner_domain()
            ),
            work()
        );
        assert_eq!(
            decide_repository(
                &repository("ssh://git@git.acme.example:1022/devops/gitops.git"),
                &owner_domain()
            ),
            work(),
            "a host under the domain carries it whatever the group"
        );
    }

    /// The case the stage started from: one server, three organisations. The host of a work
    /// group decides nothing for the groups beside it.
    #[test]
    fn another_group_on_the_same_host_is_a_question_and_not_work() {
        let owner = owner_domain();
        assert_eq!(
            decide_repository(
                &repository("git@git.shared.example:acme.example/app.git"),
                &owner
            ),
            work()
        );
        for sibling in [
            "git@git.shared.example:org-two/team/app.git",
            "git@git.shared.example:org-three/team/tool.git",
        ] {
            assert_eq!(
                decide_repository(&repository(sibling), &owner),
                SplitDecision::Ask(AskBecause::ForeignGroup),
                "{sibling}"
            );
        }
    }

    /// A domain is matched as a whole label, never as a piece of a longer name.
    #[test]
    fn a_domain_inside_a_longer_name_is_not_carried() {
        let owner = owner_domain();
        for url in [
            "git@notacme.example:team/app.git",
            "git@github.com:acme.example-fans/app.git",
            "https://github.com/acme/app.git",
        ] {
            assert_eq!(
                decide_repository(&repository(url), &owner),
                SplitDecision::Ask(AskBecause::ForeignGroup),
                "{url}"
            );
        }
    }

    /// Nothing the disk says makes a repository somebody's own, so an account on a public
    /// forge is a question: the rule would have to guess whose account it is.
    #[test]
    fn an_account_on_a_public_forge_is_asked_about_and_never_called_personal() {
        let decision = decide_repository(
            &repository("git@github.com:alice/dotfiles.git"),
            &owner_domain(),
        );

        assert_eq!(decision, SplitDecision::Ask(AskBecause::ForeignGroup));
        assert_ne!(decision.side(), Some(ContextSide::Personal));
    }

    /// The owner's address in a reflog is the machine's one identity; it is recorded in a
    /// clone of somebody else's project as readily as in the owner's own work, so it is not
    /// allowed to decide anything about the repository it was found in.
    #[test]
    fn the_owners_mail_in_a_reflog_does_not_decide_the_repository() {
        let reflog = ReflogIdentities::of_reflog(
            "0000 1111 Alice <alice@acme.example> 1786445503 +0500\tclone: from github.com:someone/tool.git\n",
        );
        assert_eq!(
            reflog.domains().count(),
            1,
            "the reflog does name the owner's domain"
        );

        assert_eq!(
            decide_repository(
                &repository("git@github.com:someone/tool.git"),
                &owner_domain()
            ),
            SplitDecision::Ask(AskBecause::ForeignGroup)
        );
    }

    #[test]
    fn the_rules_speak_in_their_order() {
        let owner = owner_domain();
        let own = Identity::of_address("alice@own-studio.example");

        assert_eq!(
            decide_repository(
                &RepositoryEvidence::new(
                    origin("git@git.shared.example:acme.example/app.git"),
                    own
                ),
                &owner
            ),
            SplitDecision::Ask(AskBecause::OwnIdentity),
            "rule 1 before rule 5: a repository set up with its own identity is not the machine's"
        );
        assert_eq!(
            decide_repository(
                &RepositoryEvidence::new(
                    origin("git@git.shared.example:acme.example/app.git"),
                    Identity::of_address("alice@acme.example")
                ),
                &owner
            ),
            work(),
            "an own identity that is the owner's own domain says nothing new"
        );
        assert_eq!(
            decide_repository(&RepositoryEvidence::new(Origin::Absent, None), &owner),
            SplitDecision::Ask(AskBecause::NoRemote)
        );
        assert_eq!(
            decide_repository(&repository("/srv/git/app.git"), &owner),
            SplitDecision::Ask(AskBecause::NoRemote)
        );
        assert_eq!(
            decide_repository(&repository("git@git.acme.example:app.git"), &owner),
            SplitDecision::Ask(AskBecause::NoGroup),
            "rule 3 before rule 5: no group, and the host alone is not allowed to decide"
        );
    }

    #[test]
    fn without_one_owner_domain_nothing_is_matched() {
        let url = "git@git.shared.example:acme.example/app.git";

        assert_eq!(
            decide_repository(&repository(url), &OwnerDomain::of(Vec::new())),
            SplitDecision::Ask(AskBecause::NoOwnerDomain)
        );
        assert_eq!(
            decide_repository(
                &repository(url),
                &OwnerDomain::of(mail_domains(&[EMPLOYER, "own-studio.example"]))
            ),
            SplitDecision::Ask(AskBecause::SeveralOwnerDomains),
            "which of two domains is the employer's is not written on the disk"
        );
        assert_eq!(
            OwnerDomain::of(mail_domains(&[EMPLOYER, EMPLOYER])),
            owner_domain(),
            "the same domain seen twice is one domain"
        );
    }
}
