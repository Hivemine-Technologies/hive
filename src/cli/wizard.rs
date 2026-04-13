use std::io::{self, Write};
use std::path::PathBuf;

use crate::config::project::{
    GitHubConfig, NotificationConfig, PhaseConfig, ProjectConfig, StatusMappings, TrackerConfig,
};
use crate::error::{HiveError, Result};

/// Run the interactive project wizard.
///
/// When `existing` is `Some`, each prompt pre-fills with the current config value,
/// allowing the user to press Enter to keep it unchanged. When `None`, the wizard
/// uses auto-detected or hardcoded defaults (same flow as the original `hive init`).
pub fn run_wizard(existing: Option<ProjectConfig>) -> Result<()> {
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
    let default_name = existing
        .as_ref()
        .map(|c| c.name.clone())
        .unwrap_or_else(|| {
            cwd.file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("project")
                .to_string()
        });

    let name = prompt_with_default("Project name", &default_name)?;

    let default_repo = existing
        .as_ref()
        .map(|c| c.repo_path.clone())
        .unwrap_or_else(|| cwd.to_string_lossy().to_string());
    let repo_path = prompt_with_default("Repository path", &default_repo)?;

    let default_worktree = existing
        .as_ref()
        .map(|c| c.worktree_dir.clone())
        .unwrap_or_else(|| ".worktrees".to_string());
    let worktree_dir = prompt_with_default("Worktree directory", &default_worktree)?;

    let default_tracker = existing
        .as_ref()
        .map(|c| c.tracker.clone())
        .unwrap_or_else(|| "linear".to_string());
    let tracker = prompt_with_default("Issue tracker (linear/jira)", &default_tracker)?;

    let default_team = existing
        .as_ref()
        .map(|c| c.tracker_config.team.clone())
        .unwrap_or_default();
    let team = prompt_with_default("Tracker team/project", &default_team)?;

    let default_ready = existing
        .as_ref()
        .map(|c| c.tracker_config.ready_filter.join(", "))
        .unwrap_or_else(|| "Todo".to_string());
    let ready_input = prompt_with_default("Ready status name(s), comma-separated", &default_ready)?;
    let ready_filter: Vec<String> = ready_input
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let default_start = existing
        .as_ref()
        .map(|c| c.tracker_config.statuses.start.clone())
        .unwrap_or_else(|| "In Progress".to_string());
    let start_status = prompt_with_default("In-progress status name", &default_start)?;

    let default_review = existing
        .as_ref()
        .map(|c| c.tracker_config.statuses.review.clone())
        .unwrap_or_else(|| "In Review".to_string());
    let review_status = prompt_with_default("In-review status name", &default_review)?;

    let default_done = existing
        .as_ref()
        .map(|c| c.tracker_config.statuses.done.clone())
        .unwrap_or_else(|| "Done".to_string());
    let done_status = prompt_with_default("Done status name", &default_done)?;

    // Auto-detect GitHub remote (or use existing values)
    let (detected_owner, detected_repo) = detect_github_remote(&PathBuf::from(&repo_path));
    let default_gh_owner = existing
        .as_ref()
        .map(|c| c.github.owner.clone())
        .or(detected_owner)
        .unwrap_or_default();
    let default_gh_repo = existing
        .as_ref()
        .map(|c| c.github.repo.clone())
        .or(detected_repo)
        .unwrap_or_default();

    let gh_owner = prompt_with_default("GitHub owner", &default_gh_owner)?;
    let gh_repo = prompt_with_default("GitHub repo", &default_gh_repo)?;

    let default_notifier = existing
        .as_ref()
        .and_then(|c| c.notifier.clone())
        .unwrap_or_else(|| "none".to_string());
    let notifier_choice =
        prompt_with_default("Notifications (discord/slack/none)", &default_notifier)?;
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
        let existing_enabled = existing
            .as_ref()
            .and_then(|c| c.phases.get(*phase_name))
            .map(|p| p.enabled);

        let default = match existing_enabled {
            Some(true) => "y",
            Some(false) => "n",
            None => {
                if *phase_name == "cross-review" {
                    "n"
                } else {
                    "y"
                }
            }
        };
        let answer = prompt_with_default(&format!("Enable {phase_name}? (y/n)"), default)?;
        let enabled = answer.starts_with('y');

        // Preserve existing phase config fields when reconfiguring
        let existing_phase = existing
            .as_ref()
            .and_then(|c| c.phases.get(*phase_name));
        phases.insert(
            phase_name.to_string(),
            PhaseConfig {
                enabled,
                runner: existing_phase.and_then(|p| p.runner.clone()),
                model: existing_phase.and_then(|p| p.model.clone()),
                max_attempts: existing_phase.and_then(|p| p.max_attempts),
                poll_interval: existing_phase.and_then(|p| p.poll_interval.clone()),
                max_fix_attempts: existing_phase.and_then(|p| p.max_fix_attempts),
                max_fix_cycles: existing_phase.and_then(|p| p.max_fix_cycles),
                fix_runner: existing_phase.and_then(|p| p.fix_runner.clone()),
                fix_model: existing_phase.and_then(|p| p.fix_model.clone()),
                wait_for: existing_phase.and_then(|p| p.wait_for.clone()),
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
            fields: existing
                .as_ref()
                .map(|c| c.tracker_config.fields.clone())
                .unwrap_or_default(),
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

    // Check and remind about required env vars
    println!("\n📋 Before running `hive`, make sure these environment variables are set:\n");

    let mut all_set = true;

    // GitHub token
    let has_gh_token = std::env::var("GITHUB_TOKEN").is_ok() || std::env::var("GH_TOKEN").is_ok();
    if has_gh_token {
        println!("   ✓ GITHUB_TOKEN is set");
    } else {
        println!("   ✗ GITHUB_TOKEN — required for PR creation and CI polling");
        println!("     export GITHUB_TOKEN=ghp_...");
        all_set = false;
    }

    // Tracker API key
    if project_config.tracker == "linear" {
        if std::env::var("LINEAR_API_KEY").is_ok() {
            println!("   ✓ LINEAR_API_KEY is set");
        } else {
            println!("   ✗ LINEAR_API_KEY — required for issue tracker queries");
            println!("     export LINEAR_API_KEY=lin_api_...");
            all_set = false;
        }
    } else if project_config.tracker == "jira" {
        if std::env::var("JIRA_API_TOKEN").is_ok() {
            println!("   ✓ JIRA_API_TOKEN is set");
        } else {
            println!("   ✗ JIRA_API_TOKEN — required for Jira integration");
            all_set = false;
        }
    }

    // Notifier webhook
    if let Some(ref n) = project_config.notifier {
        let var_name = match n.as_str() {
            "discord" => Some("HIVE_DISCORD_WEBHOOK"),
            "slack" => Some("HIVE_SLACK_WEBHOOK"),
            _ => None,
        };
        if let Some(var) = var_name {
            if std::env::var(var).is_ok() {
                println!("   ✓ {var} is set");
            } else {
                println!("   ✗ {var} — required for {n} notifications (optional)");
                all_set = false;
            }
        }
    }

    if all_set {
        println!("\n   All set! Run `hive` in your repo to launch the dashboard.");
    } else {
        println!("\n   Set the missing variables above, then run `hive` to launch.");
    }

    Ok(())
}

/// Prompt the user with a message and a default value shown in brackets.
/// Returns the default if the user presses Enter without typing anything.
fn prompt_with_default(message: &str, default: &str) -> Result<String> {
    print!("{message} [{default}]: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        Ok(default.to_string())
    } else {
        Ok(trimmed.to_string())
    }
}

fn create_global_config(config_dir: &PathBuf) -> Result<()> {
    std::fs::create_dir_all(config_dir)?;
    let config = r#"[runners.claude]
command = "claude"
protocol = "acp"
default_model = "opus"
permission_mode = "bypassPermissions"

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
