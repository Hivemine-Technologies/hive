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
        assert_eq!(
            config.phases["ci-watch"].poll_interval,
            Some("30s".to_string())
        );
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
