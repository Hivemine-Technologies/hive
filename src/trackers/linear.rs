use async_trait::async_trait;
use serde_json::Value;

use super::IssueTracker;
use crate::config::TrackerConfig;
use crate::domain::{FollowUpContent, Issue, IssueDetail, IssueFilters};
use crate::error::{HiveError, Result};

pub struct LinearTracker {
    api_key: String,
    tracker_config: TrackerConfig,
    client: reqwest::Client,
}

const LINEAR_API_URL: &str = "https://api.linear.app/graphql";

impl LinearTracker {
    pub fn new(api_key: String, tracker_config: TrackerConfig) -> Self {
        Self {
            api_key,
            tracker_config,
            client: reqwest::Client::new(),
        }
    }

    async fn graphql(&self, query: &str) -> Result<Value> {
        let body = serde_json::json!({ "query": query });
        let resp = self
            .client
            .post(LINEAR_API_URL)
            .header("Authorization", &self.api_key)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "Linear API error ({status}): {text}"
            )));
        }
        let v: Value = serde_json::from_str(&text)?;
        if let Some(errors) = v.get("errors") {
            return Err(HiveError::Tracker(format!(
                "Linear GraphQL errors: {errors}"
            )));
        }
        Ok(v)
    }

    async fn transition_issue(&self, id: &str, target_status: &str) -> Result<()> {
        let team = &self.tracker_config.team;
        let query = format!(
            r#"query {{ workflowStates(filter: {{ team: {{ name: {{ eq: "{team}" }} }}, name: {{ eq: "{target_status}" }} }}) {{ nodes {{ id name }} }} }}"#
        );
        let resp = self.graphql(&query).await?;
        let state_id = resp["data"]["workflowStates"]["nodes"][0]["id"]
            .as_str()
            .ok_or_else(|| {
                HiveError::Tracker(format!(
                    "workflow state '{target_status}' not found for team '{team}'"
                ))
            })?;
        let mutation = format!(
            r#"mutation {{ issueUpdate(id: "{id}", input: {{ stateId: "{state_id}" }}) {{ issue {{ identifier state {{ name }} }} }} }}"#
        );
        self.graphql(&mutation).await?;
        Ok(())
    }
}

#[async_trait]
impl IssueTracker for LinearTracker {
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>> {
        let team = filters
            .team
            .as_deref()
            .unwrap_or(&self.tracker_config.team);
        let status = filters
            .status
            .as_deref()
            .unwrap_or(&self.tracker_config.ready_filter);
        let query = build_issues_query(team, status, filters.project.as_deref());
        let resp = self.graphql(&query).await?;
        let issues = parse_issues_response(&resp.to_string())?;

        if filters.labels.is_empty() {
            return Ok(issues);
        }

        let filtered = issues
            .into_iter()
            .filter(|issue| {
                filters
                    .labels
                    .iter()
                    .any(|label| issue.labels.contains(label))
            })
            .collect();
        Ok(filtered)
    }

    async fn start_issue(&self, id: &str) -> Result<()> {
        self.transition_issue(id, &self.tracker_config.statuses.start)
            .await
    }

    async fn finish_issue(&self, id: &str) -> Result<()> {
        self.transition_issue(id, &self.tracker_config.statuses.review)
            .await
    }

    async fn create_followup(&self, parent_id: &str, content: FollowUpContent) -> Result<String> {
        let team = &self.tracker_config.team;
        let team_query = format!(
            r#"query {{ teams(filter: {{ name: {{ eq: "{team}" }} }}) {{ nodes {{ id }} }} }}"#
        );
        let team_resp = self.graphql(&team_query).await?;
        let team_id = team_resp["data"]["teams"]["nodes"][0]["id"]
            .as_str()
            .ok_or_else(|| HiveError::Tracker(format!("team '{team}' not found")))?;

        let title = content.title.replace('"', r#"\""#);
        let description = content.description.replace('"', r#"\""#);
        let labels_fragment = if content.labels.is_empty() {
            String::new()
        } else {
            let label_query = format!(
                r#"query {{ issueLabels(filter: {{ team: {{ name: {{ eq: "{team}" }} }} }}) {{ nodes {{ id name }} }} }}"#
            );
            let label_resp = self.graphql(&label_query).await?;
            let empty_vec = vec![];
            let label_nodes = label_resp["data"]["issueLabels"]["nodes"]
                .as_array()
                .unwrap_or(&empty_vec);
            let label_ids: Vec<String> = label_nodes
                .iter()
                .filter_map(|node| {
                    let name = node["name"].as_str()?;
                    if content.labels.contains(&name.to_string()) {
                        node["id"].as_str().map(|id| format!("\"{id}\""))
                    } else {
                        None
                    }
                })
                .collect();
            if label_ids.is_empty() {
                String::new()
            } else {
                format!(", labelIds: [{}]", label_ids.join(", "))
            }
        };

        let parent_id_escaped = parent_id.replace('"', r#"\""#);
        let mutation = format!(
            r#"mutation {{ issueCreate(input: {{ teamId: "{team_id}", title: "{title}", description: "{description}", parentId: "{parent_id_escaped}"{labels_fragment} }}) {{ issue {{ identifier }} }} }}"#
        );
        let resp = self.graphql(&mutation).await?;
        let identifier = resp["data"]["issueCreate"]["issue"]["identifier"]
            .as_str()
            .ok_or_else(|| HiveError::Tracker("failed to create follow-up issue".to_string()))?;
        Ok(identifier.to_string())
    }

    async fn get_issue(&self, id: &str) -> Result<IssueDetail> {
        let query = format!(
            r#"query {{ issue(id: "{id}") {{ identifier title description url priority labels {{ nodes {{ name }} }} }} }}"#
        );
        let resp = self.graphql(&query).await?;
        let issue = &resp["data"]["issue"];
        let labels: Vec<String> = issue["labels"]["nodes"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let priority_num = issue["priority"].as_u64().unwrap_or(0) as u8;

        Ok(IssueDetail {
            id: issue["identifier"]
                .as_str()
                .unwrap_or(id)
                .to_string(),
            title: issue["title"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            description: issue["description"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            acceptance_criteria: None,
            priority: Some(priority_label(priority_num).to_string()),
            labels,
            url: issue["url"].as_str().unwrap_or_default().to_string(),
        })
    }

    fn name(&self) -> &str {
        "linear"
    }
}

pub fn build_issues_query(team: &str, status: &str, project: Option<&str>) -> String {
    let project_filter = match project {
        Some(proj) => format!(r#", project: {{ name: {{ eq: "{proj}" }} }}"#),
        None => String::new(),
    };
    format!(
        r#"query {{ issues(filter: {{ team: {{ name: {{ eq: "{team}" }} }}, state: {{ name: {{ eq: "{status}" }} }}{project_filter} }}) {{ nodes {{ identifier title priority url labels {{ nodes {{ name }} }} project {{ name }} }} }} }}"#
    )
}

pub fn parse_issues_response(json: &str) -> Result<Vec<Issue>> {
    let v: Value = serde_json::from_str(json)?;
    let nodes = v["data"]["issues"]["nodes"]
        .as_array()
        .ok_or_else(|| HiveError::Tracker("missing issues.nodes in response".to_string()))?;

    let mut issues = Vec::with_capacity(nodes.len());
    for node in nodes {
        let labels: Vec<String> = node["labels"]["nodes"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|l| l["name"].as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let priority_num = node["priority"].as_u64().unwrap_or(0) as u8;

        issues.push(Issue {
            id: node["identifier"]
                .as_str()
                .unwrap_or_default()
                .to_string(),
            title: node["title"].as_str().unwrap_or_default().to_string(),
            priority: Some(priority_label(priority_num).to_string()),
            project: node["project"]["name"].as_str().map(|s| s.to_string()),
            labels,
            url: node["url"].as_str().unwrap_or_default().to_string(),
        });
    }

    Ok(issues)
}

pub fn priority_label(priority: u8) -> &'static str {
    match priority {
        1 => "Urgent",
        2 => "High",
        3 => "Medium",
        4 => "Low",
        _ => "None",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_build_ready_issues_query() {
        let query = build_issues_query("Hivemine", "Todo", None);
        assert!(query.contains("Hivemine"));
        assert!(query.contains("Todo"));
        assert!(query.contains("issues"));
    }

    #[test]
    fn test_build_issues_query_with_project() {
        let query = build_issues_query("Hivemine", "Todo", Some("Phase 77"));
        assert!(query.contains("Hivemine"));
        assert!(query.contains("Phase 77"));
    }

    #[test]
    fn test_parse_issues_response() {
        let json = r#"{
            "data": {
                "issues": {
                    "nodes": [
                        {
                            "identifier": "APX-245",
                            "title": "Add NumberSequenceService",
                            "priority": 2,
                            "url": "https://linear.app/hivemine/issue/APX-245",
                            "labels": { "nodes": [{ "name": "backend" }] },
                            "project": { "name": "Phase 77" }
                        }
                    ]
                }
            }
        }"#;
        let issues = parse_issues_response(json).unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "APX-245");
        assert_eq!(issues[0].title, "Add NumberSequenceService");
        assert_eq!(issues[0].labels, vec!["backend"]);
    }

    #[test]
    fn test_parse_issues_response_multiple() {
        let json = r#"{
            "data": {
                "issues": {
                    "nodes": [
                        {
                            "identifier": "APX-1",
                            "title": "First",
                            "priority": 1,
                            "url": "https://linear.app/1",
                            "labels": { "nodes": [] },
                            "project": null
                        },
                        {
                            "identifier": "APX-2",
                            "title": "Second",
                            "priority": 4,
                            "url": "https://linear.app/2",
                            "labels": { "nodes": [{ "name": "frontend" }, { "name": "urgent" }] },
                            "project": { "name": "Alpha" }
                        }
                    ]
                }
            }
        }"#;
        let issues = parse_issues_response(json).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(issues[0].priority, Some("Urgent".to_string()));
        assert_eq!(issues[1].priority, Some("Low".to_string()));
        assert_eq!(issues[1].labels, vec!["frontend", "urgent"]);
        assert_eq!(issues[1].project, Some("Alpha".to_string()));
    }

    #[test]
    fn test_parse_issues_response_empty() {
        let json = r#"{ "data": { "issues": { "nodes": [] } } }"#;
        let issues = parse_issues_response(json).unwrap();
        assert!(issues.is_empty());
    }

    #[test]
    fn test_priority_number_to_label() {
        assert_eq!(priority_label(0), "None");
        assert_eq!(priority_label(1), "Urgent");
        assert_eq!(priority_label(2), "High");
        assert_eq!(priority_label(3), "Medium");
        assert_eq!(priority_label(4), "Low");
    }

    #[test]
    fn test_priority_out_of_range() {
        assert_eq!(priority_label(5), "None");
        assert_eq!(priority_label(255), "None");
    }
}
