# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What is Hive

Hive is a Rust TUI application that orchestrates autonomous coding agents through a story-to-PR pipeline. It pulls issues from a tracker (Linear or Jira), creates git worktrees, runs AI agents through a multi-phase pipeline (understand, implement, review, PR, CI watch, bot review handling, follow-ups, handoff), and manages the full lifecycle with crash recovery, cost tracking, and Discord notifications.

## Build & Test Commands

```bash
cargo build                    # build
cargo test                     # run all tests
cargo test -- test_name        # run a single test by name
cargo clippy                   # lint
cargo run                      # launch TUI (requires project config)
cargo run -- init              # interactive project setup wizard
cargo run -- configure         # reconfigure existing project
cargo run -- status            # print summary of active story runs
```

## Architecture

### Data Flow

```
TUI <--mpsc--> Orchestrator ---> AgentRunner (claude subprocess)
                   |                    |
                   +---> IssueTracker   +---> stream-json stdout parsing
                   +---> GitHubClient
                   +---> Notifier
```

The `app::run()` function wires everything together: it loads config, constructs the trait-object dependencies (`Arc<dyn AgentRunner>`, `Arc<dyn IssueTracker>`, etc.), spawns the Orchestrator on a background tokio task, and runs the TUI on the main task. Communication is via two `mpsc` channels: `OrchestratorEvent` (orchestrator -> TUI) and `TuiCommand` (TUI -> orchestrator).

### Phase Pipeline

Each story runs through a configurable sequence of phases, defined in `src/domain/phase.rs`:

```
Queued -> Understand -> Implement -> SelfReview -> CrossReview -> RaisePr -> CiWatch -> BotReviews -> FollowUps -> Handoff -> PrWatch -> Complete
```

Phases are categorized into three execution types:
- **Agent phases** (Understand, Implement, SelfReview, CrossReview, FollowUps): spawn a Claude subprocess with a tailored prompt, stream output
- **Direct phases** (RaisePr, Handoff): perform GitHub API calls with no agent
- **Polling phases** (CiWatch, BotReviews, PrWatch): poll GitHub on an interval, spawn fix agents when issues are found

Any phase can be disabled in project config. The `transitions::advance()` function skips disabled phases. Failed agent phases retry up to `max_attempts` (configurable per phase, default 3 for SelfReview, 1 for others), then escalate to `NeedsAttention`.

PrWatch polls the PR every 5 minutes (configurable via `poll_interval`, default `"5m"`). When GitHub reports merge conflicts (`mergeable: false`), it attempts a clean `git rebase`. If the rebase has conflicts, it spawns an agent to resolve them interactively, then force-pushes with `--force-with-lease`. When the PR is merged or closed, the worktree is automatically cleaned up.

### Key Modules

- **`orchestrator/`**: Core pipeline logic. `mod.rs` has `Orchestrator` (command loop, story lifecycle, crash recovery), `engine.rs` has phase execution functions, `transitions.rs` handles phase advancement, `prompts.rs` builds agent prompts.
- **`runners/claude.rs`**: Spawns `claude -p --output-format stream-json` subprocesses, parses the JSON stream into `AgentEvent`s. The `-p` flag is a boolean print flag; the prompt is the last positional argument (not `-p <prompt>`).
- **`trackers/`**: `IssueTracker` trait with Linear (GraphQL) and Jira (REST v3) implementations. Jira uses ADF-to-text flattening for descriptions and forward-only idempotent transitions.
- **`git/`**: `worktree.rs` manages git worktrees (one per story), `github.rs` wraps octocrab for PR creation, CI polling, review comment handling, and GraphQL thread resolution.
- **`tui/`**: Ratatui-based TUI with 4 tabs (Agents, Stories, Worktrees, Config). Tabs have their own state structs in `tabs/`. Key bindings: `1-4` switch tabs, `j/k` navigate, `?` help overlay.
- **`config/`**: Two-level TOML config. Global at `~/.config/hive/config.toml` (runners, tracker connections, notification webhooks). Per-project at `~/.config/hive/projects/<name>/project.toml` (phases, tracker settings, GitHub info). Config values prefixed with `env:` are resolved from environment variables at runtime.
- **`state/`**: JSON persistence for story runs (`<issue_id>.json`) and append-only agent transcript logs (`<issue_id>.agent.log`), both under `~/.config/hive/projects/<name>/runs/`.
- **`domain/`**: Core types — `Phase`, `StoryRun`, `PhaseOutcome`, `OrchestratorEvent`/`TuiCommand`/`AgentEvent` (channel messages), `Issue`/`IssueDetail`.

### Trait Architecture

The codebase uses trait objects (`Arc<dyn T>`) for its three external integration points, enabling future backends:
- `AgentRunner` — currently only `ClaudeRunner`
- `IssueTracker` — `LinearTracker` and `JiraTracker`
- `Notifier` — currently only `DiscordNotifier`

### Logging

TUI mode logs only to file (`~/.config/hive/logs/hive.log`, daily rotation) to avoid corrupting the terminal. CLI subcommands log to both stderr and file. Controlled via `tracing` with `tracing-appender`.

## Conventions

- Error handling uses `thiserror` via `HiveError` enum in `src/error.rs`. All fallible functions return `Result<T>` (aliased to `std::result::Result<T, HiveError>`).
- **Never silently swallow `Result` in a function that returns `Result`.** The antipattern `if let Err(e) = foo().await { tracing::warn!(...) } Ok(())` is banned: the signature promises callers they'll learn about failures, but the body lies by always returning `Ok(())`. This bit us on PR thread resolution — the TUI reported "Resolved N thread(s)" when N actually failed, masking a token-scope problem and causing PrWatch→BotReviews regression loops. The same shape at `spawn_story_task` hid every force-push / rebase failure for a whole day, leaving stories zombied as "Running" on the TUI. If a function returns `Result<()>`, either propagate the error with `?` (or `map_err` + `?`) or change the signature to infallible. If the caller genuinely wants to log-and-continue, that decision lives at the call site — not hidden in the impl. For spawned tasks, the boundary still has to surface failure: update run status, emit a `StoryUpdated` event, and/or write to stderr so the user sees the crash.
- **Check for these antipatterns with `ast-grep scan src/`** before committing. Rules live in `.ast-grep/rules/` and cover the impl-body swallow and the spawn-swallow variants. Clippy's `question_mark` and `must_use_candidate` lints (wired in `Cargo.toml`) also help catch the simpler shapes.
- Async runtime is tokio with `features = ["full"]`. Each story runs in its own spawned task with a `CancellationToken` for clean shutdown.
- Agent prompts reference "the project's verification command" generically rather than hardcoding build commands.
- Commit messages in agent prompts use conventional format: `feat(<issue_id>): description` or `fix(<issue_id>): description`.
- Phase config keys use kebab-case: `self-review`, `cross-review`, `ci-watch`, `bot-reviews`, `follow-ups`, `raise-pr`, `pr-watch`.
- The `env:` prefix pattern in config values (e.g., `api_key = "env:LINEAR_API_KEY"`) is resolved by `config::resolve::resolve_env()`.

## Environment Variables

- `GITHUB_TOKEN` or `GH_TOKEN` — required for PR creation, CI polling, bot review handling
- `LINEAR_API_KEY` — required when using Linear tracker
- `JIRA_API_TOKEN` + `JIRA_EMAIL` — required when using Jira tracker
- `HIVE_DISCORD_WEBHOOK` — optional, for Discord notifications
