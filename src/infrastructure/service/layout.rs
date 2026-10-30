// Where the service keeps what it is and what it did.
use std::path::{Path, PathBuf};

use super::super::app_directories::{self, DirectoryError};

/// The service's files, all under the data directory except the one launchd reads.
///
/// The program is a copy in the data directory, not the binary `cargo build` writes: a build
/// replaces `target/release/crosskit` in place, and a service started from it would run
/// whatever the last build left there — or nothing, halfway through a link. The copy changes
/// only when `install` or `restart` makes it, which is when the person asked for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceLayout {
    data_directory: PathBuf,
    launch_agents: PathBuf,
}

impl ServiceLayout {
    /// The layout of this run: its data directory, and the launch agents of its home.
    pub fn resolve() -> Result<Self, DirectoryError> {
        let data_directory = app_directories::data_directory()?;
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .ok_or(DirectoryError::HomeNotSet)?;
        Ok(Self::at(data_directory, &home))
    }

    pub fn at(data_directory: PathBuf, home: &Path) -> Self {
        Self {
            data_directory,
            launch_agents: home.join("Library").join("LaunchAgents"),
        }
    }

    pub fn program(&self) -> PathBuf {
        self.data_directory.join("bin").join("crosskit")
    }

    pub fn launch_agent(&self) -> PathBuf {
        self.launch_agents
            .join(format!("{}.plist", super::SERVICE_LABEL))
    }

    pub fn launch_agents(&self) -> &Path {
        &self.launch_agents
    }

    /// The directory of everything the running service writes.
    pub fn service_directory(&self) -> PathBuf {
        self.data_directory.join("service")
    }

    pub fn socket(&self) -> PathBuf {
        self.service_directory().join(super::SOCKET_NAME)
    }

    pub fn status(&self) -> PathBuf {
        self.service_directory().join("status.json")
    }

    pub fn journal(&self) -> PathBuf {
        self.service_directory().join("service.log")
    }

    /// What launchd itself captures from the process: only what the service could not put
    /// into its own journal — a panic, or a refusal before the journal opened.
    pub fn launchd_output(&self) -> PathBuf {
        self.service_directory().join("launchd.log")
    }

    pub fn instance_lock(&self) -> PathBuf {
        self.service_directory().join("service.lock")
    }
}

#[cfg(test)]
mod tests {
    use std::path::{Path, PathBuf};

    use super::ServiceLayout;

    #[test]
    fn the_program_and_the_journal_live_in_the_data_directory_and_the_agent_in_the_home() {
        let layout = ServiceLayout::at(
            PathBuf::from("/Users/alice/Library/Application Support/Crosskit"),
            Path::new("/Users/alice"),
        );
        assert_eq!(
            layout.program(),
            Path::new("/Users/alice/Library/Application Support/Crosskit/bin/crosskit")
        );
        assert_eq!(
            layout.launch_agent(),
            Path::new("/Users/alice/Library/LaunchAgents/dev.crosskit.watch.plist")
        );
        assert!(layout.journal().starts_with(layout.service_directory()));
    }
}
