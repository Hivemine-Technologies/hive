use octocrab::models::CommentId;
use octocrab::params::repos::Commitish;
use octocrab::Octocrab;
use serde_json::Value;

use crate::domain::story_run::PrHandle;
use crate::error::{HiveError, Result};

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

    pub async fn create_pr(&self, branch: &str, base: &str, title: &str, body: &str) -> Result<PrHandle> {
        let result = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .create(title, branch, base)
            .body(body)
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

    pub async fn poll_ci(&self, pr_number: u64) -> Result<CiStatus> {
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

    pub async fn poll_reviews(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
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

    /// Fetch unresolved review thread IDs for bot authors on a PR.
    ///
    /// Returns GraphQL node IDs that can be passed to `resolve_review_thread`.
    pub async fn list_unresolved_bot_threads(
        &self,
        pr_number: u64,
        bot_authors: &[String],
    ) -> Result<Vec<String>> {
        let query = format!(
            r#"query {{
              repository(owner: "{}", name: "{}") {{
                pullRequest(number: {}) {{
                  reviewThreads(first: 100) {{
                    nodes {{
                      id
                      isResolved
                      comments(first: 1) {{
                        nodes {{
                          author {{ login }}
                        }}
                      }}
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
            if thread["isResolved"].as_bool() == Some(true) {
                continue;
            }
            let author = thread["comments"]["nodes"]
                .as_array()
                .and_then(|c| c.first())
                .and_then(|c| c["author"]["login"].as_str())
                .unwrap_or("");
            let is_match = bot_authors.is_empty()
                || bot_authors
                    .iter()
                    .any(|b| author.to_lowercase().contains(&b.to_lowercase()));
            if is_match {
                if let Some(id) = thread["id"].as_str() {
                    thread_ids.push(id.to_string());
                }
            }
        }

        Ok(thread_ids)
    }

    /// Resolve a PR review thread by its GraphQL node ID.
    pub async fn resolve_review_thread(&self, thread_id: &str) -> Result<()> {
        let query = format!(
            r#"mutation {{
              resolveReviewThread(input: {{ threadId: "{thread_id}" }}) {{
                thread {{ isResolved }}
              }}
            }}"#
        );
        if let Err(e) = self.graphql(&query).await {
            tracing::warn!("Failed to resolve review thread {thread_id}: {e}");
        }
        Ok(())
    }

    /// Push the current branch from a worktree. Assumes upstream is already set
    /// (via `push -u` during RaisePr).
    pub async fn push_current_branch(
        &self,
        worktree_path: &std::path::Path,
    ) -> Result<()> {
        let output = std::process::Command::new("git")
            .args(["push"])
            .current_dir(worktree_path)
            .output()?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            return Err(HiveError::Git(git2::Error::from_str(&format!(
                "push failed: {stderr}"
            ))));
        }
        Ok(())
    }

    /// Reply to an inline review comment on a PR.
    pub async fn reply_to_inline_comment(
        &self,
        pr_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        if let Err(e) = self
            .octocrab
            .pulls(&self.owner, &self.repo)
            .reply_to_comment(pr_number, CommentId(comment_id), body)
            .await
        {
            tracing::warn!(
                "Failed to reply to inline comment {comment_id} on PR #{pr_number}: {e}"
            );
        }
        Ok(())
    }

    /// Post a general comment on a PR (issue comment endpoint).
    pub async fn post_pr_comment(
        &self,
        pr_number: u64,
        body: &str,
    ) -> Result<()> {
        if let Err(e) = self
            .octocrab
            .issues(&self.owner, &self.repo)
            .create_comment(pr_number, body)
            .await
        {
            tracing::warn!("Failed to post comment on PR #{pr_number}: {e}");
        }
        Ok(())
    }

    pub async fn push_branch(
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

#[derive(Debug, Clone)]
pub struct ReviewComment {
    pub id: String,
    pub author: String,
    pub body: String,
    pub is_bot: bool,
}
