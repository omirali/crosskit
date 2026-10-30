mod ask;
#[cfg(test)]
mod bridge_measurement_tests;
mod confirmation;
mod context_draft;
mod context_wording;
mod contexts;
mod doctor;
mod exit;
mod graph;
mod graph_state;
mod help;
mod index;
mod index_context;
mod index_jobs;
mod index_recover;
mod index_review;
mod index_runs;
mod index_watch;
mod knowledge;
mod machine_answer;
mod machine_graph;
mod machine_knowledge;
mod migrate;
mod migrate_from_earlier_name;
#[cfg(test)]
mod one_voice_tests;
mod period;
mod policy;
mod reading_ground;
mod refusal;
mod root_report;
mod run_cost;
mod scope_directory;
mod search;
mod search_index;
mod served_reads;
mod service;
mod service_client;
mod service_control;
mod service_install;
mod service_pause;
mod service_run;
mod service_status;
mod setup;
mod setup_dialog;
mod setup_draft;
mod setup_indexing;
mod setup_report;
mod setup_wording;
mod status;
/// Stage 37: where a file was worked on, and what a conversation worked on.
mod task_map;
mod tasks;
mod touches;
mod watch_narration;
mod workspace;

use std::env;
use std::io::{self, IsTerminal};
use std::path::PathBuf;

use crate::application::{GreetingService, OnboardingState};
use crate::domain::{AICodingTool, AICodingToolKind, AIToolSource, AIToolStats};
use crate::infrastructure::{
    ConsoleGreetingPublisher, OnboardingStorage, analyze_tool, earlier_name_environment,
    list_detected_tools,
};

const RESET: &str = "\x1b[0m";
const BOLD: &str = "\x1b[1m";
const DIM: &str = "\x1b[2m";
const GREEN: &str = "\x1b[32m";
const CYAN: &str = "\x1b[36m";

struct CliStyle {
    color: bool,
}

impl CliStyle {
    fn detect() -> Self {
        Self {
            color: io::stdout().is_terminal() && env::var_os("NO_COLOR").is_none(),
        }
    }

    fn paint(&self, text: &str, style: &str) -> String {
        if self.color {
            format!("{style}{text}{RESET}")
        } else {
            text.to_string()
        }
    }
}

pub use exit::run_to_exit_code;

fn run() -> Result<(), String> {
    refuse_variables_under_the_earlier_name()?;
    // The context is named the same way for every command, so it is taken out once here
    // and the command grammars stay as they were.
    let (context, args) = workspace::take_context(env::args().skip(1).collect())?;
    if answered_a_question_about_the_program(&args)? {
        return Ok(());
    }
    let context = context.as_deref();
    if args.as_slice() == ["doctor"] {
        return doctor::run(context);
    }
    if args.first().is_some_and(|argument| argument == "status") {
        return status::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "setup") {
        // Setup proposes the contexts and writes them, so it cannot be run inside one. The
        // option would name a context that does not exist yet.
        if let Some(named) = context {
            return Err(format!(
                "setup proposes the contexts for a directory and writes them, so it does not \
                 run in one; drop --context {named}, or use `crosskit --context {named} index \
                 init <directory>` to index a context that already exists"
            ));
        }
        return setup::run(&args[1..]);
    }
    if args.first().is_some_and(|argument| argument == "index") {
        return index::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "migrate") {
        return migrate::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "policy") {
        return policy::run(&args[1..]);
    }
    if args.first().is_some_and(|argument| argument == "contexts") {
        // The command proposes what the contexts could be, so it cannot be run inside one.
        // Ignoring the option instead would answer a question that was not asked.
        if let Some(named) = context {
            return Err(format!(
                "contexts propose reads a directory and proposes the contexts for it, so it \
                 does not run in one; drop --context {named}"
            ));
        }
        return contexts::run(&args[1..]);
    }
    if args.first().is_some_and(|argument| argument == "search") {
        return search::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "ask") {
        return ask::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "tasks") {
        return tasks::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "touches") {
        return touches::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "graph") {
        return graph::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "knowledge") {
        return knowledge::run(&args[1..], context);
    }
    if args.first().is_some_and(|argument| argument == "service") {
        return service::run(&args[1..], context);
    }

    // A word that is not a command is refused by name, and by the same sentence `--help`
    // uses. Before this it fell through to the overview below: a mistyped command printed a
    // list of AI tools and ended with success, so a script could not tell that nothing it
    // asked for had happened. A run that named no command at all still reaches the overview,
    // which is what `crosskit` on its own has always printed.
    if let Some(name) = args.first().filter(|argument| !argument.starts_with('-')) {
        return Err(help::unknown_command(name));
    }

    let home = env::var("HOME").unwrap_or_else(|_| ".".to_string());
    let onboarding_path = PathBuf::from(home).join(".crosskit_onboarding");
    let storage = OnboardingStorage::new(onboarding_path.clone());

    if args.iter().any(|arg| arg == "--reset-onboarding")
        && let Err(err) = storage.reset()
    {
        eprintln!(
            "the onboarding marker could not be removed, so the overview below may not be shown again: {err}"
        );
    }

    let onboarding_state = OnboardingState::from_marker_exists(storage.has_completed());
    let should_show = onboarding_state.should_show_onboarding();
    let tools = list_detected_tools();

    render_tool_overview(&tools);

    if should_show && let Err(err) = storage.mark_completed() {
        eprintln!(
            "the onboarding marker could not be written, so this overview will be shown again: {err}"
        );
    }

    let service = GreetingService;
    let greeting = service.execute();
    let publisher = ConsoleGreetingPublisher;

    publisher.publish(greeting.message());
    Ok(())
}

/// A variable still set under the product's earlier name is never ignored in silence: alone it
/// stops the run with the name to set instead, beside its current name it loses and says so.
/// Said on stderr, so a `--json` answer on stdout stays one object.
fn refuse_variables_under_the_earlier_name() -> Result<(), String> {
    let found = earlier_name_environment();
    for variable in &found.overridden {
        eprintln!(
            "{} is set and ignored: {} is set too, and it is the one read.",
            variable.earlier, variable.current
        );
    }
    if found.unrenamed.is_empty() {
        return Ok(());
    }
    Err(format!(
        "the product was renamed from Crosscode and these variables still have its earlier \
         name; nothing was read or written. Set {} instead",
        found
            .unrenamed
            .iter()
            .map(|variable| format!("{} (not {})", variable.current, variable.earlier))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}

/// Whether this run was a question about the program rather than a command to it.
///
/// Answered here, before any command is reached, because `--help` and `--version` mean the
/// same thing wherever they appear and no command should have to know them. `true` means the
/// run has been answered and there is nothing else to do.
///
/// The arguments are compared whole, never searched for a substring, so a question that
/// contains the word — `crosskit ask "what does --help do"` — is a question and not a
/// request for help.
fn answered_a_question_about_the_program(args: &[String]) -> Result<bool, String> {
    if args.iter().any(|argument| argument == "--version") {
        println!("crosskit {}", help::version());
        return Ok(true);
    }
    if !args.iter().any(|argument| argument == "--help") {
        return Ok(false);
    }
    // `crosskit <command> --help` is about that command; `crosskit --help` is about all
    // of them. A word that is not a command is refused by name rather than answered with
    // the overview, because the overview would not say what was wrong with what was typed.
    let named = args
        .first()
        .filter(|argument| !argument.starts_with("--"))
        .map(String::as_str);
    match named {
        None => help::print_overview(),
        Some(name) => match help::command_named(name) {
            Some(command) => help::print_command(command),
            None => return Err(help::unknown_command(name)),
        },
    }
    Ok(true)
}

fn render_tool_overview(tools: &[AICodingTool]) {
    let style = CliStyle::detect();

    println!();
    println!("╭──────────────────────────────────────────────────────────╮");
    println!(
        "│  {}  {}                                            │",
        style.paint("◆", CYAN),
        style.paint("CROSSKIT", BOLD)
    );
    println!("│     Your AI coding workspace                             │");
    println!("╰──────────────────────────────────────────────────────────╯");
    println!();
    println!(
        "  {}  Found {}",
        style.paint("●", GREEN),
        tool_count_label(tools.len())
    );
    println!();

    if tools.is_empty() {
        println!("  ◌  No supported tools found");
        println!(
            "     {}",
            style.paint("Checked common CLI and application paths.", DIM)
        );
    } else {
        for tool in tools {
            render_tool(tool, &style);
        }
    }

    println!();
    println!(
        "  {} Run {} to scan again.",
        style.paint("›", CYAN),
        style.paint("crosskit --reset-onboarding", BOLD)
    );
    println!();
}

fn render_tool(tool: &AICodingTool, style: &CliStyle) {
    let path = tool.path.as_deref().unwrap_or("Unknown path");
    let stats = analyze_tool(tool);
    let source = match tool.source {
        AIToolSource::Cli => "CLI",
        AIToolSource::AppBundle => "APP",
        AIToolSource::CustomPath => "CUSTOM",
    };

    println!(
        "  {}  {}  {}",
        tool_icon(&tool.kind),
        style.paint(tool_display_name(&tool.kind), BOLD),
        style.paint(&format!("[{source}]"), GREEN)
    );
    println!("     {}", render_stats(&stats, style));
    println!("     {}", style.paint(path, DIM));
    println!();
}

fn render_stats(stats: &AIToolStats, style: &CliStyle) -> String {
    let chats = stats
        .chats
        .map(format_number)
        .unwrap_or_else(|| "—".to_string());
    let tokens = stats
        .tokens_used
        .map(format_compact_number)
        .unwrap_or_else(|| "—".to_string());
    let limit = stats
        .limit_remaining_percent
        .map(|value| format!("{value:.0}% left"))
        .unwrap_or_else(|| "—".to_string());
    let disk = format_bytes(stats.disk_bytes);

    format!(
        "{}  {}   {}  {}   {}  {}   {}  {}",
        style.paint("Chats", DIM),
        chats,
        style.paint("Tokens", DIM),
        tokens,
        style.paint("Limit", DIM),
        limit,
        style.paint("Disk", DIM),
        disk
    )
}

pub(super) fn format_number(value: u64) -> String {
    let digits = value.to_string();
    let mut result = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            result.push(',');
        }
        result.push(character);
    }
    result
}

fn format_compact_number(value: u64) -> String {
    match value {
        0..=999 => value.to_string(),
        1_000..=999_999 => format_compact(value, 1_000, "K"),
        1_000_000..=999_999_999 => format_compact(value, 1_000_000, "M"),
        _ => format_compact(value, 1_000_000_000, "B"),
    }
}

fn format_compact(value: u64, divisor: u64, suffix: &str) -> String {
    let amount = value as f64 / divisor as f64;
    if amount >= 100.0 || amount.fract() < 0.05 {
        format!("{amount:.0}{suffix}")
    } else {
        format!("{amount:.1}{suffix}")
    }
}

pub(super) fn format_bytes(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = KIB * 1024;
    const GIB: u64 = MIB * 1024;

    if bytes < KIB {
        format!("{bytes} B")
    } else if bytes < MIB {
        format!("{:.1} KB", bytes as f64 / KIB as f64)
    } else if bytes < GIB {
        format!("{:.1} MB", bytes as f64 / MIB as f64)
    } else {
        format!("{:.1} GB", bytes as f64 / GIB as f64)
    }
}

fn tool_icon(kind: &AICodingToolKind) -> &'static str {
    match kind {
        AICodingToolKind::ClaudeCode => "✦",
        AICodingToolKind::Codex => "◎",
        AICodingToolKind::Cursor => "◆",
        AICodingToolKind::OpenCode => "◇",
        AICodingToolKind::GitHubCopilot => "◈",
        AICodingToolKind::ChatGPT => "◉",
        AICodingToolKind::Other(_) => "•",
    }
}

fn tool_display_name(kind: &AICodingToolKind) -> &str {
    match kind {
        AICodingToolKind::ClaudeCode => "Claude Code",
        AICodingToolKind::Codex => "Codex",
        AICodingToolKind::Cursor => "Cursor",
        AICodingToolKind::OpenCode => "OpenCode",
        AICodingToolKind::GitHubCopilot => "GitHub Copilot",
        AICodingToolKind::ChatGPT => "ChatGPT",
        AICodingToolKind::Other(value) => value.as_str(),
    }
}

fn tool_count_label(count: usize) -> String {
    match count {
        0 => "no AI coding tools".to_string(),
        1 => "1 AI coding tool".to_string(),
        count => format!("{count} AI coding tools"),
    }
}

#[cfg(test)]
mod tests {
    use super::{
        format_bytes, format_compact_number, format_number, tool_count_label, tool_display_name,
        tool_icon,
    };
    use crate::domain::AICodingToolKind;

    #[test]
    fn every_supported_tool_has_cli_identity() {
        let tools = [
            (AICodingToolKind::ClaudeCode, "Claude Code", "✦"),
            (AICodingToolKind::Codex, "Codex", "◎"),
            (AICodingToolKind::Cursor, "Cursor", "◆"),
            (AICodingToolKind::OpenCode, "OpenCode", "◇"),
            (AICodingToolKind::GitHubCopilot, "GitHub Copilot", "◈"),
            (AICodingToolKind::ChatGPT, "ChatGPT", "◉"),
        ];

        for (kind, name, icon) in tools {
            assert_eq!(tool_display_name(&kind), name);
            assert_eq!(tool_icon(&kind), icon);
        }
    }

    #[test]
    fn tool_count_is_readable() {
        assert_eq!(tool_count_label(0), "no AI coding tools");
        assert_eq!(tool_count_label(1), "1 AI coding tool");
        assert_eq!(tool_count_label(3), "3 AI coding tools");
    }

    #[test]
    fn metrics_are_human_readable() {
        assert_eq!(format_number(12_345_678), "12,345,678");
        assert_eq!(format_compact_number(1_250), "1.2K");
        assert_eq!(format_compact_number(12_500_000), "12.5M");
        assert_eq!(format_bytes(1_610_612_736), "1.5 GB");
    }
}
