use serde_json::Value;

use crate::domain::story_run::PrHandle;
use crate::error::{HiveError, Result};

pub struct GitHubClient {
    owner: String,
    repo: String,
    client: reqwest::Client,
    token: String,
}

impl GitHubClient {
    pub fn new(owner: String, repo: String, token: String) -> Self {
        Self {
            owner,
            repo,
            client: reqwest::Client::new(),
            token,
        }
    }

    fn api_url(&self, path: &str) -> String {
        format!(
            "https://api.github.com/repos/{}/{}{}",
            self.owner, self.repo, path
        )
    }

    async fn get(&self, path: &str) -> Result<Value> {
        let resp = self
            .client
            .get(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "hive")
            .header("Accept", "application/vnd.github+json")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "GitHub API error ({status}): {text}"
            )));
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post(&self, path: &str, body: &Value) -> Result<Value> {
        let resp = self
            .client
            .post(self.api_url(path))
            .header("Authorization", format!("Bearer {}", self.token))
            .header("User-Agent", "hive")
            .header("Accept", "application/vnd.github+json")
            .json(body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "GitHub API error ({status}): {text}"
            )));
        }
        Ok(serde_json::from_str(&text)?)
    }

    pub async fn create_pr(&self, branch: &str, title: &str, body: &str) -> Result<PrHandle> {
        let payload = serde_json::json!({
            "title": title,
            "body": body,
            "head": branch,
            "base": "master",
        });
        let resp = self.post("/pulls", &payload).await?;
        Ok(PrHandle {
            number: resp["number"].as_u64().unwrap_or(0),
            url: resp["html_url"].as_str().unwrap_or("").to_string(),
            head_sha: resp["head"]["sha"].as_str().unwrap_or("").to_string(),
        })
    }

    pub async fn poll_ci(&self, pr_number: u64) -> Result<CiStatus> {
        let resp = self
            .get(&format!("/pulls/{pr_number}/commits"))
            .await?;
        let head_sha = resp
            .as_array()
            .and_then(|c| c.last())
            .and_then(|c| c["sha"].as_str())
            .unwrap_or("");
        if head_sha.is_empty() {
            return Ok(CiStatus::Pending);
        }
        let status_resp = self
            .get(&format!("/commits/{head_sha}/check-runs"))
            .await?;
        let check_runs = status_resp["check_runs"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if check_runs.is_empty() {
            return Ok(CiStatus::Pending);
        }
        let all_complete = check_runs
            .iter()
            .all(|r| r["status"].as_str() == Some("completed"));
        if !all_complete {
            return Ok(CiStatus::Pending);
        }
        let any_failed = check_runs
            .iter()
            .any(|r| r["conclusion"].as_str() != Some("success"));
        if any_failed {
            let failures = check_runs
                .iter()
                .filter(|r| r["conclusion"].as_str() != Some("success"))
                .map(|r| {
                    format!(
                        "{}: {}",
                        r["name"].as_str().unwrap_or("unknown"),
                        r["conclusion"].as_str().unwrap_or("unknown")
                    )
                })
                .collect();
            Ok(CiStatus::Failed { failures })
        } else {
            Ok(CiStatus::Passed)
        }
    }

    pub async fn poll_reviews(&self, pr_number: u64) -> Result<Vec<ReviewComment>> {
        let resp = self
            .get(&format!("/pulls/{pr_number}/comments"))
            .await?;
        let comments = resp
            .as_array()
            .unwrap_or(&Vec::new())
            .iter()
            .map(|c| ReviewComment {
                id: c["id"].as_u64().unwrap_or(0),
                author: c["user"]["login"].as_str().unwrap_or("").to_string(),
                body: c["body"].as_str().unwrap_or("").to_string(),
                path: c["path"].as_str().map(String::from),
                is_bot: c["user"]["type"].as_str() == Some("Bot"),
            })
            .collect();
        Ok(comments)
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
    pub id: u64,
    pub author: String,
    pub body: String,
    pub path: Option<String>,
    pub is_bot: bool,
}
