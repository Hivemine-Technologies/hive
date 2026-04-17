use std::collections::VecDeque;
use std::path::Path;
use std::sync::Mutex;

use async_trait::async_trait;

use super::github::{CiStatus, GitHub, PrStatus, ReviewComment};
use crate::domain::story_run::PrHandle;
use crate::error::Result;

/// Mock GitHub implementation for integration testing.
///
/// Queue up responses with the `*_responses` fields. Each call pops
/// the next response from the front. If the queue is empty, a sensible
/// default is returned. Call recordings let tests assert what was called.
pub struct MockGitHub {
    pub pr_status_responses: Mutex<VecDeque<PrStatus>>,
    pub ci_status_responses: Mutex<VecDeque<CiStatus>>,
    pub review_responses: Mutex<VecDeque<Vec<ReviewComment>>>,
    pub unresolved_threads_responses: Mutex<VecDeque<Vec<String>>>,
    pub create_pr_response: Mutex<Option<PrHandle>>,

    // Call recording
    pub created_prs: Mutex<Vec<(String, String, String, String)>>,
    pub posted_comments: Mutex<Vec<(u64, String)>>,
    pub pushed_branches: Mutex<Vec<String>>,
    pub force_push_count: Mutex<u32>,
    pub resolved_threads: Mutex<Vec<String>>,
    pub replied_comments: Mutex<Vec<(u64, u64, String)>>,
}

impl MockGitHub {
    pub fn new() -> Self {
        Self {
            pr_status_responses: Mutex::new(VecDeque::new()),
            ci_status_responses: Mutex::new(VecDeque::new()),
            review_responses: Mutex::new(VecDeque::new()),
            unresolved_threads_responses: Mutex::new(VecDeque::new()),
            create_pr_response: Mutex::new(Some(PrHandle {
                number: 1,
                url: "https://github.com/test/repo/pull/1".to_string(),
                head_sha: "abc123".to_string(),
            })),
            created_prs: Mutex::new(Vec::new()),
            posted_comments: Mutex::new(Vec::new()),
            pushed_branches: Mutex::new(Vec::new()),
            force_push_count: Mutex::new(0),
            resolved_threads: Mutex::new(Vec::new()),
            replied_comments: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl GitHub for MockGitHub {
    async fn create_pr(
        &self,
        branch: &str,
        base: &str,
        title: &str,
        body: &str,
    ) -> Result<PrHandle> {
        self.created_prs.lock().unwrap().push((
            branch.to_string(),
            base.to_string(),
            title.to_string(),
            body.to_string(),
        ));
        Ok(self
            .create_pr_response
            .lock()
            .unwrap()
            .clone()
            .unwrap_or(PrHandle {
                number: 1,
                url: "https://github.com/test/repo/pull/1".to_string(),
                head_sha: "abc123".to_string(),
            }))
    }

    async fn poll_ci(&self, _pr_number: u64) -> Result<CiStatus> {
        Ok(self
            .ci_status_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(CiStatus::Passed))
    }

    async fn poll_pr_status(&self, _pr_number: u64) -> Result<PrStatus> {
        Ok(self
            .pr_status_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(PrStatus::Clean))
    }

    async fn poll_reviews(&self, _pr_number: u64) -> Result<Vec<ReviewComment>> {
        Ok(self
            .review_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }

    async fn push_branch(&self, _worktree_path: &Path, branch: &str) -> Result<()> {
        self.pushed_branches
            .lock()
            .unwrap()
            .push(branch.to_string());
        Ok(())
    }

    async fn force_push_current_branch(&self, _worktree_path: &Path) -> Result<()> {
        *self.force_push_count.lock().unwrap() += 1;
        Ok(())
    }

    async fn post_pr_comment(&self, pr_number: u64, body: &str) -> Result<()> {
        self.posted_comments
            .lock()
            .unwrap()
            .push((pr_number, body.to_string()));
        Ok(())
    }

    async fn reply_to_inline_comment(
        &self,
        pr_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        self.replied_comments
            .lock()
            .unwrap()
            .push((pr_number, comment_id, body.to_string()));
        Ok(())
    }

    async fn list_unresolved_review_threads(
        &self,
        _pr_number: u64,
    ) -> Result<Vec<String>> {
        Ok(self
            .unresolved_threads_responses
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or_default())
    }

    async fn resolve_review_thread(&self, thread_id: &str) -> Result<()> {
        self.resolved_threads
            .lock()
            .unwrap()
            .push(thread_id.to_string());
        Ok(())
    }
}

use super::worktree::{GitOps, RebaseResult};

/// Mock git operations for integration testing.
pub struct MockGitOps {
    pub rebase_responses: Mutex<VecDeque<RebaseResult>>,
    pub remove_count: Mutex<u32>,
}

impl MockGitOps {
    pub fn new() -> Self {
        Self {
            rebase_responses: Mutex::new(VecDeque::new()),
            remove_count: Mutex::new(0),
        }
    }
}

impl GitOps for MockGitOps {
    fn rebase(&self, _worktree_path: &Path, _default_branch: &str) -> crate::error::Result<RebaseResult> {
        Ok(self.rebase_responses.lock().unwrap()
            .pop_front()
            .unwrap_or(RebaseResult::Success))
    }

    fn remove(&self, _repo_path: &Path, _issue_id: &str, _worktree_dir: &Path) -> crate::error::Result<()> {
        *self.remove_count.lock().unwrap() += 1;
        Ok(())
    }
}
