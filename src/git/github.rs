use async_trait::async_trait;
use octocrab::models::CommentId;
use octocrab::params::repos::Commitish;
use octocrab::Octocrab;
use serde_json::Value;

use crate::domain::story_run::PrHandle;
use crate::error::{HiveError, Result};

#[async_trait]
pub trait GitHub: Send + Sync {
    async fn create_pr(&self, branch: &str, base: &str, title: &str, body: &str) -> Result<PrHandle>;
    async fn poll_ci(&self, pr_number: u64) -> Result<CiStatus>;
    async fn poll_pr_status(&self, pr_number: u64) -> Result<PrStatus>;
    async fn poll_reviews(&self, pr_number: u64) -> Result<Vec<ReviewComment>>;
    async fn push_branch(&self, worktree_path: &std::path::Path, branch: &str) -> Result<()>;
    async fn force_push_current_branch(&self, worktree_path: &std::path::Path) -> Result<()>;
    async fn post_pr_comment(&self, pr_number: u64, body: &str) -> Result<()>;
    async fn reply_to_inline_comment(&self, pr_number: u64, comment_id: u64, body: &str) -> Result<()>;
    async fn list_unresolved_review_threads(&self, pr_number: u64) -> Result<Vec<String>>;
    async fn resolve_review_thread(&self, thread_id: &str) -> Result<()>;
}

pub struct GitHubClient {
    owner: String,
    repo: String,
    octocrab: Octocrab,
}

impl GitHubClient {
    pub fn new(owner: String, repo: String, token: String) -> Result<Self> {
        let octocrab = Octocrab::builder()
            .personal_token(token)
            .build()
            .map_err(|e| HiveError::GitHub(e.to_string()))?;
        Ok(Self {
            owner,
            repo,
            octocrab,
        })
    }

    async fn graphql(&self, query: &str) -> Result<Value> {
        let payload = serde_json::json!({ "query": query });
        let resp: Value = self
            .octocrab
            .graphql(&payload)
            .await
            .map_err(|e| HiveError::GitHub(format!("GitHub GraphQL error: {e}")))?;
        if let Some(errors) = resp.get("errors") {
            return Err(HiveError::GitHub(format!(
                "GitHub GraphQL errors: {errors}"
            )));
        }
        Ok(resp)
    }

    async fn find_existing_pr(&self, branch: &str) -> Result<PrHandle> {
        let page = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list()
            .head(format!("{}:{}", self.owner, branch))
            .state(octocrab::params::State::Open)
            .send()
            .await
            .map_err(|e| HiveError::GitHub(e.to_string()))?;

        let pr = page.items.first().ok_or_else(|| {
            HiveError::GitHub(format!(
                "PR already exists for branch '{branch}' but could not be found"
            ))
        })?;

        Ok(PrHandle {
            number: pr.number,
            url: pr.html_url.as_ref().map(|u| u.to_string()).unwrap_or_default(),
            head_sha: pr.head.sha.clone(),
        })
    }
}

#[async_trait]
impl GitHub for GitHubClient {
    async fn create_pr(&self, branch: &str, base: &str, title: &str, body: &str) -> Result<PrHandle> {
        let result = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .create(title, branch, base)
            .body(body)
            .draft(true)
            .send()
            .await;

        match result {
            Ok(pr) => Ok(PrHandle {
                number: pr.number,
                url: pr.html_url.map(|u| u.to_string()).unwrap_or_default(),
                head_sha: pr.head.sha,
            }),
            Err(ref e) => match e {
                octocrab::Error::GitHub { source, .. }
                    if source.status_code.as_u16() == 422
                        && source.message.contains("A pull request already exists") =>
                {
                    self.find_existing_pr(branch).await
                }
                _ => Err(HiveError::GitHub(format!("GitHub API error: {e}"))),
            },
        }
    }

    async fn poll_pr_status(&self, pr_number: u64) -> Result<PrStatus> {
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

        // Use mergeable_state (not mergeable) — the bool collapses "dirty,"
        // "draft," "blocked," and "behind" into a single false, which caused
        // PrWatch to loop-rebase draft PRs forever. The enum is the truth.
        use octocrab::models::pulls::MergeableState;
        match pr.mergeable_state {
            Some(MergeableState::Dirty) => Ok(PrStatus::Conflicts),
            Some(MergeableState::Behind) => Ok(PrStatus::NeedsRebase),
            // Clean, Draft, Blocked, HasHooks, Unknown, Unstable, None —
            // none indicate conflicts or need-to-rebase.
            _ => Ok(PrStatus::Clean),
        }
    }

    async fn poll_ci(&self, pr_number: u64) -> Result<CiStatus> {
        let pr = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .get(pr_number)
            .await
            .map_err(|e| HiveError::GitHub(e.to_string()))?;

        let head_sha = &pr.head.sha;
        if head_sha.is_empty() {
            return Ok(CiStatus::Pending);
        }

        let check_runs = self
            .octocrab
            .checks(&self.owner, &self.repo)
            .list_check_runs_for_git_ref(Commitish(head_sha.clone()))
            .send()
            .await
            .map_err(|e| HiveError::GitHub(e.to_string()))?;

        if check_runs.check_runs.is_empty() {
            return Ok(CiStatus::Pending);
        }

        let all_complete = check_runs
            .check_runs
            .iter()
            .all(|r| r.completed_at.is_some());

        if !all_complete {
            return Ok(CiStatus::Pending);
        }

        let any_failed = check_runs
            .check_runs
            .iter()
            .any(|r| r.conclusion.as_deref() != Some("success"));

        if any_failed {
            let failures = check_runs
                .check_runs
                .iter()
                .filter(|r| r.conclusion.as_deref() != Some("success"))
                .map(|r| {
                    let conclusion = r.conclusion.as_deref().unwrap_or("no conclusion");
                    format!("{}: {conclusion}", r.name)
                })
                .collect();
            Ok(CiStatus::Failed { failures })
        } else {
            Ok(CiStatus::Passed)
        }
    }

    async fn poll_reviews(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
        let mut comments = Vec::new();

        // 1. Inline diff comments (line-level annotations)
        if let Ok(page) = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list_comments(Some(pr_number))
            .send()
            .await
        {
            for c in &page.items {
                comments.push(ReviewComment {
                    id: format!("inline-{}", c.id.into_inner()),
                    author: c
                        .user
                        .as_ref()
                        .map(|u| u.login.clone())
                        .unwrap_or_default(),
                    body: c.body.clone(),
                    is_bot: c
                        .user
                        .as_ref()
                        .map(|u| u.r#type == "Bot")
                        .unwrap_or(false),
                });
            }
        }

        // 2. PR review bodies (top-level reviews from bots like CodeRabbit)
        if let Ok(page) = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .list_reviews(pr_number)
            .send()
            .await
        {
            for r in &page.items {
                let body = r.body.as_deref().unwrap_or("");
                if body.is_empty() {
                    continue;
                }
                comments.push(ReviewComment {
                    id: format!("review-{}", r.id.into_inner()),
                    author: r
                        .user
                        .as_ref()
                        .map(|u| u.login.clone())
                        .unwrap_or_default(),
                    body: body.to_string(),
                    is_bot: r
                        .user
                        .as_ref()
                        .map(|u| u.r#type == "Bot")
                        .unwrap_or(false),
                });
            }
        }

        // 3. Issue comments (general PR comments)
        if let Ok(page) = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .list_comments(pr_number)
            .send()
            .await
        {
            for c in &page.items {
                let body = c.body.as_deref().unwrap_or("");
                if body.is_empty() {
                    continue;
                }
                comments.push(ReviewComment {
                    id: format!("issue-{}", c.id.into_inner()),
                    author: c.user.login.clone(),
                    body: body.to_string(),
                    is_bot: c.user.r#type == "Bot",
                });
            }
        }

        Ok(comments)
    }

    /// Fetch all unresolved review thread IDs on a PR, regardless of author.
    ///
    /// BotReviews handles any outstanding review thread — bot or human — so
    /// we resolve every unresolved thread after a fix cycle. PrWatch uses
    /// the same list to decide when to regress.
    ///
    /// Returns GraphQL node IDs that can be passed to `resolve_review_thread`.
    async fn list_unresolved_review_threads(&self, pr_number: u64) -> Result<Vec<String>> {
        let query = format!(
            r#"query {{
              repository(owner: "{}", name: "{}") {{
                pullRequest(number: {}) {{
                  reviewThreads(first: 100) {{
                    nodes {{
                      id
                      isResolved
                    }}
                  }}
                }}
              }}
            }}"#,
            self.owner, self.repo, pr_number
        );

        let resp = self.graphql(&query).await?;
        let threads = resp["data"]["repository"]["pullRequest"]["reviewThreads"]["nodes"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        let mut thread_ids = Vec::new();
        for thread in &threads {
            match thread["isResolved"].as_bool() {
                Some(true) => continue,
                Some(false) => {} // fall through
                None => {
                    // Field missing / malformed — likely a GraphQL schema
                    // change or an unexpected response shape. Surface it so
                    // we notice, but keep going (resolveReviewThread is
                    // idempotent, so re-resolving is wasteful not harmful).
                    tracing::warn!(
                        "GraphQL review thread missing `isResolved` field \
                         (schema change?); treating as unresolved. \
                         thread_id={:?}",
                        thread["id"].as_str().unwrap_or("<missing>")
                    );
                }
            }
            if let Some(id) = thread["id"].as_str() {
                thread_ids.push(id.to_string());
            }
        }

        Ok(thread_ids)
    }

    /// Resolve a PR review thread by its GraphQL node ID.
    async fn resolve_review_thread(&self, thread_id: &str) -> Result<()> {
        let query = format!(
            r#"mutation {{
              resolveReviewThread(input: {{ threadId: "{thread_id}" }}) {{
                thread {{ isResolved }}
              }}
            }}"#
        );
        let resp = self.graphql(&query).await?;
        let is_resolved = resp["data"]["resolveReviewThread"]["thread"]["isResolved"]
            .as_bool()
            .unwrap_or(false);
        if !is_resolved {
            return Err(HiveError::GitHub(format!(
                "resolveReviewThread for {thread_id} did not mark thread resolved \
                 (likely missing pull_requests:write scope); response: {resp}"
            )));
        }
        Ok(())
    }

    /// Force-push the current branch from a worktree using --force-with-lease.
    /// Assumes upstream is already set (via `push -u` during RaisePr).
    ///
    /// Retries transparently on "stale info" rejection by re-fetching and
    /// retrying the push. Stale-lease happens when the remote ref advanced
    /// between our fetch and our push — usually a benign race with GitHub's
    /// async ref propagation or with another push targeting the same ref.
    /// Genuine divergence (non-fast-forward without --force) is NOT retried.
    async fn force_push_current_branch(
        &self,
        worktree_path: &std::path::Path,
    ) -> Result<()> {
        const MAX_ATTEMPTS: u8 = 3;
        let mut last_stderr = String::new();

        for attempt in 1..=MAX_ATTEMPTS {
            let output = std::process::Command::new("git")
                .args(["push", "--force-with-lease"])
                .current_dir(worktree_path)
                .output()?;
            if output.status.success() {
                if attempt > 1 {
                    tracing::info!(
                        target: "hive::git",
                        "git push --force-with-lease succeeded on attempt {attempt}/{MAX_ATTEMPTS}"
                    );
                }
                return Ok(());
            }

            let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::warn!(
                target: "hive::git",
                "git push --force-with-lease attempt {attempt}/{MAX_ATTEMPTS} failed in {}:\nstderr:\n{stderr}\nstdout:\n{stdout}",
                worktree_path.display()
            );
            last_stderr = stderr;

            // Only retry on stale-lease races. Any other rejection (diverged
            // history, protected branch, auth failure) is not retriable.
            let is_stale_lease = last_stderr.contains("stale info");
            if !is_stale_lease || attempt == MAX_ATTEMPTS {
                break;
            }

            // Re-fetch to update our cached view of origin's refs, then retry.
            let fetch = std::process::Command::new("git")
                .args(["fetch", "origin"])
                .current_dir(worktree_path)
                .output()?;
            if !fetch.status.success() {
                let fetch_err = String::from_utf8_lossy(&fetch.stderr);
                tracing::warn!(
                    target: "hive::git",
                    "git fetch during force-push retry failed: {fetch_err}"
                );
                break;
            }
        }

        Err(HiveError::Git(git2::Error::from_str(&format!(
            "force push failed after {MAX_ATTEMPTS} attempt(s): {last_stderr}"
        ))))
    }

    /// Reply to an inline review comment on a PR.
    async fn reply_to_inline_comment(
        &self,
        pr_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        self.octocrab
            .pulls(&self.owner, &self.repo)
            .reply_to_comment(pr_number, CommentId(comment_id), body)
            .await
            .map_err(|e| {
                HiveError::GitHub(format!(
                    "Failed to reply to inline comment {comment_id} on PR #{pr_number}: {e}"
                ))
            })?;
        Ok(())
    }

    /// Post a general comment on a PR (issue comment endpoint).
    async fn post_pr_comment(
        &self,
        pr_number: u64,
        body: &str,
    ) -> Result<()> {
        self.octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(pr_number, body)
            .await
            .map_err(|e| {
                HiveError::GitHub(format!(
                    "Failed to post comment on PR #{pr_number}: {e}"
                ))
            })?;
        Ok(())
    }

    async fn push_branch(
        &self,
        worktree_path: &std::path::Path,
        branch: &str,
    ) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(["push", "-u", "origin", branch])
            .current_dir(worktree_path)
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            tracing::warn!(
                target: "hive::git",
                "git push -u origin {branch} failed in {}:\nstderr:\n{stderr}\nstdout:\n{stdout}",
                worktree_path.display()
            );
            return Err(HiveError::Git(git2::Error::from_str(&format!(
                "push failed: {stderr}"
            ))));
        }
        Ok(())
    }
}

#[derive(Debug, Clone)]
pub enum CiStatus {
    Pending,
    Passed,
    Failed { failures: Vec<String> },
}

#[derive(Debug, PartialEq)]
pub enum PrStatus {
    /// PR was merged into the base branch.
    Merged,
    /// PR was closed without merging.
    Closed,
    /// PR is open and mergeable (or in a state that needs no action —
    /// draft, blocked on checks, mergeability not yet computed).
    Clean,
    /// PR is open but behind base branch (mergeable_state == "behind").
    /// A plain rebase is expected to succeed without conflicts.
    NeedsRebase,
    /// PR has real merge conflicts (mergeable_state == "dirty") that may
    /// require an agent to resolve.
    Conflicts,
}

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub is_bot: bool,
}
