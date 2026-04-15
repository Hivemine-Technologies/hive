# Persistent Logging & Agent Transcripts Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add persistent file logging for hive internals and agent output transcripts so both survive TUI sessions.

**Architecture:** Dual-layer tracing subscriber (stderr + daily rolling file) for hive logs. A `log_agent_event` function in `state/agent_log.rs` writes agent events to per-issue transcript files. A `send_and_log` helper in `engine.rs` replaces raw `event_tx.send()` calls to tee events to both the TUI channel and disk.

**Tech Stack:** `tracing-appender` for rolling file logs, `chrono` (already a dep) for timestamps, `std::fs::OpenOptions` for append-mode agent logs.

---

## File Map

| File | Action | Responsibility |
|------|--------|----------------|
| `Cargo.toml` | Modify | Add `tracing-appender` dependency |
| `src/main.rs` | Modify | Dual-layer tracing subscriber setup |
| `src/state/mod.rs` | Modify | Add `pub mod agent_log;` |
| `src/state/agent_log.rs` | Create | `log_agent_event()` function + tests |
| `src/orchestrator/engine.rs` | Modify | Add `send_and_log` helper, add `runs_dir` param to public functions, replace all 15 `AgentOutput` send sites |
| `src/orchestrator/mod.rs` | Modify | Pass `runs_dir` to engine function calls |

---

### Task 1: Add `tracing-appender` dependency

**Files:**
- Modify: `Cargo.toml:21`

- [ ] **Step 1: Add the dependency**

In `Cargo.toml`, after the `tracing-subscriber` line (line 21), add:

```toml
tracing-appender = "0.2"
```

- [ ] **Step 2: Verify it resolves**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles with no new errors

- [ ] **Step 3: Commit**

```bash
git add Cargo.toml Cargo.lock
git commit -m "chore: add tracing-appender dependency"
```

---

### Task 2: Create `state/agent_log.rs` with tests

**Files:**
- Create: `src/state/agent_log.rs`
- Modify: `src/state/mod.rs:1`

- [ ] **Step 1: Write the tests first**

Create `src/state/agent_log.rs` with the following content:

```rust
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use chrono::Utc;

use crate::domain::AgentEvent;

/// Append a formatted agent event to `{runs_dir}/{issue_id}.agent.log`.
///
/// Opens in append mode on each call for crash safety. Silently drops
/// write errors — logging must never crash the orchestrator.
pub fn log_agent_event(runs_dir: &Path, issue_id: &str, event: &AgentEvent) {
    todo!()
}

fn format_event(event: &AgentEvent) -> String {
    todo!()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_text_delta() {
        let event = AgentEvent::TextDelta("hello world\n".to_string());
        let formatted = format_event(&event);
        assert!(formatted.contains("TEXT: hello world\n"));
        // Starts with ISO timestamp in brackets
        assert!(formatted.starts_with('['));
        assert!(formatted.contains("] TEXT:"));
    }

    #[test]
    fn test_format_tool_use() {
        let event = AgentEvent::ToolUse {
            tool: "Edit".to_string(),
            input_preview: "src/main.rs".to_string(),
        };
        let formatted = format_event(&event);
        assert!(formatted.contains("TOOL: Edit { src/main.rs }"));
    }

    #[test]
    fn test_format_error() {
        let event = AgentEvent::Error("compilation failed".to_string());
        let formatted = format_event(&event);
        assert!(formatted.contains("ERROR: compilation failed"));
    }

    #[test]
    fn test_format_complete() {
        let event = AgentEvent::Complete { cost_usd: 0.42 };
        let formatted = format_event(&event);
        assert!(formatted.contains("COMPLETE: cost=$0.42"));
    }

    #[test]
    fn test_log_agent_event_writes_to_file() {
        let dir = tempfile::tempdir().unwrap();
        let event = AgentEvent::TextDelta("test line\n".to_string());
        log_agent_event(dir.path(), "APX-100", &event);
        log_agent_event(dir.path(), "APX-100", &event);

        let content = std::fs::read_to_string(dir.path().join("APX-100.agent.log")).unwrap();
        let lines: Vec<&str> = content.lines().collect();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("TEXT: test line"));
        assert!(lines[1].contains("TEXT: test line"));
    }

    #[test]
    fn test_log_agent_event_separates_issues() {
        let dir = tempfile::tempdir().unwrap();
        log_agent_event(dir.path(), "APX-1", &AgentEvent::TextDelta("one\n".to_string()));
        log_agent_event(dir.path(), "APX-2", &AgentEvent::TextDelta("two\n".to_string()));

        assert!(dir.path().join("APX-1.agent.log").exists());
        assert!(dir.path().join("APX-2.agent.log").exists());

        let c1 = std::fs::read_to_string(dir.path().join("APX-1.agent.log")).unwrap();
        let c2 = std::fs::read_to_string(dir.path().join("APX-2.agent.log")).unwrap();
        assert!(c1.contains("one"));
        assert!(!c1.contains("two"));
        assert!(c2.contains("two"));
    }
}
```

- [ ] **Step 2: Register the module**

In `src/state/mod.rs`, add after line 1:

```rust
pub mod agent_log;
```

So the file becomes:

```rust
pub mod agent_log;
pub mod persistence;
```

- [ ] **Step 3: Run tests to verify they fail**

Run: `cargo test --lib state::agent_log 2>&1`
Expected: FAIL — `todo!()` panics

- [ ] **Step 4: Implement `format_event`**

Replace the `format_event` `todo!()` with:

```rust
fn format_event(event: &AgentEvent) -> String {
    let timestamp = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    match event {
        AgentEvent::TextDelta(text) => format!("[{timestamp}] TEXT: {text}"),
        AgentEvent::ToolUse { tool, input_preview } => {
            format!("[{timestamp}] TOOL: {tool} {{ {input_preview} }}")
        }
        AgentEvent::Error(msg) => format!("[{timestamp}] ERROR: {msg}"),
        AgentEvent::Complete { cost_usd } => {
            format!("[{timestamp}] COMPLETE: cost=${cost_usd:.2}")
        }
    }
}
```

- [ ] **Step 5: Implement `log_agent_event`**

Replace the `log_agent_event` `todo!()` with:

```rust
pub fn log_agent_event(runs_dir: &Path, issue_id: &str, event: &AgentEvent) {
    let path = runs_dir.join(format!("{issue_id}.agent.log"));
    let line = format_event(event);
    let Ok(mut file) = OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    let _ = writeln!(file, "{line}");
}
```

- [ ] **Step 6: Run tests to verify they pass**

Run: `cargo test --lib state::agent_log 2>&1`
Expected: all 6 tests PASS

- [ ] **Step 7: Commit**

```bash
git add src/state/agent_log.rs src/state/mod.rs
git commit -m "feat: add agent transcript logger with tests"
```

---

### Task 3: Set up dual-layer tracing subscriber

**Files:**
- Modify: `src/main.rs:42-45`

- [ ] **Step 1: Update the tracing subscriber setup**

Replace lines 42-45 in `src/main.rs`:

```rust
    tracing_subscriber::fmt()
        .with_env_filter("hive=info")
        .init();
```

With:

```rust
    // Persistent file logging — daily rotation to ~/.config/hive/logs/
    let log_dir = std::env::var("HOME")
        .map(|h| std::path::PathBuf::from(h).join(".config").join("hive").join("logs"))
        .unwrap_or_else(|_| std::path::PathBuf::from("logs"));
    std::fs::create_dir_all(&log_dir).ok();

    let file_appender = tracing_appender::rolling::daily(&log_dir, "hive.log");
    let (non_blocking, _guard) = tracing_appender::non_blocking(file_appender);

    use tracing_subscriber::{fmt, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    tracing_subscriber::registry()
        .with(
            fmt::layer()
                .with_writer(std::io::stderr)
                .with_filter(EnvFilter::new("hive=info")),
        )
        .with(
            fmt::layer()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(EnvFilter::new("hive=debug")),
        )
        .init();
```

Note: `_guard` must live until `main` returns — dropping it flushes and stops the writer. It already does because it's bound in `main`'s scope.

- [ ] **Step 2: Verify it compiles**

Run: `cargo check 2>&1 | tail -5`
Expected: compiles cleanly

- [ ] **Step 3: Smoke test — verify log file is created**

Run: `cargo run -- status 2>/dev/null; ls ~/.config/hive/logs/`
Expected: a `hive.log.YYYY-MM-DD` file exists in the directory

- [ ] **Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: add persistent file logging with daily rotation"
```

---

### Task 4: Add `send_and_log` helper and wire `runs_dir` through engine

**Files:**
- Modify: `src/orchestrator/engine.rs:1-112` (run_agent_phase), `src/orchestrator/engine.rs:149-195` (run_polling_phase), `src/orchestrator/engine.rs:197-316` (run_ci_watch), `src/orchestrator/engine.rs:318-450` (run_bot_reviews), `src/orchestrator/engine.rs:453-486` (run_fix_agent), `src/orchestrator/engine.rs:523-559` (run_direct_phase), `src/orchestrator/engine.rs:561-613` (run_raise_pr), `src/orchestrator/engine.rs:615-650` (run_handoff)

This is the bulk of the work. The approach:
1. Add a `send_and_log` helper at the top of the file
2. Add `runs_dir: &Path` parameter to public functions: `run_agent_phase`, `run_polling_phase`, `run_direct_phase`
3. Thread `runs_dir` to private functions: `run_ci_watch`, `run_bot_reviews`, `run_fix_agent`, `run_raise_pr`, `run_handoff`
4. Replace all 15 `event_tx.send(OrchestratorEvent::AgentOutput{...})` calls with `send_and_log`

- [ ] **Step 1: Add imports and the `send_and_log` helper**

At the top of `src/orchestrator/engine.rs`, add to the existing imports:

```rust
use std::path::Path;
use crate::state::agent_log;
```

Then add this helper function after the imports and before `run_agent_phase`:

```rust
/// Send an agent event to the TUI and log it to the agent transcript file.
async fn send_and_log(
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    issue_id: &str,
    event: AgentEvent,
) {
    agent_log::log_agent_event(runs_dir, issue_id, &event);
    let _ = event_tx
        .send(OrchestratorEvent::AgentOutput {
            issue_id: issue_id.to_string(),
            event,
        })
        .await;
}
```

- [ ] **Step 2: Update `run_agent_phase` signature and body**

Add `runs_dir: &Path` parameter after `event_tx` (before `retry_reason`):

```rust
pub async fn run_agent_phase(
    runner: &dyn AgentRunner,
    phase: &Phase,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    model: Option<&str>,
    permission_mode: Option<&str>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    retry_reason: Option<&str>,
    attempt: u8,
) -> Result<PhaseExecutionResult> {
```

Replace the phase-start send (lines 59-66):

```rust
    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!("[{phase}] Agent started (session: {session_id})\n")),
    )
    .await;
```

Replace the streaming loop send (lines 88-93):

```rust
        send_and_log(event_tx, runs_dir, issue_id, event).await;
```

- [ ] **Step 3: Update `run_polling_phase` signature**

Add `runs_dir: &Path` after `event_tx`:

```rust
pub async fn run_polling_phase(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    phase: &Phase,
    pr_number: u64,
    issue_id: &str,
    issue_title: &str,
    working_dir: &std::path::Path,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
```

Pass `runs_dir` through to both `run_ci_watch` and `run_bot_reviews` calls inside the match. Add `runs_dir` after `event_tx` in both calls.

- [ ] **Step 4: Update `run_ci_watch` signature and body**

Add `runs_dir: &Path` after `event_tx`:

```rust
async fn run_ci_watch(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    pr_number: u64,
    issue_id: &str,
    _issue_title: &str,
    working_dir: &std::path::Path,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
```

Replace all 5 `event_tx.send(OrchestratorEvent::AgentOutput{...})` calls in this function with `send_and_log(event_tx, runs_dir, issue_id, ...)`. Each one follows the same pattern — extract the `AgentEvent` and pass it to `send_and_log`.

Pass `runs_dir` to the `run_fix_agent` call.

- [ ] **Step 5: Update `run_bot_reviews` signature and body**

Add `runs_dir: &Path` after `event_tx`:

```rust
async fn run_bot_reviews(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    pr_number: u64,
    issue_id: &str,
    _issue_title: &str,
    working_dir: &std::path::Path,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
```

Replace all 3 `event_tx.send(OrchestratorEvent::AgentOutput{...})` calls with `send_and_log`. Pass `runs_dir` to the `run_fix_agent` call.

- [ ] **Step 6: Update `run_fix_agent` signature and body**

Add `runs_dir: &Path` after `event_tx`:

```rust
async fn run_fix_agent(
    runner: &dyn AgentRunner,
    config: SessionConfig,
    issue_id: &str,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<PhaseExecutionResult> {
```

Replace the streaming loop send with `send_and_log(event_tx, runs_dir, issue_id, event).await;`

- [ ] **Step 7: Update `run_direct_phase` signature**

Add `runs_dir: &Path` after `event_tx`:

```rust
pub async fn run_direct_phase(
    phase: &Phase,
    github: &GitHubClient,
    tracker: &dyn IssueTracker,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    branch: &str,
    pr: Option<&PrHandle>,
    cost_usd: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
```

Pass `runs_dir` to both `run_raise_pr` and `run_handoff` calls.

- [ ] **Step 8: Update `run_raise_pr` signature and body**

Add `runs_dir: &Path` after `event_tx`:

```rust
async fn run_raise_pr(
    github: &GitHubClient,
    tracker: &dyn IssueTracker,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    branch: &str,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
```

Replace all 3 `event_tx.send(OrchestratorEvent::AgentOutput{...})` calls with `send_and_log`.

- [ ] **Step 9: Update `run_handoff` signature and body**

Add `runs_dir: &Path` after `event_tx`:

```rust
async fn run_handoff(
    _tracker: &dyn IssueTracker,
    issue_id: &str,
    pr: Option<&PrHandle>,
    cost_usd: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
```

Replace the single `event_tx.send(OrchestratorEvent::AgentOutput{...})` with `send_and_log`.

- [ ] **Step 10: Verify it compiles (engine only — callers will break)**

Run: `cargo check 2>&1`
Expected: errors in `src/orchestrator/mod.rs` about missing `runs_dir` arguments. Engine itself should have no errors.

---

### Task 5: Update callers in `orchestrator/mod.rs`

**Files:**
- Modify: `src/orchestrator/mod.rs:340-460`

- [ ] **Step 1: Pass `runs_dir` to `run_agent_phase` calls**

At line 340, the first `run_agent_phase` call. Add `&runs_dir,` after `&event_tx,`:

```rust
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
                &runs_dir,
                None,
                0,
            )
            .await?;
```

At line 374, the retry `run_agent_phase` call. Same change — add `&runs_dir,` after `&event_tx,`:

```rust
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
                        &runs_dir,
                        Some(&reason),
                        attempt,
                    )
                    .await?;
```

- [ ] **Step 2: Pass `runs_dir` to `run_polling_phase`**

At line 422, add `&runs_dir,` after `&event_tx,`:

```rust
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
                        &runs_dir,
                        &cancel_token,
                    )
                    .await?;
```

- [ ] **Step 3: Pass `runs_dir` to `run_direct_phase`**

At line 446, add `&runs_dir,` after `&event_tx,`:

```rust
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
                    &runs_dir,
                )
                .await?;
```

- [ ] **Step 4: Verify full compilation**

Run: `cargo check 2>&1`
Expected: compiles cleanly with no errors or warnings

- [ ] **Step 5: Run all tests**

Run: `cargo test 2>&1`
Expected: all tests pass, including the new `state::agent_log` tests

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator/engine.rs src/orchestrator/mod.rs
git commit -m "feat: wire agent transcript logging through orchestrator engine"
```

---

### Task 6: End-to-end smoke test

- [ ] **Step 1: Verify log directory structure**

Run: `ls ~/.config/hive/logs/`
Expected: `hive.log.YYYY-MM-DD` file from the Task 3 smoke test

- [ ] **Step 2: Run hive briefly to generate logs**

Run: `cargo run -- status 2>/dev/null && cat ~/.config/hive/logs/hive.log.$(date +%Y-%m-%d) | head -20`
Expected: structured log lines with timestamps, levels, and targets

- [ ] **Step 3: Verify agent log path is correct**

Check that `runs_dir` resolves to `~/.config/hive/projects/{project}/runs/`. Agent log files will appear as `{issue_id}.agent.log` alongside the existing `{issue_id}.json` files once a story run executes.

- [ ] **Step 4: Final commit if any fixups needed**

Only if prior steps revealed issues that needed fixing.
