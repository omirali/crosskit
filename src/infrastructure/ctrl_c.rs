// The operator's stop request for the foreground watcher.
use super::filesystem::StopHandle;

/// Installs the process-wide Ctrl-C handler once. The handler only queues a stop request
/// through `stop`; the watch loop does all shutdown work on its own thread.
pub fn stop_on_ctrl_c(stop: StopHandle) -> Result<(), String> {
    ctrlc::set_handler(move || stop.request_stop())
        .map_err(|error| format!("install the Ctrl-C handler: {error}"))
}
