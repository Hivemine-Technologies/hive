# PR Watch Phase Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace manual rebase with an automatic `PrWatch` polling phase that monitors PRs until merge/close, auto-rebases on conflicts (using an LLM agent for conflict resolution), force-pushes fixes, and cleans up worktrees on completion.

**Architecture:** Add `PrWatch` as a new polling phase after `Handoff` in the pipeline. It polls the GitHub PR every 5 minutes, checking `mergeable` status. On `mergeable: false` (real conflicts, NOT just branch behind), it attempts `git rebase`, falls back to an LLM agent for conflict resolution, then force-pushes. On merge or close, it transitions to `Complete` and removes the worktree. Remove all manual rebase infrastructure (`TuiCommand::RebaseStory`, keybindings, orchestrator handler).

**Tech Stack:** Rust, tokio, octocrab 0.49 (GitHub API), ratatui (TUI)

---

### Task 1: Add `PrWatch` Phase Variant

**Files:**
- Modify: `src/domain/phase.rs`

- [ ] **Step 1: Write test for PrWatch in pipeline order**

Add a test that verifies PrWatch appears after Handoff in the pipeline:

```rust
#[test]
fn test_pr_watch_in_pipeline() {
    let phases = Phase::all_in_order();
    let handoff_idx = phases.iter().position(|p| matches!(p, Phase::Handoff)).unwrap();
    let pr_watch_idx = phases.iter().position(|p| matches!(p, Phase::PrWatch)).unwrap();
    assert_eq!(pr_watch_idx, handoff_idx + 1);
}

#[test]
fn test_pr_watch_is_polling_phase() {
    assert!(Phase::PrWatch.is_polling_phase());
}

#[test]
fn test_pr_watch_config_key() {
    assert_eq!(Phase::PrWatch.config_key(), "pr-watch");
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib domain::phase`
Expected: FAIL — `PrWatch` variant doesn't exist yet.

- [ ] **Step 3: Add PrWatch variant to Phase enum**

In `src/domain/phase.rs`, add `PrWatch` to the enum:

```rust
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    PrWatch,        // NEW
    Complete,
    NeedsAttention { reason: String },
}
```

Add `PrWatch` to `PIPELINE_PHASES` after `Handoff`:

```rust
const PIPELINE_PHASES: &[fn() -> Phase] = &[
    || Phase::Understand,
    || Phase::Implement,
    || Phase::SelfReview { attempt: 0 },
    || Phase::CrossReview,
    || Phase::RaisePr,
    || Phase::CiWatch { attempt: 0 },
    || Phase::BotReviews { cycle: 0 },
    || Phase::FollowUps,
    || Phase::Handoff,
    || Phase::PrWatch,      // NEW
];
```

Update all match arms in the `Phase` impl:

```rust
// In config_key()
Phase::PrWatch => "pr-watch",

// In is_polling_phase()
pub fn is_polling_phase(&self) -> bool {
    matches!(self, Phase::CiWatch { .. } | Phase::BotReviews { .. } | Phase::PrWatch)
}

// In Display
Phase::PrWatch => write!(f, "PR Watch"),
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test --lib domain::phase`
Expected: All pass, including the 3 new tests.

- [ ] **Step 5: Run clippy**

Run: `cargo clippy 2>&1 | head -30`
Expected: May show warnings about non-exhaustive matches in other files — that's expected and will be fixed in later tasks. Confirm no errors in `phase.rs` itself.

- [ ] **Step 6: Commit**

```bash
git add src/domain/phase.rs
git commit -m "feat: add PrWatch phase variant to pipeline after Handoff"
```

---

### Task 2: Update Phase Transitions

**Files:**
- Modify: `src/orchestrator/transitions.rs`

- [ ] **Step 1: Write test for advance through PrWatch**

```rust
#[test]
fn test_advance_from_handoff_to_pr_watch() {
    let phases_config = HashMap::new();
    let next = advance(Phase::Handoff, &phases_config);
    assert_eq!(next, Phase::PrWatch);
}

#[test]
fn test_advance_from_pr_watch_to_complete() {
    let phases_config = HashMap::new();
    let next = advance(Phase::PrWatch, &phases_config);
    assert_eq!(next, Phase::Complete);
}

#[test]
fn test_advance_skips_disabled_pr_watch() {
    let mut phases_config = HashMap::new();
    phases_config.insert(
        "pr-watch".to_string(),
        PhaseConfig {
            enabled: false,
            runner: None,
            model: None,
            max_attempts: None,
            poll_interval: None,
            max_fix_attempts: None,
            max_fix_cycles: None,
            fix_runner: None,
            fix_model: None,
            wait_for: None,
        },
    );
    let next = advance(Phase::Handoff, &phases_config);
    assert_eq!(next, Phase::Complete);
}
```

- [ ] **Step 2: Run tests to verify they pass**

Run: `cargo test --lib orchestrator::transitions`
Expected: All pass — `advance()` and `next_enabled_phase()` already use `PIPELINE_PHASES` and `config_key()`, so PrWatch is automatically included. These tests confirm the wiring works.

- [ ] **Step 3: Update existing test assertion for pipeline end**

The existing `test_next_phase_after_handoff_is_complete` now fails because Handoff is no longer the last pipeline phase. Update it:

```rust
#[test]
fn test_next_phase_after_handoff_is_pr_watch() {
    let phases_config = HashMap::new();
    let next = next_enabled_phase(&Phase::Handoff, &phases_config);
    assert_eq!(next, Some(Phase::PrWatch));
}

#[test]
fn test_next_phase_after_pr_watch_is_none() {
    let phases_config = HashMap::new();
    let next = next_enabled_phase(&Phase::PrWatch, &phases_config);
    assert_eq!(next, None);
}
```

- [ ] **Step 4: Run all transition tests**

Run: `cargo test --lib orchestrator::transitions`
Expected: All pass.

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator/transitions.rs
git commit -m "feat: wire PrWatch into phase transition pipeline after Handoff"
```

---

### Task 3: Add `PrStatus` Enum and GitHub Client Methods

**Files:**
- Modify: `src/git/github.rs`

- [ ] **Step 1: Add PrStatus enum**

At the bottom of `src/git/github.rs`, near the existing `CiStatus` enum:

```rust
#[derive(Debug, PartialEq)]
pub enum PrStatus {
    /// PR was merged into the base branch.
    Merged,
    /// PR was closed without merging.
    Closed,
    /// PR is open and clean (no conflicts), or mergeability not yet computed.
    Clean,
    /// PR is open but has merge conflicts (GitHub's mergeable == false).
    Conflicts,
}
```

- [ ] **Step 2: Add `poll_pr_status` method to GitHubClient**

Add this method to the `impl GitHubClient` block:

```rust
pub async fn poll_pr_status(&self, pr_number: u64) -> Result<PrStatus> {
    let pr = self
        .octocrab
        .pulls(&self.owner, &self.repo)
        .get(pr_number)
        .await
        .map_err(|e| HiveError::GitHub(e.to_string()))?;

    // Check merged first — a merged PR also has state=Closed
    if pr.merged_at.is_some() {
        return Ok(PrStatus::Merged);
    }

    // Closed without merge
    if matches!(pr.state, Some(octocrab::models::IssueState::Closed)) {
        return Ok(PrStatus::Closed);
    }

    // Open — check mergeability
    // GitHub computes mergeable async; None means "not yet computed"
    // Only trigger rebase when GitHub definitively says false (real conflicts)
    match pr.mergeable {
        Some(false) => Ok(PrStatus::Conflicts),
        _ => Ok(PrStatus::Clean), // Some(true) or None — both mean "no action needed"
    }
}
```

- [ ] **Step 3: Add `force_push_current_branch` method to GitHubClient**

Add this method to the `impl GitHubClient` block, near the existing `push_current_branch`:

```rust
pub async fn force_push_current_branch(
    &self,
    worktree_path: &std::path::Path,
) -> Result<()> {
    let output = std::process::Command::new("git")
        .args(["push", "--force-with-lease"])
        .current_dir(worktree_path)
        .output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(HiveError::Git(git2::Error::from_str(&format!(
            "force push failed: {stderr}"
        ))));
    }
    Ok(())
}
```

- [ ] **Step 4: Verify it compiles**

Run: `cargo build 2>&1 | head -20`
Expected: Compiles (may have warnings about unused code — fine for now).

- [ ] **Step 5: Commit**

```bash
git add src/git/github.rs
git commit -m "feat: add poll_pr_status and force_push_current_branch to GitHubClient"
```

---

### Task 4: Add Rebase Conflict Resolution Prompt

**Files:**
- Modify: `src/orchestrator/prompts.rs`

- [ ] **Step 1: Write test for the prompt**

```rust
#[test]
fn test_rebase_conflict_prompt() {
    let prompt = build_rebase_conflict_prompt("APX-245", "Add NumberSequence", "main");
    assert!(prompt.contains("APX-245"));
    assert!(prompt.contains("Add NumberSequence"));
    assert!(prompt.contains("origin/main"));
    assert!(prompt.contains("git rebase"));
    assert!(prompt.contains("rebase --continue"));
    assert!(prompt.contains("verification command"));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test --lib orchestrator::prompts::tests::test_rebase_conflict_prompt`
Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Implement `build_rebase_conflict_prompt`**

Add to `src/orchestrator/prompts.rs`:

```rust
/// Build a prompt for an agent to resolve merge conflicts via interactive rebase.
pub fn build_rebase_conflict_prompt(
    issue_id: &str,
    issue_title: &str,
    default_branch: &str,
) -> String {
    format!(
        "You are resolving merge conflicts for story {issue_id}: {issue_title}.\n\n\
         The branch has fallen behind origin/{default_branch} and has merge conflicts.\n\n\
         ## Approach\n\
         1. Run `git fetch origin` to get the latest changes\n\
         2. Run `git rebase origin/{default_branch}`\n\
         3. When conflicts appear, resolve them by understanding the intent of BOTH sides:\n\
            - YOUR changes (the feature branch) implement the story requirements\n\
            - THEIR changes (from {default_branch}) may have refactored, renamed, or \
            restructured code — respect those changes\n\
            - Merge both intents correctly — don't just pick one side blindly\n\
         4. After resolving each conflicting file: `git add <file>` then `git rebase --continue`\n\
         5. Repeat steps 3-4 until the rebase completes successfully\n\
         6. Run the project's verification command to ensure nothing is broken\n\
         7. If verification fails, fix the issues and commit: \
            `fix({issue_id}): resolve rebase conflicts`"
    )
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test --lib orchestrator::prompts::tests::test_rebase_conflict_prompt`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add src/orchestrator/prompts.rs
git commit -m "feat: add rebase conflict resolution prompt for PrWatch agent"
```

---

### Task 5: Implement `run_pr_watch` Polling Function

**Files:**
- Modify: `src/orchestrator/engine.rs`

- [ ] **Step 1: Add PrWatch to `run_polling_phase` dispatch**

Update the `run_polling_phase` function signature to accept `default_branch`, and add the PrWatch match arm:

```rust
pub async fn run_polling_phase(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    phase: &Phase,
    pr_number: u64,
    issue_id: &str,
    issue_title: &str,
    working_dir: &std::path::Path,
    default_branch: &str,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
    match phase {
        Phase::CiWatch { .. } => {
            run_ci_watch(
                github, runner, pr_number, issue_id, issue_title, working_dir,
                phase_config, event_tx, runs_dir, cancel_token,
            )
            .await
        }
        Phase::BotReviews { .. } => {
            run_bot_reviews(
                github, runner, pr_number, issue_id, issue_title, working_dir,
                phase_config, event_tx, runs_dir, cancel_token,
            )
            .await
        }
        Phase::PrWatch => {
            run_pr_watch(
                github, runner, pr_number, issue_id, issue_title, working_dir,
                default_branch, phase_config, event_tx, runs_dir, cancel_token,
            )
            .await
        }
        _ => Err(HiveError::Phase {
            phase: phase.to_string(),
            message: "not a polling phase".to_string(),
        }),
    }
}
```

- [ ] **Step 2: Implement `run_pr_watch`**

Add the `run_pr_watch` function in `src/orchestrator/engine.rs`, after `run_bot_reviews`. Import `PrStatus` at the top with the existing github imports:

```rust
use crate::git::github::{CiStatus, GitHubClient, PrStatus, ReviewComment};
```

Then the function:

```rust
async fn run_pr_watch(
    github: &GitHubClient,
    runner: &dyn AgentRunner,
    pr_number: u64,
    issue_id: &str,
    issue_title: &str,
    working_dir: &std::path::Path,
    default_branch: &str,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
    // PrWatch defaults to 5-minute polling (vs 30s for CI phases)
    let poll_interval = phase_config
        .and_then(|c| c.poll_interval.as_deref())
        .map(|s| parse_poll_interval(Some(s)))
        .unwrap_or(Duration::from_secs(300));
    let max_rebase_attempts = phase_config
        .and_then(|c| c.max_fix_attempts)
        .unwrap_or(3);
    let fix_model = phase_config.and_then(|c| c.fix_model.as_deref());

    let mut rebase_attempts: u8 = 0;
    let mut total_cost = 0.0;
    let mut interval = tokio::time::interval(poll_interval);

    loop {
        tokio::select! {
            _ = cancel_token.cancelled() => {
                return Ok(PhaseExecutionResult {
                    outcome: PhaseOutcome::Failed { reason: "Cancelled".to_string() },
                    cost_usd: total_cost,
                    session_id: None,
                });
            }
            _ = interval.tick() => {}
        }

        send_and_log(
            event_tx, runs_dir, issue_id,
            AgentEvent::TextDelta(format!(
                "[PR Watch] Checking PR #{pr_number} status...\n"
            )),
        )
        .await;

        let pr_status = github.poll_pr_status(pr_number).await?;

        match pr_status {
            PrStatus::Merged => {
                send_and_log(
                    event_tx, runs_dir, issue_id,
                    AgentEvent::TextDelta(
                        "[PR Watch] PR merged! Story complete.\n".to_string()
                    ),
                )
                .await;
                return Ok(PhaseExecutionResult {
                    outcome: PhaseOutcome::Success,
                    cost_usd: total_cost,
                    session_id: None,
                });
            }
            PrStatus::Closed => {
                send_and_log(
                    event_tx, runs_dir, issue_id,
                    AgentEvent::TextDelta(
                        "[PR Watch] PR closed without merge.\n".to_string()
                    ),
                )
                .await;
                return Ok(PhaseExecutionResult {
                    outcome: PhaseOutcome::Success,
                    cost_usd: total_cost,
                    session_id: None,
                });
            }
            PrStatus::Conflicts => {
                if rebase_attempts >= max_rebase_attempts {
                    return Ok(PhaseExecutionResult {
                        outcome: PhaseOutcome::NeedsAttention {
                            reason: format!(
                                "Rebase attempts exhausted ({rebase_attempts}/{max_rebase_attempts})"
                            ),
                        },
                        cost_usd: total_cost,
                        session_id: None,
                    });
                }
                rebase_attempts += 1;

                send_and_log(
                    event_tx, runs_dir, issue_id,
                    AgentEvent::TextDelta(format!(
                        "[PR Watch] Merge conflicts detected. \
                         Rebase attempt {rebase_attempts}/{max_rebase_attempts}...\n"
                    )),
                )
                .await;

                // Try a clean rebase first (no agent cost if no conflicts)
                let rebase_result = crate::git::worktree::rebase_worktree(
                    working_dir, default_branch,
                )?;

                match rebase_result {
                    crate::git::worktree::RebaseResult::Success => {
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Rebase succeeded cleanly. Force pushing...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                        github.force_push_current_branch(working_dir).await?;
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Force push complete. Resuming watch...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                    crate::git::worktree::RebaseResult::Conflicts => {
                        // Rebase aborted — spawn agent to resolve conflicts
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Conflicts found. Spawning agent to resolve...\n"
                                    .to_string(),
                            ),
                        )
                        .await;

                        let fix_prompt = prompts::build_rebase_conflict_prompt(
                            issue_id, issue_title, default_branch,
                        );
                        let fix_config = SessionConfig {
                            working_dir: working_dir.to_path_buf(),
                            system_prompt: fix_prompt,
                            model: fix_model.map(|s| s.to_string()),
                            permission_mode: None,
                        };

                        let fix_result = run_fix_agent(
                            runner, fix_config, issue_id, event_tx, runs_dir,
                        )
                        .await?;
                        total_cost += fix_result.cost_usd;

                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Agent completed. Force pushing...\n".to_string(),
                            ),
                        )
                        .await;
                        github.force_push_current_branch(working_dir).await?;
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Force push complete. Resuming watch...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                    crate::git::worktree::RebaseResult::Failed => {
                        return Ok(PhaseExecutionResult {
                            outcome: PhaseOutcome::NeedsAttention {
                                reason: "Git fetch failed during rebase (network issue?)"
                                    .to_string(),
                            },
                            cost_usd: total_cost,
                            session_id: None,
                        });
                    }
                }
            }
            PrStatus::Clean => {
                // PR is clean or mergeability not yet computed — keep watching
                continue;
            }
        }
    }
}
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo build 2>&1 | head -30`
Expected: Build errors in `mod.rs` at the `run_polling_phase` call site (missing `default_branch` argument). This is expected and fixed in Task 6.

- [ ] **Step 4: Commit**

```bash
git add src/orchestrator/engine.rs
git commit -m "feat: implement run_pr_watch polling function with auto-rebase"
```

---

### Task 6: Wire PrWatch into the Orchestrator

**Files:**
- Modify: `src/orchestrator/mod.rs`

- [ ] **Step 1: Update `run_polling_phase` call site to pass `default_branch`**

In `story_phase_loop`, find the `run_polling_phase` call (around line 461) and add the `default_branch` argument:

```rust
let result = run_polling_phase(
    g.as_ref(),
    runner.as_ref(),
    &run.phase,
    pr_number,
    &issue_id,
    &issue_detail.title,
    working_dir,
    &config.github.default_branch,
    phase_config,
    &event_tx,
    &runs_dir,
    &cancel_token,
)
.await?;
```

- [ ] **Step 2: Add worktree cleanup after PrWatch completes**

In `story_phase_loop`, inside the `PhaseOutcome::Success | PhaseOutcome::Skipped` handler, after the phase transition and before the notification, add worktree cleanup when transitioning from PrWatch to Complete:

```rust
PhaseOutcome::Success | PhaseOutcome::Skipped => {
    let old_phase = run.phase.clone();
    let next = advance(run.phase.clone(), &config.phases);
    run.phase = next.clone();
    run.updated_at = Utc::now();

    let _ = event_tx
        .send(OrchestratorEvent::PhaseTransition {
            issue_id: issue_id.clone(),
            from: old_phase.clone(),
            to: next.clone(),
        })
        .await;

    if matches!(next, Phase::Complete) {
        run.status = RunStatus::Complete;

        // Cleanup worktree after PrWatch (PR merged or closed)
        if matches!(old_phase, Phase::PrWatch) {
            if run.worktree.is_some() {
                let repo_path = std::path::Path::new(&*config.repo_path);
                let worktree_dir = repo_path.join(&config.worktree_dir);
                match crate::git::worktree::remove_worktree(
                    repo_path, &issue_id, &worktree_dir,
                ) {
                    Ok(()) => tracing::info!("Cleaned up worktree for {issue_id}"),
                    Err(e) => tracing::warn!(
                        "Failed to cleanup worktree for {issue_id}: {e}"
                    ),
                }
                run.worktree = None;
            }
        }

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
```

- [ ] **Step 3: Remove `rebase_story` handler**

Delete the `rebase_story` method from `impl Orchestrator` (lines 276-284):

```rust
// DELETE this entire method:
async fn rebase_story(&mut self, issue_id: &str) -> Result<()> {
    if let Some(run) = self.runs.get(issue_id) {
        if let Some(ref wt_path) = run.worktree {
            let result = crate::git::worktree::rebase_worktree(wt_path, &self.config.github.default_branch)?;
            tracing::info!("rebase result for {issue_id}: {result:?}");
        }
    }
    Ok(())
}
```

- [ ] **Step 4: Remove `RebaseStory` from the command loop**

In `Orchestrator::run()`, remove the `RebaseStory` match arm:

```rust
// DELETE these lines from the match in run():
TuiCommand::RebaseStory { issue_id } => {
    self.rebase_story(&issue_id).await?;
}
```

- [ ] **Step 5: Verify it compiles**

Run: `cargo build 2>&1 | head -30`
Expected: Build errors in TUI code referencing `TuiCommand::RebaseStory` — fixed in Task 7.

- [ ] **Step 6: Commit**

```bash
git add src/orchestrator/mod.rs
git commit -m "feat: wire PrWatch into orchestrator with worktree cleanup on completion"
```

---

### Task 7: Remove Manual Rebase from TUI and Events

**Files:**
- Modify: `src/domain/events.rs`
- Modify: `src/tui/mod.rs`
- Modify: `src/tui/tabs/agents.rs`

- [ ] **Step 1: Remove `RebaseStory` from `TuiCommand`**

In `src/domain/events.rs`, remove the variant:

```rust
#[derive(Debug, Clone)]
pub enum TuiCommand {
    StartStory { issue: Issue },
    CancelStory { issue_id: String },
    RetryStory { issue_id: String },
    // RebaseStory REMOVED
    CopyWorktreePath,
    Quit,
}
```

- [ ] **Step 2: Remove rebase keybinding from Agents tab**

In `src/tui/mod.rs`, in the `handle_agents_key` method, find the `KeyCode::Char('r')` block that sends `RebaseStory` and remove it entirely:

```rust
// DELETE this block (around line 318-325):
KeyCode::Char('r') => {
    if let Some(id) = selected_issue_id {
        let _ = self
            .command_tx
            .send(TuiCommand::RebaseStory { issue_id: id })
            .await;
    }
}
```

- [ ] **Step 3: Remove rebase keybinding from Worktrees tab**

In `src/tui/mod.rs`, in `handle_worktrees_key`, find the `KeyCode::Char('R')` block that sends `RebaseStory` and remove it entirely (around line 466-479).

- [ ] **Step 4: Update help overlay**

In `src/tui/mod.rs`, remove the rebase help entries:

For the Agents tab help (around line 185), remove:
```rust
// DELETE:
Line::from(vec![Span::styled("r   ", ...), Span::raw("Rebase selected worktree")]),
```

For the Worktrees tab help (around line 205), remove:
```rust
// DELETE:
Line::from(vec![Span::styled("R   ", ...), Span::raw("Rebase selected worktree")]),
```

- [ ] **Step 5: Remove rebase hint from agents tab bar**

In `src/tui/tabs/agents.rs`, find the hint bar spans (around line 238-239) and remove the rebase hint:

```rust
// DELETE these two spans:
Span::styled("r", Style::default().fg(Color::Cyan)),
Span::raw(" rebase  "),
```

- [ ] **Step 6: Verify full build**

Run: `cargo build`
Expected: Clean build with no errors.

- [ ] **Step 7: Run all tests**

Run: `cargo test`
Expected: All tests pass.

- [ ] **Step 8: Run clippy**

Run: `cargo clippy`
Expected: No warnings or errors.

- [ ] **Step 9: Commit**

```bash
git add src/domain/events.rs src/tui/mod.rs src/tui/tabs/agents.rs
git commit -m "refactor: remove manual rebase in favor of automatic PrWatch phase"
```

---

### Task 8: Add PrWatch Default Config Documentation

**Files:**
- Modify: `CLAUDE.md`

- [ ] **Step 1: Update the Phase Pipeline section in CLAUDE.md**

Update the phase pipeline documentation to include PrWatch:

```
Queued -> Understand -> Implement -> SelfReview -> CrossReview -> RaisePr -> CiWatch -> BotReviews -> FollowUps -> Handoff -> PrWatch -> Complete
```

Add PrWatch to the polling phases description:

```
- **Polling phases** (CiWatch, BotReviews, PrWatch): poll GitHub on an interval, spawn fix agents when issues are found
```

Add a brief description of PrWatch behavior:

```
PrWatch polls the PR every 5 minutes (configurable via `poll_interval`). When GitHub reports merge conflicts (`mergeable: false`), it attempts a clean rebase. If the rebase has conflicts, it spawns an agent to resolve them interactively, then force-pushes. When the PR is merged or closed, the worktree is automatically cleaned up.
```

- [ ] **Step 2: Update phase config keys list**

Add `pr-watch` to the kebab-case config keys list:

```
- Phase config keys use kebab-case: `self-review`, `cross-review`, `ci-watch`, `bot-reviews`, `follow-ups`, `raise-pr`, `pr-watch`.
```

- [ ] **Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: document PrWatch phase in CLAUDE.md"
```
