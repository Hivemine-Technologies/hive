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
        HiveError::Config(format!(
            "runner '{runner_name}' not configured in global config"
        ))
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
                .ok_or_else(|| HiveError::Config("linear api_key not set".to_string()))?;
            let api_key = resolve_env(api_key)?;
            Arc::new(LinearTracker::new(
                api_key,
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

    // Start orchestrator in background
    let mut orchestrator =
        Orchestrator::new(project, runs_dir, runner, tracker, notifier, event_tx, command_rx)?;
    tokio::spawn(async move {
        if let Err(e) = orchestrator.run().await {
            tracing::error!("orchestrator error: {e}");
        }
    });

    // Run TUI
    let mut tui = Tui::new(event_rx, command_tx);
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
