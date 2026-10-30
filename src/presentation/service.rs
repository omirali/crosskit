// `crosskit service`: the watcher of every context as a background service of the person's
// login session, started by launchd (ADR 0049).
use super::help;
use super::{service_control, service_install, service_run, service_status};

pub(super) const USAGE: &str = "Usage: crosskit service install     show what will be written, ask, then install and start\n       crosskit service status      whether it runs, which build, and every context's state\n       crosskit service logs        where the journal is, and its last lines\n       crosskit service stop        stop it until the next login or `service start`; the agent stays\n       crosskit service start       start it again\n       crosskit service restart     copy this build, read the configuration again, start again\n       crosskit service uninstall   stop it and remove the agent and the program copy; no data is touched";

pub(super) fn run(arguments: &[String], context: Option<&str>) -> Result<(), String> {
    if let Some(named) = context {
        return Err(format!(
            "the service watches every configured context at once, so it does not run in one; \
             drop --context {named}"
        ));
    }
    match arguments {
        [command] if command == "install" => service_install::install(),
        [command] if command == "uninstall" => service_install::uninstall(),
        [command] if command == "restart" => service_install::restart(),
        [command] if command == "status" => service_status::status(),
        [command] if command == "logs" => service_status::logs(),
        [command] if command == "stop" => service_control::stop_command(),
        [command] if command == "start" => service_control::start_command(None),
        // What a command that stopped the service for its own run leaves behind to start it
        // again once that run has ended. Not in the usage: nobody types it.
        [command, after, pid] if command == "start" && after == "--after" => {
            let pid = pid
                .parse()
                .map_err(|_| format!("--after needs a process number, not {pid:?}"))?;
            service_control::start_command(Some(pid))
        }
        // What launchd runs. Not in the usage: a person starts the service with `install`
        // or `restart`, and launchd is the one that runs this.
        [command] if command == "run" => service_run::run(),
        _ => Err(help::misuse(USAGE)),
    }
}
