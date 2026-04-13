pub mod retry;
pub mod transitions;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::mpsc;

use crate::config::ProjectConfig;
use crate::domain::{NotifyEvent, OrchestratorEvent, Phase, RunStatus, StoryRun, TuiCommand};
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
    runner: Arc<dyn AgentRunner>,
    tracker: Arc<dyn IssueTracker>,
    notifier: Option<Arc<dyn Notifier>>,
    event_tx: mpsc::Sender<OrchestratorEvent>,
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

    pub async fn run(&mut self) -> Result<()> {
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
                        TuiCommand::RefreshStories => {}
                        TuiCommand::CopyWorktreePath { .. } => {}
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
        persistence::save_run(&self.runs_dir, &run)?;
        let _ = self
            .event_tx
            .send(OrchestratorEvent::StoryUpdated(run.clone()))
            .await;
        if let Err(e) = self.tracker.start_issue(&issue.id).await {
            tracing::warn!("failed to transition issue {}: {e}", issue.id);
        }
        self.runs.insert(issue.id, run);
        Ok(())
    }

    async fn cancel_story(&mut self, issue_id: &str) -> Result<()> {
        if let Some(run) = self.runs.get_mut(issue_id) {
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

    #[allow(dead_code)]
    async fn send_notification(&self, event: NotifyEvent) {
        if let Some(ref notifier) = self.notifier {
            if let Err(e) = notifier.notify(event).await {
                tracing::warn!("notification failed: {e}");
            }
        }
    }
}
