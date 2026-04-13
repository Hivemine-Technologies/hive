# Hive — Agent Orchestration TUI

**Date:** 2026-04-13
**Status:** Draft
**Author:** Robbie Diaz + Claude

## Overview

Hive is a Rust terminal UI application that orchestrates autonomous coding agents for story implementation. It replaces a manual workflow of dispatching agents via tmux panes, monitoring log files, and context-switching between tools — with a unified control center that automates the plumbing while keeping the human as the strategist.

The user selects which stories to work on and in what order (semantic conflicts between stories, like overlapping schema changes, require human judgment). Hive handles everything else: dispatching agents, managing worktrees, sequencing workflow phases, polling CI and bot reviews, retrying failures, and notifying on completion.

## Goals

- **Automate the plumbing.** Story dispatch, worktree creation, PR management, CI/review polling, and status transitions are handled by Hive, not by the user or by an LLM burning tokens on `sleep` loops.
- **Unified visibility.** A single dashboard showing all running agents, their phases, output streams, and costs — replacing scattered tmux panes and log files.
- **Crash resilience.** Per-phase checkpointing so machine sleep, agent crashes, or process restarts don't lose progress. Recovery is automatic on restart.
- **Pluggable backends.** Swappable agent runners (Claude Code, Gemini CLI, Codex, Devin, Windsurf), issue trackers (Linear, Jira), and notifiers (Discord, Slack) — the core orchestration is independent of all three.
- **Configurable workflows.** Fixed-order phase pipeline with per-phase enable/disable toggles and parameter tuning. No YAML recipe language — phases are Rust code, composition is config.
- **Shareable.** Standalone binary, project-scoped config, suitable for other developers trusted with agent-level automation.

## Non-Goals

- **Not a project management tool.** Hive doesn't triage, reprioritize, or manage sprints. It reads ready stories from the issue tracker.
- **Not a code editor.** Manual intervention happens in a separate terminal. Hive doesn't embed an interactive Claude session.
- **Not a CI/CD replacement.** Hive polls CI status; it doesn't run builds or tests itself.

## Technology Stack

| Component | Choice | Rationale |
|-----------|--------|-----------|
| Language | Rust | Strong async (Tokio), type safety, single binary, ACP crate ecosystem |
| TUI framework | Ratatui | Constraint-based layout engine, immediate-mode rendering, `tokio::select!` integration |
| Async runtime | Tokio | Native multiplexing of agent streams, GitHub polling, user input |
| Agent protocol | ACP (Agent Client Protocol) | JSON-RPC 2.0 over stdio, supported by Claude Code, Gemini CLI, and Codex — one protocol for all agents |
| Serialization | serde + serde_json / toml | JSON for state persistence, TOML for config |
| Git operations | git2 crate + GitHub REST API | Worktree management, PR creation, CI/review polling |

## Architecture

Four layers with strict dependency direction (each layer only depends on the one below it):

```
┌─────────────────────────────────────────────────────────┐
│                    TUI Layer (Ratatui)                   │
│  Tab navigation │ Widgets │ Keyboard input │ Rendering   │
├─────────────────────────────────────────────────────────┤
│                 Orchestrator (core logic)                │
│  Workflow Engine │ Phase Sequencer │ Retry Manager        │
├──────────────┬──────────────┬──────────────┬────────────┤
│ Agent Runner │ Issue Tracker │ Git Manager  │ Notifier   │
│  (trait)     │   (trait)     │              │  (trait)   │
├──────────────┼──────────────┼──────────────┼────────────┤
│ Claude (ACP) │   Linear     │ git2/GitHub  │  Discord   │
│ Gemini (ACP) │   Jira       │   API        │  Slack     │
│ Codex (ACP)  │              │              │            │
└──────────────┴──────────────┴──────────────┴────────────┘
```

**TUI Layer** — Ratatui rendering, keyboard input, tab/panel management. Receives view models from the orchestrator via channels. Sends user commands (start story, cancel, rebase) back via channels. Knows nothing about agents or issue trackers.

**Orchestrator** — The brain. Owns the workflow state machine, drives phase transitions, manages retry budgets, coordinates between backends. Emits events that the TUI subscribes to.

**Backend Traits** — Pluggable interfaces (`AgentRunner`, `IssueTracker`, `Notifier`). Each defines a narrow contract; implementations are swappable via config.

**Implementations** — Concrete backends. Claude/Gemini/Codex via ACP, Linear via GraphQL, Jira via REST, Discord/Slack via webhooks.

**GitManager** is not behind a trait — there's only one git. Extracting a trait would be speculative abstraction.

### Communication Between Layers

```
User input (keyboard)
    │
    ▼
TUI ──[Command channel]──→ Orchestrator
    │                            │
    │                     ┌──────┴──────┐
    │                     ▼             ▼
    │              AgentRunner    IssueTracker
    │                     │             │
    │                     ▼             ▼
    ◄──[Event channel]───Orchestrator───┘
    │
    ▼
Render updated view
```

The TUI and orchestrator communicate exclusively through async channels (`tokio::mpsc`). This decoupling means the orchestrator could later drive a web UI, Discord bot, or REST API without changes.

## Core Traits

### AgentRunner

```rust
#[async_trait]
pub trait AgentRunner: Send + Sync {
    async fn start_session(&self, config: SessionConfig) -> Result<SessionHandle>;
    async fn send_prompt(&self, session: &SessionHandle, prompt: &str) -> Result<()>;
    fn output_stream(&self, session: &SessionHandle) -> Pin<Box<dyn Stream<Item = AgentEvent>>>;
    async fn cancel(&self, session: &SessionHandle) -> Result<()>;
    async fn resume(&self, session: &SessionHandle) -> Result<()>;
    fn name(&self) -> &str;
}
```

`SessionConfig` includes: working directory, model override, system prompt, permission mode, allowed tools.

`AgentEvent` enum: `TextDelta`, `ToolUse`, `ToolResult`, `Error`, `Complete`, `CostUpdate`.

`SessionHandle` contains: session ID, process handle, runner identity. Serializable for persistence (session ID + runner name), so Hive can attempt to reattach after restart.

**Implementations:**
- **Claude Code** — spawn `claude` with ACP, full streaming, session resume via `--resume`, skill invocation via `/skill-name` prompts. Primary workhorse. Reference: `agent-client-protocol-schema` crate for typed ACP structs, `cc-sdk` crate as reference.
- **Gemini CLI** — spawn `gemini --experimental-acp` for ACP mode, fall back to `gemini -p` subprocess with NDJSON parsing if ACP is unreliable.
- **Codex CLI** — spawn `codex` via ACP or app-server JSON-RPC protocol. Reference: `codex-cli-sdk` crate.
- **Future runners** (Devin, Windsurf) — implement the same trait against their respective APIs.

### IssueTracker

```rust
#[async_trait]
pub trait IssueTracker: Send + Sync {
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>>;
    async fn start_issue(&self, id: &str) -> Result<()>;
    async fn finish_issue(&self, id: &str) -> Result<()>;
    async fn create_followup(&self, parent: &str, content: FollowUpContent) -> Result<String>;
    async fn get_issue(&self, id: &str) -> Result<IssueDetail>;
    fn name(&self) -> &str;
}
```

`IssueFilters`: team, project, labels, priority. `IssueDetail`: title, description, acceptance criteria, priority, labels, blockers.

Status transitions (`start_issue`, `finish_issue`) are direct API calls — no agent needed.

Follow-up issue content generation still uses an agent (Claude) for the text, but Hive makes the API call to create the issue.

**Implementations:**
- **Linear** — GraphQL API. Maps to the existing `linear-cli` query patterns.
- **Jira** — REST API v3.

### Notifier

```rust
#[async_trait]
pub trait Notifier: Send + Sync {
    async fn notify(&self, event: NotifyEvent) -> Result<()>;
    fn name(&self) -> &str;
}
```

`NotifyEvent`: `StoryComplete { issue_id, pr_url, cost, duration }`, `NeedsAttention { issue_id, reason }`, `AllIdle`, `CiFailedMaxRetries { issue_id }`.

Notifications are entirely optional. If configured, only one notifier is active at a time per project.

**Implementations:**
- **Discord** — webhook POST with formatted embed.
- **Slack** — webhook POST with Block Kit payload.

## Workflow Engine

### Phase State Machine

Each story being processed is a state machine with fixed-order phases. The ordering is hardcoded in Rust — it encodes real dependencies (can't watch CI before creating a PR). Users control which phases are enabled and their parameters, not the ordering.

```
Queued
  │ [user selects story]
  ▼
Understand ──→ Implement ──→ SelfReview ──→ CrossReview ──→ RaisePr
                                                              │
  ┌───────────────────────────────────────────────────────────┘
  ▼
CiWatch ──→ BotReviews ──→ FollowUps ──→ Handoff ──→ Complete

Disabled phases are skipped in the chain.
Any phase can transition to NeedsAttention if retries are exhausted or errors occur.
```

### Phase Types

Phases fall into three categories based on who does the work:

**Agent phases** — require an LLM session. Hive spawns an agent via `AgentRunner`, streams output, and waits for completion.
- Understand, Implement, SelfReview, CrossReview, FollowUps

**Polling phases** — Hive makes API calls on a timer. No agent or tokens consumed during waiting.
- CiWatch (poll GitHub Actions), BotReviews (poll PR comments for Coderabbit/Gemini-assist)
- When a polling phase detects work (CI failure, new review comments), it spawns a targeted agent session to fix the issue, then resumes polling.

**Direct phases** — Hive performs the action itself via API calls. Deterministic, no agent needed.
- RaisePr (push branch, create PR via GitHub API), Handoff (move issue status, post summary comment)

### Story State

```rust
pub struct StoryRun {
    pub issue_id: String,
    pub phase: Phase,
    pub status: RunStatus,           // Running, Paused, NeedsAttention, Complete, Failed
    pub worktree: Option<PathBuf>,
    pub branch: Option<String>,
    pub pr: Option<PrHandle>,
    pub session_id: Option<String>,  // ACP session ID for resume
    pub phase_history: Vec<PhaseResult>,
    pub cost_usd: f64,
    pub started_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

pub enum Phase {
    Queued,
    Understand,
    Implement,
    SelfReview { attempt: u8 },
    CrossReview,
    RaisePr,
    CiWatch { attempt: u8 },
    BotReviews { cycle: u8 },
    FollowUps,
    Handoff,
    Complete,
    NeedsAttention { reason: String },
}

pub enum RunStatus {
    Running,
    Paused,
    NeedsAttention,
    Complete,
    Failed,
}
```

### Crash Recovery

State is serialized to disk as JSON after every phase transition.

On startup, Hive reads all persisted story runs and resumes:
- **Agent phases** — checks if the ACP session is still alive. If yes, reattaches to the output stream. If not, starts a new session in the same worktree with a "review work in progress and continue" prompt.
- **Polling phases** — resumes polling immediately. No state lost.
- **Direct phases** — re-executes idempotently (creating a PR that already exists returns the existing one).

The unit of recovery is the phase, not the pipeline. Machine sleep during CI Watch? Hive resumes polling on wake. Agent crash during Implement? New session picks up the worktree state.

## Configuration

### Directory Structure

```
~/.config/hive/
├── config.toml              # global defaults
├── projects/
│   └── apex/
│       ├── project.toml     # project-specific overrides
│       └── runs/
│           ├── APX-245.json
│           └── APX-270.json
```

### Global Config (`config.toml`)

```toml
[runners.claude]
command = "claude"
protocol = "acp"
default_model = "opus-4-6"
permission_mode = "dangerously-skip"

[runners.gemini]
command = "gemini"
protocol = "acp"             # falls back to subprocess if ACP unavailable
default_model = "flash"

[runners.codex]
command = "codex"
protocol = "acp"
default_model = "codex-mini"

[trackers.linear]
api_key = "env:LINEAR_API_KEY"

[trackers.jira]
base_url = "https://mycompany.atlassian.net"
api_token = "env:JIRA_API_TOKEN"
email = "env:JIRA_EMAIL"

# Tracker-specific status and field mappings are in project.toml
# (see Project Config section below)

[notifications.discord]
webhook_url = "env:HIVE_DISCORD_WEBHOOK"

[notifications.slack]
webhook_url = "env:HIVE_SLACK_WEBHOOK"
```

### Project Config (`project.toml`)

```toml
name = "apex"
repo_path = "/Users/robbie/dev/hivemine/gemini-chatz/apex"
worktree_dir = ".worktrees"
tracker = "linear"
notifier = "discord"                       # optional — omit or set to "none" to disable

[notifications]
events = ["complete", "needs-attention", "all-idle"]

[github]
owner = "hivemine"
repo = "gemini-chatz"

# Tracker-specific configuration
# Each tracker has different concepts for teams, statuses, and fields.
# These mappings let Hive speak the tracker's language.
[tracker_config]
team = "Hivemine"                          # Linear: team name, Jira: project key
ready_filter = "Todo"                      # status that means "ready for work"

[tracker_config.statuses]
start = "In Progress"                      # status to set when Hive starts a story
review = "In Review"                       # status to set when PR is raised
done = "Done"                              # status to set on handoff (if auto-close isn't used)

[tracker_config.fields]
# Optional custom field mappings (Jira especially needs these)
# priority = "customfield_10001"           # Jira custom field for priority
# story_points = "customfield_10002"       # Jira story points field
# acceptance_criteria = "customfield_10003"

[phases.understand]
enabled = true
runner = "claude"
model = "opus-4-6"

[phases.implement]
enabled = true
runner = "claude"
model = "opus-4-6"

[phases.self-review]
enabled = true
runner = "claude"
model = "sonnet-4-6"
max_attempts = 3

[phases.cross-review]
enabled = false
runner = "gemini"
model = "flash"

[phases.raise-pr]
enabled = true

[phases.ci-watch]
enabled = true
poll_interval = "30s"
max_fix_attempts = 3
fix_runner = "claude"
fix_model = "sonnet-4-6"

[phases.bot-reviews]
enabled = true
wait_for = ["coderabbit"]
max_fix_cycles = 3
fix_runner = "claude"
fix_model = "sonnet-4-6"

[phases.follow-ups]
enabled = true
runner = "claude"
model = "sonnet-4-6"

[phases.handoff]
enabled = true
```

### Config Resolution

Project config merges on top of global config. Phase-level `runner` and `model` override the global runner defaults. Environment variables are resolved at startup via `env:VAR_NAME` syntax.

## CLI Commands

Hive has two modes: the interactive TUI dashboard (default) and a set of setup/utility subcommands.

### `hive` (no subcommand)

Launches the TUI dashboard for the current project. Hive detects the project by looking for a `project.toml` in `~/.config/hive/projects/` whose `repo_path` matches the current working directory (or a parent). Exits with an error and suggests `hive init` if no project is found.

### `hive init`

Interactive setup wizard for a new project. Walks the user through a series of prompts:

1. **Project name** — auto-suggested from the repo directory name
2. **Repository path** — defaults to current working directory
3. **Worktree directory** — defaults to `.worktrees`
4. **Issue tracker** — choose from configured trackers in global config (Linear, Jira, etc.)
5. **Tracker team/project** — which team (Linear) or project key (Jira) to pull stories from
6. **Status mappings** — what statuses mean "ready", "in progress", "in review" in their tracker. Auto-detected where possible (Hive queries the tracker API for available statuses and presents them as choices).
7. **GitHub remote** — auto-detected from git remote, confirm owner/repo
8. **Default runner** — which agent runner for implementation work
9. **Review runner** — which agent runner for cross-review (or disable)
10. **Notifications** (optional) — enable Discord or Slack, paste webhook URL, or skip entirely
11. **Phase toggles** — walk through each phase, enable/disable, adjust defaults

Writes `~/.config/hive/projects/<name>/project.toml`. If global config doesn't exist yet, `hive init` creates it first — prompting for runner commands and tracker API keys before proceeding to project-specific steps (since steps like status auto-detection require working API credentials).

### `hive configure`

Re-runs the setup wizard for the current project. Pre-fills all prompts with existing values so the user can tab through unchanged settings and only modify what they need. Same flow as `hive init` but in edit mode.

### `hive status`

Non-interactive one-shot: prints a summary of all active story runs for the current project (issue ID, phase, status, duration, cost). Useful for scripting or quick checks without launching the full TUI.

## TUI Design

### Layout

Tab-based navigation with four views. Persistent status indicators in the tab bar (running count, attention count, total cost) visible from every tab.

**Tab 1: Agents** (primary view)
- Left sidebar: list of active agents (running, polling, needs-attention, complete) plus queued stories
- Main panel: selected agent's detail — issue title, phase progress bar, streaming log output, action shortcuts
- Phase progress bar shows the full pipeline with current phase highlighted and completed phases checked
- Action bar: `[r]` Rebase, `[c]` Cancel, `[o]` Copy worktree path to clipboard, `[l]` Full log view

**Tab 2: Stories** (backlog browser)
- Full-width filterable/sortable table of ready stories from the issue tracker
- Columns: ID, Title, Priority, Project, Labels
- Filter bar: Ready / All / In Progress / Blocked
- Actions: `[Enter]` Start story, `[Space]` Select multiple for batch start, `[/]` Search, `[s]` Sort, `[f]` Filter, `[d]` View details

**Tab 3: Worktrees**
- List of all worktrees with status: branch name, commits ahead/behind master, associated PR
- Actions: `[r]` Rebase, `[o]` Open terminal, `[d]` Delete, `[p]` Prune stale worktrees

**Tab 4: Config**
- Read-only display of current project configuration
- Actions: `[e]` Open config file in editor, `[r]` Reload config

### Keyboard Navigation

- `1-4` or `Tab/Shift-Tab`: switch tabs
- `j/k` or arrows: navigate lists/tables
- `Enter`: select/activate
- `?`: help overlay
- `q`: quit (with confirmation if agents are running)

### Event Loop

```rust
loop {
    tokio::select! {
        // Terminal input
        event = crossterm_events.next() => { /* route to active tab */ }
        
        // Agent output streams (one per running agent)
        event = agent_events.recv() => { /* update story state, push to TUI */ }
        
        // Orchestrator events (phase transitions, status changes)
        event = orchestrator_events.recv() => { /* update dashboard state */ }
        
        // Render tick
        _ = render_interval.tick() => { /* redraw TUI */ }
    }
}
```

## Project Structure

```
~/dev/hivemine/hive/
├── Cargo.toml
├── README.md
├── config.example.toml
├── src/
│   ├── main.rs                    # CLI entry, arg parsing, TUI bootstrap
│   ├── app.rs                     # Top-level App state, tab routing
│   │
│   ├── config/
│   │   ├── mod.rs
│   │   └── types.rs               # Config structs, TOML deserialization
│   │
│   ├── orchestrator/
│   │   ├── mod.rs                 # Orchestrator — drives phase transitions
│   │   ├── phase.rs               # Phase enum, transition logic
│   │   ├── state.rs               # StoryRun, persistence (JSON serde)
│   │   └── retry.rs               # Retry budget tracking
│   │
│   ├── runners/
│   │   ├── mod.rs                 # AgentRunner trait
│   │   ├── claude.rs              # Claude Code via ACP
│   │   ├── gemini.rs              # Gemini CLI via ACP/subprocess
│   │   └── codex.rs               # Codex via app-server
│   │
│   ├── trackers/
│   │   ├── mod.rs                 # IssueTracker trait
│   │   ├── linear.rs              # Linear GraphQL
│   │   └── jira.rs                # Jira REST
│   │
│   ├── notifiers/
│   │   ├── mod.rs                 # Notifier trait
│   │   ├── discord.rs             # Discord webhook
│   │   └── slack.rs               # Slack webhook
│   │
│   ├── git/
│   │   ├── mod.rs                 # GitManager
│   │   ├── worktree.rs            # Worktree operations
│   │   └── github.rs              # PR creation, CI polling, review polling
│   │
│   └── tui/
│       ├── mod.rs                 # Event loop, tokio::select! multiplexer
│       ├── tabs/
│       │   ├── agents.rs          # Agents tab (sidebar + main panel)
│       │   ├── stories.rs         # Stories tab (filterable table)
│       │   ├── worktrees.rs       # Worktrees tab
│       │   └── config.rs          # Config tab
│       ├── widgets/
│       │   ├── log_viewer.rs      # Scrollable log stream
│       │   ├── phase_bar.rs       # Phase progress indicator
│       │   └── status_bar.rs      # Tab bar + global status
│       └── theme.rs               # Colors, styles
```

### Layer Boundaries

- `orchestrator/` has zero knowledge of the TUI. Communicates via `tokio::mpsc` channels.
- `runners/`, `trackers/`, `notifiers/` are pure backend implementations. They don't know about phases or the orchestrator.
- `tui/` is purely presentational. Reads state, sends commands, renders frames.
- This separation means the orchestrator could later drive a web UI, Discord bot, or REST API without changes.

## Key Dependencies (Rust Crates)

| Crate | Purpose |
|-------|---------|
| `ratatui` | TUI rendering |
| `crossterm` | Terminal input/output |
| `tokio` | Async runtime |
| `serde` / `serde_json` / `toml` | Serialization |
| `reqwest` | HTTP client (GitHub API, webhooks, Jira) |
| `graphql-client` | Linear GraphQL queries |
| `git2` | Git/worktree operations |
| `chrono` | Timestamps |
| `clap` | CLI argument parsing |
| `tracing` | Structured logging |
| `agent-client-protocol-schema` | Typed ACP protocol structs |
| `cc-sdk` | Claude Code ACP reference (may use directly or as reference for custom impl) |

## Future Considerations

These are explicitly out of scope for v1 but noted for architectural awareness:

- **Custom phase types via plugin trait** — new phases as Rust code implementing a `Phase` trait, referenced in config. The current architecture supports this without rework.
- **Web dashboard** — the orchestrator/TUI channel separation means a web frontend could replace or complement the TUI.
- **Remote agents** — ACP supports HTTP/WebSocket transports, not just stdio. Could orchestrate agents running on remote machines.
- **Diff viewer** — showing the git diff locally in the TUI for quick review without leaving the terminal.
- **Multi-project view** — running Hive across multiple repos simultaneously from one dashboard.
