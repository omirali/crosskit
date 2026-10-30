// Who a remote says owns a repository: the host, and the group directly under it.
//
// The host alone was measured to say too little (Stage 38): one work server held three
// organisations, and which of them a repository belongs to is the group in its path. So since
// Stage 39 `.git/config` gives two things and not one — the host of `origin` and the first
// segment of its path (ADR 0042, which amends ADR 0019).
//
// Only the top-level group leaves. The subgroups, the repository name and the account a URL
// logs in with are what a remote says about somebody's work, and no value here carries them:
// the type is the rule, so a caller cannot print what it was never given.
use super::{
    ORIGIN, RemoteHost, is_local_path, lines_in_sections, remote_name, scp_like_authority, value_of,
};
use crate::domain::git_identity::MailDomain;

/// The host of `origin` and the first segment of its path, lowercased.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct RemoteOwner {
    host: RemoteHost,
    group: String,
}

/// What `origin` says about who owns a repository.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// A host and a group under it.
    Owned(RemoteOwner),
    /// A host and a repository lying directly under it.
    Ungrouped(RemoteHost),
    /// A path on this machine, or something this rule does not read as a host.
    NotAHost,
    /// The configuration declares no `origin`.
    Absent,
}

impl Origin {
    /// What the first `url` of `origin` names. Only `origin`, and deliberately: the remote a
    /// clone came from is where the work came from, and a second remote is somewhere the same
    /// work is also pushed.
    pub fn of_git_config(config: &str) -> Self {
        let url = lines_in_sections(config)
            .filter(|(header, _)| remote_name(header) == Some(ORIGIN))
            .find_map(|(_, line)| value_of(line, "url"));
        let Some(url) = url else {
            return Self::Absent;
        };
        if let Some(owner) = RemoteOwner::of_remote_url(url) {
            return Self::Owned(owner);
        }
        match RemoteHost::of_remote_url(url) {
            Some(host) => Self::Ungrouped(host),
            None => Self::NotAHost,
        }
    }
}

impl RemoteOwner {
    /// The owner one remote URL names. A path on this machine names none, and neither does a
    /// repository lying directly under its host: with one segment there is no group, only a
    /// name.
    pub fn of_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        if url.is_empty() || is_local_path(url) {
            return None;
        }
        let host = RemoteHost::of_remote_url(url)?;
        let path = match url.split_once("://") {
            Some((_, rest)) => rest.split_once('/')?.1,
            None => {
                let authority = scp_like_authority(url)?;
                &url[authority.len() + 1..]
            }
        };
        let mut segments = path.split('/').filter(|segment| !segment.is_empty());
        let group = segments.next()?.trim_start_matches('~');
        segments.next()?;
        if group.is_empty() {
            return None;
        }
        Some(Self {
            host,
            group: group.to_lowercase(),
        })
    }

    pub fn host(&self) -> &RemoteHost {
        &self.host
    }

    pub fn group(&self) -> &str {
        &self.group
    }

    /// Whether this owner stands under a mail domain: the group is named after it, or the
    /// host is the domain or lies under it. A host is compared by whole labels, so a domain
    /// is never matched inside a longer name.
    pub fn lies_under(&self, domain: &MailDomain) -> bool {
        let domain = domain.as_str();
        let host = self.host.as_str();
        self.group == domain
            || host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|label| label.ends_with('.'))
    }
}

#[cfg(test)]
mod tests {
    use super::{Origin, RemoteOwner};
    use crate::domain::git_identity::Identity;

    fn owner(url: &str) -> Option<(String, String)> {
        RemoteOwner::of_remote_url(url)
            .map(|owner| (owner.host().as_str().to_string(), owner.group().to_string()))
    }

    fn pair(host: &str, group: &str) -> Option<(String, String)> {
        Some((host.to_string(), group.to_string()))
    }

    #[test]
    fn the_group_is_the_first_segment_under_the_host_in_every_spelling() {
        assert_eq!(
            owner("git@git.example.org:acme.io/platform/billing.git"),
            pair("git.example.org", "acme.io")
        );
        assert_eq!(
            owner("ssh://git@git.example.org:2222/devops/gitops.git"),
            pair("git.example.org", "devops")
        );
        assert_eq!(
            owner("https://github.com/Someone/tool"),
            pair("github.com", "someone")
        );
        assert_eq!(
            owner("https://user:token@git.example.org/acme/app.git"),
            pair("git.example.org", "acme")
        );
    }

    #[test]
    fn a_remote_without_a_group_names_no_owner() {
        assert_eq!(owner("git@git.example.org:lonely.git"), None);
        assert_eq!(owner("https://git.example.org/"), None);
        assert_eq!(owner("/srv/git/acme/app.git"), None);
        assert_eq!(owner("file:///srv/git/acme/app.git"), None);
        assert_eq!(owner("  "), None);
    }

    #[test]
    fn only_origin_names_the_owner_and_nothing_else_of_the_file_is_read() {
        let config = "[user]\n\temail = someone@example.com\n\
                      [remote \"upstream\"]\n\turl = https://github.com/upstream/tool.git\n\
                      [remote \"origin\"]\n\turl = git@git.example.org:acme/tool.git\n\
                      \turl = git@elsewhere.example:other/tool.git\n";

        let Origin::Owned(found) = Origin::of_git_config(config) else {
            panic!("origin names a group");
        };
        assert_eq!(found.host().as_str(), "git.example.org");
        assert_eq!(
            found.group(),
            "acme",
            "the first url of origin, as git reads it"
        );
        assert_eq!(
            Origin::of_git_config(
                "[remote \"upstream\"]\n\turl = https://github.com/upstream/tool.git\n"
            ),
            Origin::Absent,
            "a repository without origin is not given another remote's owner"
        );
    }

    #[test]
    fn an_origin_without_a_group_says_which_kind_of_nothing_it_is() {
        let ungrouped =
            Origin::of_git_config("[remote \"origin\"]\n\turl = git@git.example.org:app.git\n");
        assert!(
            matches!(&ungrouped, Origin::Ungrouped(host) if host.as_str() == "git.example.org"),
            "{ungrouped:?}"
        );
        assert_eq!(
            Origin::of_git_config("[remote \"origin\"]\n\turl = /srv/git/app.git\n"),
            Origin::NotAHost
        );
        assert_eq!(
            Origin::of_git_config("[core]\n\tbare = false\n"),
            Origin::Absent
        );
    }

    /// The one comparison `setup` marks a group by. A domain is a whole label or nothing: the
    /// same letters inside a longer host or group are somebody else's.
    #[test]
    fn an_owner_lies_under_a_domain_by_its_group_or_by_whole_labels_of_its_host() {
        let domain = Identity::of_address("alice@acme.example")
            .and_then(|identity| identity.mail_domain().cloned())
            .unwrap();
        let under = |url: &str| RemoteOwner::of_remote_url(url).unwrap().lies_under(&domain);

        assert!(under(
            "git@git.shared.example:acme.example/platform/app.git"
        ));
        assert!(under("ssh://git@git.acme.example:1022/devops/gitops.git"));
        assert!(under("https://acme.example/team/app.git"));
        assert!(!under("git@notacme.example:team/app.git"));
        assert!(!under("git@github.com:acme.example-fans/app.git"));
        assert!(!under("https://github.com/acme/app.git"));
    }
}
