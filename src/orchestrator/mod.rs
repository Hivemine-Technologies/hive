pub mod engine;
pub mod prompts;
pub mod retry;
pub mod transitions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::config::ProjectConfig;
use crate::domain::{
    NotifyEvent, OrchestratorEvent, Phase, PhaseOutcome, PhaseResult, RunStatus, StoryRun,
    TuiCommand,
};
use crate::error::Result;
use crate::git::github::GitHubClient;
use crate::notifiers::Notifier;
use crate::runners::AgentRunner;
use crate::state::persistence;
use crate::trackers::IssueTracker;

use self::engine::{max_attempts_for_phase, run_agent_phase, run_direct_phase, run_polling_phase};
use self::transitions::advance;

pub struct Orchestrator {
    config: Arc<ProjectConfig>,
    runs: HashMap<String, StoryRun>,
    runs_dir: PathBuf,
    runner: Arc<dyn AgentRunner>,
    tracker: Arc<dyn IssueTracker>,
    github: Option<Arc<GitHubClient>>,
    notifier: Option<Arc<dyn Notifier>>,
    event_tx: mpsc::Sender<OrchestratorEvent>,
    command_rx: mpsc::Receiver<TuiCommand>,
    cancel_tokens: HashMap<String, CancellationToken>,
}

impl Orchestrator {
    pub fn new(
        config: ProjectConfig,
        runs_dir: PathBuf,
        runner: Arc<dyn AgentRunner>,
        tracker: Arc<dyn IssueTracker>,
        github: Option<Arc<GitHubClient>>,
        notifier: Option<Arc<dyn Notifier>>,
        event_tx: mpsc::Sender<OrchestratorEvent>,
        command_rx: mpsc::Receiver<TuiCommand>,
    ) -> Result<Self> {
        let runs_vec = persistence::load_all_runs(&runs_dir)?;
        let runs: HashMap<String, StoryRun> = runs_vec
            .into_iter()
            .map(|r| (r.issue_id.clone(), r))
            .collect();
        Ok(Self {
            config: Arc::new(config),
            runs,
            runs_dir,
            runner,
            tracker,
            github,
            notifier,
            event_tx,
            command_rx,
            cancel_tokens: HashMap::new(),
        })
    }

    pub async fn run(&mut self) -> Result<()> {
        // Send initial state to TUI
        for run in self.runs.values() {
            let _ = self
                .event_tx
                .send(OrchestratorEvent::StoryUpdated(run.clone()))
                .await;
        }

        // Crash recovery: resume any runs that were in progress
        let to_resume: Vec<String> = self
            .runs
            .iter()
            .filter(|(_, run)| !matches!(run.status, RunStatus::Complete | RunStatus::Failed))
            .map(|(id, _)| id.clone())
            .collect();

        for issue_id in to_resume {
            if let Some(run) = self.runs.get(&issue_id).cloned() {
                tracing::info!(
                    "Resuming interrupted story: {issue_id} at phase {}",
                    run.phase
                );
                self.spawn_story_task(run);
            }
        }

        // Main command loop
        loop {
            tokio::select! {
                Some(cmd) = self.command_rx.recv() => {
                    match cmd {
                        TuiCommand::Quit => {
                            // Detach cleanly — leave agents running.
                            // On relaunch, crash recovery will reattach or resume.
                            break;
                        }
                        TuiCommand::StartStory { issue } => {
                            self.start_story(issue).await?;
                        }
                        TuiCommand::CancelStory { issue_id } => {
                            self.cancel_story(&issue_id).await?;
                        }
                        TuiCommand::RetryStory { issue_id } => {
                            self.retry_story(&issue_id).await?;
                        }
                        TuiCommand::RebaseStory { issue_id } => {
                            self.rebase_story(&issue_id).await?;
                        }
                        TuiCommand::RefreshStories => {}
                        TuiCommand::CopyWorktreePath { .. } => {}
                    }
                }
            }
        }
        Ok(())
    }

    async fn start_story(&mut self, issue: crate::domain::Issue) -> Result<()> {
        let repo_path = PathBuf::from(&self.config.repo_path);
        let worktree_dir = repo_path.join(&self.config.worktree_dir);

        // Create branch name from issue ID
        let branch = crate::git::worktree::branch_name(
            &issue.id,
            &slug_from_title(&issue.title),
        );

        // Create worktree
        let wt_path = crate::git::worktree::create_worktree(
            &repo_path,
            &worktree_dir,
            &issue.id,
            &branch,
        )?;

        let mut run = StoryRun::new(issue.id.clone(), issue.title.clone());
        run.worktree = Some(wt_path);
        run.branch = Some(branch);

        // Advance from Queued to first enabled phase
        let next = advance(Phase::Queued, &self.config.phases);
        run.phase = next;

        persistence::save_run(&self.runs_dir, &run)?;
        let _ = self
            .event_tx
            .send(OrchestratorEvent::StoryUpdated(run.clone()))
            .await;

        // Transition issue in tracker
        if let Err(e) = self.tracker.start_issue(&issue.id).await {
            tracing::warn!("failed to transition issue {}: {e}", issue.id);
        }

        self.runs.insert(issue.id.clone(), run.clone());
        self.spawn_story_task(run);

        Ok(())
    }

    fn spawn_story_task(&mut self, run: StoryRun) {
        let token = CancellationToken::new();
        self.cancel_tokens
            .insert(run.issue_id.clone(), token.clone());

        let config = self.config.clone();
        let runner = self.runner.clone();
        let tracker = self.tracker.clone();
        let github = self.github.clone();
        let notifier = self.notifier.clone();
        let event_tx = self.event_tx.clone();
        let runs_dir = self.runs_dir.clone();

        tokio::spawn(async move {
            let result = story_phase_loop(
                run, config, runner, tracker, github, notifier, event_tx, runs_dir, token,
            )
            .await;
            if let Err(e) = result {
                tracing::error!("Story task error: {e}");
            }
        });
    }

    async fn cancel_story(&mut self, issue_id: &str) -> Result<()> {
        // Signal the story task to stop
        if let Some(token) = self.cancel_tokens.get(issue_id) {
            token.cancel();
        }

        if let Some(run) = self.runs.get_mut(issue_id) {
            // Also cancel any running agent session
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

    async fn retry_story(&mut self, issue_id: &str) -> Result<()> {
        let run_to_spawn = {
            let Some(run) = self.runs.get_mut(issue_id) else {
                return Ok(());
            };

            // Only retry stories that are in NeedsAttention or Failed state
            if !matches!(run.status, RunStatus::NeedsAttention | RunStatus::Failed) {
                tracing::warn!("cannot retry {issue_id}: status is {:?}", run.status);
                return Ok(());
            }

            // Reset to the phase it failed on (strip NeedsAttention wrapper)
            let retry_phase = match &run.phase {
                Phase::NeedsAttention { .. } => {
                    run.phase_history
                        .last()
                        .map(|pr| pr.phase.clone())
                        .unwrap_or(Phase::Queued)
                }
                other => other.clone(),
            };

            run.phase = retry_phase;
            run.status = RunStatus::Running;
            run.updated_at = Utc::now();

            persistence::save_run(&self.runs_dir, run)?;
            let _ = self
                .event_tx
                .send(OrchestratorEvent::StoryUpdated(run.clone()))
                .await;

            run.clone()
        };

        self.spawn_story_task(run_to_spawn);
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

    #[allow(dead_code)]
    async fn send_notification(&self, event: NotifyEvent) {
        if let Some(ref notifier) = self.notifier {
            if let Err(e) = notifier.notify(event).await {
                tracing::warn!("notification failed: {e}");
            }
        }
    }
}

/// The main phase execution loop for a single story, running in its own tokio task.
async fn story_phase_loop(
    mut run: StoryRun,
    config: Arc<ProjectConfig>,
    runner: Arc<dyn AgentRunner>,
    tracker: Arc<dyn IssueTracker>,
    github: Option<Arc<GitHubClient>>,
    notifier: Option<Arc<dyn Notifier>>,
    event_tx: mpsc::Sender<OrchestratorEvent>,
    runs_dir: PathBuf,
    cancel_token: CancellationToken,
) -> Result<()> {
    let issue_id = run.issue_id.clone();

    // Fetch issue details for prompts
    let issue_detail = match tracker.get_issue(&issue_id).await {
        Ok(detail) => detail,
        Err(e) => {
            tracing::warn!("Failed to fetch issue details for {issue_id}: {e}");
            crate::domain::IssueDetail {
                id: issue_id.clone(),
                title: run.issue_title.clone(),
                description: String::new(),
                acceptance_criteria: None,
                priority: None,
                labels: vec![],
                url: String::new(),
            }
        }
    };

    loop {
        // Check cancellation between phases
        if cancel_token.is_cancelled() {
            run.status = RunStatus::Failed;
            run.updated_at = Utc::now();
            persistence::save_run(&runs_dir, &run)?;
            let _ = event_tx
                .send(OrchestratorEvent::StoryUpdated(run.clone()))
                .await;
            return Ok(());
        }

        // Terminal states
        if matches!(run.phase, Phase::Complete | Phase::NeedsAttention { .. }) {
            break;
        }

        let phase_start = std::time::Instant::now();
        let phase_config = config.phases.get(run.phase.config_key());

        let working_dir = run
            .worktree
            .as_deref()
            .unwrap_or_else(|| std::path::Path::new("."));

        // Execute phase based on type
        let phase_outcome = if run.phase.is_agent_phase() {
            let (_, model) = engine::resolve_phase_runner_config(&run.phase, phase_config);

            let result = run_agent_phase(
                runner.as_ref(),
                &run.phase,
                &issue_id,
                &issue_detail.title,
                &issue_detail.description,
                working_dir,
                model,
                None,
                &event_tx,
                None,
                0,
            )
            .await?;

            run.cost_usd += result.cost_usd;
            if let Some(sid) = result.session_id {
                run.session_id = Some(sid);
            }

            // Handle retries for agent phases
            let mut outcome = result.outcome;
            if matches!(outcome, PhaseOutcome::Failed { .. }) {
                let max = max_attempts_for_phase(&run.phase, phase_config);
                let mut attempt = 1u8;
                while attempt < max {
                    if cancel_token.is_cancelled() {
                        break;
                    }
                    let reason = match &outcome {
                        PhaseOutcome::Failed { reason } => reason.clone(),
                        _ => "unknown".to_string(),
                    };
                    attempt += 1;
                    let retry_result = run_agent_phase(
                        runner.as_ref(),
                        &run.phase,
                        &issue_id,
                        &issue_detail.title,
                        &issue_detail.description,
                        working_dir,
                        model,
                        None,
                        &event_tx,
                        Some(&reason),
                        attempt,
                    )
                    .await?;
                    run.cost_usd += retry_result.cost_usd;
                    if let Some(sid) = retry_result.session_id {
                        run.session_id = Some(sid);
                    }
                    outcome = retry_result.outcome;
                    if matches!(outcome, PhaseOutcome::Success) {
                        break;
                    }
                }

                // If still failed after retries, escalate
                if matches!(outcome, PhaseOutcome::Failed { .. }) {
                    let reason = match &outcome {
                        PhaseOutcome::Failed { reason } => reason.clone(),
                        _ => "unknown failure".to_string(),
                    };
                    outcome = PhaseOutcome::NeedsAttention {
                        reason: format!(
                            "Phase {} failed after {attempt} attempt(s): {reason}",
                            run.phase
                        ),
                    };
                }
            }

            outcome
        } else if run.phase.is_polling_phase() {
            if let Some(g) = &github {
                let pr_number = run.pr.as_ref().map(|p| p.number).unwrap_or(0);
                if pr_number == 0 {
                    PhaseOutcome::Failed {
                        reason: "No PR number available for polling phase".to_string(),
                    }
                } else {
                    let result = run_polling_phase(
                        g.as_ref(),
                        runner.as_ref(),
                        &run.phase,
                        pr_number,
                        &issue_id,
                        &issue_detail.title,
                        working_dir,
                        phase_config,
                        &event_tx,
                        &cancel_token,
                    )
                    .await?;
                    run.cost_usd += result.cost_usd;
                    result.outcome
                }
            } else {
                PhaseOutcome::Failed {
                    reason: "GitHubClient not configured".to_string(),
                }
            }
        } else if run.phase.is_direct_phase() {
            if let Some(g) = &github {
                let branch = run.branch.as_deref().unwrap_or("unknown-branch");
                let result = run_direct_phase(
                    &run.phase,
                    g.as_ref(),
                    tracker.as_ref(),
                    &issue_id,
                    &issue_detail.title,
                    &issue_detail.description,
                    working_dir,
                    branch,
                    run.pr.as_ref(),
                    run.cost_usd,
                    run.started_at,
                    &event_tx,
                )
                .await?;

                // Store PR handle from RaisePr
                if let Some(pr) = result.pr {
                    run.pr = Some(pr);
                }

                result.outcome
            } else {
                PhaseOutcome::Failed {
                    reason: "GitHubClient not configured".to_string(),
                }
            }
        } else {
            // Unknown phase type, skip
            PhaseOutcome::Skipped
        };

        let phase_duration = phase_start.elapsed().as_secs();

        // Record phase result in history
        let phase_cost = run.cost_usd;
        run.phase_history.push(PhaseResult {
            phase: run.phase.clone(),
            outcome: phase_outcome.clone(),
            duration_secs: phase_duration,
            cost_usd: phase_cost,
        });

        // Handle outcome
        match phase_outcome {
            PhaseOutcome::Success | PhaseOutcome::Skipped => {
                let old_phase = run.phase.clone();
                let next = advance(run.phase.clone(), &config.phases);
                run.phase = next.clone();
                run.updated_at = Utc::now();

                let _ = event_tx
                    .send(OrchestratorEvent::PhaseTransition {
                        issue_id: issue_id.clone(),
                        from: old_phase,
                        to: next.clone(),
                    })
                    .await;

                if matches!(next, Phase::Complete) {
                    run.status = RunStatus::Complete;
                    send_notification_if_configured(
                        &notifier,
                        NotifyEvent::StoryComplete {
                            issue_id: issue_id.clone(),
                            pr_url: run
                                .pr
                                .as_ref()
                                .map(|p| p.url.clone())
                                .unwrap_or_default(),
                            cost_usd: run.cost_usd,
                            duration_secs: Utc::now()
                                .signed_duration_since(run.started_at)
                                .num_seconds()
                                .max(0) as u64,
                        },
                    )
                    .await;
                }
            }
            PhaseOutcome::Failed { reason } => {
                run.phase = Phase::NeedsAttention {
                    reason: reason.clone(),
                };
                run.status = RunStatus::NeedsAttention;
                run.updated_at = Utc::now();
                send_notification_if_configured(
                    &notifier,
                    NotifyEvent::NeedsAttention {
                        issue_id: issue_id.clone(),
                        reason,
                    },
                )
                .await;
            }
            PhaseOutcome::NeedsAttention { reason } => {
                // Send a specific CiFailedMaxRetries notification for CI/bot-review phases
                if matches!(
                    run.phase,
                    Phase::CiWatch { .. } | Phase::BotReviews { .. }
                ) {
                    send_notification_if_configured(
                        &notifier,
                        NotifyEvent::CiFailedMaxRetries {
                            issue_id: issue_id.clone(),
                        },
                    )
                    .await;
                }

                run.phase = Phase::NeedsAttention {
                    reason: reason.clone(),
                };
                run.status = RunStatus::NeedsAttention;
                run.updated_at = Utc::now();
                send_notification_if_configured(
                    &notifier,
                    NotifyEvent::NeedsAttention {
                        issue_id: issue_id.clone(),
                        reason,
                    },
                )
                .await;
            }
        }

        // Persist after every phase transition
        persistence::save_run(&runs_dir, &run)?;
        let _ = event_tx
            .send(OrchestratorEvent::StoryUpdated(run.clone()))
            .await;
    }

    Ok(())
}

async fn send_notification_if_configured(notifier: &Option<Arc<dyn Notifier>>, event: NotifyEvent) {
    if let Some(notifier) = notifier {
        if let Err(e) = notifier.notify(event).await {
            tracing::warn!("notification failed: {e}");
        }
    }
}

/// Create a URL-safe slug from a title.
fn slug_from_title(title: &str) -> String {
    title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("-")
        .chars()
        .take(40)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_slug_from_title() {
        assert_eq!(
            slug_from_title("Add NumberSequenceService"),
            "add-numbersequenceservice"
        );
    }

    #[test]
    fn test_slug_from_title_special_chars() {
        assert_eq!(
            slug_from_title("Fix bug: handle edge-case #42"),
            "fix-bug-handle-edge-case-42"
        );
    }

    #[test]
    fn test_slug_from_title_truncation() {
        let long = "a".repeat(100);
        let slug = slug_from_title(&long);
        assert!(slug.len() <= 40);
    }

    #[test]
    fn test_slug_from_title_empty() {
        assert_eq!(slug_from_title(""), "");
    }
}
