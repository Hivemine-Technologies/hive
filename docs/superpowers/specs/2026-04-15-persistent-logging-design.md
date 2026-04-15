# Persistent Logging & Agent Transcript Design

## Problem

Hive logs to stderr via `tracing_subscriber::fmt()`, which is invisible during TUI operation and lost when the session ends. Agent output (text deltas, tool uses, errors, completions) is held only in memory in `LogBuffer` (capped at 5000 lines per issue) and lost entirely when the TUI exits. Only run metadata (phase, status, cost) is persisted.

This makes debugging both hive internals and agent behavior extremely difficult.

## Solution

Two complementary changes:

### 1. Hive File Logging

Add a daily rolling file appender via `tracing-appender` as a second layer on the tracing subscriber.

- **Location:** `~/.config/hive/logs/hive.YYYY-MM-DD.log`
- **Level:** `DEBUG` for the file layer (full visibility into orchestrator, TUI, phase transitions)
- **Stderr layer:** unchanged (`hive=info`), kept for non-TUI usage
- **Writer:** non-blocking (`tracing_appender::non_blocking`) so log I/O doesn't block the async runtime
- **Rotation:** daily, no automatic cleanup

#### Changes

- `Cargo.toml`: add `tracing-appender = "0.2"`
- `main.rs`: replace single-layer subscriber with dual-layer (stderr + file)

### 2. Agent Transcript Logging

New module `state/agent_log.rs` with a single function:

```rust
pub fn log_agent_event(runs_dir: &Path, issue_id: &str, event: &AgentEvent)
```

Writes to `{runs_dir}/{issue_id}.agent.log` in append mode.

#### Format

```
[2026-04-15T05:45:13Z] TEXT: [Implement] Agent started (session: abc123)
[2026-04-15T05:45:14Z] TOOL: Edit { src/main.rs }
[2026-04-15T05:45:20Z] ERROR: compilation failed
[2026-04-15T05:45:30Z] COMPLETE: cost=$0.42
```

#### Integration

Called alongside every `event_tx.send(OrchestratorEvent::AgentOutput{...})` in `engine.rs`:
- `run_agent_phase()` streaming loop and phase-start message
- `run_fix_agent()` streaming loop
- One-off `TextDelta` sends in `run_ci_watch()` and `run_bot_reviews()`

The function opens the file in append mode on each call. This is simple, crash-safe, and avoids buffering that could lose data.

#### Changes

- `state/mod.rs`: add `pub mod agent_log;`
- `state/agent_log.rs`: new file
- `engine.rs`: add `log_agent_event()` calls next to each agent event send

## Files Touched

| File | Change |
|------|--------|
| `Cargo.toml` | add `tracing-appender` |
| `src/main.rs` | dual-layer tracing subscriber |
| `src/state/mod.rs` | add `pub mod agent_log` |
| `src/state/agent_log.rs` | new — `log_agent_event()` function |
| `src/orchestrator/engine.rs` | add `log_agent_event()` calls |

## Non-Goals

- Log rotation/cleanup (files are small, manual cleanup for now)
- Structured/JSON log format (plain text is easier to read with `tail -f`)
- Agent log compression or indexing
