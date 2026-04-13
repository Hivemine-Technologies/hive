use super::issue::Issue;
use super::phase::Phase;
use super::story_run::StoryRun;

#[derive(Debug, Clone)]
pub enum OrchestratorEvent {
    StoryUpdated(StoryRun),
    AgentOutput {
        issue_id: String,
        event: AgentEvent,
    },
    PhaseTransition {
        issue_id: String,
        from: Phase,
        to: Phase,
    },
    Error {
        issue_id: Option<String>,
        message: String,
    },
}

#[derive(Debug, Clone)]
pub enum TuiCommand {
    StartStory { issue: Issue },
    CancelStory { issue_id: String },
    RebaseStory { issue_id: String },
    CopyWorktreePath { issue_id: String },
    RefreshStories,
    Quit,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    ToolUse {
        tool: String,
        input_preview: String,
    },
    ToolResult {
        tool: String,
        success: bool,
    },
    Error(String),
    Complete {
        cost_usd: f64,
    },
    CostUpdate(f64),
}

#[derive(Debug, Clone)]
pub enum NotifyEvent {
    StoryComplete {
        issue_id: String,
        pr_url: String,
        cost_usd: f64,
        duration_secs: u64,
    },
    NeedsAttention {
        issue_id: String,
        reason: String,
    },
    AllIdle,
    CiFailedMaxRetries {
        issue_id: String,
    },
}
