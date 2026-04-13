pub mod jira;
pub mod linear;

use async_trait::async_trait;

use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::Result;

#[async_trait]
pub trait IssueTracker: Send + Sync {
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>>;
    async fn start_issue(&self, id: &str) -> Result<()>;
    async fn finish_issue(&self, id: &str) -> Result<()>;
    async fn create_followup(&self, parent_id: &str, content: FollowUpContent) -> Result<String>;
    async fn get_issue(&self, id: &str) -> Result<IssueDetail>;
    fn name(&self) -> &str;
}
