# Hive Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust TUI that orchestrates autonomous coding agents (Claude, Gemini, Codex) through a story-to-PR pipeline with per-phase checkpointing, pluggable backends, and a dashboard for visibility and control.

**Architecture:** Bottom-up layered build. Config and domain types first, then backend traits with Claude + Linear as initial implementations, then the orchestrator state machine, then the TUI, then CLI wizards. Each layer is testable independently. The orchestrator communicates with the TUI via `tokio::mpsc` channels — no direct coupling.

**Tech Stack:** Rust, Ratatui, Tokio, serde, reqwest, graphql-client, git2, clap, crossterm, tracing

**Spec:** `docs/specs/2026-04-13-hive-tui-design.md`

---

## File Map

```
~/dev/hivemine/hive/
├── Cargo.toml
├── config.example.toml
├── src/
│   ├── main.rs                         # CLI entry, clap, TUI bootstrap
│   ├── app.rs                          # App struct, channel setup, top-level run loop
│   ├── error.rs                        # HiveError enum, Result type alias
│   │
│   ├── config/
│   │   ├── mod.rs                      # pub use re-exports
│   │   ├── global.rs                   # GlobalConfig, RunnerConfig, TrackerConfig
│   │   ├── project.rs                  # ProjectConfig, PhaseConfig, TrackerFieldConfig
│   │   └── resolve.rs                  # Config loading, merge logic, env var resolution
│   │
│   ├── domain/
│   │   ├── mod.rs                      # pub use re-exports
│   │   ├── phase.rs                    # Phase enum, phase ordering, enabled-phase iterator
│   │   ├── story_run.rs                # StoryRun struct, RunStatus, PhaseResult
│   │   ├── issue.rs                    # Issue, IssueDetail, IssueFilters, FollowUpContent
│   │   └── events.rs                   # OrchestratorEvent, TuiCommand, AgentEvent, NotifyEvent
│   │
│   ├── state/
│   │   ├── mod.rs                      # pub use re-exports
│   │   └── persistence.rs             # save_run, load_run, load_all_runs, delete_run
│   │
│   ├── runners/
│   │   ├── mod.rs                      # AgentRunner trait, SessionConfig, SessionHandle
│   │   └── claude.rs                   # ClaudeRunner — ACP over stdio
│   │
│   ├── trackers/
│   │   ├── mod.rs                      # IssueTracker trait
│   │   └── linear.rs                   # LinearTracker — GraphQL
│   │
│   ├── notifiers/
│   │   ├── mod.rs                      # Notifier trait
│   │   └── discord.rs                  # DiscordNotifier — webhook POST
│   │
│   ├── git/
│   │   ├── mod.rs                      # GitManager struct
│   │   ├── worktree.rs                 # create, list, delete, rebase worktrees
│   │   └── github.rs                   # create_pr, poll_ci, poll_reviews
│   │
│   ├── orchestrator/
│   │   ├── mod.rs                      # Orchestrator struct, main run loop
│   │   ├── transitions.rs             # next_phase(), phase transition logic
│   │   └── retry.rs                    # RetryBudget, attempt tracking
│   │
│   ├── tui/
│   │   ├── mod.rs                      # event loop, tokio::select! multiplexer
│   │   ├── tabs/
│   │   │   ├── mod.rs                  # Tab enum, tab routing
│   │   │   ├── agents.rs              # Agents tab — sidebar + main panel
│   │   │   ├── stories.rs             # Stories tab — filterable table
│   │   │   ├── worktrees.rs           # Worktrees tab
│   │   │   └── config_tab.rs          # Config tab (config_tab to avoid keyword collision)
│   │   └── widgets/
│   │       ├── mod.rs                  # pub use re-exports
│   │       ├── log_viewer.rs          # Scrollable log stream widget
│   │       ├── phase_bar.rs           # Phase progress indicator widget
│   │       └── status_bar.rs          # Tab bar + global status widget
│   │
│   └── cli/
│       ├── mod.rs                      # CLI subcommands enum
│       ├── init.rs                     # hive init wizard
│       ├── configure.rs               # hive configure wizard
│       └── status.rs                   # hive status one-shot
```

---

## Phase 1: Foundation

### Task 1: Initialize Cargo Project

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `src/error.rs`
- Create: `.gitignore`

- [ ] **Step 1: Create the Cargo project**

```bash
cd ~/dev/hivemine/hive
cargo init
```

- [ ] **Step 2: Replace Cargo.toml with full dependency list**

```toml
[package]
name = "hive"
version = "0.1.0"
edition = "2024"
description = "Agent orchestration TUI for story-to-PR automation"
license = "MIT"

[dependencies]
# Async runtime
tokio = { version = "1", features = ["full"] }

# TUI
ratatui = "0.30"
crossterm = "0.29"

# Serialization
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"

# HTTP
reqwest = { version = "0.12", features = ["json"] }

# Git
git2 = "0.20"

# CLI
clap = { version = "4", features = ["derive"] }

# Time
chrono = { version = "0.4", features = ["serde"] }

# Logging
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

# Error handling
thiserror = "2"

# Async trait
async-trait = "0.1"

# Futures/streams
futures = "0.3"
tokio-stream = "0.1"

[dev-dependencies]
tempfile = "3"
```

- [ ] **Step 3: Write the error module**

```rust
// src/error.rs
use thiserror::Error;

#[derive(Error, Debug)]
pub enum HiveError {
    #[error("config error: {0}")]
    Config(String),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("toml parse error: {0}")]
    TomlParse(#[from] toml::de::Error),

    #[error("git error: {0}")]
    Git(#[from] git2::Error),

    #[error("http error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("agent error: {0}")]
    Agent(String),

    #[error("tracker error: {0}")]
    Tracker(String),

    #[error("orchestrator error: {0}")]
    Orchestrator(String),

    #[error("phase error in {phase}: {message}")]
    Phase { phase: String, message: String },

    #[error("notification error: {0}")]
    Notification(String),
}

pub type Result<T> = std::result::Result<T, HiveError>;
```

- [ ] **Step 4: Write minimal main.rs**

```rust
// src/main.rs
mod error;

fn main() {
    println!("hive v0.1.0");
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build`
Expected: Compiles successfully. Warnings about unused dependencies are fine at this stage.

- [ ] **Step 6: Add .gitignore and commit**

```gitignore
/target
.DS_Store
```

```bash
cd ~/dev/hivemine/hive
git init
git add Cargo.toml Cargo.lock src/ .gitignore
git commit -m "feat: initialize hive project with dependencies"
```

---

### Task 2: Config Types

**Files:**
- Create: `src/config/mod.rs`
- Create: `src/config/global.rs`
- Create: `src/config/project.rs`
- Create: `src/config/resolve.rs`

- [ ] **Step 1: Write failing test for global config parsing**

```rust
// src/config/global.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_global_config() {
        let toml_str = r#"
[runners.claude]
command = "claude"
protocol = "acp"
default_model = "opus-4-6"
permission_mode = "dangerously-skip"

[runners.gemini]
command = "gemini"
protocol = "acp"
default_model = "flash"

[trackers.linear]
api_key = "env:LINEAR_API_KEY"

[notifications.discord]
webhook_url = "env:HIVE_DISCORD_WEBHOOK"
"#;
        let config: GlobalConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.runners.len(), 2);
        assert_eq!(config.runners["claude"].command, "claude");
        assert_eq!(config.runners["claude"].protocol, Protocol::Acp);
        assert_eq!(config.trackers["linear"].api_key, "env:LINEAR_API_KEY");
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test test_parse_global_config`
Expected: FAIL — `GlobalConfig` not defined.

- [ ] **Step 3: Implement global config types**

```rust
// src/config/global.rs
use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub runners: HashMap<String, RunnerConfig>,
    #[serde(default)]
    pub trackers: HashMap<String, TrackerConnectionConfig>,
    #[serde(default)]
    pub notifications: HashMap<String, NotificationConnectionConfig>,
}

#[derive(Debug, Deserialize)]
pub struct RunnerConfig {
    pub command: String,
    pub protocol: Protocol,
    pub default_model: String,
    #[serde(default)]
    pub permission_mode: Option<String>,
}

#[derive(Debug, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum Protocol {
    Acp,
    Subprocess,
}

#[derive(Debug, Deserialize)]
pub struct TrackerConnectionConfig {
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub base_url: Option<String>,
    #[serde(default)]
    pub api_token: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct NotificationConnectionConfig {
    pub webhook_url: String,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test test_parse_global_config`
Expected: PASS

- [ ] **Step 5: Write failing test for project config parsing**

```rust
// src/config/project.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_project_config() {
        let toml_str = r#"
name = "apex"
repo_path = "/Users/robbie/dev/hivemine/gemini-chatz/apex"
worktree_dir = ".worktrees"
tracker = "linear"

[github]
owner = "hivemine"
repo = "gemini-chatz"

[tracker_config]
team = "Hivemine"
ready_filter = "Todo"

[tracker_config.statuses]
start = "In Progress"
review = "In Review"
done = "Done"

[phases.understand]
enabled = true
runner = "claude"
model = "opus-4-6"

[phases.implement]
enabled = true
runner = "claude"
model = "opus-4-6"

[phases.cross-review]
enabled = false
runner = "gemini"
model = "flash"

[phases.ci-watch]
enabled = true
poll_interval = "30s"
max_fix_attempts = 3
fix_runner = "claude"
fix_model = "sonnet-4-6"
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.name, "apex");
        assert_eq!(config.tracker, "linear");
        assert!(config.notifier.is_none());
        assert_eq!(config.tracker_config.team, "Hivemine");
        assert_eq!(config.tracker_config.statuses.start, "In Progress");
        assert!(!config.phases["cross-review"].enabled);
        assert_eq!(config.phases["ci-watch"].poll_interval, Some("30s".to_string()));
    }

    #[test]
    fn test_notifier_is_optional() {
        let toml_str = r#"
name = "minimal"
repo_path = "/tmp/repo"
tracker = "linear"

[github]
owner = "test"
repo = "test"

[tracker_config]
team = "Test"
ready_filter = "Todo"

[tracker_config.statuses]
start = "In Progress"
review = "In Review"
done = "Done"
"#;
        let config: ProjectConfig = toml::from_str(toml_str).unwrap();
        assert!(config.notifier.is_none());
        assert!(config.phases.is_empty());
    }
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test test_parse_project_config test_notifier_is_optional`
Expected: FAIL — `ProjectConfig` not defined.

- [ ] **Step 7: Implement project config types**

```rust
// src/config/project.rs
use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    pub repo_path: String,
    #[serde(default = "default_worktree_dir")]
    pub worktree_dir: String,
    pub tracker: String,
    pub notifier: Option<String>,
    #[serde(default)]
    pub notifications: Option<NotificationConfig>,
    pub github: GitHubConfig,
    pub tracker_config: TrackerConfig,
    #[serde(default)]
    pub phases: HashMap<String, PhaseConfig>,
}

fn default_worktree_dir() -> String {
    ".worktrees".to_string()
}

#[derive(Debug, Deserialize)]
pub struct NotificationConfig {
    #[serde(default)]
    pub events: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct GitHubConfig {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Deserialize)]
pub struct TrackerConfig {
    pub team: String,
    pub ready_filter: String,
    pub statuses: StatusMappings,
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct StatusMappings {
    pub start: String,
    pub review: String,
    pub done: String,
}

#[derive(Debug, Deserialize)]
pub struct PhaseConfig {
    pub enabled: bool,
    #[serde(default)]
    pub runner: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub max_attempts: Option<u8>,
    #[serde(default)]
    pub poll_interval: Option<String>,
    #[serde(default)]
    pub max_fix_attempts: Option<u8>,
    #[serde(default)]
    pub max_fix_cycles: Option<u8>,
    #[serde(default)]
    pub fix_runner: Option<String>,
    #[serde(default)]
    pub fix_model: Option<String>,
    #[serde(default)]
    pub wait_for: Option<Vec<String>>,
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test config`
Expected: All config tests PASS.

- [ ] **Step 9: Write failing test for config resolution (env var expansion)**

```rust
// src/config/resolve.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_var() {
        std::env::set_var("HIVE_TEST_KEY", "secret123");
        assert_eq!(resolve_env("env:HIVE_TEST_KEY").unwrap(), "secret123");
        assert_eq!(resolve_env("literal_value").unwrap(), "literal_value");
        std::env::remove_var("HIVE_TEST_KEY");
    }

    #[test]
    fn test_resolve_env_var_missing() {
        let result = resolve_env("env:NONEXISTENT_VAR_XYZ");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_project_by_repo_path() {
        let dir = tempfile::tempdir().unwrap();
        let projects_dir = dir.path().join("projects").join("test");
        std::fs::create_dir_all(&projects_dir).unwrap();

        let config_content = r#"
name = "test"
repo_path = "/some/repo"
tracker = "linear"

[github]
owner = "test"
repo = "test"

[tracker_config]
team = "Test"
ready_filter = "Todo"

[tracker_config.statuses]
start = "In Progress"
review = "In Review"
done = "Done"
"#;
        std::fs::write(projects_dir.join("project.toml"), config_content).unwrap();

        let config = load_project_config(dir.path(), "/some/repo").unwrap();
        assert_eq!(config.name, "test");
    }

    #[test]
    fn test_load_project_not_found() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_project_config(dir.path(), "/nonexistent");
        assert!(result.is_err());
    }
}
```

- [ ] **Step 10: Run tests to verify they fail**

Run: `cargo test resolve`
Expected: FAIL — `resolve_env`, `load_project_config` not defined.

- [ ] **Step 11: Implement config resolution**

```rust
// src/config/resolve.rs
use std::path::Path;

use crate::config::global::GlobalConfig;
use crate::config::project::ProjectConfig;
use crate::error::{HiveError, Result};

/// Resolve a config value that may reference an environment variable.
/// Values starting with "env:" are looked up from the environment.
/// All other values are returned as-is.
pub fn resolve_env(value: &str) -> Result<String> {
    if let Some(var_name) = value.strip_prefix("env:") {
        std::env::var(var_name).map_err(|_| {
            HiveError::Config(format!("environment variable {var_name} is not set"))
        })
    } else {
        Ok(value.to_string())
    }
}

/// Load the global config from the hive config directory.
pub fn load_global_config(config_dir: &Path) -> Result<GlobalConfig> {
    let path = config_dir.join("config.toml");
    if !path.exists() {
        return Ok(GlobalConfig {
            runners: Default::default(),
            trackers: Default::default(),
            notifications: Default::default(),
        });
    }
    let content = std::fs::read_to_string(&path)?;
    let config: GlobalConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Find and load the project config whose repo_path matches the given path.
/// Searches all project directories under `config_dir/projects/`.
pub fn load_project_config(config_dir: &Path, repo_path: &str) -> Result<ProjectConfig> {
    let projects_dir = config_dir.join("projects");
    if !projects_dir.exists() {
        return Err(HiveError::Config(format!(
            "no hive projects configured (looked in {})",
            projects_dir.display()
        )));
    }

    for entry in std::fs::read_dir(&projects_dir)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let project_toml = entry.path().join("project.toml");
        if !project_toml.exists() {
            continue;
        }
        let content = std::fs::read_to_string(&project_toml)?;
        let config: ProjectConfig = toml::from_str(&content)?;
        if config.repo_path == repo_path {
            return Ok(config);
        }
    }

    Err(HiveError::Config(format!(
        "no project configured for repo path: {repo_path}. Run `hive init` to set one up."
    )))
}
```

- [ ] **Step 12: Wire up the config module**

```rust
// src/config/mod.rs
pub mod global;
pub mod project;
pub mod resolve;

pub use global::*;
pub use project::*;
pub use resolve::*;
```

Update `src/main.rs` to declare the module:

```rust
// src/main.rs
mod config;
mod error;

fn main() {
    println!("hive v0.1.0");
}
```

- [ ] **Step 13: Run all tests**

Run: `cargo test`
Expected: All tests PASS.

- [ ] **Step 14: Commit**

```bash
git add src/config/ src/main.rs src/error.rs
git commit -m "feat: add config types and resolution with env var expansion"
```

---

### Task 3: Domain Types — Phase and StoryRun

**Files:**
- Create: `src/domain/mod.rs`
- Create: `src/domain/phase.rs`
- Create: `src/domain/story_run.rs`
- Create: `src/domain/issue.rs`
- Create: `src/domain/events.rs`

- [ ] **Step 1: Write failing test for phase ordering and enabled-phase iteration**

```rust
// src/domain/phase.rs
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::PhaseConfig;

    #[test]
    fn test_all_phases_in_order() {
        let phases = Phase::all_in_order();
        assert_eq!(phases[0], Phase::Understand);
        assert_eq!(phases[1], Phase::Implement);
        assert_eq!(phases[2], Phase::SelfReview { attempt: 0 });
        assert_eq!(phases[3], Phase::CrossReview);
        assert_eq!(phases[4], Phase::RaisePr);
        assert_eq!(phases[5], Phase::CiWatch { attempt: 0 });
        assert_eq!(phases[6], Phase::BotReviews { cycle: 0 });
        assert_eq!(phases[7], Phase::FollowUps);
        assert_eq!(phases[8], Phase::Handoff);
    }

    #[test]
    fn test_next_phase_skips_disabled() {
        let mut phases_config = HashMap::new();
        phases_config.insert("understand".to_string(), PhaseConfig {
            enabled: true, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        });
        phases_config.insert("implement".to_string(), PhaseConfig {
            enabled: true, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        });
        phases_config.insert("self-review".to_string(), PhaseConfig {
            enabled: true, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        });
        phases_config.insert("cross-review".to_string(), PhaseConfig {
            enabled: false, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        });
        phases_config.insert("raise-pr".to_string(), PhaseConfig {
            enabled: true, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        });

        let next = next_enabled_phase(&Phase::SelfReview { attempt: 0 }, &phases_config);
        // cross-review is disabled, so it should skip to raise-pr
        assert_eq!(next, Some(Phase::RaisePr));
    }

    #[test]
    fn test_next_phase_after_handoff_is_complete() {
        let phases_config = HashMap::new(); // all defaults = enabled
        let next = next_enabled_phase(&Phase::Handoff, &phases_config);
        assert_eq!(next, None); // Handoff is the last phase, returns None (→ Complete)
    }

    #[test]
    fn test_phase_config_key() {
        assert_eq!(Phase::Understand.config_key(), "understand");
        assert_eq!(Phase::SelfReview { attempt: 2 }.config_key(), "self-review");
        assert_eq!(Phase::CiWatch { attempt: 1 }.config_key(), "ci-watch");
        assert_eq!(Phase::BotReviews { cycle: 3 }.config_key(), "bot-reviews");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test domain::phase`
Expected: FAIL — types not defined.

- [ ] **Step 3: Implement Phase enum and ordering logic**

```rust
// src/domain/phase.rs
use std::collections::HashMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::config::PhaseConfig;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Phase {
    Queued,
    Understand,
    Implement,
    SelfReview { attempt: u8 },
    CrossReview,
    RaisePr,
    CiWatch { attempt: u8 },
    BotReviews { cycle: u8 },
    FollowUps,
    Handoff,
    Complete,
    NeedsAttention { reason: String },
}

impl Phase {
    /// The fixed pipeline order. Queued, Complete, and NeedsAttention are not
    /// part of the pipeline — they are entry/exit/error states.
    pub fn all_in_order() -> Vec<Phase> {
        vec![
            Phase::Understand,
            Phase::Implement,
            Phase::SelfReview { attempt: 0 },
            Phase::CrossReview,
            Phase::RaisePr,
            Phase::CiWatch { attempt: 0 },
            Phase::BotReviews { cycle: 0 },
            Phase::FollowUps,
            Phase::Handoff,
        ]
    }

    /// The TOML config key for this phase (strips counters).
    pub fn config_key(&self) -> &'static str {
        match self {
            Phase::Queued => "queued",
            Phase::Understand => "understand",
            Phase::Implement => "implement",
            Phase::SelfReview { .. } => "self-review",
            Phase::CrossReview => "cross-review",
            Phase::RaisePr => "raise-pr",
            Phase::CiWatch { .. } => "ci-watch",
            Phase::BotReviews { .. } => "bot-reviews",
            Phase::FollowUps => "follow-ups",
            Phase::Handoff => "handoff",
            Phase::Complete => "complete",
            Phase::NeedsAttention { .. } => "needs-attention",
        }
    }

    /// Whether this phase requires an agent (LLM session).
    pub fn is_agent_phase(&self) -> bool {
        matches!(
            self,
            Phase::Understand
                | Phase::Implement
                | Phase::SelfReview { .. }
                | Phase::CrossReview
                | Phase::FollowUps
        )
    }

    /// Whether this phase is a polling phase (no agent, Hive polls APIs).
    pub fn is_polling_phase(&self) -> bool {
        matches!(self, Phase::CiWatch { .. } | Phase::BotReviews { .. })
    }

    /// Whether this phase is a direct phase (Hive acts via API, no agent).
    pub fn is_direct_phase(&self) -> bool {
        matches!(self, Phase::RaisePr | Phase::Handoff)
    }

    /// The index of this phase in the pipeline (for progress display).
    pub fn pipeline_index(&self) -> Option<usize> {
        let all = Self::all_in_order();
        all.iter().position(|p| p.config_key() == self.config_key())
    }

    /// Total number of phases in the pipeline.
    pub fn pipeline_len() -> usize {
        Self::all_in_order().len()
    }
}

impl fmt::Display for Phase {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Phase::Queued => write!(f, "Queued"),
            Phase::Understand => write!(f, "Understand"),
            Phase::Implement => write!(f, "Implement"),
            Phase::SelfReview { attempt } => write!(f, "Self-Review (attempt {attempt})"),
            Phase::CrossReview => write!(f, "Cross-Review"),
            Phase::RaisePr => write!(f, "Raise PR"),
            Phase::CiWatch { attempt } => write!(f, "CI Watch (attempt {attempt})"),
            Phase::BotReviews { cycle } => write!(f, "Bot Reviews (cycle {cycle})"),
            Phase::FollowUps => write!(f, "Follow-Ups"),
            Phase::Handoff => write!(f, "Handoff"),
            Phase::Complete => write!(f, "Complete"),
            Phase::NeedsAttention { reason } => write!(f, "Needs Attention: {reason}"),
        }
    }
}

/// Given the current phase, return the next enabled phase in the pipeline.
/// Returns None if the current phase is the last enabled phase (→ Complete).
/// Phases not in the config are treated as enabled by default.
pub fn next_enabled_phase(
    current: &Phase,
    phases_config: &HashMap<String, PhaseConfig>,
) -> Option<Phase> {
    let all = Phase::all_in_order();
    let current_key = current.config_key();

    let current_idx = all
        .iter()
        .position(|p| p.config_key() == current_key)?;

    for phase in &all[current_idx + 1..] {
        let key = phase.config_key();
        let enabled = phases_config
            .get(key)
            .map(|c| c.enabled)
            .unwrap_or(true);
        if enabled {
            return Some(phase.clone());
        }
    }

    None // past the last phase → Complete
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test domain::phase`
Expected: All PASS.

- [ ] **Step 5: Implement StoryRun, Issue, and Event types**

```rust
// src/domain/story_run.rs
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::phase::Phase;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryRun {
    pub issue_id: String,
    pub issue_title: String,
    pub phase: Phase,
    pub status: RunStatus,
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    pub pr: Option<PrHandle>,
    pub session_id: Option<String>,
    pub phase_history: Vec<PhaseResult>,
    pub cost_usd: f64,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Paused,
    NeedsAttention,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrHandle {
    pub number: u64,
    pub url: String,
    pub head_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    pub phase: Phase,
    pub outcome: PhaseOutcome,
    pub duration_secs: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PhaseOutcome {
    Success,
    Skipped,
    Failed { reason: String },
    NeedsAttention { reason: String },
}

impl StoryRun {
    pub fn new(issue_id: String, issue_title: String) -> Self {
        let now = Utc::now();
        Self {
            issue_id,
            issue_title,
            phase: Phase::Queued,
            status: RunStatus::Running,
            worktree: None,
            branch: None,
            pr: None,
            session_id: None,
            phase_history: Vec::new(),
            cost_usd: 0.0,
            started_at: now,
            updated_at: now,
        }
    }
}
```

```rust
// src/domain/issue.rs
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub priority: Option<String>,
    pub project: Option<String>,
    pub labels: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueDetail {
    pub id: String,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct IssueFilters {
    pub team: Option<String>,
    pub project: Option<String>,
    pub labels: Vec<String>,
    pub status: Option<String>,
}

#[derive(Debug, Clone)]
pub struct FollowUpContent {
    pub title: String,
    pub description: String,
    pub labels: Vec<String>,
}
```

```rust
// src/domain/events.rs
use super::issue::Issue;
use super::phase::Phase;
use super::story_run::{PrHandle, RunStatus, StoryRun};

/// Events sent from the orchestrator to the TUI for rendering.
#[derive(Debug, Clone)]
pub enum OrchestratorEvent {
    StoryUpdated(StoryRun),
    AgentOutput {
        issue_id: String,
        event: AgentEvent,
    },
    PhaseTransition {
        issue_id: String,
        from: Phase,
        to: Phase,
    },
    Error {
        issue_id: Option<String>,
        message: String,
    },
}

/// Commands sent from the TUI to the orchestrator.
#[derive(Debug, Clone)]
pub enum TuiCommand {
    StartStory { issue: Issue },
    CancelStory { issue_id: String },
    RebaseStory { issue_id: String },
    CopyWorktreePath { issue_id: String },
    RefreshStories,
    Quit,
}

/// Events streamed from an agent session.
#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    ToolUse { tool: String, input_preview: String },
    ToolResult { tool: String, success: bool },
    Error(String),
    Complete { cost_usd: f64 },
    CostUpdate(f64),
}

/// Events that trigger notifications.
#[derive(Debug, Clone)]
pub enum NotifyEvent {
    StoryComplete {
        issue_id: String,
        pr_url: String,
        cost_usd: f64,
        duration_secs: u64,
    },
    NeedsAttention {
        issue_id: String,
        reason: String,
    },
    AllIdle,
    CiFailedMaxRetries {
        issue_id: String,
    },
}
```

```rust
// src/domain/mod.rs
pub mod events;
pub mod issue;
pub mod phase;
pub mod story_run;

pub use events::*;
pub use issue::*;
pub use phase::*;
pub use story_run::*;
```

- [ ] **Step 6: Wire up domain module in main.rs**

```rust
// src/main.rs
mod config;
mod domain;
mod error;

fn main() {
    println!("hive v0.1.0");
}
```

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: All PASS. Code compiles.

- [ ] **Step 8: Commit**

```bash
git add src/domain/ src/main.rs
git commit -m "feat: add domain types — Phase, StoryRun, Issue, events"
```

---

### Task 4: State Persistence

**Files:**
- Create: `src/state/mod.rs`
- Create: `src/state/persistence.rs`

- [ ] **Step 1: Write failing tests for save and load**

```rust
// src/state/persistence.rs
#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::story_run::StoryRun;

    #[test]
    fn test_save_and_load_run() {
        let dir = tempfile::tempdir().unwrap();
        let run = StoryRun::new("APX-245".to_string(), "Add NumberSequenceService".to_string());

        save_run(dir.path(), &run).unwrap();
        let loaded = load_run(dir.path(), "APX-245").unwrap();

        assert_eq!(loaded.issue_id, "APX-245");
        assert_eq!(loaded.issue_title, "Add NumberSequenceService");
    }

    #[test]
    fn test_load_all_runs() {
        let dir = tempfile::tempdir().unwrap();
        let run1 = StoryRun::new("APX-245".to_string(), "Story 1".to_string());
        let run2 = StoryRun::new("APX-270".to_string(), "Story 2".to_string());

        save_run(dir.path(), &run1).unwrap();
        save_run(dir.path(), &run2).unwrap();

        let all = load_all_runs(dir.path()).unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_delete_run() {
        let dir = tempfile::tempdir().unwrap();
        let run = StoryRun::new("APX-245".to_string(), "Story".to_string());

        save_run(dir.path(), &run).unwrap();
        assert!(dir.path().join("APX-245.json").exists());

        delete_run(dir.path(), "APX-245").unwrap();
        assert!(!dir.path().join("APX-245.json").exists());
    }

    #[test]
    fn test_load_nonexistent_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let result = load_run(dir.path(), "NOPE-999");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_all_from_empty_dir() {
        let dir = tempfile::tempdir().unwrap();
        let all = load_all_runs(dir.path()).unwrap();
        assert!(all.is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test state::persistence`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement persistence functions**

```rust
// src/state/persistence.rs
use std::path::Path;

use crate::domain::story_run::StoryRun;
use crate::error::Result;

/// Save a story run to disk as JSON. Overwrites any existing file for this issue.
pub fn save_run(runs_dir: &Path, run: &StoryRun) -> Result<()> {
    std::fs::create_dir_all(runs_dir)?;
    let path = runs_dir.join(format!("{}.json", run.issue_id));
    let json = serde_json::to_string_pretty(run)?;
    std::fs::write(path, json)?;
    Ok(())
}

/// Load a single story run by issue ID.
pub fn load_run(runs_dir: &Path, issue_id: &str) -> Result<StoryRun> {
    let path = runs_dir.join(format!("{issue_id}.json"));
    let content = std::fs::read_to_string(&path)?;
    let run: StoryRun = serde_json::from_str(&content)?;
    Ok(run)
}

/// Load all story runs from the runs directory.
pub fn load_all_runs(runs_dir: &Path) -> Result<Vec<StoryRun>> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut runs = Vec::new();
    for entry in std::fs::read_dir(runs_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("json") {
            let content = std::fs::read_to_string(&path)?;
            let run: StoryRun = serde_json::from_str(&content)?;
            runs.push(run);
        }
    }
    Ok(runs)
}

/// Delete a story run file.
pub fn delete_run(runs_dir: &Path, issue_id: &str) -> Result<()> {
    let path = runs_dir.join(format!("{issue_id}.json"));
    if path.exists() {
        std::fs::remove_file(path)?;
    }
    Ok(())
}
```

```rust
// src/state/mod.rs
pub mod persistence;

pub use persistence::*;
```

Update `src/main.rs`:

```rust
mod config;
mod domain;
mod error;
mod state;

fn main() {
    println!("hive v0.1.0");
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test state`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/state/ src/main.rs
git commit -m "feat: add state persistence — save, load, delete story runs as JSON"
```

---

## Phase 2: Backend Traits + First Implementations

### Task 5: AgentRunner Trait

**Files:**
- Create: `src/runners/mod.rs`

- [ ] **Step 1: Define the AgentRunner trait and supporting types**

```rust
// src/runners/mod.rs
pub mod claude;

use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;

use crate::domain::AgentEvent;
use crate::error::Result;

/// Configuration for starting a new agent session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub working_dir: PathBuf,
    pub system_prompt: String,
    pub model: Option<String>,
    pub permission_mode: Option<String>,
}

/// Handle to a running or resumable agent session.
#[derive(Debug, Clone)]
pub struct SessionHandle {
    pub session_id: String,
    pub runner_name: String,
    pub pid: Option<u32>,
}

#[async_trait]
pub trait AgentRunner: Send + Sync {
    /// Start a new agent session in the given working directory.
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle>;

    /// Send a prompt/message to an existing session.
    async fn send_prompt(&self, session: &SessionHandle, prompt: &str) -> Result<()>;

    /// Get a stream of output events from the session.
    fn output_stream(
        &self,
        session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>>;

    /// Cancel a running session.
    async fn cancel(&self, session: &SessionHandle) -> Result<()>;

    /// Attempt to resume a previously interrupted session.
    async fn resume(&self, session: &SessionHandle) -> Result<()>;

    /// Check if a session is still alive.
    async fn is_alive(&self, session: &SessionHandle) -> bool;

    /// Runner identity for display and logging.
    fn name(&self) -> &str;
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles (claude module will be empty for now).

- [ ] **Step 3: Commit**

```bash
git add src/runners/
git commit -m "feat: add AgentRunner trait and session types"
```

---

### Task 6: Claude Code ACP Runner

**Files:**
- Create: `src/runners/claude.rs`

This is the primary agent runner. It spawns `claude` as a subprocess in headless mode (`-p` flag with `stream-json` output) and parses the NDJSON event stream.

- [ ] **Step 1: Write tests for NDJSON event parsing**

```rust
// src/runners/claude.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_text_delta_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Reading file..."}]}}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::TextDelta(text)) => assert_eq!(text, "Reading file..."),
            other => panic!("expected TextDelta, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_tool_use_event() {
        let line = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","name":"Read","input":{"file_path":"/src/main.rs"}}]}}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::ToolUse { tool, .. }) => assert_eq!(tool, "Read"),
            other => panic!("expected ToolUse, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_result_event() {
        let line = r#"{"type":"result","result":"Done","session_id":"abc","is_error":false,"total_cost_usd":0.42}"#;
        let event = parse_claude_event(line).unwrap();
        match event {
            Some(AgentEvent::Complete { cost_usd }) => {
                assert!((cost_usd - 0.42).abs() < f64::EPSILON);
            }
            other => panic!("expected Complete, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_system_init_event_returns_none() {
        let line = r#"{"type":"system","subtype":"init","session_id":"abc123"}"#;
        let event = parse_claude_event(line).unwrap();
        assert!(event.is_none()); // system events are metadata, not displayed
    }

    #[test]
    fn test_parse_malformed_json_returns_error() {
        let line = "not json at all";
        let result = parse_claude_event(line);
        assert!(result.is_err());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test runners::claude`
Expected: FAIL — `parse_claude_event` not defined.

- [ ] **Step 3: Implement event parsing and ClaudeRunner struct**

```rust
// src/runners/claude.rs
use std::pin::Pin;
use std::process::Stdio;

use async_trait::async_trait;
use futures::Stream;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{AgentRunner, SessionConfig, SessionHandle};
use crate::domain::AgentEvent;
use crate::error::{HiveError, Result};

pub struct ClaudeRunner {
    command: String,
    default_model: String,
    permission_mode: Option<String>,
}

impl ClaudeRunner {
    pub fn new(command: String, default_model: String, permission_mode: Option<String>) -> Self {
        Self {
            command,
            default_model,
            permission_mode,
        }
    }

    fn build_command(&self, config: &SessionConfig) -> Command {
        let mut cmd = Command::new(&self.command);
        cmd.arg("--bare")
            .arg("-p")
            .arg(&config.system_prompt)
            .arg("--output-format")
            .arg("stream-json")
            .arg("--verbose");

        let model = config.model.as_deref().unwrap_or(&self.default_model);
        cmd.arg("--model").arg(model);

        if let Some(ref pm) = config
            .permission_mode
            .as_ref()
            .or(self.permission_mode.as_ref())
        {
            cmd.arg("--permission-mode").arg(pm);
        }

        cmd.current_dir(&config.working_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        cmd
    }
}

#[async_trait]
impl AgentRunner for ClaudeRunner {
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle> {
        let mut child = self
            .build_command(&config)
            .spawn()
            .map_err(|e| HiveError::Agent(format!("failed to spawn claude: {e}")))?;

        let pid = child.id();

        // We'll extract session_id from the init event in the output stream.
        // For now, use the PID as a temporary identifier.
        let session_id = pid
            .map(|p| p.to_string())
            .unwrap_or_else(|| "unknown".to_string());

        // Store the child process handle for lifecycle management.
        // In a real implementation, this would be stored in a concurrent map
        // keyed by session_id, managed by the ClaudeRunner.
        // For now, we leak the child — Task 13 (Orchestrator) will manage process handles.
        let _stdout = child.stdout.take();
        let _child = child;

        Ok(SessionHandle {
            session_id,
            runner_name: "claude".to_string(),
            pid,
        })
    }

    async fn send_prompt(&self, session: &SessionHandle, prompt: &str) -> Result<()> {
        // For Claude Code, sending a follow-up prompt means using --resume.
        // This will be implemented when the orchestrator drives multi-turn sessions.
        let _ = (session, prompt);
        Ok(())
    }

    fn output_stream(
        &self,
        _session: &SessionHandle,
    ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        // Placeholder — the real implementation connects stdout from the child process.
        // This will be wired up in Task 13 when the orchestrator manages process handles.
        let (_tx, rx) = mpsc::channel(1);
        Box::pin(ReceiverStream::new(rx))
    }

    async fn cancel(&self, session: &SessionHandle) -> Result<()> {
        if let Some(pid) = session.pid {
            // Send SIGTERM to the process group
            unsafe {
                libc::kill(pid as i32, libc::SIGTERM);
            }
        }
        Ok(())
    }

    async fn resume(&self, session: &SessionHandle) -> Result<()> {
        let _ = session;
        // Resume will spawn a new process with --resume <session_id>
        // Implemented when orchestrator handles crash recovery (Task 15)
        Ok(())
    }

    async fn is_alive(&self, session: &SessionHandle) -> bool {
        if let Some(pid) = session.pid {
            // Check if process is still running
            unsafe { libc::kill(pid as i32, 0) == 0 }
        } else {
            false
        }
    }

    fn name(&self) -> &str {
        "claude"
    }
}

/// Parse a single NDJSON line from Claude's stream-json output into an AgentEvent.
/// Returns Ok(None) for events we don't surface to the TUI (system/init, etc).
pub fn parse_claude_event(line: &str) -> Result<Option<AgentEvent>> {
    let v: Value =
        serde_json::from_str(line).map_err(|e| HiveError::Agent(format!("bad json: {e}")))?;

    let event_type = v["type"].as_str().unwrap_or("");

    match event_type {
        "assistant" => {
            let content = &v["message"]["content"];
            if let Some(items) = content.as_array() {
                for item in items {
                    match item["type"].as_str() {
                        Some("text") => {
                            if let Some(text) = item["text"].as_str() {
                                return Ok(Some(AgentEvent::TextDelta(text.to_string())));
                            }
                        }
                        Some("tool_use") => {
                            let tool = item["name"]
                                .as_str()
                                .unwrap_or("unknown")
                                .to_string();
                            let input_preview = item["input"].to_string();
                            let input_preview = if input_preview.len() > 100 {
                                format!("{}...", &input_preview[..100])
                            } else {
                                input_preview
                            };
                            return Ok(Some(AgentEvent::ToolUse {
                                tool,
                                input_preview,
                            }));
                        }
                        _ => {}
                    }
                }
            }
            Ok(None)
        }
        "result" => {
            let is_error = v["is_error"].as_bool().unwrap_or(false);
            let cost = v["total_cost_usd"].as_f64().unwrap_or(0.0);
            if is_error {
                let msg = v["result"]
                    .as_str()
                    .unwrap_or("unknown error")
                    .to_string();
                Ok(Some(AgentEvent::Error(msg)))
            } else {
                Ok(Some(AgentEvent::Complete { cost_usd: cost }))
            }
        }
        "system" => {
            // System events (init, api_retry) are metadata — don't surface to TUI
            Ok(None)
        }
        _ => Ok(None),
    }
}
```

Add `libc` to Cargo.toml:

```toml
# Under [dependencies]
libc = "0.2"
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test runners::claude`
Expected: All PASS.

- [ ] **Step 5: Commit**

```bash
git add src/runners/ Cargo.toml
git commit -m "feat: add ClaudeRunner with ACP subprocess management and NDJSON parsing"
```

---

### Task 7: IssueTracker Trait

**Files:**
- Modify: `src/trackers/mod.rs`

- [ ] **Step 1: Define the IssueTracker trait**

```rust
// src/trackers/mod.rs
pub mod linear;

use async_trait::async_trait;

use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::Result;

#[async_trait]
pub trait IssueTracker: Send + Sync {
    /// Fetch issues that are ready for work based on the configured filters.
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>>;

    /// Transition an issue to the "in progress" status.
    async fn start_issue(&self, id: &str) -> Result<()>;

    /// Transition an issue to the "in review" status.
    async fn finish_issue(&self, id: &str) -> Result<()>;

    /// Create a follow-up issue linked to the parent.
    async fn create_followup(&self, parent_id: &str, content: FollowUpContent) -> Result<String>;

    /// Get full issue details including description and acceptance criteria.
    async fn get_issue(&self, id: &str) -> Result<IssueDetail>;

    /// Tracker identity for display and logging.
    fn name(&self) -> &str;
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/trackers/
git commit -m "feat: add IssueTracker trait"
```

---

### Task 8: Linear Tracker Implementation

**Files:**
- Create: `src/trackers/linear.rs`

This implements `IssueTracker` against Linear's GraphQL API. Linear uses team-scoped queries, workflow state IDs for transitions, and issue identifiers like `APX-245`.

- [ ] **Step 1: Write test for GraphQL query building**

```rust
// src/trackers/linear.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ready_issues_query() {
        let query = build_issues_query("Hivemine", "Todo", None);
        assert!(query.contains("Hivemine"));
        assert!(query.contains("Todo"));
        assert!(query.contains("issues"));
    }

    #[test]
    fn test_parse_issues_response() {
        let json = r#"{
            "data": {
                "issues": {
                    "nodes": [
                        {
                            "identifier": "APX-245",
                            "title": "Add NumberSequenceService",
                            "priority": 2,
                            "url": "https://linear.app/hivemine/issue/APX-245",
                            "labels": { "nodes": [{ "name": "backend" }] },
                            "project": { "name": "Phase 77" }
                        }
                    ]
                }
            }
        }"#;
        let issues = parse_issues_response(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "APX-245");
        assert_eq!(issues[0].title, "Add NumberSequenceService");
        assert_eq!(issues[0].labels, vec!["backend"]);
    }

    #[test]
    fn test_priority_number_to_label() {
        assert_eq!(priority_label(0), "None");
        assert_eq!(priority_label(1), "Urgent");
        assert_eq!(priority_label(2), "High");
        assert_eq!(priority_label(3), "Medium");
        assert_eq!(priority_label(4), "Low");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test trackers::linear`
Expected: FAIL — functions not defined.

- [ ] **Step 3: Implement Linear tracker**

```rust
// src/trackers/linear.rs
use async_trait::async_trait;
use serde_json::Value;

use super::IssueTracker;
use crate::config::TrackerConfig;
use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::{HiveError, Result};

pub struct LinearTracker {
    api_key: String,
    tracker_config: TrackerConfig,
    client: reqwest::Client,
}

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

impl LinearTracker {
    pub fn new(api_key: String, tracker_config: TrackerConfig) -> Self {
        Self {
            api_key,
            tracker_config,
            client: reqwest::Client::new(),
        }
    }

    async fn graphql(&self, query: &str) -> Result<Value> {
        let body = serde_json::json!({ "query": query });
        let resp = self
            .client
            .post(LINEAR_API_URL)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "Linear API error ({status}): {text}"
            )));
        }

        let v: Value = serde_json::from_str(&text)?;
        if let Some(errors) = v.get("errors") {
            return Err(HiveError::Tracker(format!("Linear GraphQL errors: {errors}")));
        }
        Ok(v)
    }
}

#[async_trait]
impl IssueTracker for LinearTracker {
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>> {
        let team = filters
            .team
            .as_deref()
            .unwrap_or(&self.tracker_config.team);
        let status = filters
            .status
            .as_deref()
            .unwrap_or(&self.tracker_config.ready_filter);
        let query = build_issues_query(team, status, filters.project.as_deref());
        let resp = self.graphql(&query).await?;
        parse_issues_response(&resp.to_string())
    }

    async fn start_issue(&self, id: &str) -> Result<()> {
        self.transition_issue(id, &self.tracker_config.statuses.start)
            .await
    }

    async fn finish_issue(&self, id: &str) -> Result<()> {
        self.transition_issue(id, &self.tracker_config.statuses.review)
            .await
    }

    async fn create_followup(&self, parent_id: &str, content: FollowUpContent) -> Result<String> {
        let labels_json = content
            .labels
            .iter()
            .map(|l| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(", ");

        let query = format!(
            r#"mutation {{
                issueCreate(input: {{
                    title: "{title}"
                    description: "{description}"
                    teamId: "{team}"
                    parentId: "{parent_id}"
                    labelIds: [{labels}]
                }}) {{
                    issue {{ identifier }}
                }}
            }}"#,
            title = content.title.replace('"', r#"\""#),
            description = content.description.replace('"', r#"\""#),
            team = self.tracker_config.team,
            labels = labels_json,
        );

        let resp = self.graphql(&query).await?;
        let id = resp["data"]["issueCreate"]["issue"]["identifier"]
            .as_str()
            .unwrap_or("unknown")
            .to_string();
        Ok(id)
    }

    async fn get_issue(&self, id: &str) -> Result<IssueDetail> {
        let query = format!(
            r#"query {{
                issue(id: "{id}") {{
                    identifier
                    title
                    description
                    priority
                    url
                    labels {{ nodes {{ name }} }}
                }}
            }}"#,
        );

        let resp = self.graphql(&query).await?;
        let issue = &resp["data"]["issue"];

        Ok(IssueDetail {
            id: issue["identifier"]
                .as_str()
                .unwrap_or(id)
                .to_string(),
            title: issue["title"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            description: issue["description"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            acceptance_criteria: None, // Linear doesn't have a dedicated AC field
            priority: issue["priority"]
                .as_u64()
                .map(|p| priority_label(p).to_string()),
            labels: issue["labels"]["nodes"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            url: issue["url"].as_str().unwrap_or("").to_string(),
        })
    }

    fn name(&self) -> &str {
        "linear"
    }
}

impl LinearTracker {
    async fn transition_issue(&self, id: &str, target_status: &str) -> Result<()> {
        // First, find the workflow state ID for the target status
        let team = &self.tracker_config.team;
        let query = format!(
            r#"query {{
                workflowStates(filter: {{ team: {{ name: {{ eq: "{team}" }} }}, name: {{ eq: "{target_status}" }} }}) {{
                    nodes {{ id name }}
                }}
            }}"#,
        );
        let resp = self.graphql(&query).await?;
        let state_id = resp["data"]["workflowStates"]["nodes"][0]["id"]
            .as_str()
            .ok_or_else(|| {
                HiveError::Tracker(format!(
                    "workflow state '{target_status}' not found for team '{team}'"
                ))
            })?;

        // Then update the issue
        let mutation = format!(
            r#"mutation {{
                issueUpdate(id: "{id}", input: {{ stateId: "{state_id}" }}) {{
                    issue {{ identifier state {{ name }} }}
                }}
            }}"#,
        );
        self.graphql(&mutation).await?;
        Ok(())
    }
}

pub fn build_issues_query(team: &str, status: &str, project: Option<&str>) -> String {
    let project_filter = project
        .map(|p| format!(r#", project: {{ name: {{ eq: "{p}" }} }}"#))
        .unwrap_or_default();

    format!(
        r#"query {{
            issues(filter: {{
                team: {{ name: {{ eq: "{team}" }} }}
                state: {{ name: {{ eq: "{status}" }} }}
                {project_filter}
            }}, orderBy: updatedAt, first: 50) {{
                nodes {{
                    identifier
                    title
                    priority
                    url
                    labels {{ nodes {{ name }} }}
                    project {{ name }}
                }}
            }}
        }}"#,
    )
}

pub fn parse_issues_response(json: &str) -> Result<Vec<Issue>> {
    let v: Value = serde_json::from_str(json)?;
    let nodes = v["data"]["issues"]["nodes"]
        .as_array()
        .ok_or_else(|| HiveError::Tracker("unexpected response shape".to_string()))?;

    let issues = nodes
        .iter()
        .map(|node| Issue {
            id: node["identifier"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            title: node["title"].as_str().unwrap_or("").to_string(),
            priority: node["priority"]
                .as_u64()
                .map(|p| priority_label(p).to_string()),
            project: node["project"]["name"]
                .as_str()
                .map(String::from),
            labels: node["labels"]["nodes"]
                .as_array()
                .map(|arr| {
                    arr.iter()
                        .filter_map(|l| l["name"].as_str().map(String::from))
                        .collect()
                })
                .unwrap_or_default(),
            url: node["url"].as_str().unwrap_or("").to_string(),
        })
        .collect();

    Ok(issues)
}

pub fn priority_label(priority: u64) -> &'static str {
    match priority {
        0 => "None",
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "Unknown",
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test trackers::linear`
Expected: All PASS.

- [ ] **Step 5: Wire up trackers module in main.rs and commit**

Add `mod trackers;` to `src/main.rs`.

```bash
git add src/trackers/ src/main.rs
git commit -m "feat: add Linear issue tracker with GraphQL queries and status transitions"
```

---

### Task 9: Notifier Trait + Discord Implementation

**Files:**
- Create: `src/notifiers/mod.rs`
- Create: `src/notifiers/discord.rs`

- [ ] **Step 1: Define the Notifier trait and Discord implementation**

```rust
// src/notifiers/mod.rs
pub mod discord;

use async_trait::async_trait;

use crate::domain::NotifyEvent;
use crate::error::Result;

#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, event: NotifyEvent) -> Result<()>;
    fn name(&self) -> &str;
}
```

```rust
// src/notifiers/discord.rs
use async_trait::async_trait;
use serde_json::json;

use super::Notifier;
use crate::domain::NotifyEvent;
use crate::error::{HiveError, Result};

pub struct DiscordNotifier {
    webhook_url: String,
    client: reqwest::Client,
}

impl DiscordNotifier {
    pub fn new(webhook_url: String) -> Self {
        Self {
            webhook_url,
            client: reqwest::Client::new(),
        }
    }

    fn format_message(&self, event: &NotifyEvent) -> serde_json::Value {
        match event {
            NotifyEvent::StoryComplete {
                issue_id,
                pr_url,
                cost_usd,
                duration_secs,
            } => {
                let mins = duration_secs / 60;
                json!({
                    "embeds": [{
                        "title": format!("✅ {issue_id} — PR Ready"),
                        "description": format!("PR: {pr_url}\nCost: ${cost_usd:.2}\nDuration: {mins}m"),
                        "color": 3066993 // green
                    }]
                })
            }
            NotifyEvent::NeedsAttention { issue_id, reason } => {
                json!({
                    "embeds": [{
                        "title": format!("⚠️ {issue_id} — Needs Attention"),
                        "description": reason,
                        "color": 15844367 // yellow
                    }]
                })
            }
            NotifyEvent::AllIdle => {
                json!({
                    "embeds": [{
                        "title": "💤 All agents idle",
                        "description": "No stories in progress. Queue more work or take a break.",
                        "color": 9807270 // gray
                    }]
                })
            }
            NotifyEvent::CiFailedMaxRetries { issue_id } => {
                json!({
                    "embeds": [{
                        "title": format!("❌ {issue_id} — CI Failed (max retries)"),
                        "description": "CI failures exhausted all retry attempts. Manual intervention required.",
                        "color": 15158332 // red
                    }]
                })
            }
        }
    }
}

#[async_trait]
impl Notifier for DiscordNotifier {
    async fn notify(&self, event: NotifyEvent) -> Result<()> {
        let body = self.format_message(&event);
        let resp = self
            .client
            .post(&self.webhook_url)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(HiveError::Notification(format!(
                "Discord webhook failed ({status}): {text}"
            )));
        }
        Ok(())
    }

    fn name(&self) -> &str {
        "discord"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_complete_message() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::StoryComplete {
            issue_id: "APX-245".to_string(),
            pr_url: "https://github.com/hivemine/gemini-chatz/pull/847".to_string(),
            cost_usd: 1.42,
            duration_secs: 1800,
        };
        let msg = notifier.format_message(&event);
        let title = msg["embeds"][0]["title"].as_str().unwrap();
        assert!(title.contains("APX-245"));
        assert!(title.contains("PR Ready"));
    }

    #[test]
    fn test_format_needs_attention_message() {
        let notifier = DiscordNotifier::new("https://example.com".to_string());
        let event = NotifyEvent::NeedsAttention {
            issue_id: "APX-282".to_string(),
            reason: "Bot review fix attempts exhausted".to_string(),
        };
        let msg = notifier.format_message(&event);
        let desc = msg["embeds"][0]["description"].as_str().unwrap();
        assert!(desc.contains("Bot review fix attempts exhausted"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test notifiers`
Expected: All PASS.

- [ ] **Step 3: Wire up and commit**

Add `mod notifiers;` to `src/main.rs`.

```bash
git add src/notifiers/ src/main.rs
git commit -m "feat: add Notifier trait and Discord webhook implementation"
```

---

### Task 10: GitManager — Worktree Operations

**Files:**
- Create: `src/git/mod.rs`
- Create: `src/git/worktree.rs`
- Create: `src/git/github.rs`

- [ ] **Step 1: Write tests for worktree path construction and listing**

```rust
// src/git/worktree.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_worktree_path() {
        let base = PathBuf::from("/repo/.worktrees");
        let path = worktree_path(&base, "APX-245");
        assert_eq!(path, PathBuf::from("/repo/.worktrees/APX-245"));
    }

    #[test]
    fn test_branch_name() {
        let branch = branch_name("APX-245", "add-number-sequence");
        assert_eq!(branch, "APX-245/add-number-sequence");
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test git::worktree`
Expected: FAIL.

- [ ] **Step 3: Implement worktree operations**

```rust
// src/git/worktree.rs
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::error::{HiveError, Result};

pub fn worktree_path(worktree_dir: &Path, issue_id: &str) -> PathBuf {
    worktree_dir.join(issue_id)
}

pub fn branch_name(issue_id: &str, suffix: &str) -> String {
    format!("{issue_id}/{suffix}")
}

/// Create a new git worktree for the given issue.
pub fn create_worktree(
    repo_path: &Path,
    worktree_dir: &Path,
    issue_id: &str,
    branch: &str,
) -> Result<PathBuf> {
    let wt_path = worktree_path(worktree_dir, issue_id);

    if wt_path.exists() {
        return Ok(wt_path);
    }

    std::fs::create_dir_all(worktree_dir)?;

    let output = Command::new("git")
        .args(["worktree", "add", "-b", branch])
        .arg(&wt_path)
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HiveError::Git(git2::Error::from_str(&format!(
            "failed to create worktree: {stderr}"
        ))));
    }

    Ok(wt_path)
}

/// List all worktrees for the repository.
pub fn list_worktrees(repo_path: &Path) -> Result<Vec<WorktreeInfo>> {
    let output = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        return Err(HiveError::Git(git2::Error::from_str("git worktree list failed")));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut worktrees = Vec::new();
    let mut current: Option<WorktreeInfo> = None;

    for line in stdout.lines() {
        if let Some(path) = line.strip_prefix("worktree ") {
            if let Some(wt) = current.take() {
                worktrees.push(wt);
            }
            current = Some(WorktreeInfo {
                path: PathBuf::from(path),
                branch: None,
                is_bare: false,
            });
        } else if let Some(branch) = line.strip_prefix("branch refs/heads/") {
            if let Some(ref mut wt) = current {
                wt.branch = Some(branch.to_string());
            }
        } else if line == "bare" {
            if let Some(ref mut wt) = current {
                wt.is_bare = true;
            }
        }
    }
    if let Some(wt) = current {
        worktrees.push(wt);
    }

    Ok(worktrees)
}

/// Remove a worktree and optionally delete its branch.
pub fn remove_worktree(repo_path: &Path, issue_id: &str, worktree_dir: &Path) -> Result<()> {
    let wt_path = worktree_path(worktree_dir, issue_id);

    let output = Command::new("git")
        .args(["worktree", "remove", "--force"])
        .arg(&wt_path)
        .current_dir(repo_path)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HiveError::Git(git2::Error::from_str(&format!(
            "failed to remove worktree: {stderr}"
        ))));
    }

    Ok(())
}

/// Rebase a worktree's branch onto the latest master.
pub fn rebase_worktree(worktree_path: &Path) -> Result<RebaseResult> {
    // Fetch latest
    let fetch = Command::new("git")
        .args(["fetch", "origin", "master"])
        .current_dir(worktree_path)
        .output()?;

    if !fetch.status.success() {
        return Ok(RebaseResult::Failed {
            reason: String::from_utf8_lossy(&fetch.stderr).to_string(),
        });
    }

    let rebase = Command::new("git")
        .args(["rebase", "origin/master"])
        .current_dir(worktree_path)
        .output()?;

    if rebase.status.success() {
        Ok(RebaseResult::Success)
    } else {
        // Abort the failed rebase
        let _ = Command::new("git")
            .args(["rebase", "--abort"])
            .current_dir(worktree_path)
            .output();

        Ok(RebaseResult::Conflicts {
            message: String::from_utf8_lossy(&rebase.stderr).to_string(),
        })
    }
}

#[derive(Debug)]
pub struct WorktreeInfo {
    pub path: PathBuf,
    pub branch: Option<String>,
    pub is_bare: bool,
}

#[derive(Debug)]
pub enum RebaseResult {
    Success,
    Conflicts { message: String },
    Failed { reason: String },
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test git::worktree`
Expected: PASS.

- [ ] **Step 5: Implement GitHub API operations (PR, CI, reviews)**

```rust
// src/git/github.rs
use serde_json::Value;

use crate::domain::story_run::PrHandle;
use crate::error::{HiveError, Result};

pub struct GitHubClient {
    owner: String,
    repo: String,
    client: reqwest::Client,
    token: String,
}

impl GitHubClient {
    pub fn new(owner: String, repo: String, token: String) -> Self {
        Self {
            owner,
            repo,
            client: reqwest::Client::new(),
            token,
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}{path}",
            self.owner, self.repo
        )
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self
            .client
            .get(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "hive")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!("GitHub API error ({status}): {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .client
            .post(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "hive")
            .header("Accept", "application/vnd.github+json")
            .json(body)
            .send()
            .await?;

        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!("GitHub API error ({status}): {text}")));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn create_pr(&self, branch: &str, title: &str, body: &str) -> Result<PrHandle> {
        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "head": branch,
            "base": "master"
        });

        let resp = self.post("/pulls", &payload).await?;

        Ok(PrHandle {
            number: resp["number"].as_u64().unwrap_or(0),
            url: resp["html_url"]
                .as_str()
                .unwrap_or("")
                .to_string(),
            head_sha: resp["head"]["sha"]
                .as_str()
                .unwrap_or("")
                .to_string(),
        })
    }

    pub async fn poll_ci(&self, pr_number: u64) -> Result<CiStatus> {
        let resp = self
            .get(&format!("/pulls/{pr_number}/commits"))
            .await?;

        let head_sha = resp
            .as_array()
            .and_then(|commits| commits.last())
            .and_then(|c| c["sha"].as_str())
            .unwrap_or("");

        if head_sha.is_empty() {
            return Ok(CiStatus::Pending);
        }

        let status_resp = self
            .get(&format!("/commits/{head_sha}/check-runs"))
            .await?;

        let check_runs = status_resp["check_runs"]
            .as_array()
            .unwrap_or(&Vec::new())
            .clone();

        if check_runs.is_empty() {
            return Ok(CiStatus::Pending);
        }

        let all_complete = check_runs
            .iter()
            .all(|r| r["status"].as_str() == Some("completed"));

        if !all_complete {
            return Ok(CiStatus::Pending);
        }

        let any_failed = check_runs
            .iter()
            .any(|r| r["conclusion"].as_str() != Some("success"));

        if any_failed {
            let failures: Vec<String> = check_runs
                .iter()
                .filter(|r| r["conclusion"].as_str() != Some("success"))
                .map(|r| {
                    format!(
                        "{}: {}",
                        r["name"].as_str().unwrap_or("unknown"),
                        r["conclusion"].as_str().unwrap_or("unknown")
                    )
                })
                .collect();
            Ok(CiStatus::Failed { failures })
        } else {
            Ok(CiStatus::Passed)
        }
    }

    pub async fn poll_reviews(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
        let resp = self
            .get(&format!("/pulls/{pr_number}/comments"))
            .await?;

        let comments = resp
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|c| ReviewComment {
                id: c["id"].as_u64().unwrap_or(0),
                author: c["user"]["login"]
                    .as_str()
                    .unwrap_or("")
                    .to_string(),
                body: c["body"].as_str().unwrap_or("").to_string(),
                path: c["path"].as_str().map(String::from),
                is_bot: c["user"]["type"].as_str() == Some("Bot"),
            })
            .collect();

        Ok(comments)
    }

    pub async fn push_branch(&self, worktree_path: &std::path::Path, branch: &str) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(worktree_path)
            .output()?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(HiveError::Git(git2::Error::from_str(&format!(
                "push failed: {stderr}"
            ))));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum CiStatus {
    Pending,
    Passed,
    Failed { failures: Vec<String> },
}

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub is_bot: bool,
}
```

```rust
// src/git/mod.rs
pub mod github;
pub mod worktree;
```

- [ ] **Step 6: Wire up, build, and commit**

Add `mod git;` to `src/main.rs`.

Run: `cargo build`
Expected: Compiles.

```bash
git add src/git/ src/main.rs
git commit -m "feat: add GitManager with worktree ops and GitHub API (PR, CI, reviews)"
```

---

## Phase 3: Orchestrator

### Task 11: Phase Transition Logic

**Files:**
- Create: `src/orchestrator/mod.rs`
- Create: `src/orchestrator/transitions.rs`
- Create: `src/orchestrator/retry.rs`

- [ ] **Step 1: Write failing tests for retry budget**

```rust
// src/orchestrator/retry.rs
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_retry_budget_default() {
        let budget = RetryBudget::new(3);
        assert_eq!(budget.remaining(), 3);
        assert!(!budget.exhausted());
    }

    #[test]
    fn test_retry_budget_consume() {
        let mut budget = RetryBudget::new(3);
        assert!(budget.consume());  // 2 left
        assert!(budget.consume());  // 1 left
        assert!(budget.consume());  // 0 left
        assert!(!budget.consume()); // exhausted
        assert!(budget.exhausted());
    }

    #[test]
    fn test_retry_budget_zero() {
        let budget = RetryBudget::new(0);
        assert!(budget.exhausted());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test orchestrator::retry`
Expected: FAIL.

- [ ] **Step 3: Implement RetryBudget**

```rust
// src/orchestrator/retry.rs
#[derive(Debug, Clone)]
pub struct RetryBudget {
    max: u8,
    used: u8,
}

impl RetryBudget {
    pub fn new(max: u8) -> Self {
        Self { max, used: 0 }
    }

    pub fn consume(&mut self) -> bool {
        if self.used < self.max {
            self.used += 1;
            true
        } else {
            false
        }
    }

    pub fn remaining(&self) -> u8 {
        self.max.saturating_sub(self.used)
    }

    pub fn exhausted(&self) -> bool {
        self.used >= self.max
    }

    pub fn attempt(&self) -> u8 {
        self.used
    }
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test orchestrator::retry`
Expected: All PASS.

- [ ] **Step 5: Write tests for phase transition orchestration**

```rust
// src/orchestrator/transitions.rs
#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;
    use crate::config::PhaseConfig;
    use crate::domain::Phase;

    fn enabled_config() -> PhaseConfig {
        PhaseConfig {
            enabled: true, runner: None, model: None, max_attempts: None,
            poll_interval: None, max_fix_attempts: None, max_fix_cycles: None,
            fix_runner: None, fix_model: None, wait_for: None,
        }
    }

    fn disabled_config() -> PhaseConfig {
        PhaseConfig {
            enabled: false, ..enabled_config()
        }
    }

    #[test]
    fn test_advance_from_queued() {
        let phases = HashMap::new();
        let next = advance(Phase::Queued, &phases);
        assert_eq!(next, Phase::Understand);
    }

    #[test]
    fn test_advance_skips_disabled() {
        let mut phases = HashMap::new();
        phases.insert("cross-review".to_string(), disabled_config());
        let next = advance(Phase::SelfReview { attempt: 0 }, &phases);
        assert_eq!(next, Phase::RaisePr);
    }

    #[test]
    fn test_advance_past_handoff_is_complete() {
        let phases = HashMap::new();
        let next = advance(Phase::Handoff, &phases);
        assert_eq!(next, Phase::Complete);
    }

    #[test]
    fn test_advance_all_remaining_disabled() {
        let mut phases = HashMap::new();
        phases.insert("follow-ups".to_string(), disabled_config());
        phases.insert("handoff".to_string(), disabled_config());
        let next = advance(Phase::BotReviews { cycle: 0 }, &phases);
        assert_eq!(next, Phase::Complete);
    }
}
```

- [ ] **Step 6: Run tests to verify they fail**

Run: `cargo test orchestrator::transitions`
Expected: FAIL.

- [ ] **Step 7: Implement advance function**

```rust
// src/orchestrator/transitions.rs
use std::collections::HashMap;

use crate::config::PhaseConfig;
use crate::domain::phase::{next_enabled_phase, Phase};

/// Given the current phase, determine the next phase in the pipeline.
/// Skips disabled phases. Returns Phase::Complete when the pipeline is done.
pub fn advance(current: Phase, phases_config: &HashMap<String, PhaseConfig>) -> Phase {
    if matches!(current, Phase::Queued) {
        // Queued → first enabled phase
        let all = Phase::all_in_order();
        for phase in &all {
            let key = phase.config_key();
            let enabled = phases_config
                .get(key)
                .map(|c| c.enabled)
                .unwrap_or(true);
            if enabled {
                return phase.clone();
            }
        }
        return Phase::Complete;
    }

    match next_enabled_phase(&current, phases_config) {
        Some(phase) => phase,
        None => Phase::Complete,
    }
}
```

- [ ] **Step 8: Run tests to verify they pass**

Run: `cargo test orchestrator`
Expected: All PASS.

- [ ] **Step 9: Create orchestrator module skeleton**

```rust
// src/orchestrator/mod.rs
pub mod retry;
pub mod transitions;
```

Wire up in `src/main.rs`: add `mod orchestrator;`.

- [ ] **Step 10: Commit**

```bash
git add src/orchestrator/ src/main.rs
git commit -m "feat: add orchestrator phase transitions and retry budget"
```

---

### Task 12: Orchestrator Core Event Loop

**Files:**
- Modify: `src/orchestrator/mod.rs`

This is the central coordinator. It receives commands from the TUI, drives phase transitions, manages agent sessions, and emits events back to the TUI.

- [ ] **Step 1: Implement the Orchestrator struct**

```rust
// src/orchestrator/mod.rs
pub mod retry;
pub mod transitions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::config::ProjectConfig;
use crate::domain::{
    AgentEvent, NotifyEvent, OrchestratorEvent, Phase, RunStatus, StoryRun, TuiCommand,
};
use crate::error::Result;
use crate::notifiers::Notifier;
use crate::runners::AgentRunner;
use crate::state::persistence;
use crate::trackers::IssueTracker;

use self::transitions::advance;

pub struct Orchestrator {
    config: ProjectConfig,
    runs: HashMap<String, StoryRun>,
    runs_dir: PathBuf,

    // Backend implementations (resolved at startup from config)
    runner: Arc<dyn AgentRunner>,
    tracker: Arc<dyn IssueTracker>,
    notifier: Option<Arc<dyn Notifier>>,

    // Channel to send events to the TUI
    event_tx: mpsc::Sender<OrchestratorEvent>,

    // Channel to receive commands from the TUI
    command_rx: mpsc::Receiver<TuiCommand>,
}

impl Orchestrator {
    pub fn new(
        config: ProjectConfig,
        runs_dir: PathBuf,
        runner: Arc<dyn AgentRunner>,
        tracker: Arc<dyn IssueTracker>,
        notifier: Option<Arc<dyn Notifier>>,
        event_tx: mpsc::Sender<OrchestratorEvent>,
        command_rx: mpsc::Receiver<TuiCommand>,
    ) -> Result<Self> {
        // Load persisted runs on startup
        let runs_vec = persistence::load_all_runs(&runs_dir)?;
        let runs: HashMap<String, StoryRun> = runs_vec
            .into_iter()
            .map(|r| (r.issue_id.clone(), r))
            .collect();

        Ok(Self {
            config,
            runs,
            runs_dir,
            runner,
            tracker,
            notifier,
            event_tx,
            command_rx,
        })
    }

    /// Main event loop. Runs until the TUI sends Quit.
    pub async fn run(&mut self) -> Result<()> {
        // Emit initial state to TUI
        for run in self.runs.values() {
            let _ = self
                .event_tx
                .send(OrchestratorEvent::StoryUpdated(run.clone()))
                .await;
        }

        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        TuiCommand::Quit => break,
                        TuiCommand::StartStory { issue } => {
                            self.start_story(issue).await?;
                        }
                        TuiCommand::CancelStory { issue_id } => {
                            self.cancel_story(&issue_id).await?;
                        }
                        TuiCommand::RebaseStory { issue_id } => {
                            self.rebase_story(&issue_id).await?;
                        }
                        TuiCommand::RefreshStories => {
                            // Handled by TUI directly via tracker
                        }
                        TuiCommand::CopyWorktreePath { .. } => {
                            // Handled by TUI directly
                        }
                    }
                }
            }
        }

        Ok(())
    }

    async fn start_story(&mut self, issue: crate::domain::Issue) -> Result<()> {
        let mut run = StoryRun::new(issue.id.clone(), issue.title.clone());
        let next = advance(Phase::Queued, &self.config.phases);
        run.phase = next.clone();

        // Persist and notify TUI
        persistence::save_run(&self.runs_dir, &run)?;
        let _ = self
            .event_tx
            .send(OrchestratorEvent::StoryUpdated(run.clone()))
            .await;

        // Transition issue to "In Progress" in tracker
        if let Err(e) = self.tracker.start_issue(&issue.id).await {
            tracing::warn!("failed to transition issue {}: {e}", issue.id);
        }

        self.runs.insert(issue.id, run);
        Ok(())
    }

    async fn cancel_story(&mut self, issue_id: &str) -> Result<()> {
        if let Some(run) = self.runs.get_mut(issue_id) {
            // Cancel any running agent session
            if let Some(ref session_id) = run.session_id {
                let handle = crate::runners::SessionHandle {
                    session_id: session_id.clone(),
                    runner_name: self.runner.name().to_string(),
                    pid: None,
                };
                let _ = self.runner.cancel(&handle).await;
            }

            run.status = RunStatus::Failed;
            run.updated_at = Utc::now();
            persistence::save_run(&self.runs_dir, run)?;
            let _ = self
                .event_tx
                .send(OrchestratorEvent::StoryUpdated(run.clone()))
                .await;
        }
        Ok(())
    }

    async fn rebase_story(&mut self, issue_id: &str) -> Result<()> {
        if let Some(run) = self.runs.get(issue_id) {
            if let Some(ref wt_path) = run.worktree {
                let result = crate::git::worktree::rebase_worktree(wt_path)?;
                tracing::info!("rebase result for {issue_id}: {result:?}");
            }
        }
        Ok(())
    }

    async fn send_notification(&self, event: NotifyEvent) {
        if let Some(ref notifier) = self.notifier {
            if let Err(e) = notifier.notify(event).await {
                tracing::warn!("notification failed: {e}");
            }
        }
    }
}
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo build`
Expected: Compiles.

- [ ] **Step 3: Commit**

```bash
git add src/orchestrator/
git commit -m "feat: add Orchestrator core with command handling and story lifecycle"
```

---

## Phase 4: TUI

### Task 13: App Skeleton and Event Loop

**Files:**
- Create: `src/app.rs`
- Create: `src/tui/mod.rs`
- Create: `src/tui/tabs/mod.rs`
- Create: `src/tui/widgets/mod.rs`
- Modify: `src/main.rs`

- [ ] **Step 1: Implement the Tab enum and App state**

```rust
// src/tui/tabs/mod.rs
pub mod agents;
pub mod config_tab;
pub mod stories;
pub mod worktrees;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tab {
    Agents,
    Stories,
    Worktrees,
    Config,
}

impl Tab {
    pub fn all() -> &'static [Tab] {
        &[Tab::Agents, Tab::Stories, Tab::Worktrees, Tab::Config]
    }

    pub fn label(&self) -> &'static str {
        match self {
            Tab::Agents => "Agents",
            Tab::Stories => "Stories",
            Tab::Worktrees => "Worktrees",
            Tab::Config => "Config",
        }
    }

    pub fn index(&self) -> usize {
        match self {
            Tab::Agents => 0,
            Tab::Stories => 1,
            Tab::Worktrees => 2,
            Tab::Config => 3,
        }
    }

    pub fn from_index(i: usize) -> Self {
        match i {
            0 => Tab::Agents,
            1 => Tab::Stories,
            2 => Tab::Worktrees,
            3 => Tab::Config,
            _ => Tab::Agents,
        }
    }

    pub fn next(&self) -> Self {
        Self::from_index((self.index() + 1) % Self::all().len())
    }

    pub fn prev(&self) -> Self {
        let len = Self::all().len();
        Self::from_index((self.index() + len - 1) % len)
    }
}
```

- [ ] **Step 2: Implement the main TUI event loop**

```rust
// src/tui/mod.rs
pub mod tabs;
pub mod widgets;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use ratatui::DefaultTerminal;
use tokio::sync::mpsc;

use crate::domain::{OrchestratorEvent, StoryRun, TuiCommand};

use self::tabs::Tab;

pub struct Tui {
    active_tab: Tab,
    runs: Vec<StoryRun>,
    selected_agent: usize,
    should_quit: bool,

    event_rx: mpsc::Receiver<OrchestratorEvent>,
    command_tx: mpsc::Sender<TuiCommand>,
}

impl Tui {
    pub fn new(
        event_rx: mpsc::Receiver<OrchestratorEvent>,
        command_tx: mpsc::Sender<TuiCommand>,
    ) -> Self {
        Self {
            active_tab: Tab::Agents,
            runs: Vec::new(),
            selected_agent: 0,
            should_quit: false,
            event_rx,
            command_tx,
        }
    }

    pub async fn run(&mut self, terminal: &mut DefaultTerminal) -> io::Result<()> {
        while !self.should_quit {
            terminal.draw(|frame| self.render(frame))?;

            // Poll for events with a short timeout for responsive UI
            tokio::select! {
                // Terminal input
                _ = tokio::time::sleep(Duration::from_millis(50)) => {
                    if event::poll(Duration::from_millis(0))? {
                        if let Event::Key(key) = event::read()? {
                            self.handle_key(key.code, key.modifiers).await;
                        }
                    }
                }
                // Orchestrator events
                Some(event) = self.event_rx.recv() => {
                    self.handle_orchestrator_event(event);
                }
            }
        }

        Ok(())
    }

    fn render(&self, frame: &mut ratatui::Frame) {
        use ratatui::layout::{Constraint, Layout};

        let [tab_area, main_area, status_area] = Layout::vertical([
            Constraint::Length(1),
            Constraint::Fill(1),
            Constraint::Length(1),
        ])
        .areas(frame.area());

        // Render tab bar
        widgets::status_bar::render_tab_bar(frame, tab_area, &self.active_tab, &self.runs);

        // Render active tab content
        match self.active_tab {
            Tab::Agents => {
                tabs::agents::render(frame, main_area, &self.runs, self.selected_agent);
            }
            Tab::Stories => {
                tabs::stories::render(frame, main_area);
            }
            Tab::Worktrees => {
                tabs::worktrees::render(frame, main_area);
            }
            Tab::Config => {
                tabs::config_tab::render(frame, main_area);
            }
        }

        // Render status bar
        widgets::status_bar::render_status_bar(frame, status_area, &self.runs);
    }

    async fn handle_key(&mut self, code: KeyCode, modifiers: KeyModifiers) {
        // Global keybindings
        match code {
            KeyCode::Char('q') => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
            }
            KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                let _ = self.command_tx.send(TuiCommand::Quit).await;
                self.should_quit = true;
            }
            KeyCode::Tab => self.active_tab = self.active_tab.next(),
            KeyCode::BackTab => self.active_tab = self.active_tab.prev(),
            KeyCode::Char('1') => self.active_tab = Tab::Agents,
            KeyCode::Char('2') => self.active_tab = Tab::Stories,
            KeyCode::Char('3') => self.active_tab = Tab::Worktrees,
            KeyCode::Char('4') => self.active_tab = Tab::Config,
            // Tab-specific keybindings
            KeyCode::Char('j') | KeyCode::Down => {
                if self.active_tab == Tab::Agents && !self.runs.is_empty() {
                    self.selected_agent = (self.selected_agent + 1).min(self.runs.len() - 1);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if self.active_tab == Tab::Agents && self.selected_agent > 0 {
                    self.selected_agent -= 1;
                }
            }
            _ => {}
        }
    }

    fn handle_orchestrator_event(&mut self, event: OrchestratorEvent) {
        match event {
            OrchestratorEvent::StoryUpdated(run) => {
                if let Some(existing) = self.runs.iter_mut().find(|r| r.issue_id == run.issue_id) {
                    *existing = run;
                } else {
                    self.runs.push(run);
                }
            }
            OrchestratorEvent::PhaseTransition { .. } => {
                // Handled via StoryUpdated
            }
            OrchestratorEvent::AgentOutput { .. } => {
                // Will be handled when log viewer is implemented
            }
            OrchestratorEvent::Error { message, .. } => {
                tracing::error!("orchestrator error: {message}");
            }
        }
    }
}
```

- [ ] **Step 3: Create placeholder tab render functions**

```rust
// src/tui/tabs/agents.rs
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Frame;

use crate::domain::{Phase, RunStatus, StoryRun};

pub fn render(frame: &mut Frame, area: Rect, runs: &[StoryRun], selected: usize) {
    let [sidebar_area, main_area] = Layout::horizontal([
        Constraint::Percentage(28),
        Constraint::Fill(1),
    ])
    .areas(area);

    render_sidebar(frame, sidebar_area, runs, selected);
    render_main_panel(frame, main_area, runs.get(selected));
}

fn render_sidebar(frame: &mut Frame, area: Rect, runs: &[StoryRun], selected: usize) {
    let items: Vec<ListItem> = runs
        .iter()
        .enumerate()
        .map(|(i, run)| {
            let (icon, color) = match run.status {
                RunStatus::Running => ("▶", Color::Green),
                RunStatus::NeedsAttention => ("⚠", Color::Yellow),
                RunStatus::Complete => ("✓", Color::Blue),
                RunStatus::Paused => ("⏸", Color::Gray),
                RunStatus::Failed => ("✗", Color::Red),
            };
            let style = if i == selected {
                Style::default().fg(color).bg(Color::DarkGray)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(vec![
                Span::raw(format!("{icon} {}", run.issue_id)),
            ]))
            .style(style)
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .title(" Agents ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(list, area);
}

fn render_main_panel(frame: &mut Frame, area: Rect, selected_run: Option<&StoryRun>) {
    let content = match selected_run {
        Some(run) => format!(
            "{} — {}\nPhase: {}\nStatus: {:?}\nCost: ${:.2}",
            run.issue_id, run.issue_title, run.phase, run.status, run.cost_usd
        ),
        None => "No agent selected. Select a story from the Stories tab to begin.".to_string(),
    };

    let paragraph = Paragraph::new(content).block(
        Block::default()
            .title(" Details ")
            .borders(Borders::ALL)
            .border_style(Style::default().fg(Color::DarkGray)),
    );

    frame.render_widget(paragraph, area);
}
```

```rust
// src/tui/tabs/stories.rs
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect) {
    let placeholder = Paragraph::new("Stories tab — will show filterable issue list")
        .block(
            Block::default()
                .title(" Stories ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(placeholder, area);
}
```

```rust
// src/tui/tabs/worktrees.rs
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect) {
    let placeholder = Paragraph::new("Worktrees tab — will show active worktrees")
        .block(
            Block::default()
                .title(" Worktrees ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(placeholder, area);
}
```

```rust
// src/tui/tabs/config_tab.rs
use ratatui::layout::Rect;
use ratatui::style::{Color, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

pub fn render(frame: &mut Frame, area: Rect) {
    let placeholder = Paragraph::new("Config tab — will show project configuration")
        .block(
            Block::default()
                .title(" Config ")
                .borders(Borders::ALL)
                .border_style(Style::default().fg(Color::DarkGray)),
        );
    frame.render_widget(placeholder, area);
}
```

- [ ] **Step 4: Create status bar widget**

```rust
// src/tui/widgets/status_bar.rs
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::domain::{RunStatus, StoryRun};
use crate::tui::tabs::Tab;

pub fn render_tab_bar(frame: &mut Frame, area: Rect, active: &Tab, runs: &[StoryRun]) {
    let running = runs.iter().filter(|r| r.status == RunStatus::Running).count();
    let attention = runs
        .iter()
        .filter(|r| r.status == RunStatus::NeedsAttention)
        .count();
    let total_cost: f64 = runs.iter().map(|r| r.cost_usd).sum();

    let mut spans = Vec::new();
    for tab in Tab::all() {
        let style = if tab == active {
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        spans.push(Span::styled(
            format!(" [{}] {} ", tab.index() + 1, tab.label()),
            style,
        ));
    }

    // Status indicators on the right
    spans.push(Span::raw("  "));
    if running > 0 {
        spans.push(Span::styled(
            format!("● {running} running "),
            Style::default().fg(Color::Green),
        ));
    }
    if attention > 0 {
        spans.push(Span::styled(
            format!("● {attention} attn "),
            Style::default().fg(Color::Yellow),
        ));
    }
    spans.push(Span::styled(
        format!("${total_cost:.2}"),
        Style::default().fg(Color::DarkGray),
    ));

    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn render_status_bar(frame: &mut Frame, area: Rect, _runs: &[StoryRun]) {
    let line = Line::from(vec![
        Span::styled(
            " q",
            Style::default().fg(Color::Cyan),
        ),
        Span::raw(" quit  "),
        Span::styled("Tab", Style::default().fg(Color::Cyan)),
        Span::raw(" switch  "),
        Span::styled("j/k", Style::default().fg(Color::Cyan)),
        Span::raw(" navigate  "),
        Span::styled("?", Style::default().fg(Color::Cyan)),
        Span::raw(" help"),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}
```

```rust
// src/tui/widgets/mod.rs
pub mod status_bar;
```

- [ ] **Step 5: Wire up main.rs with Tokio and terminal setup**

```rust
// src/main.rs
mod config;
mod domain;
mod error;
mod git;
mod notifiers;
mod orchestrator;
mod runners;
mod state;
mod trackers;
mod tui;

use std::io;

#[tokio::main]
async fn main() -> io::Result<()> {
    // For now, just launch the TUI with no orchestrator connection.
    // Full wiring comes in Task 14 (app.rs).
    let (event_tx, event_rx) = tokio::sync::mpsc::channel(256);
    let (command_tx, _command_rx) = tokio::sync::mpsc::channel(256);

    // Keep the sender alive so the channel doesn't close
    let _event_tx = event_tx;

    let mut tui = tui::Tui::new(event_rx, command_tx);

    let mut terminal = ratatui::init();
    let result = tui.run(&mut terminal).await;
    ratatui::restore();
    result
}
```

- [ ] **Step 6: Run the TUI to verify it renders**

Run: `cargo run`
Expected: TUI launches with tab bar, empty agents panel, and status bar. Press `1-4` to switch tabs, `q` to quit.

- [ ] **Step 7: Commit**

```bash
git add src/tui/ src/main.rs
git commit -m "feat: add TUI skeleton with tab navigation, agents sidebar, and status bar"
```

---

### Task 14: App Bootstrap — Wiring Everything Together

**Files:**
- Create: `src/app.rs`
- Modify: `src/main.rs`

This is where config loading, backend construction, and channel wiring come together.

- [ ] **Step 1: Implement App struct that bootstraps the full system**

```rust
// src/app.rs
use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::mpsc;

use crate::config::resolve::{load_global_config, load_project_config, resolve_env};
use crate::domain::{OrchestratorEvent, TuiCommand};
use crate::error::{HiveError, Result};
use crate::notifiers::discord::DiscordNotifier;
use crate::notifiers::Notifier;
use crate::orchestrator::Orchestrator;
use crate::runners::claude::ClaudeRunner;
use crate::runners::AgentRunner;
use crate::trackers::linear::LinearTracker;
use crate::trackers::IssueTracker;
use crate::tui::Tui;

pub async fn run(repo_path: &str) -> Result<()> {
    let config_dir = dirs_config_dir()?;
    let global = load_global_config(&config_dir)?;
    let project = load_project_config(&config_dir, repo_path)?;

    // Resolve the primary runner
    let runner_name = project
        .phases
        .values()
        .find(|p| p.enabled && p.runner.is_some())
        .and_then(|p| p.runner.clone())
        .unwrap_or_else(|| "claude".to_string());

    let runner_config = global.runners.get(&runner_name).ok_or_else(|| {
        HiveError::Config(format!("runner '{runner_name}' not configured in global config"))
    })?;

    let runner: Arc<dyn AgentRunner> = Arc::new(ClaudeRunner::new(
        runner_config.command.clone(),
        runner_config.default_model.clone(),
        runner_config.permission_mode.clone(),
    ));

    // Resolve the tracker
    let tracker_conn = global.trackers.get(&project.tracker).ok_or_else(|| {
        HiveError::Config(format!(
            "tracker '{}' not configured in global config",
            project.tracker
        ))
    })?;

    let tracker: Arc<dyn IssueTracker> = match project.tracker.as_str() {
        "linear" => {
            let api_key = tracker_conn
                .api_key
                .as_ref()
                .map(|k| resolve_env(k))
                .transpose()?
                .ok_or_else(|| HiveError::Config("linear api_key not set".to_string()))?;
            Arc::new(LinearTracker::new(api_key, project.tracker_config.clone()))
        }
        other => {
            return Err(HiveError::Config(format!(
                "unsupported tracker: {other}"
            )));
        }
    };

    // Resolve notifier (optional)
    let notifier: Option<Arc<dyn Notifier>> = if let Some(ref notifier_name) = project.notifier {
        if notifier_name == "none" {
            None
        } else if let Some(notif_config) = global.notifications.get(notifier_name) {
            let webhook_url = resolve_env(&notif_config.webhook_url)?;
            match notifier_name.as_str() {
                "discord" => Some(Arc::new(DiscordNotifier::new(webhook_url))),
                other => {
                    tracing::warn!("unsupported notifier: {other}, skipping");
                    None
                }
            }
        } else {
            None
        }
    } else {
        None
    };

    // Set up channels
    let (event_tx, event_rx) = mpsc::channel::<OrchestratorEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<TuiCommand>(64);

    let runs_dir = config_dir
        .join("projects")
        .join(&project.name)
        .join("runs");

    // Start orchestrator in background
    let mut orchestrator =
        Orchestrator::new(project, runs_dir, runner, tracker, notifier, event_tx, command_rx)?;

    tokio::spawn(async move {
        if let Err(e) = orchestrator.run().await {
            tracing::error!("orchestrator error: {e}");
        }
    });

    // Run TUI in foreground
    let mut tui = Tui::new(event_rx, command_tx);
    let mut terminal = ratatui::init();
    let result = tui.run(&mut terminal).await;
    ratatui::restore();
    result.map_err(|e| HiveError::Io(e))
}

pub fn dirs_config_dir() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .map_err(|_| HiveError::Config("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".config").join("hive"))
}
```

- [ ] **Step 2: Update main.rs to use app.rs**

```rust
// src/main.rs
mod app;
mod config;
mod domain;
mod error;
mod git;
mod notifiers;
mod orchestrator;
mod runners;
mod state;
mod trackers;
mod tui;

use clap::Parser;

#[derive(Parser)]
#[command(name = "hive", version, about = "Agent orchestration TUI")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(clap::Subcommand)]
enum Commands {
    /// Initialize a new project
    Init,
    /// Re-run the setup wizard
    Configure,
    /// Print status of all active runs
    Status,
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("hive=info")
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => {
            // Default: launch TUI for current directory
            let cwd = std::env::current_dir()
                .expect("cannot determine current directory")
                .to_string_lossy()
                .to_string();

            if let Err(e) = app::run(&cwd).await {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Some(Commands::Init) => {
            println!("hive init — not yet implemented");
        }
        Some(Commands::Configure) => {
            println!("hive configure — not yet implemented");
        }
        Some(Commands::Status) => {
            println!("hive status — not yet implemented");
        }
    }
}
```

Add `tracing-subscriber` and `dirs` dependencies if not already in Cargo.toml:

```toml
# Already listed in Task 1, just verify tracing-subscriber is there
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build`
Expected: Compiles. Running `cargo run` without config will show the error message suggesting `hive init`.

- [ ] **Step 4: Commit**

```bash
git add src/app.rs src/main.rs
git commit -m "feat: add app bootstrap — wires config, backends, orchestrator, and TUI together"
```

---

## Phase 5: CLI Commands

### Task 15: hive init Wizard

**Files:**
- Create: `src/cli/mod.rs`
- Create: `src/cli/init.rs`

- [ ] **Step 1: Implement the init wizard**

```rust
// src/cli/mod.rs
pub mod init;
```

```rust
// src/cli/init.rs
use std::io::{self, Write};
use std::path::PathBuf;

use crate::config::project::{
    GitHubConfig, NotificationConfig, PhaseConfig, ProjectConfig, StatusMappings, TrackerConfig,
};
use crate::error::{HiveError, Result};

pub fn run_init() -> Result<()> {
    println!("🐝 Hive — Project Setup\n");

    let config_dir = crate::app::dirs_config_dir()?;

    // Check if global config exists; if not, create it first
    let global_config_path = config_dir.join("config.toml");
    if !global_config_path.exists() {
        println!("No global config found. Let's set that up first.\n");
        create_global_config(&config_dir)?;
        println!();
    }

    // Project setup
    let cwd = std::env::current_dir()
        .map_err(|e| HiveError::Config(format!("cannot determine cwd: {e}")))?;

    let default_name = cwd
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("project");

    let name = prompt(&format!("Project name [{default_name}]:"))?;
    let name = if name.is_empty() {
        default_name.to_string()
    } else {
        name
    };

    let default_repo = cwd.to_string_lossy().to_string();
    let repo_path = prompt(&format!("Repository path [{default_repo}]:"))?;
    let repo_path = if repo_path.is_empty() {
        default_repo
    } else {
        repo_path
    };

    let worktree_dir = prompt("Worktree directory [.worktrees]:")?;
    let worktree_dir = if worktree_dir.is_empty() {
        ".worktrees".to_string()
    } else {
        worktree_dir
    };

    let tracker = prompt("Issue tracker (linear/jira) [linear]:")?;
    let tracker = if tracker.is_empty() {
        "linear".to_string()
    } else {
        tracker
    };

    let team = prompt("Tracker team/project:")?;
    let ready_filter = prompt("Ready status name [Todo]:")?;
    let ready_filter = if ready_filter.is_empty() {
        "Todo".to_string()
    } else {
        ready_filter
    };

    let start_status = prompt("In-progress status name [In Progress]:")?;
    let start_status = if start_status.is_empty() {
        "In Progress".to_string()
    } else {
        start_status
    };

    let review_status = prompt("In-review status name [In Review]:")?;
    let review_status = if review_status.is_empty() {
        "In Review".to_string()
    } else {
        review_status
    };

    let done_status = prompt("Done status name [Done]:")?;
    let done_status = if done_status.is_empty() {
        "Done".to_string()
    } else {
        done_status
    };

    // Auto-detect GitHub remote
    let (gh_owner, gh_repo) = detect_github_remote(&PathBuf::from(&repo_path));
    let gh_owner_str = prompt(&format!(
        "GitHub owner [{owner}]:",
        owner = gh_owner.as_deref().unwrap_or("")
    ))?;
    let gh_owner = if gh_owner_str.is_empty() {
        gh_owner.unwrap_or_default()
    } else {
        gh_owner_str
    };
    let gh_repo_str = prompt(&format!(
        "GitHub repo [{repo}]:",
        repo = gh_repo.as_deref().unwrap_or("")
    ))?;
    let gh_repo = if gh_repo_str.is_empty() {
        gh_repo.unwrap_or_default()
    } else {
        gh_repo_str
    };

    let notifier_choice = prompt("Notifications (discord/slack/none) [none]:")?;
    let notifier = if notifier_choice.is_empty() || notifier_choice == "none" {
        None
    } else {
        Some(notifier_choice)
    };

    // Build config
    let mut phases = std::collections::HashMap::new();
    let phase_names = [
        "understand",
        "implement",
        "self-review",
        "cross-review",
        "raise-pr",
        "ci-watch",
        "bot-reviews",
        "follow-ups",
        "handoff",
    ];
    for phase_name in &phase_names {
        let default = if *phase_name == "cross-review" {
            "n"
        } else {
            "y"
        };
        let answer = prompt(&format!("Enable {phase_name}? (y/n) [{default}]:"))?;
        let enabled = if answer.is_empty() {
            default == "y"
        } else {
            answer.starts_with('y')
        };
        phases.insert(
            phase_name.to_string(),
            PhaseConfig {
                enabled,
                runner: None,
                model: None,
                max_attempts: None,
                poll_interval: None,
                max_fix_attempts: None,
                max_fix_cycles: None,
                fix_runner: None,
                fix_model: None,
                wait_for: None,
            },
        );
    }

    let project_config = ProjectConfig {
        name: name.clone(),
        repo_path,
        worktree_dir,
        tracker,
        notifier,
        notifications: Some(NotificationConfig {
            events: vec![
                "complete".to_string(),
                "needs-attention".to_string(),
            ],
        }),
        github: GitHubConfig {
            owner: gh_owner,
            repo: gh_repo,
        },
        tracker_config: TrackerConfig {
            team,
            ready_filter,
            statuses: StatusMappings {
                start: start_status,
                review: review_status,
                done: done_status,
            },
            fields: std::collections::HashMap::new(),
        },
        phases,
    };

    // Write config
    let project_dir = config_dir.join("projects").join(&name);
    std::fs::create_dir_all(&project_dir)?;

    let toml_str = toml::to_string_pretty(&project_config)
        .map_err(|e| HiveError::Config(format!("failed to serialize config: {e}")))?;

    std::fs::write(project_dir.join("project.toml"), toml_str)?;

    println!("\n✅ Project '{name}' configured!");
    println!("   Config: {}", project_dir.join("project.toml").display());
    println!("   Run `hive` in your repo to launch the dashboard.");

    Ok(())
}

fn prompt(message: &str) -> Result<String> {
    print!("{message} ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    Ok(input.trim().to_string())
}

fn create_global_config(config_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;

    let config = r#"[runners.claude]
command = "claude"
protocol = "acp"
default_model = "opus-4-6"
permission_mode = "dangerously-skip"

[runners.gemini]
command = "gemini"
protocol = "acp"
default_model = "flash"

[trackers.linear]
api_key = "env:LINEAR_API_KEY"

[notifications.discord]
webhook_url = "env:HIVE_DISCORD_WEBHOOK"

[notifications.slack]
webhook_url = "env:HIVE_SLACK_WEBHOOK"
"#;

    std::fs::write(config_dir.join("config.toml"), config)?;
    println!("Created {}", config_dir.join("config.toml").display());
    Ok(())
}

fn detect_github_remote(repo_path: &PathBuf) -> (Option<String>, Option<String>) {
    let output = std::process::Command::new("git")
        .args(["remote", "get-url", "origin"])
        .current_dir(repo_path)
        .output()
        .ok();

    if let Some(output) = output {
        let url = String::from_utf8_lossy(&output.stdout).trim().to_string();
        // Parse github.com/owner/repo from various URL formats
        if let Some(path) = url
            .strip_prefix("git@github.com:")
            .or_else(|| url.strip_prefix("https://github.com/"))
        {
            let path = path.trim_end_matches(".git");
            let parts: Vec<&str> = path.splitn(2, '/').collect();
            if parts.len() == 2 {
                return (Some(parts[0].to_string()), Some(parts[1].to_string()));
            }
        }
    }

    (None, None)
}
```

Note: `ProjectConfig` needs `Serialize` derived in addition to `Deserialize`. Update `src/config/project.rs` to add `#[derive(Debug, Serialize, Deserialize)]` (and the nested types) and `use serde::Serialize;`.

- [ ] **Step 2: Wire init into main.rs**

Update the `Commands::Init` match arm:

```rust
Some(Commands::Init) => {
    if let Err(e) = cli::init::run_init() {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
```

Add `mod cli;` to `src/main.rs`.

- [ ] **Step 3: Add Serialize derives to config types**

Update `src/config/project.rs`: add `Serialize` to `ProjectConfig`, `NotificationConfig`, `GitHubConfig`, `TrackerConfig`, `StatusMappings`, `PhaseConfig`.

Update `src/config/global.rs`: add `Serialize` to all types.

- [ ] **Step 4: Test the init wizard**

Run: `cargo run -- init`
Expected: Interactive wizard prompts for project configuration.

- [ ] **Step 5: Commit**

```bash
git add src/cli/ src/config/ src/main.rs src/app.rs
git commit -m "feat: add hive init wizard for interactive project setup"
```

---

### Task 16: hive status Command

**Files:**
- Create: `src/cli/status.rs`
- Modify: `src/cli/mod.rs`

- [ ] **Step 1: Implement the status command**

```rust
// src/cli/status.rs
use crate::config::resolve::{load_project_config};
use crate::error::Result;
use crate::state::persistence;

pub fn run_status(repo_path: &str) -> Result<()> {
    let config_dir = crate::app::dirs_config_dir()?;
    let project = load_project_config(&config_dir, repo_path)?;

    let runs_dir = config_dir
        .join("projects")
        .join(&project.name)
        .join("runs");

    let runs = persistence::load_all_runs(&runs_dir)?;

    if runs.is_empty() {
        println!("No active runs for project '{}'.", project.name);
        return Ok(());
    }

    println!(
        "{:<12} {:<30} {:<18} {:<12} {:<8}",
        "Issue", "Title", "Phase", "Status", "Cost"
    );
    println!("{}", "-".repeat(80));

    for run in &runs {
        let title = if run.issue_title.len() > 28 {
            format!("{}...", &run.issue_title[..25])
        } else {
            run.issue_title.clone()
        };
        println!(
            "{:<12} {:<30} {:<18} {:<12} ${:.2}",
            run.issue_id,
            title,
            run.phase.config_key(),
            format!("{:?}", run.status),
            run.cost_usd,
        );
    }

    let total: f64 = runs.iter().map(|r| r.cost_usd).sum();
    println!("\n{} runs, ${:.2} total cost", runs.len(), total);

    Ok(())
}
```

```rust
// src/cli/mod.rs
pub mod init;
pub mod status;
```

- [ ] **Step 2: Wire into main.rs**

```rust
Some(Commands::Status) => {
    let cwd = std::env::current_dir()
        .expect("cannot determine current directory")
        .to_string_lossy()
        .to_string();
    if let Err(e) = cli::status::run_status(&cwd) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}
```

- [ ] **Step 3: Verify it compiles and commit**

Run: `cargo build`

```bash
git add src/cli/ src/main.rs
git commit -m "feat: add hive status command for quick run summaries"
```

---

## Phase 6: Additional Backends (Stubs)

### Task 17: Remaining Runner and Tracker Stubs

**Files:**
- Create: `src/runners/gemini.rs`
- Create: `src/runners/codex.rs`
- Create: `src/trackers/jira.rs`
- Create: `src/notifiers/slack.rs`

These are skeleton implementations — they implement the traits and return `todo!()` errors. This makes the architecture complete and compilable, with clear extension points.

- [ ] **Step 1: Create Gemini runner stub**

```rust
// src/runners/gemini.rs
use std::pin::Pin;

use async_trait::async_trait;
use futures::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

use super::{AgentRunner, SessionConfig, SessionHandle};
use crate::domain::AgentEvent;
use crate::error::{HiveError, Result};

pub struct GeminiRunner {
    command: String,
    default_model: String,
}

impl GeminiRunner {
    pub fn new(command: String, default_model: String) -> Self {
        Self { command, default_model }
    }
}

#[async_trait]
impl AgentRunner for GeminiRunner {
    async fn start_session(&self, _config: SessionConfig) -> Result<SessionHandle> {
        Err(HiveError::Agent("Gemini runner not yet implemented".into()))
    }

    async fn send_prompt(&self, _session: &SessionHandle, _prompt: &str) -> Result<()> {
        Err(HiveError::Agent("Gemini runner not yet implemented".into()))
    }

    fn output_stream(&self, _session: &SessionHandle) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
        let (_tx, rx) = mpsc::channel(1);
        Box::pin(ReceiverStream::new(rx))
    }

    async fn cancel(&self, _session: &SessionHandle) -> Result<()> { Ok(()) }
    async fn resume(&self, _session: &SessionHandle) -> Result<()> { Ok(()) }
    async fn is_alive(&self, _session: &SessionHandle) -> bool { false }
    fn name(&self) -> &str { "gemini" }
}
```

- [ ] **Step 2: Create Codex runner stub**

```rust
// src/runners/codex.rs
// Same pattern as gemini.rs, with "codex" as name and
// "Codex runner not yet implemented" as error messages.
```

(Follow identical structure to `gemini.rs` above, replacing "gemini" with "codex" and `GeminiRunner` with `CodexRunner`.)

- [ ] **Step 3: Create Jira tracker stub**

```rust
// src/trackers/jira.rs
use async_trait::async_trait;

use super::IssueTracker;
use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::{HiveError, Result};

pub struct JiraTracker {
    base_url: String,
    api_token: String,
    email: String,
}

impl JiraTracker {
    pub fn new(base_url: String, api_token: String, email: String) -> Self {
        Self { base_url, api_token, email }
    }
}

#[async_trait]
impl IssueTracker for JiraTracker {
    async fn list_ready(&self, _filters: &IssueFilters) -> Result<Vec<Issue>> {
        Err(HiveError::Tracker("Jira tracker not yet implemented".into()))
    }

    async fn start_issue(&self, _id: &str) -> Result<()> {
        Err(HiveError::Tracker("Jira tracker not yet implemented".into()))
    }

    async fn finish_issue(&self, _id: &str) -> Result<()> {
        Err(HiveError::Tracker("Jira tracker not yet implemented".into()))
    }

    async fn create_followup(&self, _parent_id: &str, _content: FollowUpContent) -> Result<String> {
        Err(HiveError::Tracker("Jira tracker not yet implemented".into()))
    }

    async fn get_issue(&self, _id: &str) -> Result<IssueDetail> {
        Err(HiveError::Tracker("Jira tracker not yet implemented".into()))
    }

    fn name(&self) -> &str { "jira" }
}
```

- [ ] **Step 4: Create Slack notifier stub**

```rust
// src/notifiers/slack.rs
use async_trait::async_trait;

use super::Notifier;
use crate::domain::NotifyEvent;
use crate::error::{HiveError, Result};

pub struct SlackNotifier {
    webhook_url: String,
}

impl SlackNotifier {
    pub fn new(webhook_url: String) -> Self {
        Self { webhook_url }
    }
}

#[async_trait]
impl Notifier for SlackNotifier {
    async fn notify(&self, _event: NotifyEvent) -> Result<()> {
        Err(HiveError::Notification("Slack notifier not yet implemented".into()))
    }

    fn name(&self) -> &str { "slack" }
}
```

- [ ] **Step 5: Register all stubs in their module files**

Update `src/runners/mod.rs`: add `pub mod gemini;` and `pub mod codex;`
Update `src/trackers/mod.rs`: add `pub mod jira;`
Update `src/notifiers/mod.rs`: add `pub mod slack;`

- [ ] **Step 6: Verify everything compiles and commit**

Run: `cargo build`
Expected: Compiles with no errors.

```bash
git add src/runners/ src/trackers/ src/notifiers/
git commit -m "feat: add stub implementations for Gemini, Codex, Jira, and Slack backends"
```

---

## Summary

At the end of this plan, you have:

- A compilable, runnable Rust TUI application
- Config system with global + project TOML, env var resolution
- Full domain model (Phase state machine, StoryRun, events)
- State persistence with JSON files and crash recovery support
- AgentRunner trait with Claude Code ACP implementation
- IssueTracker trait with Linear GraphQL implementation
- Notifier trait with Discord webhook implementation
- Orchestrator with phase transitions, retry budgets, and command handling
- TUI with 4 tabs (Agents, Stories, Worktrees, Config), keyboard navigation, and status bar
- CLI with `hive`, `hive init`, `hive status` commands
- Stub backends (Gemini, Codex, Jira, Slack) ready for implementation

**What comes next (not in this plan):**
1. Full agent process lifecycle management (stdout streaming, session resume)
2. Stories tab wired to live tracker queries with filtering/sorting
3. Worktrees tab wired to `git worktree list`
4. Phase execution logic (the orchestrator driving agents through phases)
5. CI and bot review polling loops
6. Log viewer widget with scrolling
7. Phase progress bar widget
8. `hive configure` wizard
9. Full Gemini, Codex, Jira, Slack implementations
