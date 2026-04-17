use std::time::Instant;

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

#[derive(Debug, Clone)]
pub struct ToolResult {
    pub output: String,
    pub is_error: bool,
    pub duration_ms: u64,
}

#[allow(dead_code)] // Used by Task 10+ fold heuristics; tests exercise this now
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

/// Fold threshold: tool results with more than this many output lines are auto-folded.
pub const FOLD_THRESHOLD: usize = 8;

/// Should this entry render folded by default?
/// Fold if: it's a tool with more than FOLD_THRESHOLD lines AND not an error.
/// Errors always render expanded so failures are impossible to miss.
// Used by flatten_entries_with_fold in log_viewer.rs
pub fn should_auto_fold(entry: &LogEntry) -> bool {
    matches!(
        entry,
        LogEntry::Tool { result: Some(r), .. }
            if !r.is_error && r.output.lines().count() > FOLD_THRESHOLD
    )
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

    #[test]
    fn test_body_line_count_marker() {
        assert_eq!(LogEntry::Marker("a\nb".into()).body_line_count(), 2);
    }

    #[test]
    fn test_body_line_count_tool_complete() {
        let entry = LogEntry::Tool {
            tool_use_id: "id1".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "line1\nline2\nline3".into(),
                is_error: false,
                duration_ms: 42,
            }),
            started_at: Instant::now(),
        };
        assert_eq!(entry.body_line_count(), 3);
    }

    #[test]
    fn test_is_error_false_for_text() {
        assert!(!LogEntry::Text("hi".into()).is_error());
    }

    #[test]
    fn test_is_error_false_for_successful_tool() {
        let entry = LogEntry::Tool {
            tool_use_id: "id1".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult { output: "ok".into(), is_error: false, duration_ms: 1 }),
            started_at: Instant::now(),
        };
        assert!(!entry.is_error());
    }

    #[test]
    fn test_auto_fold_large_success() {
        let entry = LogEntry::Tool {
            tool_use_id: "id".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "line\n".repeat(20),
                is_error: false,
                duration_ms: 10,
            }),
            started_at: Instant::now(),
        };
        assert!(should_auto_fold(&entry));
    }

    #[test]
    fn test_auto_fold_error_stays_expanded() {
        let entry = LogEntry::Tool {
            tool_use_id: "id".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "line\n".repeat(20),
                is_error: true,
                duration_ms: 10,
            }),
            started_at: Instant::now(),
        };
        assert!(!should_auto_fold(&entry));
    }

    #[test]
    fn test_auto_fold_small_stays_expanded() {
        let entry = LogEntry::Tool {
            tool_use_id: "id".into(),
            tool: "Bash".into(),
            input: "{}".into(),
            result: Some(ToolResult {
                output: "small".into(),
                is_error: false,
                duration_ms: 10,
            }),
            started_at: Instant::now(),
        };
        assert!(!should_auto_fold(&entry));
    }
}
