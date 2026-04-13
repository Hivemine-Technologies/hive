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
        assert_eq!(
            config.trackers["linear"].api_key,
            Some("env:LINEAR_API_KEY".to_string())
        );
    }
}
