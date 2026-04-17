use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::phase::Phase;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryRun {
    pub issue_id: String,
    pub issue_title: String,
    pub phase: Phase,
    pub status: RunStatus,
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    pub pr: Option<PrHandle>,
    pub session_id: Option<String>,
    pub phase_history: Vec<PhaseResult>,
    pub cost_usd: f64,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    /// When set, the story is in a regression cycle (e.g., PrWatch kicked back
    /// to BotReviews). After the regressed phase completes, jump directly to
    /// this phase instead of calling advance().
    #[serde(default)]
    pub regression_return: Option<Phase>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Paused,
    NeedsAttention,
    Complete,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PrHandle {
    pub number: u64,
    pub url: String,
    pub head_sha: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PhaseResult {
    pub phase: Phase,
    pub outcome: PhaseOutcome,
    pub duration_secs: u64,
    pub cost_usd: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PhaseOutcome {
    Success,
    Skipped,
    Failed { reason: String },
    NeedsAttention { reason: String },
    /// Regress to an earlier phase (e.g., PrWatch → BotReviews).
    /// After the regressed phase completes, the loop returns to the
    /// phase that triggered the regression.
    Regress { phase: Phase },
}

impl StoryRun {
    pub fn new(issue_id: String, issue_title: String) -> Self {
        let now = Utc::now();
        Self {
            issue_id,
            issue_title,
            phase: Phase::Queued,
            status: RunStatus::Running,
            worktree: None,
            branch: None,
            pr: None,
            session_id: None,
            phase_history: Vec::new(),
            cost_usd: 0.0,
            started_at: now,
            updated_at: now,
            regression_return: None,
        }
    }
}
