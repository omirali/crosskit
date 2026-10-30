mod descriptor_walker;
#[cfg(unix)]
mod entry_stat;
mod plain_text_evidence;
#[cfg(test)]
mod read_cost_tests;
mod scope_probe;
#[cfg(test)]
mod tests;
#[cfg(unix)]
mod unix_walk;
mod watch_notify;
#[cfg(unix)]
mod watch_probe;

pub use descriptor_walker::DescriptorRelativeWalker;
#[cfg(unix)]
pub use entry_stat::reads_creation_times;
pub use scope_probe::DiskScopeProbe;
#[cfg(unix)]
pub(crate) use unix_walk::{c_name, open_directory_at};

/// No descriptor-relative walk exists on this platform, so nothing reads a creation time
/// either.
#[cfg(not(unix))]
pub fn reads_creation_times() -> bool {
    false
}
pub use watch_notify::{NotifyWatchSource, StopHandle, default_probe_service_directory};
