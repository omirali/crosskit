/// A diagnostic build counts what Crosskit's own code allocates; a released Crosskit
/// runs on the system allocator. It is installed here and not in the library because a
/// global allocator belongs to the executable.
#[cfg(feature = "allocation-profile")]
#[global_allocator]
static ALLOCATOR: crosskit::CountingAllocator = crosskit::CountingAllocator;

fn main() {
    let code = crosskit::run_to_exit_code();
    // Reported before the exit status is acted on, so an interrupted pass is measured too.
    #[cfg(feature = "allocation-profile")]
    crosskit::report_to_stderr();
    std::process::exit(code);
}
