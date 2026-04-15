use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;

use chrono::Utc;

use crate::domain::AgentEvent;

fn format_event(event: &AgentEvent) -> String {
    let ts = Utc::now().format("%Y-%m-%dT%H:%M:%SZ");
    match event {
        AgentEvent::TextDelta(text) => format!("[{ts}] TEXT: {text}"),
        AgentEvent::ToolUse { tool, input_preview } => {
            format!("[{ts}] TOOL: {tool} {{ {input_preview} }}")
        }
        AgentEvent::Error(msg) => format!("[{ts}] ERROR: {msg}"),
        AgentEvent::Complete { cost_usd } => format!("[{ts}] COMPLETE: cost=${cost_usd:.2}"),
    }
}

pub fn log_agent_event(runs_dir: &Path, issue_id: &str, event: &AgentEvent) {
    let path = runs_dir.join(format!("{issue_id}.agent.log"));
    let Ok(mut file) = OpenOptions::new().append(true).create(true).open(&path) else {
        return;
    };
    let line = format_event(event);
    let _ = writeln!(file, "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_format_text_delta() {
        let event = AgentEvent::TextDelta("hello world".to_string());
        let result = format_event(&event);
        assert!(result.contains("TEXT: hello world"), "got: {result}");
        assert!(result.starts_with('['), "should start with '[': {result}");
    }

    #[test]
    fn test_format_tool_use() {
        let event = AgentEvent::ToolUse {
            tool: "bash".to_string(),
            input_preview: "ls -la".to_string(),
        };
        let result = format_event(&event);
        assert!(result.contains("TOOL: bash { ls -la }"), "got: {result}");
    }

    #[test]
    fn test_format_error() {
        let event = AgentEvent::Error("something went wrong".to_string());
        let result = format_event(&event);
        assert!(result.contains("ERROR: something went wrong"), "got: {result}");
    }

    #[test]
    fn test_format_complete() {
        let event = AgentEvent::Complete { cost_usd: 1.23 };
        let result = format_event(&event);
        assert!(result.contains("COMPLETE: cost=$1.23"), "got: {result}");
    }

    #[test]
    fn test_log_agent_event_writes_to_file() {
        let dir = tempdir().unwrap();
        let event1 = AgentEvent::TextDelta("line one".to_string());
        let event2 = AgentEvent::TextDelta("line two".to_string());

        log_agent_event(dir.path(), "ISSUE-1", &event1);
        log_agent_event(dir.path(), "ISSUE-1", &event2);

        let log_path = dir.path().join("ISSUE-1.agent.log");
        let contents = std::fs::read_to_string(&log_path).unwrap();
        let lines: Vec<&str> = contents.lines().collect();
        assert_eq!(lines.len(), 2, "expected 2 lines, got: {contents}");
        assert!(lines[0].contains("line one"), "first line: {}", lines[0]);
        assert!(lines[1].contains("line two"), "second line: {}", lines[1]);
    }

    #[test]
    fn test_log_agent_event_separates_issues() {
        let dir = tempdir().unwrap();
        let event_a = AgentEvent::TextDelta("for A".to_string());
        let event_b = AgentEvent::TextDelta("for B".to_string());

        log_agent_event(dir.path(), "ISSUE-A", &event_a);
        log_agent_event(dir.path(), "ISSUE-B", &event_b);

        let path_a = dir.path().join("ISSUE-A.agent.log");
        let path_b = dir.path().join("ISSUE-B.agent.log");

        assert!(path_a.exists(), "ISSUE-A log should exist");
        assert!(path_b.exists(), "ISSUE-B log should exist");

        let contents_a = std::fs::read_to_string(&path_a).unwrap();
        let contents_b = std::fs::read_to_string(&path_b).unwrap();

        assert!(contents_a.contains("for A"), "A log: {contents_a}");
        assert!(contents_b.contains("for B"), "B log: {contents_b}");
        assert!(!contents_a.contains("for B"), "A log should not contain B content");
        assert!(!contents_b.contains("for A"), "B log should not contain A content");
    }

    #[test]
    fn test_log_agent_event_nonexistent_dir_does_not_panic() {
        let dir = tempdir().unwrap();
        let missing = dir.path().join("does/not/exist");
        log_agent_event(&missing, "ISSUE-1", &AgentEvent::TextDelta("x".into()));
    }
}
