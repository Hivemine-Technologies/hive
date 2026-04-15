use std::path::Path;
use std::time::Duration;

use tokio::sync::mpsc;

use crate::config::PhaseConfig;
use crate::domain::story_run::PrHandle;
use crate::domain::{AgentEvent, OrchestratorEvent, Phase, PhaseOutcome};
use crate::error::{HiveError, Result};
use crate::git::github::{CiStatus, GitHubClient, ReviewComment};
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
    if let Some(config) = phase_config {
        if let Some(max) = config.max_attempts {
            return max;
        }
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
// Polling phases (CiWatch, BotReviews)
// ---------------------------------------------------------------------------

/// Execute a polling phase (CiWatch or BotReviews).
///
/// Polls GitHub on an interval. When issues are detected (CI failure,
/// new bot comments), spawns a fix agent and retries.
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
        _ => Err(HiveError::Phase {
            phase: phase.to_string(),
            message: "not a polling phase".to_string(),
        }),
    }
}

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

                // After fix agent completes, push and resume polling
                send_and_log(
                    event_tx,
                    runs_dir,
                    issue_id,
                    AgentEvent::TextDelta(
                        "[CI Watch] Fix agent completed. Resuming CI polling...\n".to_string(),
                    ),
                )
                .await;
            }
        }
    }
}

async fn run_bot_reviews(
    github: &GitHubClient,
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
    }
}

/// Run a fix agent (used by CI watch and bot reviews).
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

    use futures::StreamExt;
    while let Some(event) = stream.next().await {
        match &event {
            AgentEvent::Complete { cost_usd } => {
                total_cost = *cost_usd;
            }
            _ => {}
        }
        send_and_log(event_tx, runs_dir, issue_id, event).await;
    }

    Ok(PhaseExecutionResult {
        outcome: PhaseOutcome::Success,
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
// Direct phases (RaisePr, Handoff)
// ---------------------------------------------------------------------------

/// Result from a direct phase, which may include a PR handle.
#[derive(Debug)]
pub struct DirectPhaseResult {
    pub outcome: PhaseOutcome,
    pub pr: Option<PrHandle>,
}

/// Execute a direct phase (RaisePr or Handoff).
///
/// Direct phases perform actions via API calls with no agent involvement.
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
    match phase {
        Phase::RaisePr => {
            run_raise_pr(
                github,
                tracker,
                issue_id,
                issue_title,
                issue_description,
                working_dir,
                branch,
                event_tx,
                runs_dir,
            )
            .await
        }
        Phase::Handoff => {
            run_handoff(tracker, issue_id, pr, cost_usd, started_at, event_tx, runs_dir).await
        }
        _ => Err(HiveError::Phase {
            phase: phase.to_string(),
            message: "not a direct phase".to_string(),
        }),
    }
}

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
    // Push the branch
    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!("[Raise PR] Pushing branch '{branch}'...\n")),
    )
    .await;

    github.push_branch(working_dir, branch).await?;

    // Create PR
    let title = format!("{issue_id}: {issue_title}");
    let body = format!("## {issue_title}\n\n{issue_description}\n\n---\n*Automated by Hive*");

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta("[Raise PR] Creating pull request...\n".to_string()),
    )
    .await;

    let pr_handle = github.create_pr(branch, &title, &body).await?;

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
    })
}

async fn run_handoff(
    _tracker: &dyn IssueTracker,
    issue_id: &str,
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
    let pr_url = pr
        .map(|p| p.url.clone())
        .unwrap_or_else(|| "N/A".to_string());

    send_and_log(
        event_tx,
        runs_dir,
        issue_id,
        AgentEvent::TextDelta(format!(
            "[Handoff] Story complete. Cost: ${cost_usd:.2}, Duration: {}m, PR: {pr_url}\n",
            duration / 60
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
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::PhaseConfig;

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
}
