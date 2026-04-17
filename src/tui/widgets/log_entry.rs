use std::time::Instant;

#[allow(dead_code)] // Wired up in Task 9
#[derive(Debug, Clone)]
pub enum LogEntry {
    /// Plain text emitted by the agent outside any tool call.
    Text(String),
    /// A tool invocation. `result` is `None` until the matching `tool_result` arrives.
    Tool {
        tool_use_id: String,
        tool: String,
        /// Full input JSON (not the 100-char-truncated preview).
        input: String,
        result: Option<ToolResult>,
        started_at: Instant,
    },
    /// A phase-boundary or orchestrator-emitted marker (e.g. "[Understand] starting…").
    Marker(String),
}

#[allow(dead_code)] // Wired up in Task 9
#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

#[allow(dead_code)] // Wired up in Task 9
impl LogEntry {
    /// How many rendered body lines this entry produces when fully expanded.
    /// (Header not counted.) Useful for fold heuristics.
    pub fn body_line_count(&self) -> usize {
        match self {
            LogEntry::Text(s) | LogEntry::Marker(s) => s.lines().count().max(1),
            LogEntry::Tool { result: Some(r), .. } => r.output.lines().count(),
            LogEntry::Tool { result: None, .. } => 0,
        }
    }

    pub fn is_error(&self) -> bool {
        matches!(self, LogEntry::Tool { result: Some(r), .. } if r.is_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_body_line_count_text() {
        assert_eq!(LogEntry::Text("a\nb\nc".into()).body_line_count(), 3);
    }

    #[test]
    fn test_body_line_count_tool_pending() {
        assert_eq!(
            LogEntry::Tool {
                tool_use_id: "id1".into(),
                tool: "Bash".into(),
                input: "{}".into(),
                result: None,
                started_at: Instant::now(),
            }
            .body_line_count(),
            0
        );
    }

    #[test]
    fn test_is_error_true() {
        let e = LogEntry::Tool {
            tool_use_id: "id1".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "boom".into(),
                is_error: true,
                duration_ms: 1,
            }),
            started_at: Instant::now(),
        };
        assert!(e.is_error());
    }
}
