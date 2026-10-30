// Who a person acts as in git, as the configuration records it — and only the domain of the
// address.
//
// `[user] email` in a git configuration is the identity git writes into every commit. The part
// before `@` is dropped here, before any caller holds it, for the reason ADR 0019 drops
// everything but the host of a remote: what no value carries cannot be printed.
//
// What this does not say is whose work a repository is. On a machine with one configured
// identity it is the same address in every repository, a clone of somebody else's project
// included (Stage 38 measured it: one address in all 23 reflogs). So the domain is evidence
// about the person, never about a repository: `setup` uses it for one mark — a group of a
// remote named after it looks like the person's organisation — and decides nothing by it
// (ADR 0042).
use std::collections::BTreeSet;

use super::git_remote::{lines_in_sections, value_of};

/// The domain of a mail address, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MailDomain(String);

impl MailDomain {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// What one recorded address says about who acted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Identity {
    Mail(MailDomain),
    /// An address git made up because none was configured — `user@host.local` — or git's own
    /// name for the commits a stash makes (`git@stash`). It names a machine, not a person.
    MachineDefault,
}

/// Endings of a host name that the machine gave itself. Git falls back to `user@<host name>`
/// when no identity is configured, and on this platform a host name ends in `.local`; the
/// second ending is the same fallback on Linux. A list of two, because what it recognises is
/// the fallback itself and not a list of providers.
const MACHINE_NAME_ENDINGS: [&str; 2] = [".local", ".localdomain"];

impl Identity {
    /// The identity an address records, or nothing when it is not an address at all.
    pub fn of_address(address: &str) -> Option<Self> {
        let (_, domain) = address.trim().rsplit_once('@')?;
        let domain = domain.trim().trim_end_matches('.').to_lowercase();
        if domain.is_empty() {
            return None;
        }
        let made_by_the_machine = !domain.contains('.')
            || MACHINE_NAME_ENDINGS
                .iter()
                .any(|ending| domain.ends_with(ending));
        if made_by_the_machine {
            return Some(Self::MachineDefault);
        }
        Some(Self::Mail(MailDomain(domain)))
    }

    pub fn mail_domain(&self) -> Option<&MailDomain> {
        match self {
            Self::Mail(domain) => Some(domain),
            Self::MachineDefault => None,
        }
    }
}

/// The identity `[user] email` sets in one git configuration, or nothing when it sets none.
/// Git reads section names without regard to case, so `[User]` is the same section.
pub fn identity_in_git_config(config: &str) -> Option<Identity> {
    lines_in_sections(config)
        .filter(|(header, _)| header.trim().eq_ignore_ascii_case("user"))
        .find_map(|(_, line)| value_of(line, "email"))
        .and_then(Identity::of_address)
}

/// The mail domain a person acts under on this machine, as far as it is known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerDomain {
    One(MailDomain),
    Unknown,
    /// Several domains, and which of them is the employer's is not written anywhere.
    Several(Vec<MailDomain>),
}

impl OwnerDomain {
    /// The one domain every recorded identity agrees on. Addresses the machine made up are
    /// not identities of anybody and are left out by the caller, which is why this takes
    /// domains and not identities.
    pub fn of(domains: impl IntoIterator<Item = MailDomain>) -> Self {
        let mut distinct: Vec<MailDomain> = domains
            .into_iter()
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        match distinct.len() {
            0 => Self::Unknown,
            1 => Self::One(distinct.remove(0)),
            _ => Self::Several(distinct),
        }
    }

    /// The domain, when there is exactly one.
    pub fn one(&self) -> Option<&MailDomain> {
        match self {
            Self::One(domain) => Some(domain),
            Self::Unknown | Self::Several(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Identity, MailDomain, OwnerDomain, identity_in_git_config};

    fn domain(address: &str) -> Option<String> {
        Identity::of_address(address).and_then(|identity| {
            identity
                .mail_domain()
                .map(|domain| domain.as_str().to_string())
        })
    }

    #[test]
    fn only_the_domain_of_an_address_is_kept() {
        assert_eq!(
            domain("Alice.Doe@Acme.Example"),
            Some("acme.example".to_string())
        );
        assert_eq!(
            domain("a@b@mail.example."),
            Some("mail.example".to_string())
        );
        assert_eq!(Identity::of_address("not an address"), None);
        assert_eq!(Identity::of_address("alice@"), None);
    }

    /// Git's fallback identity and its stash identity say which machine acted, never whose
    /// work it was, and are told apart from a mail domain before anything counts them.
    #[test]
    fn an_address_the_machine_made_up_is_not_a_mail_domain() {
        assert_eq!(
            Identity::of_address("alice@Alices-Laptop.local"),
            Some(Identity::MachineDefault)
        );
        assert_eq!(
            Identity::of_address("git@stash"),
            Some(Identity::MachineDefault)
        );
        assert_eq!(
            Identity::of_address("alice@build-box.localdomain"),
            Some(Identity::MachineDefault)
        );
    }

    #[test]
    fn the_user_section_of_a_configuration_gives_its_identity() {
        let config =
            "[core]\n\tbare = false\n[User]\n\tname = Alice\n\temail = \"alice@acme.example\"\n";

        assert_eq!(
            identity_in_git_config(config).and_then(|identity| identity
                .mail_domain()
                .map(|domain| domain.as_str().to_string())),
            Some("acme.example".to_string())
        );
        assert_eq!(
            identity_in_git_config("[remote \"origin\"]\n\temail = x@y.example\n"),
            None,
            "a key of that name in another section is not an identity"
        );
    }

    fn domain_of(address: &str) -> MailDomain {
        Identity::of_address(address)
            .and_then(|identity| identity.mail_domain().cloned())
            .unwrap()
    }

    #[test]
    fn the_owner_domain_is_one_only_when_every_identity_agrees() {
        assert_eq!(OwnerDomain::of(Vec::new()), OwnerDomain::Unknown);
        assert_eq!(
            OwnerDomain::of([domain_of("a@acme.example"), domain_of("b@acme.example")])
                .one()
                .map(MailDomain::as_str),
            Some("acme.example"),
            "the same domain seen twice is one domain"
        );
        assert!(matches!(
            OwnerDomain::of([domain_of("a@acme.example"), domain_of("a@own.example")]),
            OwnerDomain::Several(domains) if domains.len() == 2
        ));
    }
}
