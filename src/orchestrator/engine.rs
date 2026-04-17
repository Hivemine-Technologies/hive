use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::PhaseConfig;
use crate::domain::story_run::PrHandle;
use crate::domain::{AgentEvent, OrchestratorEvent, Phase, PhaseOutcome};
use crate::error::{HiveError, Result};
use crate::git::github::{CiStatus, GitHub, PrStatus, ReviewComment};
use crate::runners::{AgentRunner, SessionConfig};
use crate::state::agent_log;
use crate::trackers::IssueTracker;

use super::prompts;

/// Send an agent event to the TUI and log it to the agent transcript file.
/// Logs to disk first so the transcript survives even if the process crashes
/// before the channel send completes.
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

/// Outcome from executing a single phase.
#[derive(Debug)]
pub struct PhaseExecutionResult {
    pub outcome: PhaseOutcome,
    pub cost_usd: f64,
    pub session_id: Option<String>,
}

/// Execute an agent phase (Understand, Implement, SelfReview, CrossReview, FollowUps).
///
/// Starts an agent session, streams output to the TUI via event_tx, and
/// waits for completion.
#[allow(clippy::too_many_arguments)]
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
    // Build prompt
    let system_prompt = if let Some(reason) = retry_reason {
        prompts::build_retry_prompt(phase, issue_id, issue_title, reason, attempt)
    } else {
        prompts::build_phase_prompt(phase, issue_id, issue_title, issue_description)
    };

    let config = SessionConfig {
        working_dir: working_dir.to_path_buf(),
        system_prompt,
        model: model.map(|s| s.to_string()),
        permission_mode: permission_mode.map(|s| s.to_string()),
    };

    // Start session
    let handle = runner.start_session(config).await?;
    let session_id = handle.session_id.clone();

    // Notify TUI of phase start
    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!("[{phase}] Agent started (session: {session_id})\n")),
    )
    .await;

    // Consume output stream
    let mut stream = runner.output_stream(&handle);
    let mut total_cost = 0.0;
    let mut completed = false;
    let mut error_msg: Option<String> = None;

    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        match &event {
            AgentEvent::Complete { cost_usd } => {
                total_cost = *cost_usd;
                completed = true;
            }
            AgentEvent::Error(msg) => {
                error_msg = Some(msg.clone());
            }
            _ => {}
        }

        // Forward all events to TUI
        send_and_log(event_tx, runs_dir, issue_id, event).await;
    }

    // Determine outcome
    let outcome = if let Some(err) = error_msg {
        PhaseOutcome::Failed { reason: err }
    } else if completed {
        PhaseOutcome::Success
    } else {
        PhaseOutcome::Failed {
            reason: "Agent stream ended without completion event".to_string(),
        }
    };

    Ok(PhaseExecutionResult {
        outcome,
        cost_usd: total_cost,
        session_id: Some(session_id),
    })
}

/// Resolve the runner and model for a given phase from config.
pub fn resolve_phase_runner_config<'a>(
    _phase: &Phase,
    phase_config: Option<&'a PhaseConfig>,
) -> (Option<&'a str>, Option<&'a str>) {
    match phase_config {
        Some(config) => (config.runner.as_deref(), config.model.as_deref()),
        None => (None, None),
    }
}

/// Get max attempts for a phase (default varies by phase type).
pub fn max_attempts_for_phase(phase: &Phase, phase_config: Option<&PhaseConfig>) -> u8 {
    if let Some(config) = phase_config
        && let Some(max) = config.max_attempts {
        return max;
    }
    // Defaults per spec
    match phase {
        Phase::SelfReview { .. } => 3,
        Phase::Understand | Phase::Implement => 1,
        Phase::CrossReview | Phase::FollowUps => 1,
        _ => 1,
    }
}

// ---------------------------------------------------------------------------
// Polling phases (CiWatch, BotReviews, PrWatch)
// ---------------------------------------------------------------------------

/// Execute a polling phase (CiWatch or BotReviews).
///
/// Polls GitHub on an interval. When issues are detected (CI failure,
/// new bot comments), spawns a fix agent and retries.
#[allow(clippy::too_many_arguments)]
pub async fn run_polling_phase(
    github: &dyn GitHub,
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
    git_ops: &dyn crate::git::worktree::GitOps,
) -> Result<PhaseExecutionResult> {
    match phase {
        Phase::CiWatch { .. } => {
            run_ci_watch(
                github,
                runner,
                pr_number,
                issue_id,
                issue_title,
                working_dir,
                phase_config,
                event_tx,
                runs_dir,
                cancel_token,
            )
            .await
        }
        Phase::BotReviews { .. } => {
            run_bot_reviews(
                github,
                runner,
                pr_number,
                issue_id,
                issue_title,
                working_dir,
                phase_config,
                event_tx,
                runs_dir,
                cancel_token,
            )
            .await
        }
        Phase::PrWatch => {
            run_pr_watch(
                github, runner, pr_number, issue_id, issue_title, working_dir,
                default_branch, phase_config, event_tx, runs_dir, cancel_token,
                git_ops,
            )
            .await
        }
        _ => Err(HiveError::Phase {
            phase: phase.to_string(),
            message: "not a polling phase".to_string(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_ci_watch(
    github: &dyn GitHub,
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
    let poll_interval =
        parse_poll_interval(phase_config.and_then(|c| c.poll_interval.as_deref()));
    let max_fix_attempts = phase_config
        .and_then(|c| c.max_fix_attempts)
        .unwrap_or(3);
    let fix_model = phase_config.and_then(|c| c.fix_model.as_deref());

    let mut fix_attempts: u8 = 0;
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
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(format!(
                "[CI Watch] Polling CI status for PR #{pr_number}...\n"
            )),
        )
        .await;

        let ci_status = github.poll_ci(pr_number).await?;

        match ci_status {
            CiStatus::Passed => {
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta("[CI Watch] CI passed!\n".to_string()),
                )
                .await;
                return Ok(PhaseExecutionResult {
                    outcome: PhaseOutcome::Success,
                    cost_usd: total_cost,
                    session_id: None,
                });
            }
            CiStatus::Pending => {
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta("[CI Watch] CI still pending...\n".to_string()),
                )
                .await;
                continue;
            }
            CiStatus::Failed { failures } => {
                if fix_attempts >= max_fix_attempts {
                    return Ok(PhaseExecutionResult {
                        outcome: PhaseOutcome::NeedsAttention {
                            reason: format!(
                                "CI fix attempts exhausted ({fix_attempts}/{max_fix_attempts}). Failures: {}",
                                failures.join(", ")
                            ),
                        },
                        cost_usd: total_cost,
                        session_id: None,
                    });
                }

                fix_attempts += 1;
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta(format!(
                        "[CI Watch] CI failed. Spawning fix agent (attempt {fix_attempts}/{max_fix_attempts})...\n"
                    )),
                )
                .await;

                let fix_prompt = prompts::build_ci_fix_prompt(issue_id, &failures);
                let fix_config = SessionConfig {
                    working_dir: working_dir.to_path_buf(),
                    system_prompt: fix_prompt,
                    model: fix_model.map(|s| s.to_string()),
                    permission_mode: None,
                };

                let fix_result =
                    run_fix_agent(runner, fix_config, issue_id, event_tx, runs_dir).await?;
                total_cost += fix_result.cost_usd;

                if !matches!(fix_result.outcome, PhaseOutcome::Success) {
                    send_and_log(
                        event_tx, runs_dir, issue_id,
                        AgentEvent::TextDelta(
                            "[CI Watch] Fix agent failed — not pushing. Will retry on next poll...\n"
                                .to_string(),
                        ),
                    )
                    .await;
                    continue;
                }

                // Push fixes so CI picks them up. Force-with-lease is
                // appropriate here: CiWatch owns this fix commit and the
                // lease protects against stomping unexpected remote updates.
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta(
                        "[CI Watch] Fix agent completed. Force-pushing fixes...\n".to_string(),
                    ),
                )
                .await;
                github.force_push_current_branch(working_dir).await?;

                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta(
                        "[CI Watch] Pushed. Resuming CI polling...\n".to_string(),
                    ),
                )
                .await;
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_bot_reviews(
    github: &dyn GitHub,
    runner: &dyn AgentRunner,
    pr_number: u64,
    issue_id: &str,
    issue_title: &str,
    working_dir: &std::path::Path,
    phase_config: Option<&PhaseConfig>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
    cancel_token: &tokio_util::sync::CancellationToken,
) -> Result<PhaseExecutionResult> {
    let poll_interval =
        parse_poll_interval(phase_config.and_then(|c| c.poll_interval.as_deref()));
    let max_fix_cycles = phase_config
        .and_then(|c| c.max_fix_cycles)
        .unwrap_or(3);
    let wait_for: Vec<String> = phase_config
        .and_then(|c| c.wait_for.clone())
        .unwrap_or_default();
    let fix_model = phase_config.and_then(|c| c.fix_model.as_deref());

    let mut fix_cycles: u8 = 0;
    let mut total_cost = 0.0;
    let mut seen_comment_ids: std::collections::HashSet<String> =
        std::collections::HashSet::new();
    let mut quiet_polls: u8 = 0;
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
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(format!(
                "[Bot Reviews] Polling reviews for PR #{pr_number}...\n"
            )),
        )
        .await;

        let comments = github.poll_reviews(pr_number).await?;

        // Filter for bot comments from the wait_for list
        let new_bot_comments: Vec<&ReviewComment> = comments
            .iter()
            .filter(|c| {
                c.is_bot
                    && !seen_comment_ids.contains(&c.id)
                    && (wait_for.is_empty()
                        || wait_for
                            .iter()
                            .any(|w| c.author.to_lowercase().contains(&w.to_lowercase())))
            })
            .collect();

        // Track all comment IDs
        for comment in &comments {
            seen_comment_ids.insert(comment.id.clone());
        }

        if new_bot_comments.is_empty() {
            quiet_polls += 1;
            if quiet_polls >= 2 {
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta(
                        "[Bot Reviews] No new bot comments after 2 quiet polls. Done.\n"
                            .to_string(),
                    ),
                )
                .await;
                return Ok(PhaseExecutionResult {
                    outcome: PhaseOutcome::Success,
                    cost_usd: total_cost,
                    session_id: None,
                });
            }
            continue;
        }

        // Reset quiet counter on new comments
        quiet_polls = 0;

        if fix_cycles >= max_fix_cycles {
            return Ok(PhaseExecutionResult {
                outcome: PhaseOutcome::NeedsAttention {
                    reason: format!(
                        "Bot review fix cycles exhausted ({fix_cycles}/{max_fix_cycles})"
                    ),
                },
                cost_usd: total_cost,
                session_id: None,
            });
        }

        fix_cycles += 1;
        let comment_bodies: Vec<String> = new_bot_comments
            .iter()
            .map(|c| format!("[{}] {}", c.author, c.body))
            .collect();

        send_and_log(
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(format!(
                "[Bot Reviews] {} new bot comment(s). Spawning fix agent (cycle {fix_cycles}/{max_fix_cycles})...\n",
                new_bot_comments.len()
            )),
        )
        .await;

        let fix_prompt = prompts::build_bot_review_fix_prompt(issue_id, issue_title, &comment_bodies);
        let fix_config = SessionConfig {
            working_dir: working_dir.to_path_buf(),
            system_prompt: fix_prompt,
            model: fix_model.map(|s| s.to_string()),
            permission_mode: None,
        };

        let fix_result = run_fix_agent(runner, fix_config, issue_id, event_tx, runs_dir).await?;
        total_cost += fix_result.cost_usd;

        if !matches!(fix_result.outcome, PhaseOutcome::Success) {
            send_and_log(
                event_tx, runs_dir, issue_id,
                AgentEvent::TextDelta(
                    "[Bot Reviews] Fix agent failed — not pushing or replying. \
                     Will retry on next poll...\n"
                        .to_string(),
                ),
            )
            .await;
            continue;
        }

        // Push fixes so bot reviewers see updated code. Force-with-lease
        // so a branch that diverged (e.g. stuck from a prior failed rebase)
        // still advances, while the lease prevents clobbering unexpected
        // remote commits. See A#3 retry for the stale-lease race path.
        send_and_log(
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(
                "[Bot Reviews] Fix agent completed. Force-pushing fixes...\n".to_string(),
            ),
        )
        .await;
        github.force_push_current_branch(working_dir).await?;

        // Reply to each comment and post summary on the PR
        send_and_log(
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(
                "[Bot Reviews] Acknowledging addressed comments on PR...\n".to_string(),
            ),
        )
        .await;

        let mut inline_replied = 0u32;
        let mut summary_items: Vec<String> = Vec::new();

        for comment in &new_bot_comments {
            if let Some(numeric_id) = comment.id.strip_prefix("inline-") {
                if let Ok(cid) = numeric_id.parse::<u64>() {
                    match github
                        .reply_to_inline_comment(pr_number, cid, "Addressed in latest push.")
                        .await
                    {
                        Ok(()) => inline_replied += 1,
                        Err(e) => tracing::warn!(
                            "Failed to reply to inline comment {cid} on PR #{pr_number}: {e}"
                        ),
                    }
                }
            } else {
                // Review body or issue comment — collect for summary
                let preview: String = comment.body.chars().take(80).collect();
                summary_items.push(format!(
                    "- **{}**: {}{}",
                    comment.author,
                    preview,
                    if comment.body.len() > 80 { "..." } else { "" }
                ));
            }
        }

        if !summary_items.is_empty() {
            let summary = format!(
                "**Hive** addressed the following review comments:\n\n{}\n\n\
                 Fixes pushed in latest commit.",
                summary_items.join("\n")
            );
            if let Err(e) = github.post_pr_comment(pr_number, &summary).await {
                tracing::warn!(
                    "Failed to post bot-review summary on PR #{pr_number}: {e}"
                );
            }
        }

        // Resolve every unresolved review thread via GraphQL. BotReviews
        // handles all review feedback — bot or human — so after a fix cycle
        // we clear the whole set rather than second-guessing which threads
        // the fix agent addressed.
        let mut resolved_count = 0u32;
        match github.list_unresolved_review_threads(pr_number).await {
            Ok(thread_ids) => {
                for tid in &thread_ids {
                    match github.resolve_review_thread(tid).await {
                        Ok(()) => resolved_count += 1,
                        Err(e) => {
                            tracing::warn!(
                                "Failed to resolve review thread {tid} on PR #{pr_number}: {e}"
                            );
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!(
                    "Failed to list unresolved threads on PR #{pr_number}: {e}"
                );
            }
        }

        send_and_log(
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(format!(
                "[Bot Reviews] Resolved {resolved_count} thread(s), \
                 replied to {inline_replied} inline comment(s), \
                 posted summary for {} review comment(s). Resuming polling...\n",
                summary_items.len()
            )),
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_pr_watch(
    github: &dyn GitHub,
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
    git_ops: &dyn crate::git::worktree::GitOps,
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
            PrStatus::NeedsRebase => {
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
                        "[PR Watch] Branch behind base. \
                         Rebase attempt {rebase_attempts}/{max_rebase_attempts}...\n"
                    )),
                )
                .await;

                match git_ops.rebase(working_dir, default_branch)? {
                    crate::git::worktree::RebaseResult::Success => {
                        github.force_push_current_branch(working_dir).await?;
                        rebase_attempts = 0;
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Rebase + force-push complete. Resuming watch...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                    crate::git::worktree::RebaseResult::Conflicts => {
                        // GitHub said "behind" (clean rebase expected) but
                        // local rebase hit conflicts — likely a race with a
                        // concurrent push. Let GitHub recompute mergeable_state
                        // and handle it as Conflicts on the next poll.
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Unexpected conflicts during rebase. \
                                 Will retry on next poll...\n".to_string(),
                            ),
                        )
                        .await;
                    }
                    crate::git::worktree::RebaseResult::Failed => {
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Git fetch failed (network issue?). Will retry on next poll...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                }
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

                // Try a clean rebase first (no agent cost if no real conflicts in files)
                let rebase_result = git_ops.rebase(working_dir, default_branch)?;

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
                        rebase_attempts = 0;
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
                        // Rebase was already aborted by rebase_worktree() — spawn agent
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

                        // Don't force-push if the agent failed — would push broken state
                        if !matches!(fix_result.outcome, PhaseOutcome::Success) {
                            send_and_log(
                                event_tx, runs_dir, issue_id,
                                AgentEvent::TextDelta(
                                    "[PR Watch] Agent failed to resolve conflicts. Will retry on next poll...\n"
                                        .to_string(),
                                ),
                            )
                            .await;
                            continue;
                        }

                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Agent completed. Force pushing...\n".to_string(),
                            ),
                        )
                        .await;
                        github.force_push_current_branch(working_dir).await?;
                        rebase_attempts = 0;
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
                        // Fetch failure is likely transient (network blip) —
                        // don't escalate immediately, just log and retry on next poll
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(
                                "[PR Watch] Git fetch failed (network issue?). Will retry on next poll...\n"
                                    .to_string(),
                            ),
                        )
                        .await;
                    }
                }
            }
            PrStatus::Clean => {
                // Check for any unresolved review threads — if found, regress
                // to BotReviews so they get handled (bot or human).
                match github.list_unresolved_review_threads(pr_number).await {
                    Ok(threads) if !threads.is_empty() => {
                        send_and_log(
                            event_tx, runs_dir, issue_id,
                            AgentEvent::TextDelta(format!(
                                "[PR Watch] Found {} unresolved review thread(s). \
                                 Regressing to Bot Reviews...\n",
                                threads.len()
                            )),
                        )
                        .await;
                        return Ok(PhaseExecutionResult {
                            outcome: PhaseOutcome::Regress {
                                phase: Phase::BotReviews { cycle: 0 },
                            },
                            cost_usd: total_cost,
                            session_id: None,
                        });
                    }
                    Err(e) => {
                        tracing::warn!(
                            "Failed to check for unresolved threads on PR #{pr_number}: {e}"
                        );
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Run a fix agent (used by CI watch, bot reviews, and PR watch conflict resolution).
///
/// Reports the true outcome: `Failed` if the agent emitted any `Error` event
/// or if the stream ended without a `Complete` event (subprocess crash /
/// truncated output). Callers rely on this to decide whether to push the
/// working-tree state — see the guard in `run_pr_watch`'s Conflicts arm.
async fn run_fix_agent(
    runner: &dyn AgentRunner,
    config: SessionConfig,
    issue_id: &str,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<PhaseExecutionResult> {
    let handle = runner.start_session(config).await?;
    let session_id = handle.session_id.clone();

    let mut stream = runner.output_stream(&handle);
    let mut total_cost = 0.0;
    let mut error_messages: Vec<String> = Vec::new();
    let mut saw_complete = false;

    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        match &event {
            AgentEvent::Complete { cost_usd } => {
                total_cost = *cost_usd;
                saw_complete = true;
            }
            AgentEvent::Error(msg) => error_messages.push(msg.clone()),
            _ => {}
        }
        send_and_log(event_tx, runs_dir, issue_id, event).await;
    }

    let outcome = if !error_messages.is_empty() {
        PhaseOutcome::Failed {
            reason: format!("Agent reported errors: {}", error_messages.join("; ")),
        }
    } else if !saw_complete {
        PhaseOutcome::Failed {
            reason: "Agent stream ended without Complete event \
                    (subprocess crash or truncated output)"
                .to_string(),
        }
    } else {
        PhaseOutcome::Success
    };

    Ok(PhaseExecutionResult {
        outcome,
        cost_usd: total_cost,
        session_id: Some(session_id),
    })
}

/// Parse poll interval string like "30s" or "2m" into Duration.
/// Defaults to 30 seconds.
pub fn parse_poll_interval(interval_str: Option<&str>) -> Duration {
    let Some(s) = interval_str else {
        return Duration::from_secs(30);
    };
    if let Some(secs) = s.strip_suffix('s') {
        secs.parse::<u64>()
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30))
    } else if let Some(mins) = s.strip_suffix('m') {
        mins.parse::<u64>()
            .map(|m| Duration::from_secs(m * 60))
            .unwrap_or(Duration::from_secs(30))
    } else {
        s.parse::<u64>()
            .map(Duration::from_secs)
            .unwrap_or(Duration::from_secs(30))
    }
}

// ---------------------------------------------------------------------------
// Cross-review fix cycle
// ---------------------------------------------------------------------------

/// After CrossReview produces REVIEW.md, check for findings and spawn a fix
/// agent to address them. Returns the fix cost, or 0 if no findings.
pub async fn fix_cross_review_findings(
    runner: &dyn AgentRunner,
    issue_id: &str,
    issue_title: &str,
    working_dir: &std::path::Path,
    model: Option<&str>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &std::path::Path,
) -> Result<f64> {
    let review_path = working_dir.join("REVIEW.md");
    let review_content = match std::fs::read_to_string(&review_path) {
        Ok(content) => content,
        Err(_) => {
            send_and_log(
                event_tx,
                runs_dir,
                issue_id,
                AgentEvent::TextDelta(
                    "[Cross Review] No REVIEW.md found — skipping fix cycle.\n".to_string(),
                ),
            )
            .await;
            return Ok(0.0);
        }
    };

    let content_lower = review_content.to_lowercase();
    if content_lower.contains("lgtm") && !content_lower.contains("must-fix") {
        send_and_log(
            event_tx,
            runs_dir,
            issue_id,
            AgentEvent::TextDelta(
                "[Cross Review] REVIEW.md is LGTM — no fixes needed.\n".to_string(),
            ),
        )
        .await;
        return Ok(0.0);
    }

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(
            "[Cross Review] Findings detected in REVIEW.md. Spawning fix agent...\n".to_string(),
        ),
    )
    .await;

    let fix_prompt = prompts::build_cross_review_fix_prompt(issue_id, issue_title, &review_content);
    let fix_config = SessionConfig {
        working_dir: working_dir.to_path_buf(),
        system_prompt: fix_prompt,
        model: model.map(|s| s.to_string()),
        permission_mode: None,
    };

    let result = run_fix_agent(runner, fix_config, issue_id, event_tx, runs_dir).await?;

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!(
            "[Cross Review] Fix agent completed. Cost: ${:.2}\n",
            result.cost_usd
        )),
    )
    .await;

    Ok(result.cost_usd)
}

// ---------------------------------------------------------------------------
// Direct phases (RaisePr, Handoff)
// ---------------------------------------------------------------------------

/// Result from a direct phase, which may include a PR handle.
#[derive(Debug)]
pub struct DirectPhaseResult {
    pub outcome: PhaseOutcome,
    pub pr: Option<PrHandle>,
    pub cost_usd: f64,
}

/// Execute a direct phase (RaisePr or Handoff).
///
/// Direct phases perform actions via API calls. RaisePr optionally spawns
/// a short agent session to generate a structured PR body.
#[allow(clippy::too_many_arguments)]
pub async fn run_direct_phase(
    phase: &Phase,
    github: &dyn GitHub,
    tracker: &dyn IssueTracker,
    runner: &dyn AgentRunner,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    branch: &str,
    default_branch: &str,
    phase_config: Option<&PhaseConfig>,
    pr: Option<&PrHandle>,
    cost_usd: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
    match phase {
        Phase::RaisePr => {
            let (_, model) = resolve_phase_runner_config(phase, phase_config);
            run_raise_pr(
                github,
                tracker,
                runner,
                issue_id,
                issue_title,
                issue_description,
                working_dir,
                branch,
                default_branch,
                model,
                event_tx,
                runs_dir,
            )
            .await
        }
        Phase::Handoff => {
            run_handoff(
                github, tracker, issue_id, issue_title, pr, cost_usd, started_at, event_tx,
                runs_dir,
            )
            .await
        }
        _ => Err(HiveError::Phase {
            phase: phase.to_string(),
            message: "not a direct phase".to_string(),
        }),
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_raise_pr(
    github: &dyn GitHub,
    tracker: &dyn IssueTracker,
    runner: &dyn AgentRunner,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    branch: &str,
    default_branch: &str,
    model: Option<&str>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
    // Push the branch
    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!("[Raise PR] Pushing branch '{branch}'...\n")),
    )
    .await;

    github.push_branch(working_dir, branch).await?;

    // Generate PR body via agent (falls back to static template on failure)
    let (body, body_cost) = generate_pr_body(
        runner, issue_id, issue_title, issue_description,
        working_dir, default_branch, model, event_tx, runs_dir,
    )
    .await;

    let title = format!("{issue_id}: {issue_title}");

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta("[Raise PR] Creating pull request...\n".to_string()),
    )
    .await;

    let pr_handle = github.create_pr(branch, default_branch, &title, &body).await?;

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!(
            "[Raise PR] PR created: {} (PR #{})\n",
            pr_handle.url, pr_handle.number
        )),
    )
    .await;

    // Transition issue to "In Review"
    if let Err(e) = tracker.finish_issue(issue_id).await {
        tracing::warn!("Failed to transition {issue_id} to review status: {e}");
    }

    Ok(DirectPhaseResult {
        outcome: PhaseOutcome::Success,
        pr: Some(pr_handle),
        cost_usd: body_cost,
    })
}

/// Generate a structured PR body using a short agent session.
///
/// Collects git data (diff stat, commit log), spawns an agent to write
/// PR_BODY.md, then reads the file. Falls back to a static template
/// if anything fails.
#[allow(clippy::too_many_arguments)]
async fn generate_pr_body(
    runner: &dyn AgentRunner,
    issue_id: &str,
    issue_title: &str,
    issue_description: &str,
    working_dir: &std::path::Path,
    default_branch: &str,
    model: Option<&str>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> (String, f64) {
    let fallback = format!(
        "## {issue_title}\n\n{issue_description}\n\n---\n*Automated by Hive*"
    );

    // Collect git data
    let log_output = std::process::Command::new("git")
        .args(["log", "--oneline", &format!("origin/{default_branch}..HEAD")])
        .current_dir(working_dir)
        .output();
    let log_text = log_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    let stat_output = std::process::Command::new("git")
        .args(["diff", &format!("origin/{default_branch}...HEAD"), "--stat"])
        .current_dir(working_dir)
        .output();
    let stat_text = stat_output
        .as_ref()
        .map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        .unwrap_or_default();

    if log_text.is_empty() && stat_text.is_empty() {
        return (fallback, 0.0);
    }

    send_and_log(
        event_tx, runs_dir, issue_id,
        AgentEvent::TextDelta("[Raise PR] Generating PR description...\n".to_string()),
    )
    .await;

    let prompt = prompts::build_pr_body_prompt(
        issue_id, issue_title, issue_description, &log_text, &stat_text,
    );
    let config = SessionConfig {
        working_dir: working_dir.to_path_buf(),
        system_prompt: prompt,
        model: model.map(|s| s.to_string()),
        permission_mode: None,
    };

    let result = run_fix_agent(runner, config, issue_id, event_tx, runs_dir).await;
    let cost = result.as_ref().map(|r| r.cost_usd).unwrap_or(0.0);

    // Read PR_BODY.md
    let body_path = working_dir.join("PR_BODY.md");
    match std::fs::read_to_string(&body_path) {
        Ok(body) if !body.trim().is_empty() => {
            let _ = std::fs::remove_file(&body_path);
            send_and_log(
                event_tx, runs_dir, issue_id,
                AgentEvent::TextDelta(format!(
                    "[Raise PR] PR description generated (cost: ${cost:.2})\n"
                )),
            )
            .await;
            (body, cost)
        }
        _ => {
            tracing::warn!("PR_BODY.md missing or empty for {issue_id}, using fallback");
            let _ = std::fs::remove_file(&body_path);
            (fallback, cost)
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_handoff(
    github: &dyn GitHub,
    _tracker: &dyn IssueTracker,
    issue_id: &str,
    issue_title: &str,
    pr: Option<&PrHandle>,
    cost_usd: f64,
    started_at: chrono::DateTime<chrono::Utc>,
    event_tx: &mpsc::Sender<OrchestratorEvent>,
    runs_dir: &Path,
) -> Result<DirectPhaseResult> {
    let duration = chrono::Utc::now()
        .signed_duration_since(started_at)
        .num_seconds()
        .max(0) as u64;
    let duration_mins = duration / 60;
    let pr_url = pr
        .map(|p| p.url.clone())
        .unwrap_or_else(|| "N/A".to_string());

    // Post summary comment on the PR
    if let Some(pr_handle) = pr {
        let summary = format!(
            "## Hive Summary\n\n\
             **Story:** {issue_id} — {issue_title}\n\
             **Cost:** ${cost_usd:.2}\n\
             **Duration:** {duration_mins}m\n\n\
             This PR was generated by [Hive](https://github.com/hivemine/hive) and is \
             ready for human review.\n\n\
             ---\n*Automated by Hive*"
        );
        if let Err(e) = github.post_pr_comment(pr_handle.number, &summary).await {
            tracing::warn!("Failed to post handoff summary on PR #{}: {e}", pr_handle.number);
        }
    }

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!(
            "[Handoff] Story complete. Cost: ${cost_usd:.2}, Duration: {duration_mins}m, PR: {pr_url}\n",
        )),
    )
    .await;

    // Note: Handoff does NOT transition to "done" status automatically.
    // The PR still needs human review + merge. The story is marked Complete
    // in Hive's state, but the issue tracker status stays at "In Review"
    // until the human merges.

    Ok(DirectPhaseResult {
        outcome: PhaseOutcome::Success,
        pr: None,
        cost_usd: 0.0,
    })
}

#[cfg(test)]
mod tests {
    use std::pin::Pin;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::Stream;
    use tokio::sync::mpsc;
    use tokio_util::sync::CancellationToken;

    use super::*;
    use crate::config::PhaseConfig;
    use crate::domain::{AgentEvent, Issue, IssueDetail, IssueFilters};
    use crate::domain::story_run::PrHandle;
    use crate::git::github::{CiStatus, PrStatus};
    use crate::git::mock_github::{MockGitHub, MockGitOps};
    use crate::git::worktree::RebaseResult;
    use crate::runners::{AgentRunner, SessionConfig, SessionHandle};
    use crate::trackers::IssueTracker;

    // -----------------------------------------------------------------------
    // MockRunner
    // -----------------------------------------------------------------------

    struct MockRunner {
        /// Events to emit from output_stream (per session, in order).
        /// Each call to output_stream pops the front item; if empty, emits Complete.
        sessions: Mutex<std::collections::VecDeque<Vec<AgentEvent>>>,
    }

    impl MockRunner {
        fn new() -> Self {
            Self {
                sessions: Mutex::new(std::collections::VecDeque::new()),
            }
        }

        /// Queue a sequence of events for the next session.
        fn push_session_events(&self, events: Vec<AgentEvent>) {
            self.sessions.lock().unwrap().push_back(events);
        }
    }

    #[async_trait]
    impl AgentRunner for MockRunner {
        async fn start_session(&self, _config: SessionConfig) -> crate::error::Result<SessionHandle> {
            Ok(SessionHandle {
                session_id: "test-session-id".to_string(),
            })
        }

        fn output_stream(
            &self,
            _session: &SessionHandle,
        ) -> Pin<Box<dyn Stream<Item = AgentEvent> + Send>> {
            let events = self
                .sessions
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| vec![AgentEvent::Complete { cost_usd: 0.0 }]);
            Box::pin(futures::stream::iter(events))
        }

        async fn cancel(&self, _session: &SessionHandle) -> crate::error::Result<()> {
            Ok(())
        }
    }

    // -----------------------------------------------------------------------
    // MockTracker
    // -----------------------------------------------------------------------

    struct MockTracker;

    #[async_trait]
    impl IssueTracker for MockTracker {
        async fn list_ready(&self, _filters: &IssueFilters) -> crate::error::Result<Vec<Issue>> {
            Ok(vec![])
        }

        async fn start_issue(&self, _id: &str) -> crate::error::Result<()> {
            Ok(())
        }

        async fn finish_issue(&self, _id: &str) -> crate::error::Result<()> {
            Ok(())
        }

        async fn get_issue(&self, id: &str) -> crate::error::Result<IssueDetail> {
            Ok(IssueDetail {
                id: id.to_string(),
                title: "Test Issue".to_string(),
                description: "Test description".to_string(),
                acceptance_criteria: None,
                priority: None,
                labels: vec![],
                url: format!("https://example.com/issues/{id}"),
            })
        }
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn short_poll_config() -> PhaseConfig {
        PhaseConfig {
            enabled: true,
            runner: None,
            model: None,
            max_attempts: None,
            poll_interval: Some("1s".to_string()),
            max_fix_attempts: Some(3),
            max_fix_cycles: Some(3),
            fix_runner: None,
            fix_model: None,
            wait_for: None,
        }
    }

    fn make_channel() -> mpsc::Sender<OrchestratorEvent> {
        let (tx, _rx) = mpsc::channel(64);
        tx
    }

    fn test_phase_config(enabled: bool) -> PhaseConfig {
        PhaseConfig {
            enabled,
            runner: Some("claude".to_string()),
            model: Some("sonnet-4-6".to_string()),
            max_attempts: Some(2),
            poll_interval: None,
            max_fix_attempts: None,
            max_fix_cycles: None,
            fix_runner: None,
            fix_model: None,
            wait_for: None,
        }
    }

    // -----------------------------------------------------------------------
    // PR Watch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_pr_watch_merged() {
        let github = MockGitHub::new();
        github
            .pr_status_responses
            .lock()
            .unwrap()
            .push_back(PrStatus::Merged);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-1",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
        assert_eq!(*github.force_push_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_pr_watch_closed() {
        let github = MockGitHub::new();
        github
            .pr_status_responses
            .lock()
            .unwrap()
            .push_back(PrStatus::Closed);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-2",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
        assert_eq!(*github.force_push_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_pr_watch_clean_stays_watching() {
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Clean);
            q.push_back(PrStatus::Merged);
        }

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-3",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success after Clean+Merged, got {:?}",
            result.outcome
        );
        assert_eq!(*github.force_push_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_pr_watch_conflicts_triggers_force_push() {
        // MockGitOps returns RebaseResult::Success → clean rebase → force push
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Conflicts);
            q.push_back(PrStatus::Merged);
        }

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-4",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success after Conflicts+Merged, got {:?}",
            result.outcome
        );
        // MockGitOps returns RebaseResult::Success, so force push is triggered
        assert_eq!(*github.force_push_count.lock().unwrap(), 1);
    }

    // -----------------------------------------------------------------------
    // CI Watch tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_ci_watch_passes() {
        let github = MockGitHub::new();
        github
            .ci_status_responses
            .lock()
            .unwrap()
            .push_back(CiStatus::Passed);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_ci_watch(
            &github,
            &runner,
            1,
            "TEST-5",
            "Test Issue",
            working_dir.path(),
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_ci_watch_fails_then_passes() {
        let github = MockGitHub::new();
        {
            let mut q = github.ci_status_responses.lock().unwrap();
            q.push_back(CiStatus::Failed {
                failures: vec!["lint".to_string()],
            });
            q.push_back(CiStatus::Passed);
        }

        // Queue one session for the fix agent
        let runner = MockRunner::new();
        runner.push_session_events(vec![AgentEvent::Complete { cost_usd: 0.5 }]);

        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_ci_watch(
            &github,
            &runner,
            1,
            "TEST-6",
            "Test Issue",
            working_dir.path(),
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success after fail+pass, got {:?}",
            result.outcome
        );
        // Fix agent completed → force_push_current_branch was called once
        assert_eq!(*github.force_push_count.lock().unwrap(), 1);
    }

    // -----------------------------------------------------------------------
    // Raise PR / Handoff direct phase tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_raise_pr_creates_draft() {
        let github = MockGitHub::new();
        let tracker = MockTracker;
        // MockRunner: generate_pr_body will call git log/diff which yields empty
        // output (no real git) so the fallback is used — no agent session needed.
        let runner = MockRunner::new();
        let event_tx = make_channel();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();

        let result = run_direct_phase(
            &Phase::RaisePr,
            &github,
            &tracker,
            &runner,
            "TEST-7",
            "Create feature",
            "Feature description",
            working_dir.path(),
            "TEST-7/create-feature",
            "main",
            None,
            None,
            0.0,
            chrono::Utc::now(),
            &event_tx,
            runs_dir.path(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
        assert_eq!(
            github.pushed_branches.lock().unwrap().len(),
            1,
            "expected one branch push"
        );
        assert_eq!(
            github.created_prs.lock().unwrap().len(),
            1,
            "expected one PR created"
        );
    }

    #[tokio::test]
    async fn test_handoff_posts_summary() {
        let github = MockGitHub::new();
        let tracker = MockTracker;
        let runner = MockRunner::new();
        let event_tx = make_channel();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();

        let pr = PrHandle {
            number: 1,
            url: "https://github.com/test/repo/pull/1".to_string(),
            head_sha: "abc123".to_string(),
        };

        let result = run_direct_phase(
            &Phase::Handoff,
            &github,
            &tracker,
            &runner,
            "TEST-8",
            "Test handoff",
            "Test description",
            working_dir.path(),
            "TEST-8/branch",
            "main",
            None,
            Some(&pr),
            1.23,
            chrono::Utc::now(),
            &event_tx,
            runs_dir.path(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );

        let comments = github.posted_comments.lock().unwrap();
        assert_eq!(comments.len(), 1, "expected one comment posted");
        assert!(
            comments[0].1.contains("Hive Summary"),
            "comment should contain 'Hive Summary', got: {}",
            comments[0].1
        );
    }

    // -----------------------------------------------------------------------
    // Bot Reviews tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_bot_reviews_no_comments_completes() {
        let github = MockGitHub::new();
        // Two empty polls → quiet_polls reaches 2 → done
        {
            let mut q = github.review_responses.lock().unwrap();
            q.push_back(vec![]);
            q.push_back(vec![]);
        }

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_bot_reviews(
            &github,
            &runner,
            1,
            "TEST-9",
            "Test Issue",
            working_dir.path(),
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success after 2 quiet polls, got {:?}",
            result.outcome
        );
    }

    #[test]
    fn test_resolve_phase_runner_config() {
        let config = test_phase_config(true);
        let (runner, model) = resolve_phase_runner_config(&Phase::Implement, Some(&config));
        assert_eq!(runner, Some("claude"));
        assert_eq!(model, Some("sonnet-4-6"));
    }

    #[test]
    fn test_resolve_phase_runner_config_none() {
        let (runner, model) = resolve_phase_runner_config(&Phase::Implement, None);
        assert!(runner.is_none());
        assert!(model.is_none());
    }

    #[test]
    fn test_max_attempts_from_config() {
        let config = test_phase_config(true);
        assert_eq!(max_attempts_for_phase(&Phase::Implement, Some(&config)), 2);
    }

    #[test]
    fn test_max_attempts_default_self_review() {
        assert_eq!(
            max_attempts_for_phase(&Phase::SelfReview { attempt: 0 }, None),
            3
        );
    }

    #[test]
    fn test_max_attempts_default_implement() {
        assert_eq!(max_attempts_for_phase(&Phase::Implement, None), 1);
    }

    #[test]
    fn test_max_attempts_default_understand() {
        assert_eq!(max_attempts_for_phase(&Phase::Understand, None), 1);
    }

    #[test]
    fn test_parse_poll_interval_seconds() {
        assert_eq!(parse_poll_interval(Some("30s")), Duration::from_secs(30));
        assert_eq!(parse_poll_interval(Some("60s")), Duration::from_secs(60));
    }

    #[test]
    fn test_parse_poll_interval_minutes() {
        assert_eq!(parse_poll_interval(Some("2m")), Duration::from_secs(120));
    }

    #[test]
    fn test_parse_poll_interval_bare_number() {
        assert_eq!(parse_poll_interval(Some("45")), Duration::from_secs(45));
    }

    #[test]
    fn test_parse_poll_interval_default() {
        assert_eq!(parse_poll_interval(None), Duration::from_secs(30));
    }

    #[test]
    fn test_parse_poll_interval_invalid() {
        assert_eq!(parse_poll_interval(Some("abc")), Duration::from_secs(30));
    }

    // -----------------------------------------------------------------------
    // PrWatch negative / edge case tests
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn test_pr_watch_rebase_attempts_exhausted() {
        // 3 consecutive Conflicts with Failed rebases → counter increments
        // without resetting → exhaustion → NeedsAttention
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Conflicts); // attempt 1: 0<3, rebase fails
            q.push_back(PrStatus::Conflicts); // attempt 2: 1<3, rebase fails
            q.push_back(PrStatus::Conflicts); // attempt 3: 2<3, rebase fails
            q.push_back(PrStatus::Conflicts); // attempt 4: 3>=3 → exhausted
        }

        // Rebase fails (fetch failure) so counter increments without resetting
        let git_ops = MockGitOps::new();
        {
            let mut q = git_ops.rebase_responses.lock().unwrap();
            q.push_back(RebaseResult::Failed);
            q.push_back(RebaseResult::Failed);
            q.push_back(RebaseResult::Failed);
        }

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-EXHAUST",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &git_ops,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::NeedsAttention { .. }),
            "expected NeedsAttention after exhausted rebase attempts, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_pr_watch_rebase_conflicts_spawns_agent() {
        // Rebase returns Conflicts → agent spawned → force push
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Conflicts);
            q.push_back(PrStatus::Merged);
        }

        let runner = MockRunner::new();
        // Queue a session for the rebase conflict agent
        runner.push_session_events(vec![AgentEvent::Complete { cost_usd: 1.0 }]);

        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        // MockGitOps returns Conflicts (not Success) → agent path
        let git_ops = MockGitOps::new();
        git_ops
            .rebase_responses
            .lock()
            .unwrap()
            .push_back(RebaseResult::Conflicts);

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-AGENT",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &git_ops,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
        // Agent succeeded → force push happened
        assert_eq!(*github.force_push_count.lock().unwrap(), 1);
        // Cost should include the agent session
        assert!(result.cost_usd > 0.0, "expected agent cost tracked");
    }

    #[tokio::test]
    async fn test_run_fix_agent_reports_failed_on_error_event() {
        let runner = MockRunner::new();
        runner.push_session_events(vec![
            AgentEvent::Error("claude subprocess crashed".to_string()),
            AgentEvent::Complete { cost_usd: 0.0 },
        ]);

        let event_tx = make_channel();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let config = SessionConfig {
            working_dir: working_dir.path().to_path_buf(),
            system_prompt: "test".to_string(),
            model: None,
            permission_mode: None,
        };

        let result = run_fix_agent(&runner, config, "TEST", &event_tx, runs_dir.path())
            .await
            .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Failed { .. }),
            "agent Error events must propagate to Failed outcome, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_run_fix_agent_reports_failed_without_complete_event() {
        let runner = MockRunner::new();
        // Stream ends without Complete — simulates subprocess crash / truncation
        runner.push_session_events(vec![AgentEvent::TextDelta("partial work".to_string())]);

        let event_tx = make_channel();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let config = SessionConfig {
            working_dir: working_dir.path().to_path_buf(),
            system_prompt: "test".to_string(),
            model: None,
            permission_mode: None,
        };

        let result = run_fix_agent(&runner, config, "TEST", &event_tx, runs_dir.path())
            .await
            .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Failed { .. }),
            "missing Complete event must propagate to Failed outcome, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_pr_watch_needs_rebase_does_clean_rebase_and_push() {
        // PrStatus::NeedsRebase → clean rebase + force-push, no agent involved
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::NeedsRebase);
            q.push_back(PrStatus::Merged);
        }

        let git_ops = MockGitOps::new();
        git_ops
            .rebase_responses
            .lock()
            .unwrap()
            .push_back(RebaseResult::Success);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-REBASE",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &git_ops,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success, got {:?}",
            result.outcome
        );
        assert_eq!(*github.force_push_count.lock().unwrap(), 1);
        assert_eq!(
            result.cost_usd, 0.0,
            "NeedsRebase must not spawn an agent — clean rebase should handle it"
        );
    }

    #[tokio::test]
    async fn test_pr_watch_fetch_failure_retries() {
        // Fetch fails (RebaseResult::Failed) → logs and continues → next poll clean → merged
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Conflicts);
            q.push_back(PrStatus::Merged);
        }

        let git_ops = MockGitOps::new();
        git_ops
            .rebase_responses
            .lock()
            .unwrap()
            .push_back(RebaseResult::Failed);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-FETCH-FAIL",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &git_ops,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success after fetch failure + merged, got {:?}",
            result.outcome
        );
        // Fetch failed → no force push on first poll, then merged on second
        assert_eq!(*github.force_push_count.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn test_pr_watch_cancellation() {
        // Cancel immediately → should return Failed with "Cancelled"
        let github = MockGitHub::new();
        // Queue Clean responses — but cancel fires before they're consumed
        github
            .pr_status_responses
            .lock()
            .unwrap()
            .push_back(PrStatus::Clean);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        cancel.cancel(); // Cancel immediately
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-CANCEL",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Failed { ref reason } if reason.contains("Cancelled")),
            "expected Failed(Cancelled), got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_ci_watch_exhausts_fix_attempts() {
        // CI fails 4 times → max_fix_attempts (3) exhausted → NeedsAttention
        let github = MockGitHub::new();
        {
            let mut q = github.ci_status_responses.lock().unwrap();
            q.push_back(CiStatus::Failed { failures: vec!["lint".into()] });
            q.push_back(CiStatus::Failed { failures: vec!["lint".into()] });
            q.push_back(CiStatus::Failed { failures: vec!["lint".into()] });
            q.push_back(CiStatus::Failed { failures: vec!["lint".into()] });
        }

        let runner = MockRunner::new();
        // Queue 3 fix agent sessions
        runner.push_session_events(vec![AgentEvent::Complete { cost_usd: 0.0 }]);
        runner.push_session_events(vec![AgentEvent::Complete { cost_usd: 0.0 }]);
        runner.push_session_events(vec![AgentEvent::Complete { cost_usd: 0.0 }]);

        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_ci_watch(
            &github,
            &runner,
            1,
            "TEST-CI-EXHAUST",
            "Test Issue",
            working_dir.path(),
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::NeedsAttention { .. }),
            "expected NeedsAttention after exhausted CI fix attempts, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_pr_watch_regresses_to_bot_reviews_on_unresolved_threads() {
        // Clean PR status but unresolved bot threads → Regress to BotReviews
        let github = MockGitHub::new();
        github
            .pr_status_responses
            .lock()
            .unwrap()
            .push_back(PrStatus::Clean);
        github
            .unresolved_threads_responses
            .lock()
            .unwrap()
            .push_back(vec!["thread-1".to_string(), "thread-2".to_string()]);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-REGRESS",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(
                result.outcome,
                PhaseOutcome::Regress { phase: Phase::BotReviews { cycle: 0 } }
            ),
            "expected Regress to BotReviews, got {:?}",
            result.outcome
        );
    }

    #[tokio::test]
    async fn test_pr_watch_no_regress_when_no_threads() {
        // Clean PR status, no unresolved threads → keep watching → Merged
        let github = MockGitHub::new();
        {
            let mut q = github.pr_status_responses.lock().unwrap();
            q.push_back(PrStatus::Clean);
            q.push_back(PrStatus::Merged);
        }
        // Empty threads response (no unresolved threads)
        github
            .unresolved_threads_responses
            .lock()
            .unwrap()
            .push_back(vec![]);

        let runner = MockRunner::new();
        let event_tx = make_channel();
        let cancel = CancellationToken::new();
        let runs_dir = tempfile::tempdir().unwrap();
        let working_dir = tempfile::tempdir().unwrap();
        let cfg = short_poll_config();

        let result = run_pr_watch(
            &github,
            &runner,
            1,
            "TEST-NO-REGRESS",
            "Test Issue",
            working_dir.path(),
            "main",
            Some(&cfg),
            &event_tx,
            runs_dir.path(),
            &cancel,
            &MockGitOps::new(),
        )
        .await
        .unwrap();

        assert!(
            matches!(result.outcome, PhaseOutcome::Success),
            "expected Success (merged), got {:?}",
            result.outcome
        );
    }
}
