// `crosskit service`: installed against a stand-in `launchctl`, run as the process launchd
// would start, and asked about through `status`.
//
// No test reaches the launchd of the machine it runs on. `launchctl` is a script first on
// PATH that writes down what it was asked and keeps one file for "loaded"; `HOME` is the
// sandbox's, so the agent lands in a sandbox `Library/LaunchAgents`. The service itself is
// real: `service run` is started here exactly as the agent names it.
mod common;

use std::fs;
use std::thread;
use std::time::{Duration, Instant};

use common::service::{BOUND, ServiceSandbox, ask_socket, stderr, stdout, terminate};
use common::{Sandbox, WatchProcess};

#[test]
fn install_shows_what_it_will_write_and_a_no_writes_nothing() {
    let service = ServiceSandbox::new("service-install-no");

    let output = service.install("n\n");

    assert!(output.status.success(), "{}", stderr(&output));
    let said = stdout(&output);
    assert!(said.contains("<key>RunAtLoad</key>"), "{said}");
    assert!(said.contains("[y/N]"), "{said}");
    assert!(said.contains("Analysis: off"), "{said}");
    assert!(said.contains("Nothing was written."), "{said}");
    assert!(!service.agent().exists());
    assert!(!service.program().exists());
    assert_eq!(service.launchctl_calls(), "");
}

#[test]
fn install_copies_the_program_writes_the_agent_and_bootstraps_it_and_uninstall_undoes_it() {
    let service = ServiceSandbox::new("service-install-yes");

    let output = service.install("y\n");

    assert!(output.status.success(), "{}", stderr(&output));
    let agent = fs::read_to_string(service.agent()).unwrap();
    let program = service.program();
    assert!(program.exists(), "the program was not copied");
    assert!(
        agent.contains(&format!(
            "<string>{}</string>\n    <string>service</string>\n    <string>run</string>",
            program.display()
        )),
        "{agent}"
    );
    assert!(agent.contains("<key>KeepAlive</key>"), "{agent}");
    assert!(agent.contains("<key>CROSSKIT_DATA_DIR</key>"), "{agent}");
    let calls = service.launchctl_calls();
    assert!(
        calls.contains("bootstrap gui/") && calls.contains("dev.crosskit.watch.plist"),
        "{calls}"
    );

    let output = service.crosskit(&["service", "uninstall"]);

    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!service.agent().exists());
    assert!(!program.exists());
    assert!(service.launchctl_calls().contains("bootout gui/"));
    assert!(
        service.sandbox.data_directory.join("config.toml").exists(),
        "uninstall touched the data"
    );
}

#[test]
fn status_without_a_service_names_install() {
    let service = ServiceSandbox::new("service-status-none");

    let said = service.status();

    assert!(said.contains("not installed"), "{said}");
    assert!(said.contains("crosskit service install"), "{said}");
}

#[test]
fn restart_of_a_service_that_was_never_installed_is_refused_with_install() {
    let service = ServiceSandbox::new("service-restart-none");

    let output = service.crosskit(&["service", "restart"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("crosskit service install"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn the_service_keeps_search_current_refuses_a_second_instance_and_stops_on_termination() {
    let service = ServiceSandbox::new("service-run");
    let running = service.start();
    let pid = running.id();

    let said = service.status_saying("Passes: 1 completed");
    assert!(said.contains(&format!("process {pid}")), "{said}");
    assert!(
        said.contains(&format!("Context {}", Sandbox::CONTEXT)),
        "{said}"
    );
    assert!(said.contains("Analysis: off"), "{said}");
    assert!(said.contains("unchanged since the start"), "{said}");

    let second = service.crosskit(&["service", "run"]);
    assert!(!second.status.success());
    assert!(
        stderr(&second).contains(&format!("as process {pid}")),
        "{}",
        stderr(&second)
    );

    fs::write(
        service.sandbox.documents.join("fresh.md"),
        "quetzalcoatl arrived while the service watched",
    )
    .unwrap();
    let deadline = Instant::now() + BOUND;
    loop {
        let found = service.crosskit(&["search", "quetzalcoatl"]);
        if found.status.success() && stdout(&found).contains("fresh.md") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "search never found the new file: {}{}",
            stdout(&found),
            stderr(&found)
        );
        thread::sleep(Duration::from_millis(300));
    }

    fs::write(
        service.sandbox.data_directory.join("config.toml"),
        format!(
            "{}\n# edited after the start\n",
            fs::read_to_string(service.sandbox.data_directory.join("config.toml")).unwrap()
        ),
    )
    .unwrap();
    let said = service.status();
    assert!(said.contains("changed after the start"), "{said}");

    let status = terminate(running);
    assert!(status.success(), "the service ended with {status}");
    let journal = service.journal();
    assert!(journal.contains("Service started"), "{journal}");
    assert!(journal.contains("Analysis: off"), "{journal}");
    assert!(journal.contains("Service stopped."), "{journal}");
    assert!(
        !journal.contains("quetzalcoatl"),
        "the journal holds file content"
    );
    assert!(
        service.status().contains("not installed"),
        "a stopped service still reads as running"
    );
}

#[test]
fn the_service_runs_in_no_context_of_its_own() {
    let service = ServiceSandbox::new("service-context");

    let output = service.crosskit(&["--context", Sandbox::CONTEXT, "service", "status"]);

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("every configured context"),
        "{}",
        stderr(&output)
    );
}

/// One context held by a terminal's `index watch` is that context's problem: it waits, says
/// why, and the other context is watched all the same.
#[test]
fn a_context_held_by_another_writer_waits_and_the_other_is_watched() {
    let service = ServiceSandbox::new("service-two-contexts");
    let documents = &service.sandbox.documents;
    for name in ["alpha", "beta"] {
        fs::create_dir_all(documents.join(name)).unwrap();
        fs::write(
            documents.join(name).join("note.md"),
            format!("a note of {name}"),
        )
        .unwrap();
    }
    let root = fs::canonicalize(documents).unwrap();
    fs::write(
        service.sandbox.data_directory.join("config.toml"),
        format!(
            "[workspace.alpha]\ninclude = [\"{0}/alpha\"]\n[workspace.beta]\ninclude = [\"{0}/beta\"]\n",
            root.display()
        ),
    )
    .unwrap();
    let mut holder =
        WatchProcess::spawn_command(service.command(&["--context", "beta", "index", "watch"]));
    holder.wait_for_line("Pass 1");
    let running = service.start();

    let said = service.status_saying("It is tried again every minute.");
    assert!(said.contains("Context beta: not watching"), "{said}");
    let said = service.status_saying("Passes: 1 completed");
    assert!(said.contains("Context alpha: watching"), "{said}");
    // The service does not hold beta, so it does not answer for it, and the command reads
    // beta the way it always has — beside the terminal's watcher.
    assert!(
        ask_socket(
            &service,
            r#"{"protocol":1,"op":"search","context":"beta","arguments":["note","--json"]}"#
        )
        .contains(r#""refused":"not-served""#)
    );
    let beta = service.crosskit(&["--context", "beta", "search", "note", "--json"]);
    assert!(beta.status.success(), "{}", stderr(&beta));
    assert!(stdout(&beta).contains("note.md"), "{}", stdout(&beta));

    assert!(terminate(running).success());
    holder.interrupt();
    holder.wait_for_exit();
}

#[test]
fn stop_and_start_leave_the_agent_installed() {
    let service = ServiceSandbox::new("service-stop-start");
    assert!(service.install("y\n").status.success());

    let stopped = service.crosskit(&["service", "stop"]);
    assert!(stopped.status.success(), "{}", stderr(&stopped));
    assert!(
        stdout(&stopped).contains("Stopped."),
        "{}",
        stdout(&stopped)
    );
    assert!(service.agent().exists(), "stop removed the agent");
    assert!(service.launchctl_calls().contains("bootout gui/"));

    let again = service.crosskit(&["service", "stop"]);
    assert!(
        stdout(&again).contains("already stopped"),
        "{}",
        stdout(&again)
    );

    let started = service.crosskit(&["service", "start"]);
    assert!(started.status.success(), "{}", stderr(&started));
    assert_eq!(
        service.launchctl_calls().matches("bootstrap gui/").count(),
        2
    );
}

/// The person says yes: the service stops, the command writes, and the service is started
/// again once the command has ended — by a process of its own, which is why the test waits
/// for the call rather than finding it when the command returns.
#[test]
fn a_write_command_held_by_the_service_asks_stops_it_and_starts_it_again() {
    let service = ServiceSandbox::new("service-pause");
    assert!(service.install("y\n").status.success());
    let running = service.start();
    service.under_launchd(&running);
    service.status_saying("Passes: 1 completed");

    let output = service.reconcile_answering(Some("y\n"));

    assert!(
        output.status.success(),
        "{}{}",
        stdout(&output),
        stderr(&output)
    );
    let said = stdout(&output);
    assert!(said.contains("held by the background service"), "{said}");
    assert!(said.contains("[y/N]"), "{said}");
    assert!(
        said.contains("starts again when this command ends"),
        "{said}"
    );
    let calls = service.launchctl_asked("bootstrap gui/", 2);
    assert!(
        calls.rfind("bootout").unwrap() < calls.rfind("bootstrap").unwrap(),
        "the service was not started after it was stopped:\n{calls}"
    );
}

/// Without anyone to answer, the refusal stays — and names the two commands, not launchctl.
#[test]
fn a_write_command_with_a_closed_input_is_refused_with_service_stop() {
    let service = ServiceSandbox::new("service-pause-closed");
    let running = service.start();
    service.status_saying("Passes: 1 completed");

    let output = service.reconcile_answering(None);

    assert!(!output.status.success());
    let refusal = stderr(&output);
    assert!(refusal.contains("`crosskit service stop`"), "{refusal}");
    assert!(!refusal.contains("launchctl"), "{refusal}");
    assert!(!service.launchctl_calls().contains("bootout"));
    assert!(terminate(running).success());
}

/// The service answers a reader with the very line the command prints by itself — and says
/// so only for what it holds, in its own protocol, from the configuration it read.
#[test]
fn the_service_answers_reads_through_its_socket_with_the_commands_own_line() {
    let service = ServiceSandbox::new("service-reads");
    let running = service.start();
    let said = service.status_saying("Passes: 1 completed");
    assert!(said.contains("Reads: answered through"), "{said}");

    let hello = ask_socket(&service, r#"{"protocol":1,"op":"hello"}"#);
    assert!(hello.contains(r#""served":["sandbox"]"#), "{hello}");

    let served = ask_socket(
        &service,
        r#"{"protocol":1,"op":"search","arguments":["keeps","--json"]}"#,
    );
    let printed = service.crosskit(&["search", "keeps", "--json"]);
    assert!(printed.status.success(), "{}", stderr(&printed));
    assert_eq!(served, stdout(&printed).trim_end());
    assert!(served.contains("notes.md"), "{served}");

    assert!(
        ask_socket(&service, r#"{"protocol":2,"op":"hello"}"#)
            .contains(r#""refused":"protocol-mismatch""#)
    );
    assert!(
        ask_socket(
            &service,
            r#"{"protocol":1,"op":"search","arguments":["keeps","--json"],"config_digest":"not-this-one"}"#
        )
        .contains(r#""refused":"configuration-changed""#)
    );
    assert!(
        ask_socket(
            &service,
            r#"{"protocol":1,"op":"ask","arguments":["what"]}"#
        )
        .contains(r#""refused":"malformed""#),
        "ask is not served"
    );

    assert!(terminate(running).success());
    assert!(
        !service
            .sandbox
            .data_directory
            .join("service/crosskit.sock")
            .exists(),
        "the socket outlived the service"
    );
}

/// A task's map is read through the socket as the line `tasks map --json` prints, with every
/// node saying whether the owner said its reply. Building is not the service's: it asks a model.
#[test]
fn the_service_shows_a_task_map_and_leaves_building_to_the_command() {
    let service = ServiceSandbox::new("service-task-map");
    common::write_task_conversations(&service.sandbox.documents);
    service
        .sandbox
        .write_config_sections("", "send_content_outside = true\n");
    let running = service.start();
    service.status_saying("Passes: 1 completed");
    let confirmed = service.crosskit(&["tasks", "confirm", "bbbb", "--name", "Скидки"]);
    assert!(confirmed.status.success(), "{}", stderr(&confirmed));

    let calls = service.sandbox.data_directory.join("tool-calls");
    let (root, directory) = common::fake_map_engine(&calls);
    let built = service
        .command(&["tasks", "map", "Скидки", "--build"])
        .env("PATH", format!("{directory}:/usr/bin:/bin"))
        .output()
        .unwrap();
    assert!(built.status.success(), "{}", stderr(&built));

    let served = ask_socket(
        &service,
        r#"{"protocol":1,"op":"task-map","arguments":["Скидки","--json"]}"#,
    );
    let printed = service.crosskit(&["tasks", "map", "Скидки", "--json"]);
    assert!(printed.status.success(), "{}", stderr(&printed));
    assert_eq!(served, stdout(&printed).trim_end());
    let answer: serde_json::Value = serde_json::from_str(&served).unwrap();
    assert_eq!(
        answer["joined"]["main_line"][0]["title"],
        "Разобраться со скидкой"
    );
    assert_eq!(answer["conversations"][0]["forks"][0]["owner"], true);

    let refused = ask_socket(
        &service,
        r#"{"protocol":1,"op":"task-map","arguments":["Скидки","--build","--json"]}"#,
    );
    assert!(refused.contains(r#""category":"usage""#), "{refused}");
    assert_eq!(
        fs::read_to_string(&calls).unwrap().lines().count(),
        5,
        "the socket asked no model"
    );

    assert!(terminate(running).success());
    fs::remove_dir_all(root).unwrap();
}
