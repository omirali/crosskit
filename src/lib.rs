//! Crosskit's core, offered as a library because the core will have more than one
//! consumer and the CLI binary beside this file is only the first of them.

mod application;
mod domain;
mod infrastructure;
mod presentation;

pub use presentation::run_to_exit_code;

/// The counters a diagnostic build reports; the global allocator itself belongs to an
/// executable, so the library only offers the wrapper and the binary installs it.
#[cfg(feature = "allocation-profile")]
pub use infrastructure::allocation_profile::{CountingAllocator, report_to_stderr};
