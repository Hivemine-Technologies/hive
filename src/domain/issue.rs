use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issue {
    pub id: String,
    pub title: String,
    pub priority: Option<String>,
    pub project: Option<String>,
    pub labels: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IssueDetail {
    pub id: String,
    pub title: String,
    pub description: String,
    pub acceptance_criteria: Option<String>,
    pub priority: Option<String>,
    pub labels: Vec<String>,
    pub url: String,
}

#[derive(Debug, Clone)]
pub struct IssueFilters {
    pub team: Option<String>,
    pub project: Option<String>,
    pub labels: Vec<String>,
    pub statuses: Vec<String>,
}
