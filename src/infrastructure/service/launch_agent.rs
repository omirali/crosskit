// The launch agent: the one file launchd reads to start the service at login and to start it
// again after a crash.
use std::collections::BTreeMap;
use std::path::Path;

/// The name launchd knows the service by.
pub const SERVICE_LABEL: &str = "dev.crosskit.watch";

/// The shortest time between two starts. launchd's own default is ten seconds; a service that
/// fails on start — a configuration that does not load, a database another process holds —
/// would then write its refusal into the journal six times a minute for as long as nobody
/// looks. A minute keeps the restart and makes the loop cheap.
const THROTTLE_SECONDS: u32 = 60;

/// How long launchd waits after asking the service to stop before it kills it. The watcher
/// stops after the pass in flight and runs one more for pending changes, and a pass over a
/// large context takes tens of seconds; launchd's default of twenty would kill it mid-pass.
const EXIT_TIMEOUT_SECONDS: u32 = 120;

/// What the agent says, before it is written: the program, the variables it runs with and
/// where launchd puts what the program could not write itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchAgent {
    program: String,
    environment: BTreeMap<String, String>,
    output: String,
}

impl LaunchAgent {
    pub fn new(program: &Path, environment: BTreeMap<String, String>, output: &Path) -> Self {
        Self {
            program: program.to_string_lossy().into_owned(),
            environment,
            output: output.to_string_lossy().into_owned(),
        }
    }

    /// The property list, exactly as it is written and as `install` shows it first.
    ///
    /// `KeepAlive` restarts the service only when it did not exit cleanly: a service asked to
    /// stop exits with success and stays stopped, a crash comes back after the throttle.
    pub fn render(&self) -> String {
        let mut text = String::from(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\">\n<dict>\n",
        );
        text.push_str(&format!(
            "  <key>Label</key>\n  <string>{}</string>\n",
            escaped(SERVICE_LABEL)
        ));
        text.push_str("  <key>ProgramArguments</key>\n  <array>\n");
        for argument in [self.program.as_str(), "service", "run"] {
            text.push_str(&format!("    <string>{}</string>\n", escaped(argument)));
        }
        text.push_str("  </array>\n");
        text.push_str("  <key>RunAtLoad</key>\n  <true/>\n");
        text.push_str(
            "  <key>KeepAlive</key>\n  <dict>\n    <key>SuccessfulExit</key>\n    <false/>\n  </dict>\n",
        );
        text.push_str(&format!(
            "  <key>ThrottleInterval</key>\n  <integer>{THROTTLE_SECONDS}</integer>\n"
        ));
        text.push_str(&format!(
            "  <key>ExitTimeOut</key>\n  <integer>{EXIT_TIMEOUT_SECONDS}</integer>\n"
        ));
        if !self.environment.is_empty() {
            text.push_str("  <key>EnvironmentVariables</key>\n  <dict>\n");
            for (name, value) in &self.environment {
                text.push_str(&format!(
                    "    <key>{}</key>\n    <string>{}</string>\n",
                    escaped(name),
                    escaped(value)
                ));
            }
            text.push_str("  </dict>\n");
        }
        for key in ["StandardOutPath", "StandardErrorPath"] {
            text.push_str(&format!(
                "  <key>{key}</key>\n  <string>{}</string>\n",
                escaped(&self.output)
            ));
        }
        text.push_str("</dict>\n</plist>\n");
        text
    }
}

/// A path or a variable is text the person chose, and `&` or `<` in it would make the file
/// something launchd refuses to load.
fn escaped(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::path::Path;

    use super::LaunchAgent;

    fn agent(environment: &[(&str, &str)]) -> String {
        LaunchAgent::new(
            Path::new("/Users/alice/Library/Application Support/Crosskit/bin/crosskit"),
            environment
                .iter()
                .map(|(name, value)| (name.to_string(), value.to_string()))
                .collect::<BTreeMap<_, _>>(),
            Path::new("/Users/alice/Library/Application Support/Crosskit/service/launchd.log"),
        )
        .render()
    }

    #[test]
    fn the_agent_starts_the_copy_at_login_and_again_only_after_a_failure() {
        let text = agent(&[]);
        assert!(text.contains("<string>dev.crosskit.watch</string>"));
        assert!(text.contains(
            "<string>/Users/alice/Library/Application Support/Crosskit/bin/crosskit</string>\n    <string>service</string>\n    <string>run</string>"
        ));
        assert!(text.contains("<key>RunAtLoad</key>\n  <true/>"));
        assert!(text.contains("<key>SuccessfulExit</key>\n    <false/>"));
        assert!(text.contains("<key>ThrottleInterval</key>\n  <integer>60</integer>"));
        assert!(!text.contains("EnvironmentVariables"));
    }

    #[test]
    fn text_the_person_chose_is_escaped() {
        let text = agent(&[("CROSSKIT_DATA_DIR", "/Users/alice/a&b <c>")]);
        assert!(text.contains("<string>/Users/alice/a&amp;b &lt;c&gt;</string>"));
    }
}
