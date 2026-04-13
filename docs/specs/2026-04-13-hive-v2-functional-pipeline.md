# Hive v2 — Full Pipeline Execution

**Date:** 2026-04-13
**Status:** Draft
**Author:** Robbie Diaz + Claude
**Depends on:** `docs/specs/2026-04-13-hive-tui-design.md` (v1 spec)

## Overview

Hive v1 established the architecture: config system, domain types, backend traits with initial implementations, an orchestrator skeleton, a TUI shell, and CLI commands. All layers compile and the type contracts are in place, but the orchestrator doesn't execute phases, the runner doesn't manage processes, and the TUI tabs are placeholders.

This spec covers the work to make Hive functional end-to-end: a user selects a story, Hive creates a worktree, drives an agent through each pipeline phase, streams output to the dashboard, polls CI and bot reviews, retries failures within budget, creates a PR, and notifies on completion.

## 1. Agent Process Lifecycle

### Problem

`ClaudeRunner::start_session()` spawns a subprocess but drops the `Child` handle. `output_stream()` returns a dead channel. The runner has no way to track, stream from, or kill its sessions.

### Design

`ClaudeRunner` gains internal session tracking:

```rust
struct RunningSession {
    child: Child,
    event_tx: mpsc::Sender<AgentEvent>,
    session_id: String,
}

pub struct ClaudeRunner {
    command: String,
    default_model: String,
    permission_mode: Option<String>,
    sessions: Arc<Mutex<HashMap<String, RunningSession>>>,
}
```

**`start_session()`:**
1. Spawn the `claude` process with `--bare -p <prompt> --output-format stream-json --verbose --model <model>`
2. Take `stdout` from the child
3. Create an `mpsc::channel(256)` for parsed events
4. Spawn a background tokio task that reads stdout line-by-line, calls `parse_claude_event()` on each line, and sends parsed `AgentEvent`s into the channel
5. Extract the session ID from the first `system/init` NDJSON event (fall back to PID if not received within 5 seconds)
6. Store `RunningSession { child, event_tx, session_id }` in the sessions map
7. Return `SessionHandle` with the session ID, runner name, and PID

**`output_stream()`:** Look up the session by ID, clone the receiver end of the channel, return it wrapped as a `Pin<Box<dyn Stream>>`. The receiver is created at session start and the caller gets a `ReceiverStream`.

Implementation detail: since `mpsc::Receiver` isn't `Clone`, the runner stores the *sender* and creates a broadcast-style relay. Simplest approach: use `tokio::sync::broadcast` instead of `mpsc` for the event channel, so multiple consumers can subscribe. Or, since there's only one consumer (the orchestrator), store the receiver in an `Option` inside `RunningSession` and `take()` it on the first `output_stream()` call — subsequent calls return a dead stream (one consumer is all we need).

**`cancel()`:** Look up the session, call `child.kill()` (async), remove from map.

**`is_alive()`:** Look up the session, call `child.try_wait()`. If it returns `Ok(Some(_))`, the process exited — return false. If `Ok(None)`, still running — return true.

**`resume()`:** Spawn a new process with `--resume <session_id>`. Replace the session entry in the map. Same stdout-reading task setup as `start_session()`.

### Scope

Only `ClaudeRunner` gets the full implementation. `GeminiRunner` and `CodexRunner` remain stubs — they already return `Err("not yet implemented")`.

## 2. Orchestrator Phase Execution Engine

### Problem

The orchestrator's `run()` loop only handles TUI commands. It never spawns agents, polls CI, or advances stories through phases.

### Design

#### Story Task Model

Each story runs as its own `tokio::spawn` task. When `start_story()` is called:

1. Create a worktree via `git::worktree::create_worktree()`
2. Set the story's branch and worktree path
3. Advance from `Queued` to the first enabled phase
4. Spawn a tokio task that drives the phase loop for this story

The story task owns the phase execution loop:

```
loop {
    match current_phase {
        agent phase → run_agent_phase()
        polling phase → run_polling_phase()
        direct phase → run_direct_phase()
        Complete → break
        NeedsAttention → break
    }
    advance to next phase
    persist state
    emit PhaseTransition event
}
```

The story task communicates back to the TUI via `event_tx` (already an `mpsc::Sender<OrchestratorEvent>` cloneable across tasks).

The orchestrator's main `run()` loop continues handling commands (cancel, rebase, quit). Cancel sends a signal to the story task (via a `tokio::sync::watch` or `CancellationToken`).

#### Agent Phase Execution

```rust
async fn run_agent_phase(
    runner: &dyn AgentRunner,
    config: &SessionConfig,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    issue_id: &str,
) -> Result<PhaseOutcome>
```

1. Build a `SessionConfig` with the worktree as working directory and a phase-appropriate system prompt
2. Call `runner.start_session(config)`
3. Get `runner.output_stream(session)`
4. Consume the stream in a loop:
   - `AgentEvent::TextDelta` / `ToolUse` / `ToolResult` → forward to TUI as `OrchestratorEvent::AgentOutput`
   - `AgentEvent::CostUpdate` → update story's `cost_usd`
   - `AgentEvent::Complete { cost_usd }` → return `PhaseOutcome::Success`
   - `AgentEvent::Error` → return `PhaseOutcome::Failed`
   - Stream ends without Complete → return `PhaseOutcome::Failed`

#### Phase-Specific System Prompts

Each agent phase gets a tailored prompt:

- **Understand:** "You are analyzing story {id}: {title}. Read the issue description and acceptance criteria. Explore the codebase to understand what needs to change. Write a brief plan as a markdown file in the worktree root (PLAN.md). Do not implement yet."
- **Implement:** "You are implementing story {id}: {title}. Follow the plan in PLAN.md. Write code, tests, and commit your work. Use conventional commit messages prefixed with the issue ID."
- **SelfReview:** "You are reviewing your own implementation of story {id}. Read the diff of all changes. Check for bugs, missing edge cases, test coverage gaps, and code quality issues. Fix anything you find and commit the fixes."
- **CrossReview:** "You are cross-reviewing the implementation of story {id} by another agent. Read all changes critically. Report issues but do not fix them — create a REVIEW.md with findings."
- **FollowUps:** "Story {id} is complete. Review the implementation and identify any follow-up work needed (tech debt, documentation, related changes). Create follow-up issues via the provided tool."

The prompts reference the issue description fetched via `tracker.get_issue()` at the start of the story.

#### Polling Phase Execution

```rust
async fn run_polling_phase(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    phase: &Phase,
    story: &StoryRun,
    config: &PhaseConfig,
) -> Result<PhaseOutcome>
```

**CiWatch:**
1. Start a `tokio::time::interval` at `config.poll_interval` (default 30s)
2. Each tick: call `github.poll_ci(pr_number)`
3. `CiStatus::Passed` → return Success
4. `CiStatus::Pending` → continue polling
5. `CiStatus::Failed { failures }` → check retry budget
   - Budget remaining: spawn fix agent with prompt "CI failed: {failures}. Fix the issues and commit.", increment attempt counter, wait for agent completion, resume polling
   - Budget exhausted: return `PhaseOutcome::NeedsAttention`

**BotReviews:**
1. Poll `github.poll_reviews(pr_number)` on interval
2. Filter for bot comments (Coderabbit, etc.) based on `config.wait_for` list
3. If new bot comments found: spawn fix agent with prompt "Address these review comments: {comments}", wait for completion, increment cycle counter
4. If no new comments after a quiet period (2 consecutive polls with no new comments): return Success
5. Max cycles exhausted: return `PhaseOutcome::NeedsAttention`

#### Direct Phase Execution

**RaisePr:**
1. `github.push_branch(worktree_path, branch)`
2. Fetch issue details for PR body: `tracker.get_issue(issue_id)`
3. `github.create_pr(branch, title, body)`
4. Store `PrHandle` on the `StoryRun`
5. Transition issue to "In Review" status: `tracker.finish_issue(issue_id)`

**Handoff:**
1. Transition issue to "Done" status (or configured done status)
2. Post a summary comment on the issue with cost, duration, PR link
3. Emit `NotifyEvent::StoryComplete`

#### Error Handling and Retries

Phase-type-dependent retry strategy:

- **Agent phases** (Understand, Implement): Use `max_attempts` from config (default 1 — no retry for the core work phases, since retrying "implement" blindly is wasteful). On failure, escalate to `NeedsAttention`.
- **SelfReview:** Use `max_attempts` (default 3). Each retry re-runs the review with a "look harder" nudge.
- **Fix agents** (CI fix, bot review fix): Use `max_fix_attempts` / `max_fix_cycles` from the parent polling phase config.
- **Direct phases:** No retry budget. Failures escalate immediately (if a PR can't be created, human intervention is needed).

On retry: the orchestrator re-invokes the same phase with an incremented attempt counter and a recovery prompt ("Previous attempt failed: {reason}. Review the current state and try again.").

On exhaustion: transition to `NeedsAttention { reason }`, persist, emit notification.

#### Cancellation

Each story task holds a `CancellationToken` (from `tokio_util`). The orchestrator stores the token when spawning the task. `cancel_story()` calls `token.cancel()`, which the story task checks between phases and during long-running operations (polling loops). On cancellation:
1. Cancel any running agent session via `runner.cancel()`
2. Set status to `Failed`
3. Persist state
4. Emit update to TUI

## 3. Functional TUI Tabs

### Stories Tab

**Data source:** `Arc<dyn IssueTracker>` held directly by the TUI struct.

**State:**
```rust
struct StoriesState {
    issues: Vec<Issue>,
    selected: usize,
    filter_text: String,
    sort_column: SortColumn,
    sort_ascending: bool,
    loading: bool,
    filter_active: bool,
}
```

**Behavior:**
- On first tab focus or `r` press: set `loading = true`, spawn async task calling `tracker.list_ready()`, results sent back via a channel, update `issues` and set `loading = false`
- Table columns: ID, Title, Priority, Project, Labels
- `j/k` navigate rows, `Enter` starts the selected story (sends `TuiCommand::StartStory`), `Space` for batch select (future)
- `/` activates filter input (type to filter by title/ID), `Esc` clears filter
- `s` cycles sort column, `S` (shift-s) toggles sort direction
- Show loading spinner while fetching

**Rendering:** Use ratatui `Table` widget with `Row`s and styled `Cell`s. Highlight selected row. Show filter bar at bottom when active.

### Worktrees Tab

**Data source:** Calls `git::worktree::list_worktrees()` directly (synchronous, sub-millisecond).

**State:**
```rust
struct WorktreesState {
    worktrees: Vec<WorktreeInfo>,
    selected: usize,
}
```

**Behavior:**
- Refreshes on tab focus
- Table columns: Branch, Path, Status (running/idle based on story run state)
- `r` rebase selected worktree (sends `TuiCommand::RebaseStory`)
- `d` delete selected worktree (with "are you sure?" inline confirmation)
- `o` copies worktree path to clipboard (via `pbcopy` on macOS)

### Agents Tab — Log Viewer

**Additions to existing agents tab:**

The TUI gains a log buffer:
```rust
log_buffers: HashMap<String, Vec<String>>,
log_scroll: usize,
```

`OrchestratorEvent::AgentOutput` events append formatted lines to the buffer for the relevant issue_id. The main panel renders the selected story's log as a scrollable view.

- `j/k` in the sidebar navigates stories; when the main panel is focused, `j/k` scrolls the log
- `Tab` within the agents tab toggles focus between sidebar and log panel (distinct from the global tab switch which uses number keys)
- `G` jumps to bottom of log (follow mode), `g` jumps to top

**Phase progress bar:** Rendered between the story header and the log. A horizontal widget showing all pipeline phases as labeled segments. Completed phases are green, current phase is cyan/pulsing, future phases are dark gray, skipped phases are struck-through.

### Config Tab

**Behavior:**
- On tab focus: loads `ProjectConfig` from disk, formats as TOML, renders as a `Paragraph`
- `e` opens the config file in `$EDITOR` (spawns process, suspends TUI with `ratatui::restore()`, resumes with `ratatui::init()` after editor exits, reloads config)
- `r` reloads config from disk without opening editor

## 4. CLI: Shared Init/Configure Wizard

### Problem

`hive configure` is stubbed. `hive init` contains all the wizard logic that `configure` needs.

### Design

New file: `src/cli/wizard.rs`

```rust
pub fn run_wizard(existing: Option<ProjectConfig>) -> Result<()>
```

When `existing` is `None` (init mode): prompts show default suggestions (directory name, etc.).
When `existing` is `Some(config)` (configure mode): prompts pre-fill with current values. The user can press Enter to keep the current value or type a new one.

The `prompt()` helper signature changes to:
```rust
fn prompt_with_default(message: &str, default: &str) -> Result<String>
```

Display format: `Project name [current-value]:` — enter keeps the value, typing replaces it.

`src/cli/init.rs` becomes:
```rust
pub fn run_init() -> Result<()> { wizard::run_wizard(None) }
```

`src/cli/configure.rs` (new):
```rust
pub fn run_configure() -> Result<()> {
    let config_dir = crate::app::dirs_config_dir()?;
    let cwd = std::env::current_dir()?.to_string_lossy().to_string();
    let existing = load_project_config(&config_dir, &cwd)?;
    wizard::run_wizard(Some(existing))
}
```

`src/cli/mod.rs` adds `pub mod configure; pub mod wizard;`.

`main.rs` wires `Commands::Configure` to `cli::configure::run_configure()`.

## 5. Notification Integration

### Problem

`Orchestrator::send_notification()` exists but is never called.

### Design

The phase execution engine calls notifications at these points:

| Transition | Event |
|-----------|-------|
| Story reaches `Phase::Complete` | `NotifyEvent::StoryComplete { issue_id, pr_url, cost_usd, duration_secs }` |
| Retry budget exhausted → `NeedsAttention` | `NotifyEvent::NeedsAttention { issue_id, reason }` |
| CI fix attempts exhausted | `NotifyEvent::CiFailedMaxRetries { issue_id }` |
| Last running story completes, nothing queued | `NotifyEvent::AllIdle` |

The story task calls `send_notification()` directly (it has access to the notifier `Arc`). The orchestrator tracks running story count to detect `AllIdle`.

## 6. Crash Recovery

### Problem

State is persisted after every phase transition, but the orchestrator doesn't resume interrupted runs on startup.

### Design

In `Orchestrator::new()`, after loading persisted runs:

```rust
for run in self.runs.values() {
    match run.status {
        RunStatus::Complete | RunStatus::Failed => continue,
        _ => self.resume_story(run).await?,
    }
}
```

`resume_story()`:
- **Agent phases:** Check `runner.is_alive(session)`. If alive, call `output_stream()` to reattach. If dead, call `start_session()` with a recovery prompt: "You are resuming work on story {id}. Review the current state of the worktree and continue from where you left off. The previous phase was {phase}."
- **Polling phases:** Resume polling immediately — no state to recover.
- **Direct phases:** Re-execute idempotently (creating a PR that already exists returns the existing one; status transitions are idempotent).

The story task is spawned just like a new story, but starting from the persisted phase rather than `Queued`.

## Dependencies

New crate dependencies:
- `tokio-util` — for `CancellationToken`

## File Map (new and modified)

```
src/
├── runners/
│   └── claude.rs              # REWRITE — session tracking, stdout streaming
├── orchestrator/
│   ├── mod.rs                 # REWRITE — story task spawning, cancellation
│   ├── engine.rs              # NEW — phase execution functions
│   ├── prompts.rs             # NEW — phase-specific system prompt builders
│   ├── transitions.rs         # unchanged
│   └── retry.rs               # unchanged
├── tui/
│   ├── mod.rs                 # MODIFY — add tracker ref, log buffers, story channels
│   ├── tabs/
│   │   ├── agents.rs          # REWRITE — log viewer, phase bar, focus management
│   │   ├── stories.rs         # REWRITE — filterable table, tracker integration
│   │   ├── worktrees.rs       # REWRITE — live worktree list, actions
│   │   └── config_tab.rs      # REWRITE — config display, editor launch
│   └── widgets/
│       ├── log_viewer.rs      # NEW — scrollable log stream widget
│       ├── phase_bar.rs       # NEW — phase progress indicator widget
│       └── status_bar.rs      # MODIFY — show active story count from live data
├── cli/
│   ├── mod.rs                 # MODIFY — add wizard, configure modules
│   ├── wizard.rs              # NEW — shared wizard logic
│   ├── init.rs                # REWRITE — thin wrapper around wizard
│   ├── configure.rs           # NEW — load config then call wizard
│   └── status.rs              # unchanged
├── app.rs                     # MODIFY — pass tracker to TUI, add GitHub token resolution
├── git/
│   └── github.rs              # MODIFY — add GitHub token from env/config
└── domain/
    └── events.rs              # MODIFY — add StoriesLoaded event for async fetch
```
