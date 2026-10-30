// The mail domain git is configured with on this machine.
//
// Two files hold the global identity, `~/.gitconfig` and `~/.config/git/config`, and git reads
// both. Of each, `[user] email` is read, and of that only the domain survives
// (`domain::git_identity`). No repository's reflog is opened: Stage 38 found the same address in
// every one of them, so it would tell nothing the global file does not.
use std::fs;
use std::io::Read;

use crate::domain::{NormalizedPath, OwnerDomain, identity_in_git_config};

/// The global configuration files, relative to the home directory.
const GLOBAL_CONFIGS: [&str; 2] = [".gitconfig", ".config/git/config"];
/// A configuration a person wrote is a few kilobytes; a larger file is not read.
const CONFIG_LIMIT_BYTES: u64 = 256 * 1024;

/// The one mail domain the global configuration names, if it names exactly one.
pub fn owner_mail_domain(home: &NormalizedPath) -> OwnerDomain {
    OwnerDomain::of(
        GLOBAL_CONFIGS
            .iter()
            .filter_map(|relative| text_of(&format!("{}/{relative}", home.as_str())))
            .filter_map(|text| identity_in_git_config(&text))
            .filter_map(|identity| identity.mail_domain().cloned()),
    )
}

/// The file as text, or nothing when it is missing, too large or not text.
fn text_of(path: &str) -> Option<String> {
    let file = fs::File::open(path).ok()?;
    if !file.metadata().ok()?.is_file() {
        return None;
    }
    let mut text = String::new();
    file.take(CONFIG_LIMIT_BYTES + 1)
        .read_to_string(&mut text)
        .ok()?;
    (text.len() as u64 <= CONFIG_LIMIT_BYTES).then_some(text)
}
