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
use crate::trackers::jira::JiraTracker;
use crate::trackers::linear::LinearTracker;
use crate::trackers::IssueTracker;
use crate::tui::Tui;

pub async fn run(repo_path: &str) -> Result<()> {
    let config_dir = dirs_config_dir()?;
    let global = load_global_config(&config_dir)?;
    let project = load_project_config(&config_dir, repo_path)?;

    // Build all configured runners
    let mut runners: std::collections::HashMap<String, Arc<dyn AgentRunner>> =
        std::collections::HashMap::new();
    for (name, runner_config) in &global.runners {
        let runner: Arc<dyn AgentRunner> = Arc::new(ClaudeRunner::new(
            runner_config.command.clone(),
            runner_config.default_model.clone(),
            runner_config.permission_mode.clone(),
        ));
        runners.insert(name.clone(), runner);
    }

    // Determine the default runner (first enabled phase's runner, or "claude")
    let default_runner = project
        .phases
        .values()
        .find(|p| p.enabled && p.runner.is_some())
        .and_then(|p| p.runner.clone())
        .unwrap_or_else(|| "claude".to_string());

    if !runners.contains_key(&default_runner) {
        return Err(HiveError::Config(format!(
            "default runner '{default_runner}' not configured in global config"
        )));
    }

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
                .ok_or_else(|| HiveError::Config("linear api_key not set".to_string()))?;
            let api_key = resolve_env(api_key)?;
            Arc::new(LinearTracker::new(
                api_key,
                project.tracker_config.clone(),
            ))
        }
        "jira" => {
            let base_url = tracker_conn
                .base_url
                .as_ref()
                .ok_or_else(|| HiveError::Config("jira base_url not set".to_string()))?;
            let api_token = tracker_conn
                .api_token
                .as_ref()
                .ok_or_else(|| HiveError::Config("jira api_token not set".to_string()))?;
            let email = tracker_conn
                .email
                .as_ref()
                .ok_or_else(|| HiveError::Config("jira email not set".to_string()))?;
            let base_url = resolve_env(base_url)?;
            let api_token = resolve_env(api_token)?;
            let email = resolve_env(email)?;
            Arc::new(JiraTracker::new(
                base_url,
                email,
                api_token,
                project.tracker_config.clone(),
            ))
        }
        other => return Err(HiveError::Config(format!("unsupported tracker: {other}"))),
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

    // Channels
    let (event_tx, event_rx) = mpsc::channel::<OrchestratorEvent>(256);
    let (command_tx, command_rx) = mpsc::channel::<TuiCommand>(64);

    let runs_dir = config_dir
        .join("projects")
        .join(&project.name)
        .join("runs");

    // Extract values for TUI before project is consumed by orchestrator
    let repo_path_buf = PathBuf::from(repo_path);
    let tui_tracker = tracker.clone();
    let tui_tracker_config = project.tracker_config.clone();
    let tui_project_name = project.name.clone();

    // Resolve GitHub client
    let github_client: Option<Arc<dyn crate::git::github::GitHub>> = {
        let token = std::env::var("GITHUB_TOKEN")
            .or_else(|_| std::env::var("GH_TOKEN"))
            .ok();
        match token {
            Some(t) => Some(Arc::new(crate::git::github::GitHubClient::new(
                project.github.owner.clone(),
                project.github.repo.clone(),
                t,
            )?)),
            None => {
                eprintln!("⚠ GITHUB_TOKEN (or GH_TOKEN) is not set.");
                eprintln!("  PR creation, CI polling, and bot review phases will fail.");
                eprintln!("  Set it with: export GITHUB_TOKEN=ghp_...\n");
                None
            }
        }
    };

    // Validate tracker credentials are resolvable
    if project.tracker == "linear" {
        if let Some(ref key) = tracker_conn.api_key {
            if key.starts_with("env:") {
                let var_name = &key[4..];
                if std::env::var(var_name).is_err() {
                    eprintln!("⚠ {var_name} is not set. Issue tracker queries will fail.");
                    eprintln!("  Set it with: export {var_name}=lin_api_...\n");
                }
            }
        }
    } else if project.tracker == "jira" {
        for (label, value) in [
            ("api_token", tracker_conn.api_token.as_ref()),
            ("email", tracker_conn.email.as_ref()),
            ("base_url", tracker_conn.base_url.as_ref()),
        ] {
            if let Some(v) = value {
                if let Some(var_name) = v.strip_prefix("env:") {
                    if std::env::var(var_name).is_err() {
                        eprintln!(
                            "⚠ {var_name} (jira {label}) is not set. Jira tracker calls will fail."
                        );
                    }
                }
            }
        }
    }

    // Start orchestrator in background
    let mut orchestrator = Orchestrator::new(
        project,
        runs_dir,
        runners,
        default_runner,
        tracker,
        github_client,
        notifier,
        event_tx,
        command_rx,
    )?;
    tokio::spawn(async move {
        if let Err(e) = orchestrator.run().await {
            tracing::error!("orchestrator error: {e}");
        }
    });

    // Run TUI
    let mut tui = Tui::new(
        event_rx,
        command_tx,
        tui_tracker,
        tui_tracker_config,
        repo_path_buf,
        config_dir.clone(),
        tui_project_name,
    );
    let mut terminal = ratatui::init();
    let result = tui.run(&mut terminal).await;
    ratatui::restore();
    result.map_err(HiveError::Io)
}

pub fn dirs_config_dir() -> Result<PathBuf> {
    let home =
        std::env::var("HOME").map_err(|_| HiveError::Config("HOME not set".to_string()))?;
    Ok(PathBuf::from(home).join(".config").join("hive"))
}
