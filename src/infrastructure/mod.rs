use std::fs;
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use crate::domain::{AICodingTool, AICodingToolKind, AIToolStats};

#[cfg(feature = "allocation-profile")]
pub mod allocation_profile;
pub mod analysis;
pub mod anthropic;
mod app_directories;
mod ask_cache;
#[cfg(test)]
mod ask_pipeline_tests;
/// Transcripts read into conversations. The reader and the evidence builder are production;
/// Stage 31's measurement lives beside them behind its feature — see `chats/mod.rs`.
pub mod chats;
pub mod config;
#[cfg(test)]
mod context_proposal_tests;
/// Stage 38's split measurement, behind its own feature and never in a release build.
#[cfg(all(test, feature = "context-split-measurement"))]
mod context_split;
/// Stage 34's coverage audit, behind its own feature and never in a release build.
#[cfg(all(test, feature = "coverage-audit"))]
mod coverage;
mod ctrl_c;
/// The stage «Карта решений»: its measurement, behind its own feature and never in a release
/// build.
#[cfg(all(test, feature = "decision-map-measurement"))]
mod decision_map;
/// The product's earlier name: variables still set under it, and the one move of its data.
mod earlier_name_environment;
mod earlier_name_move;
#[cfg(test)]
mod exclusion_noise_tests;
pub mod filesystem;
#[cfg(test)]
mod index_pipeline_tests;
pub mod insight_run;
#[cfg(test)]
mod insight_run_tests;
pub mod ladybug;
mod local_calendar;
/// What `setup` reads beside the walk: transcript folders, folders without git, Codex, git's
/// mail domain.
pub mod machine_survey;
/// The service raises its own limit on open files (ADR 0060).
mod open_file_limit;
mod output_stream;
/// Where a measurement may write real material: outside the repository.
#[cfg(all(
    test,
    any(
        feature = "task-stitching-measurement",
        feature = "decision-map-measurement"
    )
))]
mod outside_repository;
#[cfg(test)]
mod period_measurement_tests;
#[cfg(test)]
mod period_pipeline_tests;
pub mod search;
#[cfg(test)]
mod search_pipeline_tests;
/// The background service: its copy of the program, its launch agent, journal and status.
pub mod service;
mod system_clock;
/// The owner's decisions about tasks, in a file beside the graph (ADR 0053).
mod task_book_file;
/// Every conversation's decision map, in files beside the graph (ADR 0055).
mod task_map_file;
#[cfg(test)]
mod task_pipeline_tests;
/// The stage «Сшивка бесед»: its measurement, behind its own feature and never in a release
/// build.
#[cfg(all(test, feature = "task-stitching-measurement"))]
mod task_stitching;
mod utc_stamp;
#[cfg(test)]
mod watch_pipeline_tests;
/// Replacing a small file of decisions whole.
mod whole_file;

pub use app_directories::{data_directory, home_directory};
pub use ask_cache::{FileAnswerCache, ask_cache_directory};
pub use config::{
    ConfigError, LoadedIndexPolicy, config_file, load_index_policy, write_index_policy,
};
pub use ctrl_c::stop_on_ctrl_c;
pub use earlier_name_environment::earlier_name_environment;
pub use earlier_name_move::{EarlierNameMove, MoveRefusal, move_from_earlier_name};
pub use filesystem::{
    DescriptorRelativeWalker, DiskScopeProbe, NotifyWatchSource, default_probe_service_directory,
    reads_creation_times,
};
pub use ladybug::{
    GraphPresence, LadybugDatabaseManager, LadybugGraphMaintenance, LadybugSearchGraph,
    context_free_graph, context_free_graph_reason, graph_presence_of,
};
pub use local_calendar::local_today;
pub use open_file_limit::raise_open_file_limit;
pub use output_stream::{OutputStream, reading_end_is_gone};
pub use search::{ContextSearchIndex, built_in_period_phrases, search_directory};
pub use system_clock::SystemClock;
pub use task_book_file::TaskBookFile;
pub use task_map_file::TaskMapFiles;
pub use utc_stamp::{utc_moment, utc_stamp};

pub struct ConsoleGreetingPublisher;

impl ConsoleGreetingPublisher {
    pub fn publish(&self, message: &str) {
        println!("{}", message);
    }
}

pub struct OnboardingStorage {
    path: PathBuf,
}

impl OnboardingStorage {
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn has_completed(&self) -> bool {
        self.path.exists()
    }

    pub fn reset(&self) -> std::io::Result<()> {
        if self.path.exists() {
            fs::remove_file(&self.path)?;
        }
        Ok(())
    }

    pub fn mark_completed(&self) -> std::io::Result<()> {
        let parent = self
            .path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        fs::create_dir_all(parent)?;
        fs::write(&self.path, "completed")?;
        Ok(())
    }
}

pub struct AICodingToolDetector;

impl AICodingToolDetector {
    pub fn detect_installed_tools(&self) -> Vec<AICodingTool> {
        let supported: &[(&str, &[&str])] = &[
            ("claude", &["claude", "claude-code"]),
            ("codex", &["codex"]),
            ("cursor", &["cursor"]),
            ("opencode", &["opencode", "open-code"]),
            (
                "github-copilot",
                &["github-copilot", "gh-copilot", "copilot"],
            ),
            ("chatgpt", &["chatgpt", "chat-gpt", "ChatGPT"]),
        ];

        let mut found = Vec::new();
        let search_roots = Self::search_roots();

        for (display_name, aliases) in supported.iter() {
            for alias in *aliases {
                if let Some(path) = Self::resolve_tool_path(alias, &search_roots) {
                    found.push(AICodingTool::new(display_name, Some(path), None));
                    break;
                }
            }
        }

        found
    }

    fn search_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();

        if let Ok(home) = std::env::var("HOME") {
            roots.push(PathBuf::from(&home));
            roots.push(PathBuf::from(&home).join(".local/bin"));
            roots.push(PathBuf::from(&home).join(".cargo/bin"));
            roots.push(PathBuf::from(&home).join("Applications"));
            roots.push(PathBuf::from(&home).join(".nvm/versions/node"));
        }

        roots.push(PathBuf::from("/opt/homebrew/bin"));
        roots.push(PathBuf::from("/usr/local/bin"));
        roots.push(PathBuf::from("/usr/bin"));
        roots.push(PathBuf::from("/Applications"));

        roots
    }

    fn resolve_tool_path(tool_name: &str, search_roots: &[PathBuf]) -> Option<String> {
        let mut candidates = Vec::new();

        for root in search_roots {
            candidates.push(root.join(tool_name));
            candidates.push(root.join(format!("{tool_name}.app")));
            candidates.push(root.join(format!("{tool_name}.app/Contents/MacOS/{tool_name}")));
            candidates.push(root.join("bin").join(tool_name));
            candidates.push(root.join("Applications").join(format!("{tool_name}.app")));
        }

        if let Ok(path) = std::process::Command::new("sh")
            .arg("-c")
            .arg(format!("which {} 2>/dev/null || true", tool_name))
            .output()
        {
            let resolved = String::from_utf8_lossy(&path.stdout).trim().to_string();
            if !resolved.is_empty() {
                return Some(resolved);
            }
        }

        for candidate in candidates {
            if candidate.exists() {
                return Some(candidate.to_string_lossy().to_string());
            }
        }

        None
    }
}

pub fn list_detected_tools() -> Vec<AICodingTool> {
    let detector = AICodingToolDetector;
    detector.detect_installed_tools()
}

pub fn analyze_tool(tool: &AICodingTool) -> AIToolStats {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return AIToolStats::default();
    };

    let mut stats = match &tool.kind {
        AICodingToolKind::Codex => analyze_codex(&home),
        AICodingToolKind::ClaudeCode => analyze_claude(&home),
        AICodingToolKind::Cursor => stats_with_disk(&[
            home.join(".cursor"),
            home.join("Library/Application Support/Cursor"),
        ]),
        AICodingToolKind::OpenCode => stats_with_disk(&[
            home.join(".config/opencode"),
            home.join(".local/share/opencode"),
            home.join("Library/Application Support/opencode"),
        ]),
        AICodingToolKind::GitHubCopilot => stats_with_disk(&[
            home.join(".config/github-copilot"),
            home.join(".config/gh-copilot"),
        ]),
        AICodingToolKind::ChatGPT => {
            stats_with_disk(&[home.join("Library/Application Support/ChatGPT")])
        }
        AICodingToolKind::Other(_) => AIToolStats::default(),
    };

    if let Some(path) = &tool.path {
        let path = PathBuf::from(path);
        let resolved = fs::canonicalize(&path).unwrap_or(path);
        stats.disk_bytes = stats.disk_bytes.saturating_add(directory_size(&resolved));
    }

    stats
}

fn analyze_codex(home: &std::path::Path) -> AIToolStats {
    let codex_root = home.join(".codex");
    let sessions_root = codex_root.join("sessions");
    let session_files = files_with_extension(&sessions_root, "jsonl");
    let mut tokens_used = 0_u64;
    let mut has_token_data = false;
    let mut latest_rate_limit: Option<(u64, f64)> = None;

    for path in &session_files {
        let Ok(file) = fs::File::open(path) else {
            continue;
        };
        let mut session_total = None;

        for line in BufReader::new(file).lines().map_while(Result::ok) {
            if !line.contains("\"token_count\"") {
                continue;
            }
            let Ok(value) = serde_json::from_str::<Value>(&line) else {
                continue;
            };
            let payload = &value["payload"];

            if let Some(total) = payload["info"]["total_token_usage"]["total_tokens"].as_u64() {
                session_total = Some(total);
            }

            if let Some(used_percent) = payload["rate_limits"]["primary"]["used_percent"].as_f64() {
                let timestamp = payload["rate_limits"]["primary"]["resets_at"]
                    .as_u64()
                    .unwrap_or_default();
                if latest_rate_limit.is_none_or(|(latest, _)| timestamp >= latest) {
                    latest_rate_limit = Some((timestamp, (100.0 - used_percent).clamp(0.0, 100.0)));
                }
            }
        }

        if let Some(total) = session_total {
            tokens_used = tokens_used.saturating_add(total);
            has_token_data = true;
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs());

    AIToolStats {
        chats: Some(session_files.len() as u64),
        tokens_used: has_token_data.then_some(tokens_used),
        limit_remaining_percent: latest_rate_limit
            .filter(|(resets_at, _)| *resets_at > now)
            .map(|(_, remaining)| remaining),
        disk_bytes: directory_size(&codex_root),
    }
}

fn analyze_claude(home: &std::path::Path) -> AIToolStats {
    let claude_root = home.join(".claude");
    let stats_path = claude_root.join("stats-cache.json");
    let cached_stats = fs::read_to_string(stats_path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok());

    let chats = Some(count_claude_sessions(&claude_root.join("projects")));
    let tokens_used = cached_stats.as_ref().and_then(|stats| {
        let models = stats["modelUsage"].as_object()?;
        Some(
            models
                .values()
                .map(|usage| {
                    [
                        "inputTokens",
                        "outputTokens",
                        "cacheCreationInputTokens",
                        "cacheReadInputTokens",
                    ]
                    .iter()
                    .filter_map(|key| usage[*key].as_u64())
                    .sum::<u64>()
                })
                .sum(),
        )
    });

    AIToolStats {
        chats,
        tokens_used,
        limit_remaining_percent: None,
        disk_bytes: directory_size(&claude_root),
    }
}

fn count_claude_sessions(projects_root: &std::path::Path) -> u64 {
    let Ok(projects) = fs::read_dir(projects_root) else {
        return 0;
    };

    projects
        .filter_map(Result::ok)
        .filter(|project| project.path().is_dir())
        .filter_map(|project| fs::read_dir(project.path()).ok())
        .flat_map(|entries| entries.filter_map(Result::ok))
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "jsonl"))
        .count() as u64
}

fn stats_with_disk(paths: &[PathBuf]) -> AIToolStats {
    AIToolStats {
        disk_bytes: paths.iter().map(|path| directory_size(path)).sum(),
        ..AIToolStats::default()
    }
}

fn files_with_extension(root: &std::path::Path, extension: &str) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];

    while let Some(path) = pending.pop() {
        let Ok(entries) = fs::read_dir(path) else {
            continue;
        };
        for entry in entries.filter_map(Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().is_some_and(|ext| ext == extension) {
                files.push(path);
            }
        }
    }

    files
}

fn directory_size(root: &std::path::Path) -> u64 {
    let Ok(metadata) = fs::symlink_metadata(root) else {
        return 0;
    };
    if metadata.is_file() || metadata.file_type().is_symlink() {
        return metadata.len();
    }

    let Ok(entries) = fs::read_dir(root) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| directory_size(&entry.path()))
        .sum()
}
