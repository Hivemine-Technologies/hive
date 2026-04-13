use std::io::{self, Write};
use std::path::PathBuf;

use crate::config::project::{
    GitHubConfig, NotificationConfig, PhaseConfig, ProjectConfig, StatusMappings, TrackerConfig,
};
use crate::error::{HiveError, Result};

pub fn run_init() -> Result<()> {
    println!("🐝 Hive — Project Setup\n");
    let config_dir = crate::app::dirs_config_dir()?;

    // Create global config if missing
    let global_config_path = config_dir.join("config.toml");
    if !global_config_path.exists() {
        println!("No global config found. Let's set that up first.\n");
        create_global_config(&config_dir)?;
        println!();
    }

    // Project setup prompts
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

    // Phase toggles
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

    // Write project config
    let project_dir = config_dir.join("projects").join(&name);
    std::fs::create_dir_all(&project_dir)?;
    let toml_str = toml::to_string_pretty(&project_config)
        .map_err(|e| HiveError::Config(format!("failed to serialize config: {e}")))?;
    std::fs::write(project_dir.join("project.toml"), toml_str)?;

    println!("\n✅ Project '{name}' configured!");
    println!(
        "   Config: {}",
        project_dir.join("project.toml").display()
    );
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
