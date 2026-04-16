use async_trait::async_trait;
use serde_json::{Value, json};

use super::IssueTracker;
use crate::config::TrackerConfig;
use crate::domain::{Issue, IssueDetail, IssueFilters};
use crate::error::{HiveError, Result};

pub struct JiraTracker {
    base_url: String,
    email: String,
    api_token: String,
    tracker_config: TrackerConfig,
    client: reqwest::Client,
}

/// Default JQL clause name for the Advanced Roadmaps "Team" custom field.
/// Overridable via `tracker_config.fields.jira_team_field` when an instance
/// exposes it as `team`, `cf[10001]`, etc.
const DEFAULT_JIRA_TEAM_FIELD: &str = "\"Team[Team]\"";

impl JiraTracker {
    pub fn new(
        base_url: String,
        email: String,
        api_token: String,
        tracker_config: TrackerConfig,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            email,
            api_token,
            tracker_config,
            client: reqwest::Client::new(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn send(
        &self,
        req: reqwest::RequestBuilder,
    ) -> Result<(reqwest::StatusCode, String)> {
        let resp = req
            .basic_auth(&self.email, Some(&self.api_token))
            .header("Accept", "application/json")
            .send()
            .await?;
        let status = resp.status();
        let text = resp.text().await?;
        Ok((status, text))
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let (status, text) = self.send(self.client.get(self.url(path))).await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "Jira GET {path} failed ({status}): {text}"
            )));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn post_json(&self, path: &str, body: Value) -> Result<Value> {
        let (status, text) = self
            .send(self.client.post(self.url(path)).json(&body))
            .await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "Jira POST {path} failed ({status}): {text}"
            )));
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        Ok(serde_json::from_str(&text)?)
    }

    async fn put_empty(&self, path: &str, body: Value) -> Result<()> {
        let (status, text) = self
            .send(self.client.put(self.url(path)).json(&body))
            .await?;
        if !status.is_success() {
            return Err(HiveError::Tracker(format!(
                "Jira PUT {path} failed ({status}): {text}"
            )));
        }
        Ok(())
    }

    fn project_key(&self) -> Result<&str> {
        self.tracker_config
            .fields
            .get("jira_project")
            .map(|s| s.as_str())
            .ok_or_else(|| {
                HiveError::Config(
                    "jira tracker requires tracker_config.fields.jira_project \
                     (the Jira project key, e.g. \"APEX\")"
                        .into(),
                )
            })
    }

    fn team_field(&self) -> String {
        self.tracker_config
            .fields
            .get("jira_team_field")
            .cloned()
            .unwrap_or_else(|| DEFAULT_JIRA_TEAM_FIELD.to_string())
    }

    async fn myself_account_id(&self) -> Result<String> {
        let v = self.get_json("/rest/api/3/myself").await?;
        v["accountId"].as_str().map(String::from).ok_or_else(|| {
            HiveError::Tracker("Jira /myself did not return an accountId".into())
        })
    }

    async fn assign_to_self(&self, issue_key: &str) -> Result<()> {
        let account_id = self.myself_account_id().await?;
        let path = format!("/rest/api/3/issue/{issue_key}/assignee");
        self.put_empty(&path, json!({ "accountId": account_id }))
            .await
    }

    async fn current_status(&self, issue_key: &str) -> Result<String> {
        let path = format!("/rest/api/3/issue/{issue_key}?fields=status");
        let v = self.get_json(&path).await?;
        v["fields"]["status"]["name"]
            .as_str()
            .map(String::from)
            .ok_or_else(|| {
                HiveError::Tracker(format!(
                    "Jira issue {issue_key} has no status.name in response"
                ))
            })
    }

    /// Forward-only transition with idempotent skip.
    ///
    /// Behavior (matches decisions A+C): if the issue is already in `target`
    /// or in any status we consider "past" `target`, silently succeed.
    /// Otherwise fetch available transitions, find one whose `to.name`
    /// equals `target`, and POST it. On no-match we hard-fail with the
    /// list of available next states so the error is actionable.
    async fn transition(&self, issue_key: &str, target: &str) -> Result<()> {
        let current = self.current_status(issue_key).await?;
        if self.past_set_for(target).iter().any(|s| s == &current) {
            return Ok(());
        }

        let path = format!("/rest/api/3/issue/{issue_key}/transitions");
        let v = self.get_json(&path).await?;
        let empty = vec![];
        let transitions = v["transitions"].as_array().unwrap_or(&empty);

        let matched = transitions.iter().find(|t| {
            t["to"]["name"]
                .as_str()
                .map(|n| n == target)
                .unwrap_or(false)
        });

        let Some(t) = matched else {
            let available: Vec<String> = transitions
                .iter()
                .filter_map(|t| t["to"]["name"].as_str().map(String::from))
                .collect();
            return Err(HiveError::Tracker(format!(
                "Jira issue {issue_key} cannot transition to '{target}' from '{current}'. \
                 Available transitions: [{}]",
                available.join(", ")
            )));
        };

        let transition_id = t["id"].as_str().ok_or_else(|| {
            HiveError::Tracker(format!(
                "Jira transition for {issue_key} → {target} has no id"
            ))
        })?;

        self.post_json(&path, json!({ "transition": { "id": transition_id } }))
            .await?;
        Ok(())
    }

    /// Builds the set of statuses that should count as "already at or past
    /// `target`" for idempotency. The three hive-managed targets are `start`,
    /// `review`, and `done`; anything downstream of `target` (plus any
    /// configured `past_review` states) means "no-op".
    fn past_set_for(&self, target: &str) -> Vec<String> {
        let s = &self.tracker_config.statuses;
        let mut set: Vec<String> = vec![target.to_string()];
        if target == s.start {
            set.push(s.review.clone());
            set.push(s.done.clone());
            set.extend(s.past_review.iter().cloned());
        } else if target == s.review {
            set.push(s.done.clone());
            set.extend(s.past_review.iter().cloned());
        }
        set
    }
}

#[async_trait]
impl IssueTracker for JiraTracker {
    async fn list_ready(&self, filters: &IssueFilters) -> Result<Vec<Issue>> {
        let jql = if let Some(ref raw) = self.tracker_config.raw_jql {
            raw.clone()
        } else {
            let project_key = self.project_key()?.to_string();
            let team_field = self.team_field();
            let team = filters
                .team
                .as_deref()
                .unwrap_or(&self.tracker_config.team);
            let statuses: &[String] = if filters.statuses.is_empty() {
                &self.tracker_config.ready_filter
            } else {
                &filters.statuses
            };

            build_jql(
                &project_key,
                &team_field,
                team,
                statuses,
                filters.project.as_deref(),
                &filters.labels,
            )
        };
        tracing::info!(target: "hive::jira", %jql, "Jira list_ready JQL");

        let body = json!({
            "jql": jql,
            "fields": ["summary", "priority", "status", "labels", "project"],
            "maxResults": 100
        });

        let v = self.post_json("/rest/api/3/search/jql", body).await?;
        parse_search_response(&v, &self.base_url)
    }

    async fn start_issue(&self, id: &str) -> Result<()> {
        // Jira workflow validator: assignee is required before New → In Progress.
        // Assign the caller ("myself") first, then transition.
        self.assign_to_self(id).await?;
        self.transition(id, &self.tracker_config.statuses.start)
            .await
    }

    async fn finish_issue(&self, id: &str) -> Result<()> {
        self.transition(id, &self.tracker_config.statuses.review)
            .await
    }

    async fn get_issue(&self, id: &str) -> Result<IssueDetail> {
        let path =
            format!("/rest/api/3/issue/{id}?fields=summary,description,priority,labels,status");
        let v = self.get_json(&path).await?;
        let fields = &v["fields"];
        let key = v["key"].as_str().unwrap_or(id).to_string();

        let labels: Vec<String> = fields["labels"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|l| l.as_str().map(String::from)).collect())
            .unwrap_or_default();

        Ok(IssueDetail {
            id: key.clone(),
            title: fields["summary"].as_str().unwrap_or_default().to_string(),
            description: adf_to_text(&fields["description"]),
            acceptance_criteria: None,
            priority: fields["priority"]["name"].as_str().map(String::from),
            labels,
            url: format!("{}/browse/{key}", self.base_url),
        })
    }

}

/// Build a JQL query. Pure function so it can be unit-tested without a
/// live Jira instance.
///
/// Label semantics are **AND** (issue must have every label) — flip the
/// join to `OR` below if you prefer any-match instead. We picked AND to
/// mirror how `cargo` feature flags compose: narrowing, not widening.
pub fn build_jql(
    project_key: &str,
    team_field: &str,
    team: &str,
    statuses: &[String],
    project: Option<&str>,
    labels: &[String],
) -> String {
    let mut clauses: Vec<String> = Vec::new();
    // Jira accepts either the project key ("APEX") or the numeric project ID
    // (10038) in the `project` clause. Quoting a numeric ID can trip the
    // parser in some instances, so emit all-digit values unquoted.
    if project_key.chars().all(|c| c.is_ascii_digit()) && !project_key.is_empty() {
        clauses.push(format!("project = {project_key}"));
    } else {
        clauses.push(format!(r#"project = "{project_key}""#));
    }
    if !team.is_empty() {
        clauses.push(format!(r#"{team_field} = "{team}""#));
    }
    if !statuses.is_empty() {
        let quoted: Vec<String> = statuses.iter().map(|s| format!(r#""{s}""#)).collect();
        clauses.push(format!("status in ({})", quoted.join(", ")));
    }
    if let Some(p) = project {
        // Maps IssueFilters.project to fixVersion. Swap to `"Epic Link"` if
        // your team groups by epic instead of release.
        clauses.push(format!(r#"fixVersion = "{p}""#));
    }
    for label in labels {
        clauses.push(format!(r#"labels = "{label}""#));
    }
    format!(
        "{} ORDER BY priority DESC, created ASC",
        clauses.join(" AND ")
    )
}

/// Parse the `issues` array from a `/rest/api/3/search/jql` response.
pub fn parse_search_response(v: &Value, base_url: &str) -> Result<Vec<Issue>> {
    let empty = vec![];
    let nodes = v["issues"].as_array().unwrap_or(&empty);
    let mut issues = Vec::with_capacity(nodes.len());
    for node in nodes {
        let key = node["key"].as_str().unwrap_or_default().to_string();
        let fields = &node["fields"];
        let labels: Vec<String> = fields["labels"]
            .as_array()
            .map(|arr| arr.iter().filter_map(|l| l.as_str().map(String::from)).collect())
            .unwrap_or_default();
        issues.push(Issue {
            id: key.clone(),
            title: fields["summary"].as_str().unwrap_or_default().to_string(),
            priority: fields["priority"]["name"].as_str().map(String::from),
            project: fields["project"]["name"].as_str().map(String::from),
            labels,
            url: format!("{base_url}/browse/{key}"),
        });
    }
    Ok(issues)
}

/// Recursively walk an Atlassian Document Format (ADF) tree and concatenate
/// all `text` leaves. Jira Cloud v3 returns description as ADF JSON rather
/// than plain text or Markdown, so we flatten it for display in the TUI.
pub fn adf_to_text(v: &Value) -> String {
    let mut out = String::new();
    walk_adf(v, &mut out);
    out.trim_end().to_string()
}

fn walk_adf(v: &Value, out: &mut String) {
    if let Some(text) = v["text"].as_str() {
        out.push_str(text);
    }
    if let Some(content) = v["content"].as_array() {
        for child in content {
            walk_adf(child, out);
        }
    }
    // Add a newline after block-level nodes so paragraphs don't run together.
    if let Some(kind) = v["type"].as_str() {
        if matches!(
            kind,
            "paragraph" | "heading" | "bulletList" | "orderedList" | "listItem" | "codeBlock"
        ) {
            out.push('\n');
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jql_basic_project_and_status() {
        let jql = build_jql(
            "APEX",
            "\"Team[Team]\"",
            "",
            &["To Do".to_string()],
            None,
            &[],
        );
        assert!(jql.contains(r#"project = "APEX""#));
        assert!(jql.contains(r#"status in ("To Do")"#));
        assert!(!jql.contains("Team[Team]"));
        assert!(jql.contains("ORDER BY priority DESC"));
    }

    #[test]
    fn jql_with_team_field() {
        let jql = build_jql(
            "APEX",
            "\"Team[Team]\"",
            "Platform Squad",
            &["To Do".to_string(), "In Progress".to_string()],
            None,
            &[],
        );
        assert!(jql.contains(r#""Team[Team]" = "Platform Squad""#));
        assert!(jql.contains(r#"status in ("To Do", "In Progress")"#));
    }

    #[test]
    fn jql_with_custom_team_field_name() {
        let jql = build_jql(
            "APEX",
            "cf[10001]",
            "squad-42",
            &[],
            None,
            &[],
        );
        assert!(jql.contains(r#"cf[10001] = "squad-42""#));
    }

    #[test]
    fn jql_numeric_project_id_is_unquoted() {
        // Matches the shape Jira's board filters emit for project IDs:
        //   project in (10038) AND cf[10001] in (<uuid>) ...
        let jql = build_jql(
            "10038",
            "cf[10001]",
            "fa8776d1-995b-46af-aac8-6a5221d0cb02",
            &[],
            None,
            &[],
        );
        assert!(jql.contains("project = 10038"));
        assert!(!jql.contains(r#"project = "10038""#));
        assert!(jql.contains(r#"cf[10001] = "fa8776d1-995b-46af-aac8-6a5221d0cb02""#));
    }

    #[test]
    fn jql_with_project_and_labels() {
        let jql = build_jql(
            "APEX",
            "\"Team[Team]\"",
            "Platform",
            &["To Do".to_string()],
            Some("Release-1.2"),
            &["backend".to_string(), "urgent".to_string()],
        );
        assert!(jql.contains(r#"fixVersion = "Release-1.2""#));
        assert!(jql.contains(r#"labels = "backend""#));
        assert!(jql.contains(r#"labels = "urgent""#));
        // AND semantics: labels appear as separate clauses, not in a list
        assert_eq!(jql.matches(" AND ").count(), 5);
    }

    #[test]
    fn parse_search_response_basic() {
        let json = serde_json::json!({
            "issues": [
                {
                    "key": "APEX-123",
                    "fields": {
                        "summary": "Add NumberSequenceService",
                        "priority": { "name": "High" },
                        "status": { "name": "To Do" },
                        "labels": ["backend", "urgent"],
                        "project": { "name": "Apex Platform" }
                    }
                }
            ]
        });
        let issues = parse_search_response(&json, "https://example.atlassian.net").unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].id, "APEX-123");
        assert_eq!(issues[0].title, "Add NumberSequenceService");
        assert_eq!(issues[0].priority, Some("High".to_string()));
        assert_eq!(issues[0].labels, vec!["backend", "urgent"]);
        assert_eq!(issues[0].project, Some("Apex Platform".to_string()));
        assert_eq!(
            issues[0].url,
            "https://example.atlassian.net/browse/APEX-123"
        );
    }

    #[test]
    fn parse_search_response_empty() {
        let json = serde_json::json!({ "issues": [] });
        let issues = parse_search_response(&json, "https://example.atlassian.net").unwrap();
        assert!(issues.is_empty());
    }

    #[test]
    fn parse_search_response_missing_optional_fields() {
        let json = serde_json::json!({
            "issues": [
                {
                    "key": "APEX-1",
                    "fields": {
                        "summary": "No priority, no project",
                        "labels": []
                    }
                }
            ]
        });
        let issues = parse_search_response(&json, "https://example.atlassian.net").unwrap();
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].priority, None);
        assert_eq!(issues[0].project, None);
        assert!(issues[0].labels.is_empty());
    }

    #[test]
    fn adf_to_text_flattens_nested_tree() {
        let adf = serde_json::json!({
            "type": "doc",
            "version": 1,
            "content": [
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "First paragraph." }
                    ]
                },
                {
                    "type": "paragraph",
                    "content": [
                        { "type": "text", "text": "Second " },
                        { "type": "text", "text": "paragraph." }
                    ]
                }
            ]
        });
        let text = adf_to_text(&adf);
        assert!(text.contains("First paragraph."));
        assert!(text.contains("Second paragraph."));
    }

    #[test]
    fn adf_to_text_handles_null() {
        let text = adf_to_text(&Value::Null);
        assert!(text.is_empty());
    }

}
