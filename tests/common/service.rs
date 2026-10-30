// A sandbox with a home of its own, a stand-in `launchctl` first on PATH and the service run
// as launchd would run it — shared by the test binaries about `crosskit service`.
//
// No test reaches the launchd of the machine it runs on. `launchctl` is a script that writes
// down what it was asked and keeps one file for "loaded"; `HOME` is the sandbox's, so the
// agent lands in a sandbox `Library/LaunchAgents`.
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use super::Sandbox;

/// Long enough for FSEvents, a bootstrap pass and the production debounce on a loaded
/// machine; every wait below ends as soon as what it waits for is there.
pub const BOUND: Duration = Duration::from_secs(60);

pub struct ServiceSandbox {
    pub sandbox: Sandbox,
    home: PathBuf,
    tools: PathBuf,
}

impl ServiceSandbox {
    pub fn new(name: &str) -> Self {
        let sandbox = Sandbox::new(name);
        let root = sandbox.data_directory.parent().unwrap().to_path_buf();
        let home = root.join("home");
        let tools = root.join("tools");
        fs::create_dir_all(&home).unwrap();
        fs::create_dir_all(&tools).unwrap();
        let calls = tools.join("calls");
        let loaded = tools.join("loaded");
        let script = format!(
            "#!/bin/sh\n\
             echo \"$*\" >> '{calls}'\n\
             case \"$1\" in\n\
               bootstrap) touch '{loaded}' ;;\n\
               bootout) rm -f '{loaded}'; [ -f '{service}' ] && kill -TERM \"$(cat '{service}')\"; true ;;\n\
               print) if [ -f '{loaded}' ]; then printf '\\tstate = not running\\n\\tlast exit code = 1\\n'; \
                      else echo 'Could not find service' >&2; exit 113; fi ;;\n\
             esac\n",
            calls = calls.display(),
            loaded = loaded.display(),
            service = tools.join("service.pid").display()
        );
        let launchctl = tools.join("launchctl");
        fs::write(&launchctl, script).unwrap();
        fs::set_permissions(&launchctl, fs::Permissions::from_mode(0o755)).unwrap();
        Self {
            sandbox,
            home,
            tools,
        }
    }

    pub fn command(&self, arguments: &[&str]) -> Command {
        let mut command = self.sandbox.command(arguments);
        command
            .env("HOME", &self.home)
            .env("PATH", format!("{}:/usr/bin:/bin", self.tools.display()));
        command
    }

    pub fn crosskit(&self, arguments: &[&str]) -> Output {
        self.command(arguments).output().unwrap()
    }

    /// `install`, answering the question with `answer`.
    pub fn install(&self, answer: &str) -> Output {
        let mut child = self
            .command(&["service", "install"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    pub fn launchctl_calls(&self) -> String {
        fs::read_to_string(self.tools.join("calls")).unwrap_or_default()
    }

    pub fn agent(&self) -> PathBuf {
        self.home
            .join("Library/LaunchAgents/dev.crosskit.watch.plist")
    }

    pub fn program(&self) -> PathBuf {
        self.sandbox.data_directory.join("bin/crosskit")
    }

    pub fn status(&self) -> String {
        let output = self.crosskit(&["service", "status"]);
        assert!(output.status.success(), "{}", stderr(&output));
        stdout(&output)
    }

    /// `status` until it says `needle`, or a failure with the last thing it said.
    pub fn status_saying(&self, needle: &str) -> String {
        let deadline = Instant::now() + BOUND;
        loop {
            let said = self.status();
            if said.contains(needle) {
                return said;
            }
            assert!(
                Instant::now() < deadline,
                "status never said {needle:?}; last:\n{said}"
            );
            thread::sleep(Duration::from_millis(200));
        }
    }

    pub fn start(&self) -> RunningService {
        RunningService(Some(
            self.command(&["service", "run"])
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        ))
    }

    /// Lets the stand-in `launchctl` stop this service on `bootout`, the way launchd would.
    pub fn under_launchd(&self, running: &RunningService) {
        fs::write(self.tools.join("service.pid"), running.id().to_string()).unwrap();
    }

    /// The stand-in's calls, waited for until `needle` has been asked `times` times.
    pub fn launchctl_asked(&self, needle: &str, times: usize) -> String {
        let deadline = Instant::now() + BOUND;
        loop {
            let calls = self.launchctl_calls();
            if calls.matches(needle).count() >= times {
                return calls;
            }
            assert!(
                Instant::now() < deadline,
                "launchctl was not asked {needle:?} {times} time(s); calls:\n{calls}"
            );
            thread::sleep(Duration::from_millis(200));
        }
    }

    /// A write command, answering its question with `answer`, or with a closed input.
    pub fn reconcile_answering(&self, answer: Option<&str>) -> Output {
        let mut command = self.command(&["index", "reconcile"]);
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let Some(answer) = answer else {
            return command.stdin(Stdio::null()).output().unwrap();
        };
        let mut child = command.stdin(Stdio::piped()).spawn().unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(answer.as_bytes())
            .unwrap();
        child.wait_with_output().unwrap()
    }

    pub fn journal(&self) -> String {
        fs::read_to_string(self.sandbox.data_directory.join("service/service.log"))
            .unwrap_or_default()
    }
}

/// One request line to the service's socket, and its answer line.
///
/// The socket's full path does not fit a socket address in a temporary directory, so the
/// test connects by its bare name from inside `service/` — under a lock, because the working
/// directory belongs to the whole test process.
pub fn ask_socket(service: &ServiceSandbox, request: &str) -> String {
    static WORKING_DIRECTORY: Mutex<()> = Mutex::new(());
    let _held = WORKING_DIRECTORY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let previous = std::env::current_dir().unwrap();
    std::env::set_current_dir(service.sandbox.data_directory.join("service")).unwrap();
    let stream = UnixStream::connect("crosskit.sock");
    std::env::set_current_dir(previous).unwrap();
    let mut stream = stream.expect("the service's socket answers");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    stream.write_all(request.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line).unwrap();
    line.trim_end().to_string()
}

pub fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

pub fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// A service this test started, killed when the test ends however it ends: a `Child` is not
/// killed when dropped, and a service that outlived a failed assertion kept watching — and
/// writing into — a sandbox that had already been removed.
pub struct RunningService(Option<Child>);

impl RunningService {
    pub fn id(&self) -> u32 {
        self.0.as_ref().map_or(0, Child::id)
    }
}

impl Drop for RunningService {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// launchd's way of stopping the service, and a bounded wait for it to end.
pub fn terminate(mut running: RunningService) -> std::process::ExitStatus {
    let mut child = running.0.take().expect("the service is running");
    Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .unwrap();
    let deadline = Instant::now() + BOUND;
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            return status;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            panic!("the service did not stop within {BOUND:?}");
        }
        thread::sleep(Duration::from_millis(100));
    }
}
