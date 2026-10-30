// What a repository's `.git/config` says about where it is hosted — and nothing else.
//
// Two things leave this module: the host of a remote, and — in `remote_owner` — the first group
// of `origin`'s path (ADR 0042). The account, the subgroups, the repository name, the port and
// the rest of the path are what a remote URL says about a person's work, and they are dropped
// here rather than at the printer: a caller cannot print what no value carries. That is the
// whole reason this rule lives in the domain instead of beside the file that holds it.
use std::fmt;

/// Who a remote says owns the repository — the host and the group under it.
pub(crate) mod remote_owner;

/// The host a git remote points at, lowercased. It is evidence about a repository, not an
/// address: nothing reconstructs a URL from it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RemoteHost(String);

impl RemoteHost {
    /// The host a remote URL names, or nothing when it names none.
    ///
    /// Three spellings reach a `.git/config`: a URL with a scheme, the scp-like
    /// `user@host:path`, and a plain filesystem path. Only the first two carry a host; a
    /// filesystem path names a directory on this machine and says nothing about where the
    /// work belongs.
    pub fn of_remote_url(url: &str) -> Option<Self> {
        let url = url.trim();
        if url.is_empty() || is_local_path(url) {
            return None;
        }
        let authority = match url.split_once("://") {
            Some((_, rest)) => rest.split('/').next().unwrap_or_default(),
            None => scp_like_authority(url)?,
        };
        let after_user = match authority.rsplit_once('@') {
            Some((_, host)) => host,
            None => authority,
        };
        Self::parse(strip_port(after_user))
    }

    /// A host as such: non-empty and made only of the characters a host name is written
    /// with. Anything else is not turned into a host that merely looks plausible.
    fn parse(host: &str) -> Option<Self> {
        if host.is_empty() || host == "." || host == ".." {
            return None;
        }
        let spelled_like_a_host = match bracketed_literal(host) {
            Some(literal) => literal
                .chars()
                .all(|character| character.is_ascii_hexdigit() || character == ':'),
            None => host.chars().all(|character| {
                character.is_alphanumeric() || matches!(character, '-' | '.' | '_')
            }),
        };
        if !spelled_like_a_host {
            return None;
        }
        Some(Self(host.to_lowercase()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for RemoteHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// A path on this machine, in any of the spellings git accepts for one.
fn is_local_path(url: &str) -> bool {
    url.starts_with('/') || url.starts_with("./") || url.starts_with("../") || url.starts_with('~')
}

/// `user@host:path` and `host:path`, the spelling ssh accepts without a scheme. The part
/// before the colon is the authority, and only when it holds no `/` — otherwise the colon
/// belongs to a path, not to a host.
fn scp_like_authority(url: &str) -> Option<&str> {
    let (authority, _) = url.split_once(':')?;
    if authority.contains('/') {
        return None;
    }
    Some(authority)
}

/// An IPv6 literal keeps its brackets and its colons; every other host ends at the port.
fn strip_port(host: &str) -> &str {
    if let Some(literal) = bracketed_literal(host) {
        return &host[..literal.len() + 2];
    }
    host.split(':').next().unwrap_or_default()
}

/// What stands inside the brackets of an IPv6 literal, brackets excluded.
fn bracketed_literal(host: &str) -> Option<&str> {
    let rest = host.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(&rest[..end])
}

/// One `[remote "<name>"]` section that declares a URL. It never leaves this module: what
/// a caller may learn is which host the repository stands under, and that is
/// `RepositoryRemote`.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GitRemote {
    name: String,
    /// `None` when the remote points at a path on this machine, or at something this rule
    /// does not read as a host.
    host: Option<RemoteHost>,
}

/// The name git gives the remote a repository was cloned from.
const ORIGIN: &str = "origin";

/// The remotes a `.git/config` declares, in the order it declares them. The first `url` of
/// each remote is the one read, which is the one git clones from; the file's other keys —
/// user names, mail addresses, signing keys, hook paths — are never looked at.
fn remotes_in_git_config(config: &str) -> Vec<GitRemote> {
    let mut remotes: Vec<GitRemote> = Vec::new();
    for (header, line) in lines_in_sections(config) {
        let Some(name) = remote_name(header) else {
            continue;
        };
        let Some(url) = value_of(line, "url") else {
            continue;
        };
        if remotes.iter().any(|remote| remote.name == name) {
            continue;
        }
        remotes.push(GitRemote {
            name: name.to_string(),
            host: RemoteHost::of_remote_url(url),
        });
    }
    remotes
}

/// Every line of a git configuration that stands inside a section, comments removed, beside
/// the header of that section. The one reading of the file's layout: which key of which
/// section matters is each caller's own rule, and a line before the first header belongs to
/// none.
pub(super) fn lines_in_sections(config: &str) -> impl Iterator<Item = (&str, &str)> {
    let mut section: Option<&str> = None;
    config.lines().filter_map(move |line| {
        let line = strip_comment(line).trim();
        if let Some(header) = section_header(line) {
            section = Some(header);
            return None;
        }
        section.map(|header| (header, line))
    })
}

/// What a repository's remotes say about where it belongs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepositoryRemote {
    /// No `.git/config` was read: the `.git` entry is not a directory, the file is not
    /// there, or it could not be read as text.
    NoConfig,
    /// The configuration was read and declares no remote at all.
    NoRemote,
    /// Every remote points at a path on this machine.
    NoHost,
    Host(RemoteHost),
    /// Several hosts and no `origin` to settle it. Nothing here decides which one the
    /// repository belongs to, so nothing pretends to.
    SeveralHosts(Vec<RemoteHost>),
}

impl RepositoryRemote {
    /// What the text of one `.git/config` says about where its repository belongs.
    pub fn of_git_config(config: &str) -> Self {
        Self::of_remotes(&remotes_in_git_config(config))
    }

    /// The one host a repository stands under. `origin` settles it when it names a host,
    /// because that is the remote a clone comes from; otherwise the remotes have to agree.
    fn of_remotes(remotes: &[GitRemote]) -> Self {
        if remotes.is_empty() {
            return Self::NoRemote;
        }
        let origin = remotes
            .iter()
            .find(|remote| remote.name == ORIGIN)
            .and_then(|remote| remote.host.clone());
        if let Some(origin) = origin {
            return Self::Host(origin);
        }
        let mut hosts: Vec<RemoteHost> = remotes
            .iter()
            .filter_map(|remote| remote.host.clone())
            .collect();
        hosts.sort();
        hosts.dedup();
        match hosts.len() {
            0 => Self::NoHost,
            1 => Self::Host(hosts.remove(0)),
            _ => Self::SeveralHosts(hosts),
        }
    }
}

/// A `#` or `;` starts a comment that runs to the end of the line.
fn strip_comment(line: &str) -> &str {
    match line.find(['#', ';']) {
        Some(start) => &line[..start],
        None => line,
    }
}

fn section_header(line: &str) -> Option<&str> {
    line.strip_prefix('[')?.strip_suffix(']')
}

/// `remote "origin"` and `remote.origin`, the two spellings of one section header. A
/// section that is not a remote yields nothing, which is how every other key is ignored.
fn remote_name(header: &str) -> Option<&str> {
    let rest = header.strip_prefix("remote")?;
    let rest = rest.trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        return quoted.strip_suffix('"');
    }
    rest.strip_prefix('.').filter(|name| !name.is_empty())
}

/// The value `key` is given on one line, unquoted. Git reads key names without regard to
/// case, so `URL` and `url` are one key.
pub(super) fn value_of<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let (name, value) = line.split_once('=')?;
    if !name.trim().eq_ignore_ascii_case(key) {
        return None;
    }
    let value = value.trim();
    Some(
        value
            .strip_prefix('"')
            .and_then(|rest| rest.strip_suffix('"'))
            .unwrap_or(value),
    )
}

#[cfg(test)]
mod tests {
    use super::{RemoteHost, RepositoryRemote, remotes_in_git_config};

    fn host(url: &str) -> Option<String> {
        RemoteHost::of_remote_url(url).map(|host| host.as_str().to_string())
    }

    fn one_host(name: &str) -> RepositoryRemote {
        RepositoryRemote::Host(RemoteHost::of_remote_url(&format!("https://{name}/repo")).unwrap())
    }

    #[test]
    fn every_spelling_of_a_remote_url_yields_its_host_alone() {
        assert_eq!(
            host("https://github.com/owner/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host("git@github.com:owner/repo.git").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            host("ssh://git@git.example.org:2222/owner/repo").as_deref(),
            Some("git.example.org")
        );
        assert_eq!(
            host("git://Git.Example.ORG/repo").as_deref(),
            Some("git.example.org")
        );
        assert_eq!(
            host("https://user:token@gitlab.example.com/owner/repo").as_deref(),
            Some("gitlab.example.com")
        );
        assert_eq!(
            host("ssh://git@[2001:db8::1]:22/repo").as_deref(),
            Some("[2001:db8::1]")
        );
    }

    #[test]
    fn a_remote_that_names_no_host_yields_none() {
        assert_eq!(host("/srv/git/repo.git"), None);
        assert_eq!(host("../sibling"), None);
        assert_eq!(host("~/repositories/repo"), None);
        assert_eq!(host("file:///srv/git/repo.git"), None);
        assert_eq!(host("   "), None);
        assert_eq!(host("../weird:name/repo"), None);
    }

    #[test]
    fn a_configuration_without_a_remote_declares_none() {
        let remotes = remotes_in_git_config(
            "[core]\n\trepositoryformatversion = 0\n[user]\n\temail = someone@example.com\n",
        );

        assert!(remotes.is_empty());
        assert_eq!(
            RepositoryRemote::of_remotes(&remotes),
            RepositoryRemote::NoRemote
        );
    }

    #[test]
    fn several_remotes_are_read_and_origin_settles_the_host() {
        let remotes = remotes_in_git_config(
            "[remote \"upstream\"]\n\turl = https://github.com/upstream/repo.git\n\
             [remote \"origin\"]\n\turl = git@git.example.org:owner/repo.git\n\tfetch = +refs/heads/*\n",
        );

        assert_eq!(
            remotes
                .iter()
                .map(|remote| remote.name.as_str())
                .collect::<Vec<_>>(),
            ["upstream", "origin"],
            "the file's own order is kept, so nothing depends on which remote comes first"
        );
        assert_eq!(
            RepositoryRemote::of_remotes(&remotes),
            one_host("git.example.org")
        );
    }

    #[test]
    fn several_hosts_without_an_origin_are_left_undecided() {
        let remotes = remotes_in_git_config(
            "[remote \"github\"]\n\turl = https://github.com/owner/repo.git\n\
             [remote \"internal\"]\n\turl = git@git.example.org:owner/repo.git\n",
        );

        let RepositoryRemote::SeveralHosts(hosts) = RepositoryRemote::of_remotes(&remotes) else {
            panic!("two hosts and no origin must stay undecided");
        };
        assert_eq!(
            hosts.iter().map(RemoteHost::as_str).collect::<Vec<_>>(),
            ["git.example.org", "github.com"]
        );
    }

    #[test]
    fn several_remotes_that_agree_on_one_host_decide_it() {
        let remotes = remotes_in_git_config(
            "[remote \"fork\"]\n\turl = https://github.com/me/repo.git\n\
             [remote \"upstream\"]\n\turl = https://github.com/them/repo.git\n",
        );

        assert_eq!(
            RepositoryRemote::of_remotes(&remotes),
            one_host("github.com")
        );
    }

    #[test]
    fn remotes_that_only_name_local_paths_have_no_host() {
        let remotes = remotes_in_git_config("[remote \"origin\"]\n\turl = /srv/git/repo.git\n");

        assert_eq!(
            RepositoryRemote::of_remotes(&remotes),
            RepositoryRemote::NoHost
        );
    }

    #[test]
    fn comments_quotes_and_the_dotted_header_are_all_read() {
        let remotes = remotes_in_git_config(
            "; a comment\n[remote.origin]\n\tURL = \"https://github.com/owner/repo.git\" # inline\n",
        );

        assert_eq!(remotes.len(), 1);
        assert_eq!(
            remotes[0].host.as_ref().map(RemoteHost::as_str),
            Some("github.com")
        );
    }

    #[test]
    fn a_key_outside_a_remote_section_is_never_read_as_a_url() {
        let remotes = remotes_in_git_config("[core]\n\turl = https://example.com/not-a-remote\n");

        assert!(remotes.is_empty());
    }
}
