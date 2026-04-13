use std::collections::HashMap;

use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
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

#[derive(Debug, Serialize, Deserialize)]
pub struct NotificationConfig {
    #[serde(default)]
    pub events: Vec<String>,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct GitHubConfig {
    pub owner: String,
    pub repo: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrackerConfig {
    pub team: String,
    /// One or more statuses that mean "ready for work".
    /// Accepts a single string ("Todo") or a list (["Todo", "Ready"]).
    #[serde(deserialize_with = "deserialize_string_or_vec", default)]
    pub ready_filter: Vec<String>,
    pub statuses: StatusMappings,
    #[serde(default)]
    pub fields: HashMap<String, String>,
}

/// Deserialize either a single string or a vec of strings.
fn deserialize_string_or_vec<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de;

    struct StringOrVec;

    impl<'de> de::Visitor<'de> for StringOrVec {
        type Value = Vec<String>;

        fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
            f.write_str("a string or list of strings")
        }

        fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Vec<String>, E> {
            Ok(vec![v.to_string()])
        }

        fn visit_seq<A: de::SeqAccess<'de>>(self, mut seq: A) -> std::result::Result<Vec<String>, A::Error> {
            let mut vec = Vec::new();
            while let Some(s) = seq.next_element()? {
                vec.push(s);
            }
            Ok(vec)
        }
    }

    deserializer.deserialize_any(StringOrVec)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusMappings {
    pub start: String,
    pub review: String,
    pub done: String,
}

#[derive(Debug, Serialize, Deserialize)]
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
