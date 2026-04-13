use async_trait::async_trait;

use super::IssueTracker;
use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::{HiveError, Result};

pub struct JiraTracker {
    base_url: String,
    api_token: String,
    email: String,
}

impl JiraTracker {
    pub fn new(base_url: String, api_token: String, email: String) -> Self {
        Self {
            base_url,
            api_token,
            email,
        }
    }
}

#[async_trait]
impl IssueTracker for JiraTracker {
    async fn list_ready(&self, _filters: &IssueFilters) -> Result<Vec<Issue>> {
        Err(HiveError::Tracker(
            "Jira tracker not yet implemented".into(),
        ))
    }

    async fn start_issue(&self, _id: &str) -> Result<()> {
        Err(HiveError::Tracker(
            "Jira tracker not yet implemented".into(),
        ))
    }

    async fn finish_issue(&self, _id: &str) -> Result<()> {
        Err(HiveError::Tracker(
            "Jira tracker not yet implemented".into(),
        ))
    }

    async fn create_followup(&self, _parent_id: &str, _content: FollowUpContent) -> Result<String> {
        Err(HiveError::Tracker(
            "Jira tracker not yet implemented".into(),
        ))
    }

    async fn get_issue(&self, _id: &str) -> Result<IssueDetail> {
        Err(HiveError::Tracker(
            "Jira tracker not yet implemented".into(),
        ))
    }

    fn name(&self) -> &str {
        "jira"
    }
}
