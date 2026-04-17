use super::issue::Issue;
use super::phase::Phase;
use super::story_run::StoryRun;

#[derive(Debug, Clone)]
#[allow(clippy::large_enum_variant)]
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
}

#[derive(Debug, Clone)]
pub enum TuiCommand {
    StartStory { issue: Issue },
    CancelStory { issue_id: String },
    RetryStory { issue_id: String },
    CopyWorktreePath,
    Quit,
}

#[derive(Debug, Clone)]
pub enum AgentEvent {
    TextDelta(String),
    ToolUse {
        #[allow(dead_code)]
        tool_use_id: String,
        tool: String,
        /// Full input JSON. Renderer decides how/whether to truncate.
        input: String,
    },
    ToolResult {
        tool_use_id: String,
        output: String,
        is_error: bool,
    },
    Error(String),
    Complete {
        cost_usd: f64,
    },
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
    CiFailedMaxRetries {
        issue_id: String,
    },
}
