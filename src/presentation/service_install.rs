// `service install`, `restart` and `uninstall`: what the service is on this machine — the
// program it runs, the agent launchd reads — and nothing about what it watches.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use crate::infrastructure::service::{
    LaunchAgent, Launchctl, ServiceLayout, copy_program, file_stamp,
};
use crate::infrastructure::{LoadedIndexPolicy, load_index_policy, utc_moment};

use super::confirmation::confirmed;
use super::service_control;
use super::service_run::ANALYSIS_OFF;

/// The variables the service is started with, beside what launchd always sets.
///
/// `PATH`, because launchd's own is `/usr/bin:/bin:/usr/sbin:/sbin` and the analysis engine is
/// a program found on the person's — without it a service told to analyse would find no
/// `claude`. The two data variables only when set, so a service installed from a terminal
/// that points at another data directory watches that one and not the default.
const CARRIED_VARIABLES: &[&str] = &["PATH", "CROSSKIT_DATA_DIR", "CROSSKIT_CONFIG"];

pub(super) fn install() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let loaded = load_index_policy().map_err(|error| error.to_string())?;
    let contexts: Vec<String> = loaded
        .workspaces()
        .names()
        .iter()
        .map(ToString::to_string)
        .collect();
    if contexts.is_empty() {
        return Err(
            "the configuration declares no context, so a service would have nothing to watch; \
             `crosskit setup` proposes them"
                .to_string(),
        );
    }
    let source = this_program()?;
    let agent = LaunchAgent::new(
        &layout.program(),
        carried_environment(),
        &layout.launchd_output(),
    )
    .render();

    println!(
        "The background service watches every context ({}) from login on, and launchd starts \
         it again if it stops on a failure.",
        contexts.join(", ")
    );
    println!();
    println!("It will:");
    println!("  copy this program  {}", source.display());
    println!(
        "  to                 {} (build of {})",
        layout.program().display(),
        build_time(&source)
    );
    println!(
        "  write              {}{}",
        layout.launch_agent().display(),
        if layout.launch_agent().exists() {
            " (replacing the one there)"
        } else {
            ""
        }
    );
    for line in agent.lines() {
        println!("    {line}");
    }
    println!("  and load it into launchd for this login session, which starts it now.");
    println!();
    println!("{}", analysis_sentence(&loaded));
    println!(
        "Journal: {}. State: `crosskit service status`.",
        layout.journal().display()
    );
    println!();
    if !confirmed("Install and start the service?")? {
        println!("Nothing was written.");
        return Ok(());
    }

    copy_program(&source, &layout.program())
        .map_err(|error| format!("the program could not be copied: {error}"))?;
    fs::create_dir_all(layout.launch_agents())
        .and_then(|()| fs::write(layout.launch_agent(), &agent))
        .map_err(|error| format!("the launch agent could not be written: {error}"))?;
    let launchctl = Launchctl::new();
    service_control::stop(&launchctl, &layout)?;
    launchctl.bootstrap(&layout.launch_agent())?;
    println!(
        "Installed and started. `crosskit service status` shows each context once it has \
         opened; after a new build, `crosskit service restart` runs it."
    );
    Ok(())
}

pub(super) fn restart() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    if !layout.launch_agent().exists() {
        return Err(
            "the service is not installed, so there is nothing to restart; `crosskit service \
             install` installs it"
                .to_string(),
        );
    }
    let source = this_program()?;
    let target = layout.program();
    let before = file_stamp(&target).ok();
    if !same_file(&source, &target) {
        copy_program(&source, &target)
            .map_err(|error| format!("the program could not be copied: {error}"))?;
    }
    let launchctl = Launchctl::new();
    service_control::stop(&launchctl, &layout)?;
    launchctl.bootstrap(&layout.launch_agent())?;

    let after = file_stamp(&target).ok();
    let build = match (before, after) {
        (Some(before), Some(after)) if before == after => {
            format!(
                "the same build as before, of {}",
                utc_moment(after.modified_ms)
            )
        }
        (before, Some(after)) => format!(
            "build of {}{}",
            utc_moment(after.modified_ms),
            before.map_or_else(String::new, |before| format!(
                ", replacing the build of {}",
                utc_moment(before.modified_ms)
            ))
        ),
        (_, None) => "a program that cannot be described".to_string(),
    };
    println!("Restarted: {} — {build}.", target.display());
    println!("The configuration is read again; `crosskit service status` shows each context.");
    Ok(())
}

pub(super) fn uninstall() -> Result<(), String> {
    let layout = ServiceLayout::resolve().map_err(|error| error.to_string())?;
    let launchctl = Launchctl::new();
    let was_loaded = launchctl.state()?.loaded;
    let agent = layout.launch_agent();
    let program = layout.program();
    if !was_loaded && !agent.exists() && !program.exists() {
        println!("The service is not installed; nothing was changed.");
        return Ok(());
    }
    service_control::stop(&launchctl, &layout)?;
    remove_if_present(&agent, "the launch agent")?;
    remove_if_present(&program, "the program copy")?;
    println!(
        "Uninstalled: launchd no longer starts the service, and {} and {} are removed.",
        agent.display(),
        program.display()
    );
    println!(
        "No data was touched: the graphs, search indexes, configuration, journal ({}) and \
         status stay where they are.",
        layout.journal().display()
    );
    Ok(())
}

fn remove_if_present(path: &Path, what: &str) -> Result<(), String> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("{what} could not be removed: {error}")),
    }
}

/// What the service will do with the queue, said before it is installed: nothing that costs
/// money happens unless the configuration says so.
fn analysis_sentence(loaded: &LoadedIndexPolicy) -> String {
    match loaded.watch_analysis_limit() {
        None => ANALYSIS_OFF.to_string(),
        Some(limit) => format!(
            "Analysis: on — `[watch] analyze` is set, so after each pass every context works at \
             most {limit} job(s) of its queue; a context that keeps its content to itself is \
             analysed locally, one that allows it is sent to the configured engine."
        ),
    }
}

fn this_program() -> Result<PathBuf, String> {
    std::env::current_exe()
        .and_then(fs::canonicalize)
        .map_err(|error| format!("this program's own file could not be found: {error}"))
}

fn same_file(source: &Path, target: &Path) -> bool {
    fs::canonicalize(target).is_ok_and(|target| target == source)
}

fn build_time(program: &Path) -> String {
    file_stamp(program).map_or_else(
        |_| "an unknown time".to_string(),
        |stamp| utc_moment(stamp.modified_ms),
    )
}

fn carried_environment() -> BTreeMap<String, String> {
    CARRIED_VARIABLES
        .iter()
        .filter_map(|name| {
            std::env::var(name)
                .ok()
                .map(|value| (name.to_string(), value))
        })
        .collect()
}
