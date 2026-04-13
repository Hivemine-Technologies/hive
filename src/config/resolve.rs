use std::path::Path;

use crate::config::global::GlobalConfig;
use crate::config::project::ProjectConfig;
use crate::error::{HiveError, Result};

pub fn resolve_env(value: &str) -> Result<String> {
    if let Some(var_name) = value.strip_prefix("env:") {
        std::env::var(var_name).map_err(|_| {
            HiveError::Config(format!("environment variable {var_name} is not set"))
        })
    } else {
        Ok(value.to_string())
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_var() {
        // SAFETY: This test runs in isolation; no other threads depend on this env var.
        unsafe {
            std::env::set_var("HIVE_TEST_KEY", "secret123");
        }
        assert_eq!(resolve_env("env:HIVE_TEST_KEY").unwrap(), "secret123");
        assert_eq!(resolve_env("literal_value").unwrap(), "literal_value");
        unsafe {
            std::env::remove_var("HIVE_TEST_KEY");
        }
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
